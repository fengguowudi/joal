//! Stateful per-torrent announcer + the read-only snapshot shared with the UI.
//!
//! Port of Java `org.araymond.joal.core.ttorrent.client.announcer.Announcer`
//! and `AnnouncerFacade`. One instance per torrent, holds the tracker
//! cursor, failure counter, and last-known response snapshot. Async-ready:
//! `announce(...)` is an `async` method that uses [`tokio`] under the hood
//! via [`reqwest`].

use std::sync::Mutex;
use std::time::Instant;

use tracing::{debug, info, warn};

use crate::client::RequestEvent;
use crate::torrent::{InfoHash, MockedTorrent};

use super::error::{AnnouncerError, TooManyFailuresError};
use super::request::AnnounceDataAccessor;
use super::response::SuccessAnnounceResponse;
use super::tracker::TrackerClient;

/// Consecutive-failure threshold after which the announcer gives up and
/// surfaces [`TooManyFailuresError`]. Matches Java's hard-coded `>= 5`
/// check in `Announcer.announce(...)`.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 5;

/// Read-only snapshot of an [`Announcer`]'s observable state.
///
/// Returned by [`Announcer::facade_snapshot`] — one lock acquisition yields
/// all fields the UI / merger task needs.
#[derive(Clone, Debug)]
pub struct AnnouncerSnapshot {
    pub last_known_interval: i32,
    pub consecutive_fails: u32,
    pub last_known_leechers: Option<i32>,
    pub last_known_seeders: Option<i32>,
    pub last_announced_at: Option<Instant>,
    pub torrent_name: String,
    pub torrent_size: u64,
    pub torrent_info_hash: InfoHash,
}

/// Mutable per-torrent state kept behind a [`Mutex`]. Split out of the main
/// struct so the `async fn announce` can hold the tracker client + data
/// accessor across the `.await` point without a lock guard in scope.
#[derive(Debug, Default)]
struct AnnouncerState {
    last_known_interval: i32,
    consecutive_fails: u32,
    last_known_leechers: Option<i32>,
    last_known_seeders: Option<i32>,
    last_announced_at: Option<Instant>,
    reported_upload_bytes: u64,
}

impl AnnouncerState {
    fn new_default() -> Self {
        Self {
            // Java default: `private int lastKnownInterval = 5;`
            last_known_interval: 5,
            ..Default::default()
        }
    }
}

/// Stateful announcer for a single torrent.
///
/// Mirrors Java `Announcer`. Equality is delegated to the torrent's
/// info-hash so `HashSet<Announcer>` deduplicates by torrent identity the
/// same way the Java code does.
pub struct Announcer {
    torrent: MockedTorrent,
    tracker_client: TrackerClient,
    data_accessor: AnnounceDataAccessor,
    upload_ratio_target: f32,
    state: Mutex<AnnouncerState>,
    /// Sticky bit set by the merger when the simulated download crosses the
    /// `total_size` boundary (or the user toggles "initial completed" on).
    /// On the next `RequestEvent::None` announce the executor swaps the
    /// event to [`RequestEvent::Completed`] and clears this bit. Stored as
    /// a `Mutex<bool>` rather than an `AtomicBool` because all access is
    /// already serialised through the existing announcer-level lock paths,
    /// and a mutex keeps the API uniform with the rest of the announcer
    /// state.
    pending_completed: Mutex<bool>,
}

impl std::fmt::Debug for Announcer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Announcer")
            .field("info_hash", &self.torrent.info_hash)
            .field("name", &self.torrent.name)
            .finish_non_exhaustive()
    }
}

impl Announcer {
    /// Assemble an announcer for one torrent.
    ///
    /// `upload_ratio_target` is the Java `appConfiguration.uploadRatioTarget`
    /// value: use `-1.0` to disable the ratio gate.
    #[must_use]
    pub fn new(
        torrent: MockedTorrent,
        tracker_client: TrackerClient,
        data_accessor: AnnounceDataAccessor,
        upload_ratio_target: f32,
    ) -> Self {
        Self {
            torrent,
            tracker_client,
            data_accessor,
            upload_ratio_target,
            state: Mutex::new(AnnouncerState::new_default()),
            pending_completed: Mutex::new(false),
        }
    }

    /// Set the sticky `pending_completed` bit. Called by the merger task
    /// when the bandwidth dispatcher reports a torrent just reached
    /// `total_size`, or by the orchestrator when the user toggles "initial
    /// completed" on at runtime.
    pub fn mark_completed_pending(&self) {
        let mut flag = self
            .pending_completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *flag = true;
    }

    /// Take and clear the `pending_completed` bit in one shot. Returns the
    /// previous value, so the caller can decide whether to swap an
    /// outgoing `None` event for `Completed`.
    pub fn take_pending_completed(&self) -> bool {
        let mut flag = self
            .pending_completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut *flag, false)
    }

    #[must_use]
    pub fn info_hash(&self) -> &InfoHash {
        &self.torrent.info_hash
    }

    /// Send one announce and update the internal counters.
    ///
    /// Mirrors Java `Announcer.announce(event)` including:
    /// - the `consecutiveFails++` / `>= 5` escalation
    /// - the `reportedUploadBytes = uploaded()` update on success
    /// - the `interval` / `complete - 1` / `incomplete` snapshot
    pub async fn announce(
        &self,
        event: RequestEvent,
    ) -> Result<SuccessAnnounceResponse, AnnouncerError> {
        debug!(
            info_hash = %self.torrent.info_hash,
            event = event.event_name(),
            "attempting announce"
        );
        let query = self
            .data_accessor
            .http_request_query_for_torrent(&self.torrent.info_hash, event)?;
        let headers = self.data_accessor.http_headers_for_torrent();

        // Record the attempt timestamp up-front so UI queries that fire
        // between attempt and completion see the attempt.
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.last_announced_at = Some(Instant::now());
        }

        match self.tracker_client.announce(&query, headers).await {
            Ok(resp) => {
                info!(
                    info_hash = %self.torrent.info_hash,
                    seeders = resp.seeders(),
                    leechers = resp.leechers(),
                    interval = resp.interval(),
                    "announce succeeded"
                );
                {
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.reported_upload_bytes =
                        self.data_accessor.uploaded(&self.torrent.info_hash);
                    state.last_known_interval = resp.interval();
                    state.last_known_leechers = Some(resp.leechers());
                    state.last_known_seeders = Some(resp.seeders());
                    state.consecutive_fails = 0;
                }
                Ok(resp)
            }
            Err(err) => {
                let consecutive_fails = {
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.consecutive_fails += 1;
                    state.consecutive_fails
                };
                if consecutive_fails >= MAX_CONSECUTIVE_FAILURES {
                    warn!(
                        info_hash = %self.torrent.info_hash,
                        consecutive_fails,
                        "announcer has exceeded the failure threshold"
                    );
                    return Err(AnnouncerError::TooManyFailures(TooManyFailuresError {
                        info_hash: self.torrent.info_hash.clone(),
                        consecutive_fails,
                    }));
                }
                info!(
                    info_hash = %self.torrent.info_hash,
                    consecutive_fails,
                    cause = %err,
                    "announce failed"
                );
                Err(err)
            }
        }
    }

    /// Returns `true` when the torrent has reached the configured upload-
    /// ratio cap. `-1.0` disables the gate, matching Java `hasReachedUploadRatioLimit`.
    #[must_use]
    pub fn has_reached_upload_ratio_limit(&self) -> bool {
        if self.upload_ratio_target == -1.0 {
            return false;
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let size = self.torrent.total_size;
        #[allow(clippy::cast_precision_loss)]
        let target_bytes = self.upload_ratio_target * (size as f32);
        #[allow(clippy::cast_precision_loss)]
        let uploaded = state.reported_upload_bytes as f32;
        uploaded >= target_bytes
    }

    #[must_use]
    pub fn torrent(&self) -> &MockedTorrent {
        &self.torrent
    }

    /// Immutable ref to the data accessor, primarily for integration tests.
    #[must_use]
    pub fn data_accessor(&self) -> &AnnounceDataAccessor {
        &self.data_accessor
    }

    /// Immutable ref to the tracker client, primarily for integration tests.
    #[must_use]
    pub fn tracker_client(&self) -> &TrackerClient {
        &self.tracker_client
    }
}

impl Announcer {
    #[must_use]
    pub fn last_known_interval(&self) -> i32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_known_interval
    }

    #[must_use]
    pub fn consecutive_fails(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .consecutive_fails
    }

    #[must_use]
    pub fn torrent_name(&self) -> &str {
        &self.torrent.name
    }

    #[must_use]
    pub fn torrent_size(&self) -> u64 {
        self.torrent.total_size
    }

    #[must_use]
    pub fn torrent_info_hash(&self) -> &InfoHash {
        &self.torrent.info_hash
    }

    /// Single-lock snapshot of all UI-visible fields.
    #[must_use]
    pub fn facade_snapshot(&self) -> AnnouncerSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        AnnouncerSnapshot {
            last_known_interval: state.last_known_interval,
            consecutive_fails: state.consecutive_fails,
            last_known_leechers: state.last_known_leechers,
            last_known_seeders: state.last_known_seeders,
            last_announced_at: state.last_announced_at,
            torrent_name: self.torrent.name.clone(),
            torrent_size: self.torrent.total_size,
            torrent_info_hash: self.torrent.info_hash.clone(),
        }
    }
}

impl PartialEq for Announcer {
    fn eq(&self, other: &Self) -> bool {
        self.torrent.info_hash == other.torrent.info_hash
    }
}

impl Eq for Announcer {}

impl std::hash::Hash for Announcer {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.torrent.info_hash.hash(state);
    }
}
