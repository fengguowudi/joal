//! Announce response handler chain + concrete handlers.
//!
//! Port of Java
//! `org.araymond.joal.core.ttorrent.client.announcer.response.*`. The Rust
//! chain is a typed struct with fields in execution order; handlers are
//! invoked sequentially and must never panic (the executor task will be
//! torn down if they do).
//!
//! Concrete handlers:
//!
//! - [`AnnounceReEnqueuer`] — feeds the next request back into the
//!   [`DelayQueue`][crate::orchestrator::DelayQueue]. Success uses the
//!   server-supplied interval; failure uses `announcer.last_known_interval`.
//! - [`BandwidthDispatcherNotifier`] — register / unregister / update-peers
//!   calls against the [`BandwidthDispatcher`][crate::bandwidth::BandwidthDispatcher].
//! - [`ClientNotifier`] — callbacks into the [`ClientOrchestrator`][crate::orchestrator::ClientOrchestrator]
//!   for drop / refill decisions.
//! - [`AnnounceEventPublisher`] — fans out public-facing events on the
//!   [`EngineEventSink`][crate::events::EngineEventSink].

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::announcer::{
    AnnounceRequest, Announcer, AnnouncerError, SuccessAnnounceResponse, TooManyFailuresError,
};
use crate::bandwidth::BandwidthDispatcher;
use crate::client::RequestEvent;
use crate::events::{EngineEvent, EngineEventSink};
use crate::orchestrator::announcer_executor::{AnnounceResponseCallback, OrchestratorControl};
use crate::orchestrator::delay_queue::DelayQueue;
use crate::snapshot::MergerPoke;

/// Outcome of an announce round-trip.
#[derive(Debug)]
pub enum AnnounceOutcome {
    Success(SuccessAnnounceResponse),
    Failure(AnnouncerError),
    TooManyFailures(TooManyFailuresError),
}

/// Typed handler pipeline. Field order is execution order:
/// 1. event_publisher — UX parity with Java (publish first)
/// 2. re_enqueuer — schedule next announce
/// 3. bandwidth_notifier — register/unregister/update peers
/// 4. client_notifier — orchestrator drop/refill decisions
/// 5. merger_poke — snapshot trigger (runs last so facade fields are up-to-date)
pub struct AnnounceResponseHandlerChain {
    event_publisher: AnnounceEventPublisher,
    re_enqueuer: AnnounceReEnqueuer,
    bandwidth_notifier: BandwidthDispatcherNotifier,
    client_notifier: ClientNotifier,
    merger_poke: Option<MergerPokeNotifier>,
}

impl AnnounceResponseHandlerChain {
    #[must_use]
    pub fn new(
        event_publisher: AnnounceEventPublisher,
        re_enqueuer: AnnounceReEnqueuer,
        bandwidth_notifier: BandwidthDispatcherNotifier,
        client_notifier: ClientNotifier,
        merger_poke: Option<MergerPokeNotifier>,
    ) -> Self {
        Self {
            event_publisher,
            re_enqueuer,
            bandwidth_notifier,
            client_notifier,
            merger_poke,
        }
    }

    #[must_use]
    pub fn into_callback(self) -> Arc<dyn AnnounceResponseCallback> {
        Arc::new(self)
    }
}

#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for AnnounceResponseHandlerChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnounceResponseHandlerChain")
            .field("merger_poke", &self.merger_poke.is_some())
            .finish()
    }
}

impl AnnounceResponseCallback for AnnounceResponseHandlerChain {
    fn on_will_announce(&self, event: RequestEvent, announcer: &Arc<Announcer>) {
        self.event_publisher.on_will_announce(announcer, event);
    }

    fn on_announce_result(
        &self,
        event: RequestEvent,
        announcer: &Arc<Announcer>,
        outcome: &AnnounceOutcome,
    ) {
        self.event_publisher
            .on_announce_result(announcer, event, outcome);
        self.re_enqueuer
            .on_announce_result(announcer, event, outcome);
        self.bandwidth_notifier
            .on_announce_result(announcer, event, outcome);
        self.client_notifier
            .on_announce_result(announcer, event, outcome);
        if let Some(ref poke) = self.merger_poke {
            poke.on_announce_result(announcer, event, outcome);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Concrete handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Re-enqueues the next announce based on the Java rules: success uses the
/// server-supplied interval; failure uses the announcer's `last_known_interval`.
pub struct AnnounceReEnqueuer {
    delay_queue: Arc<DelayQueue<AnnounceRequest>>,
}

impl AnnounceReEnqueuer {
    #[must_use]
    pub fn new(delay_queue: Arc<DelayQueue<AnnounceRequest>>) -> Self {
        Self { delay_queue }
    }

    fn on_announce_result(
        &self,
        announcer: &Arc<Announcer>,
        event: RequestEvent,
        outcome: &AnnounceOutcome,
    ) {
        let ih = announcer.torrent_info_hash().clone();
        match (event, outcome) {
            (RequestEvent::Started, AnnounceOutcome::Success(r)) => {
                debug!(info_hash = %ih, "enqueue regular after start success");
                self.delay_queue
                    .add_or_replace(AnnounceRequest::create_regular(ih), seconds(r.interval()));
            }
            (RequestEvent::Started, AnnounceOutcome::Failure(_)) => {
                debug!(info_hash = %ih, "enqueue start (retry) after start failure");
                self.delay_queue.add_or_replace(
                    AnnounceRequest::create_start(ih),
                    seconds(announcer.last_known_interval()),
                );
            }
            (RequestEvent::None, AnnounceOutcome::Success(r)) => {
                debug!(info_hash = %ih, "enqueue regular after regular success");
                self.delay_queue
                    .add_or_replace(AnnounceRequest::create_regular(ih), seconds(r.interval()));
            }
            (RequestEvent::None, AnnounceOutcome::Failure(_)) => {
                debug!(info_hash = %ih, "enqueue regular (retry) after regular failure");
                self.delay_queue.add_or_replace(
                    AnnounceRequest::create_regular(ih),
                    seconds(announcer.last_known_interval()),
                );
            }
            (RequestEvent::Stopped, AnnounceOutcome::Failure(_)) => {
                debug!(info_hash = %ih, "enqueue stop (retry) after stop failure");
                self.delay_queue
                    .add_or_replace(AnnounceRequest::create_stop(ih), Duration::ZERO);
            }
            _ => {}
        }
    }
}

/// Calls [`BandwidthDispatcher::register_torrent`] /
/// [`BandwidthDispatcher::unregister_torrent`] /
/// [`BandwidthDispatcher::update_torrent_peers`] at the appropriate events.
pub struct BandwidthDispatcherNotifier {
    bandwidth: Arc<BandwidthDispatcher>,
    state_store: Arc<crate::torrent::TorrentStateStore>,
}

impl BandwidthDispatcherNotifier {
    #[must_use]
    pub fn new(
        bandwidth: Arc<BandwidthDispatcher>,
        state_store: Arc<crate::torrent::TorrentStateStore>,
    ) -> Self {
        Self {
            bandwidth,
            state_store,
        }
    }

    fn on_announce_result(
        &self,
        announcer: &Arc<Announcer>,
        event: RequestEvent,
        outcome: &AnnounceOutcome,
    ) {
        let info_hash = announcer.torrent_info_hash().clone();
        match (event, outcome) {
            (RequestEvent::Started, AnnounceOutcome::Success(r)) => {
                debug!(info_hash = %info_hash, "register torrent with bandwidth dispatcher");
                let total_size = announcer.torrent_size();
                let initial_completed = self.state_store.is_initial_completed(&info_hash);
                self.bandwidth
                    .register_torrent(info_hash.clone(), total_size, initial_completed);
                self.bandwidth.update_torrent_peers(
                    info_hash,
                    r.seeders().max(0) as u32,
                    r.leechers().max(0) as u32,
                );
            }
            (RequestEvent::None, AnnounceOutcome::Success(r)) => {
                debug!(info_hash = %info_hash, "update torrent peers in bandwidth dispatcher");
                self.bandwidth.update_torrent_peers(
                    info_hash,
                    r.seeders().max(0) as u32,
                    r.leechers().max(0) as u32,
                );
            }
            (RequestEvent::Stopped, AnnounceOutcome::Success(_)) => {
                debug!(info_hash = %info_hash, "unregister torrent from bandwidth dispatcher");
                self.bandwidth.unregister_torrent(&info_hash);
            }
            (_, AnnounceOutcome::TooManyFailures(_)) => {
                debug!(info_hash = %info_hash, "unregister torrent after too many failures");
                self.bandwidth.unregister_torrent(&info_hash);
            }
            _ => {}
        }
    }
}

/// Bridges the handler chain into the orchestrator's drop/refill logic.
pub struct ClientNotifier {
    control: Arc<dyn OrchestratorControl>,
}

impl ClientNotifier {
    #[must_use]
    pub fn new(control: Arc<dyn OrchestratorControl>) -> Self {
        Self { control }
    }

    fn on_announce_result(
        &self,
        announcer: &Arc<Announcer>,
        event: RequestEvent,
        outcome: &AnnounceOutcome,
    ) {
        match (event, outcome) {
            (RequestEvent::Started, AnnounceOutcome::Success(r)) if r.leechers() < 1 => {
                self.control.on_no_more_peers(announcer.torrent_info_hash());
            }
            (RequestEvent::None, AnnounceOutcome::Success(r)) => {
                if r.leechers() < 1 {
                    self.control.on_no_more_peers(announcer.torrent_info_hash());
                    return;
                }
                if announcer.has_reached_upload_ratio_limit() {
                    self.control
                        .on_upload_ratio_limit_reached(announcer.torrent_info_hash());
                }
            }
            (RequestEvent::Stopped, AnnounceOutcome::Success(_)) => {
                debug!(info_hash = %announcer.torrent_info_hash(), "torrent has stopped");
                self.control
                    .on_torrent_has_stopped(announcer.torrent_info_hash());
            }
            (_, AnnounceOutcome::TooManyFailures(_)) => {
                debug!(info_hash = %announcer.torrent_info_hash(), "torrent has failed too many times");
                self.control
                    .on_too_many_failed(announcer.torrent_info_hash());
            }
            _ => {}
        }
    }
}

/// Fans out visible events to the [`EngineEventSink`] for UI / logging.
pub struct AnnounceEventPublisher {
    sink: Arc<dyn EngineEventSink>,
}

impl AnnounceEventPublisher {
    #[must_use]
    pub fn new(sink: Arc<dyn EngineEventSink>) -> Self {
        Self { sink }
    }

    fn on_will_announce(&self, announcer: &Arc<Announcer>, _event: RequestEvent) {
        let tracker_url = announcer.tracker_client().uri_provider().current();
        self.sink.publish(EngineEvent::AnnounceStarted {
            info_hash: announcer.torrent_info_hash().clone(),
            name: announcer.torrent().name.clone(),
            tracker_url,
        });
    }

    fn on_announce_result(
        &self,
        announcer: &Arc<Announcer>,
        _event: RequestEvent,
        outcome: &AnnounceOutcome,
    ) {
        match outcome {
            AnnounceOutcome::Success(r) => {
                self.sink.publish(EngineEvent::AnnounceSucceeded {
                    info_hash: announcer.torrent_info_hash().clone(),
                    name: announcer.torrent().name.clone(),
                    seeders: r.seeders().max(0) as u32,
                    leechers: r.leechers().max(0) as u32,
                    interval: r.interval().max(0) as u32,
                });
            }
            AnnounceOutcome::Failure(e) => {
                self.sink.publish(EngineEvent::AnnounceFailed {
                    info_hash: announcer.torrent_info_hash().clone(),
                    name: announcer.torrent().name.clone(),
                    error: e.to_string(),
                });
            }
            AnnounceOutcome::TooManyFailures(_) => {
                self.sink
                    .publish(EngineEvent::TooManyAnnouncesFailedInARow {
                        info_hash: announcer.torrent_info_hash().clone(),
                        name: announcer.torrent().name.clone(),
                    });
            }
        }
    }
}

/// Pokes the merger task whenever announcer state has changed.
pub struct MergerPokeNotifier {
    poke: mpsc::Sender<MergerPoke>,
}

impl MergerPokeNotifier {
    #[must_use]
    pub fn new(poke: mpsc::Sender<MergerPoke>) -> Self {
        Self { poke }
    }

    fn fire(&self) {
        if self.poke.try_send(MergerPoke::AnnouncerUpdated).is_err() {
            warn!("merger poke channel is full or closed; announcer update will be coalesced");
        }
    }

    fn on_announce_result(
        &self,
        _announcer: &Arc<Announcer>,
        _event: RequestEvent,
        _outcome: &AnnounceOutcome,
    ) {
        self.fire();
    }
}

fn seconds(n: i32) -> Duration {
    Duration::from_secs(n.max(0) as u64)
}
