//! Stateful per-torrent announcer + the read-only facade shared with the UI.
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

/// Read-only view of an [`Announcer`]. Mirror of Java `AnnouncerFacade`.
///
/// The UI layer and the CoreEventListener chain consume this; they must not
/// be able to reach into the announcer's failure counter or tracker client
/// directly.
pub trait AnnouncerFacade: Send + Sync {
    fn last_known_interval(&self) -> i32;
    fn consecutive_fails(&self) -> u32;
    fn last_known_leechers(&self) -> Option<i32>;
    fn last_known_seeders(&self) -> Option<i32>;
    /// Wall-clock instant of the most recent announce attempt, if any.
    fn last_announced_at(&self) -> Option<Instant>;
    fn torrent_name(&self) -> &str;
    fn torrent_size(&self) -> u64;
    fn torrent_info_hash(&self) -> &InfoHash;
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
        }
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

impl AnnouncerFacade for Announcer {
    fn last_known_interval(&self) -> i32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_known_interval
    }

    fn consecutive_fails(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .consecutive_fails
    }

    fn last_known_leechers(&self) -> Option<i32> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_known_leechers
    }

    fn last_known_seeders(&self) -> Option<i32> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_known_seeders
    }

    fn last_announced_at(&self) -> Option<Instant> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_announced_at
    }

    fn torrent_name(&self) -> &str {
        &self.torrent.name
    }

    fn torrent_size(&self) -> u64 {
        self.torrent.total_size
    }

    fn torrent_info_hash(&self) -> &InfoHash {
        &self.torrent.info_hash
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::announcer::error::AnnouncerError;
    use crate::announcer::request::AnnounceDataAccessor;
    use crate::announcer::state::{Announcer, MAX_CONSECUTIVE_FAILURES};
    use crate::announcer::tracker::{TrackerClient, TrackerClientUriProvider};
    use crate::bandwidth::{BandwidthDispatcher, RandomSpeedProvider};
    use crate::client::{BitTorrentClient, BitTorrentClientConfig, ConnectionHandler};
    use crate::torrent::{InfoHash, MockedTorrent};

    fn qb_client() -> BitTorrentClient {
        let json = include_str!("../../../../resources/clients/qbittorrent-4.5.0.client");
        let cfg: BitTorrentClientConfig = json.try_into().unwrap();
        BitTorrentClient::new(cfg).unwrap()
    }

    fn sample_torrent() -> MockedTorrent {
        let mut bytes = [0u8; 20];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8;
        }
        MockedTorrent {
            info_hash: InfoHash::from_bytes(bytes),
            name: "unit-test-torrent".to_owned(),
            total_size: 1024,
            piece_length: 512,
            piece_count: 2,
            announce: "http://127.0.0.1:1/announce".to_owned(),
            announce_tiers: Vec::new(),
        }
    }

    fn data_accessor() -> AnnounceDataAccessor {
        let cfg = crate::config::AppConfiguration {
            min_upload_rate: 0,
            max_upload_rate: 0,
            simultaneous_seed: 1,
            client: "x.client".into(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
            proxy_host: None,
            proxy_port: None,
        };
        let dispatcher = Arc::new(BandwidthDispatcher::new(
            std::time::Duration::from_millis(100),
            RandomSpeedProvider::new(&cfg),
        ));
        dispatcher.register_torrent(sample_torrent().info_hash);
        AnnounceDataAccessor::new(
            Arc::new(qb_client()),
            dispatcher,
            Arc::new(ConnectionHandler::with_port_only(12_345)),
        )
    }

    #[tokio::test]
    async fn announcer_escalates_after_threshold_failures() {
        // Tracker URI points at a non-routable loopback that will refuse
        // connections → every announce errors. After the Nth failure we
        // should see AnnouncerError::TooManyFailures rather than the
        // underlying transport error.
        let provider =
            TrackerClientUriProvider::new(vec!["http://127.0.0.1:1/announce".to_owned()]).unwrap();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(50))
            .build()
            .unwrap();
        let tracker = TrackerClient::with_http_client(provider, http);
        let announcer = Announcer::new(sample_torrent(), tracker, data_accessor(), -1.0);

        let mut last_err = None;
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            last_err = Some(
                announcer
                    .announce(crate::client::RequestEvent::Started)
                    .await
                    .unwrap_err(),
            );
        }
        assert!(matches!(
            last_err.unwrap(),
            AnnouncerError::TooManyFailures(_)
        ));
    }
}
