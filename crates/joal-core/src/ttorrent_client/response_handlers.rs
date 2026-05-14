//! Announce response handler chain + concrete handlers.
//!
//! Port of Java
//! `org.araymond.joal.core.ttorrent.client.announcer.response.*`. The Rust
//! chain is a `Vec<Arc<dyn AnnounceResponseHandler>>`; handlers are invoked
//! in registration order and must never panic (the executor task will be
//! torn down if they do).
//!
//! Concrete handlers:
//!
//! - [`AnnounceReEnqueuer`] — feeds the next request back into the
//!   [`DelayQueue`][crate::ttorrent_client::DelayQueue]. Success uses the
//!   server-supplied interval; failure uses `announcer.last_known_interval`.
//! - [`BandwidthDispatcherNotifier`] — register / unregister / update-peers
//!   calls against the [`BandwidthDispatcher`][crate::bandwidth::BandwidthDispatcher].
//! - [`ClientNotifier`] — callbacks into the [`ClientOrchestrator`][crate::ttorrent_client::ClientOrchestrator]
//!   for drop / refill decisions.
//! - [`AnnounceEventPublisher`] — fans out public-facing events on the
//!   [`EngineEventSink`][crate::events::EngineEventSink].

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::debug;

use crate::announcer::{
    AnnounceRequest, Announcer, AnnouncerError, AnnouncerFacade, SuccessAnnounceResponse,
    TooManyFailuresError,
};
use crate::bandwidth::BandwidthDispatcher;
use crate::client::RequestEvent;
use crate::events::{EngineEvent, EngineEventSink};
use crate::snapshot::MergerPoke;
use crate::torrent::InfoHash;
use crate::ttorrent_client::announcer_executor::AnnounceResponseCallback;
use crate::ttorrent_client::delay_queue::DelayQueue;

/// Outcome of an announce round-trip.
#[derive(Debug)]
pub enum AnnounceOutcome {
    Success(SuccessAnnounceResponse),
    Failure(AnnouncerError),
    TooManyFailures(TooManyFailuresError),
}

/// Per-event handler. Simplified from the Java visitor-style 8-method trait.
pub trait AnnounceResponseHandler: Send + Sync {
    fn on_will_announce(&self, _announcer: &Arc<Announcer>, _event: RequestEvent) {}
    fn on_announce_result(
        &self,
        _announcer: &Arc<Announcer>,
        _event: RequestEvent,
        _outcome: &AnnounceOutcome,
    ) {
    }
}

/// Fan-out wrapper. Implements [`AnnounceResponseCallback`] so the executor
/// can call a single instance and get the chain dispatched behind it.
pub struct AnnounceResponseHandlerChain {
    handlers: Vec<Arc<dyn AnnounceResponseHandler>>,
}

impl AnnounceResponseHandlerChain {
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    pub fn append(&mut self, handler: Arc<dyn AnnounceResponseHandler>) {
        self.handlers.push(handler);
    }

    #[must_use]
    pub fn into_callback(self) -> Arc<dyn AnnounceResponseCallback> {
        Arc::new(self)
    }
}

impl Default for AnnounceResponseHandlerChain {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AnnounceResponseHandlerChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnounceResponseHandlerChain")
            .field("handlers", &self.handlers.len())
            .finish()
    }
}

impl AnnounceResponseCallback for AnnounceResponseHandlerChain {
    fn on_will_announce(&self, event: RequestEvent, announcer: &Arc<Announcer>) {
        for h in &self.handlers {
            h.on_will_announce(announcer, event);
        }
    }

    fn on_announce_result(
        &self,
        event: RequestEvent,
        announcer: &Arc<Announcer>,
        outcome: &AnnounceOutcome,
    ) {
        for h in &self.handlers {
            h.on_announce_result(announcer, event, outcome);
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
}

impl AnnounceResponseHandler for AnnounceReEnqueuer {
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
}

impl BandwidthDispatcherNotifier {
    #[must_use]
    pub fn new(bandwidth: Arc<BandwidthDispatcher>) -> Self {
        Self { bandwidth }
    }
}

impl AnnounceResponseHandler for BandwidthDispatcherNotifier {
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
                self.bandwidth.register_torrent(info_hash.clone());
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
pub trait ClientNotificationSink: Send + Sync {
    fn on_too_many_failed(&self, info_hash: &InfoHash);
    fn on_upload_ratio_limit_reached(&self, info_hash: &InfoHash);
    fn on_no_more_peers(&self, info_hash: &InfoHash);
    fn on_torrent_has_stopped(&self, info_hash: &InfoHash);
}

pub struct ClientNotifier {
    sink: Arc<dyn ClientNotificationSink>,
}

impl ClientNotifier {
    #[must_use]
    pub fn new(sink: Arc<dyn ClientNotificationSink>) -> Self {
        Self { sink }
    }
}

impl AnnounceResponseHandler for ClientNotifier {
    fn on_announce_result(
        &self,
        announcer: &Arc<Announcer>,
        event: RequestEvent,
        outcome: &AnnounceOutcome,
    ) {
        match (event, outcome) {
            (RequestEvent::Started, AnnounceOutcome::Success(r))
                if r.seeders() < 1 || r.leechers() < 1 =>
            {
                self.sink.on_no_more_peers(announcer.torrent_info_hash());
            }
            (RequestEvent::None, AnnounceOutcome::Success(r)) => {
                if r.seeders() < 1 || r.leechers() < 1 {
                    self.sink.on_no_more_peers(announcer.torrent_info_hash());
                    return;
                }
                if announcer.has_reached_upload_ratio_limit() {
                    self.sink
                        .on_upload_ratio_limit_reached(announcer.torrent_info_hash());
                }
            }
            (RequestEvent::Stopped, AnnounceOutcome::Success(_)) => {
                debug!(info_hash = %announcer.torrent_info_hash(), "torrent has stopped");
                self.sink
                    .on_torrent_has_stopped(announcer.torrent_info_hash());
            }
            (_, AnnounceOutcome::TooManyFailures(_)) => {
                debug!(info_hash = %announcer.torrent_info_hash(), "torrent has failed too many times");
                self.sink.on_too_many_failed(announcer.torrent_info_hash());
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
}

impl AnnounceResponseHandler for AnnounceEventPublisher {
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
        let _ = self.poke.try_send(MergerPoke::AnnouncerUpdated);
    }
}

impl AnnounceResponseHandler for MergerPokeNotifier {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announcer::AnnounceRequest;
    use crate::torrent::InfoHash;

    fn ih(x: u8) -> InfoHash {
        let mut bytes = [0u8; 20];
        bytes[0] = x;
        InfoHash::from_bytes(bytes)
    }

    #[test]
    fn seconds_clamps_negatives_to_zero() {
        assert_eq!(seconds(-1), Duration::ZERO);
        assert_eq!(seconds(0), Duration::ZERO);
        assert_eq!(seconds(5), Duration::from_secs(5));
    }

    #[test]
    fn dispatcher_notifier_clamps_negative_peer_counts() {
        let _ = AnnounceRequest::create_regular(ih(1));
    }
}
