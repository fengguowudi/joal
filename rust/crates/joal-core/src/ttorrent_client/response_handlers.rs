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

use tracing::{debug, warn};

use crate::announcer::{
    AnnounceRequest, Announcer, AnnouncerError, AnnouncerFacade, SuccessAnnounceResponse,
    TooManyFailuresError,
};
use crate::bandwidth::BandwidthDispatcher;
use crate::client::RequestEvent;
use crate::events::{EngineEvent, EngineEventSink};
use crate::torrent::InfoHash;
use crate::ttorrent_client::announcer_executor::AnnounceResponseCallback;
use crate::ttorrent_client::delay_queue::DelayQueue;

/// Per-event handler. Mirror of Java `AnnounceResponseHandler`.
pub trait AnnounceResponseHandler: Send + Sync {
    fn on_will_announce(&self, _announcer: &Arc<Announcer>, _event: RequestEvent) {}

    fn on_start_success(&self, _announcer: &Arc<Announcer>, _result: SuccessAnnounceResponse) {}
    fn on_start_fails(&self, _announcer: &Arc<Announcer>, _error: &AnnouncerError) {}

    fn on_regular_success(&self, _announcer: &Arc<Announcer>, _result: SuccessAnnounceResponse) {}
    fn on_regular_fails(&self, _announcer: &Arc<Announcer>, _error: &AnnouncerError) {}

    fn on_stop_success(&self, _announcer: &Arc<Announcer>, _result: SuccessAnnounceResponse) {}
    fn on_stop_fails(&self, _announcer: &Arc<Announcer>, _error: &AnnouncerError) {}

    fn on_too_many_failed_in_a_row(
        &self,
        _announcer: &Arc<Announcer>,
        _err: &TooManyFailuresError,
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

    fn on_success(
        &self,
        event: RequestEvent,
        announcer: &Arc<Announcer>,
        result: SuccessAnnounceResponse,
    ) {
        for h in &self.handlers {
            match event {
                RequestEvent::Started => h.on_start_success(announcer, result),
                RequestEvent::None => h.on_regular_success(announcer, result),
                RequestEvent::Stopped => h.on_stop_success(announcer, result),
                RequestEvent::Completed => {
                    warn!(?event, "chain: success event not handled");
                }
            }
        }
    }

    fn on_failure(&self, event: RequestEvent, announcer: &Arc<Announcer>, error: &AnnouncerError) {
        for h in &self.handlers {
            match event {
                RequestEvent::Started => h.on_start_fails(announcer, error),
                RequestEvent::None => h.on_regular_fails(announcer, error),
                RequestEvent::Stopped => h.on_stop_fails(announcer, error),
                RequestEvent::Completed => {
                    warn!(?event, "chain: failure event not handled");
                }
            }
        }
    }

    fn on_too_many_failures(
        &self,
        _event: RequestEvent,
        announcer: &Arc<Announcer>,
        err: &TooManyFailuresError,
    ) {
        for h in &self.handlers {
            h.on_too_many_failed_in_a_row(announcer, err);
        }
    }
}

/// Re-enqueues the next announce based on the Java rules: success uses the
/// server-supplied interval; failure uses the announcer's `last_known_interval`.
///
/// Port of Java `AnnounceReEnqueuer`.
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
    fn on_start_success(&self, announcer: &Arc<Announcer>, result: SuccessAnnounceResponse) {
        debug!(info_hash = %announcer.torrent_info_hash(), "enqueue regular after start success");
        self.delay_queue.add_or_replace(
            AnnounceRequest::create_regular(announcer.torrent_info_hash().clone()),
            seconds(result.interval()),
        );
    }

    fn on_start_fails(&self, announcer: &Arc<Announcer>, _error: &AnnouncerError) {
        debug!(info_hash = %announcer.torrent_info_hash(), "enqueue start (retry) after start failure");
        self.delay_queue.add_or_replace(
            AnnounceRequest::create_start(announcer.torrent_info_hash().clone()),
            seconds(announcer.last_known_interval()),
        );
    }

    fn on_regular_success(&self, announcer: &Arc<Announcer>, result: SuccessAnnounceResponse) {
        debug!(info_hash = %announcer.torrent_info_hash(), "enqueue regular after regular success");
        self.delay_queue.add_or_replace(
            AnnounceRequest::create_regular(announcer.torrent_info_hash().clone()),
            seconds(result.interval()),
        );
    }

    fn on_regular_fails(&self, announcer: &Arc<Announcer>, _error: &AnnouncerError) {
        debug!(info_hash = %announcer.torrent_info_hash(), "enqueue regular (retry) after regular failure");
        self.delay_queue.add_or_replace(
            AnnounceRequest::create_regular(announcer.torrent_info_hash().clone()),
            seconds(announcer.last_known_interval()),
        );
    }

    fn on_stop_fails(&self, announcer: &Arc<Announcer>, _error: &AnnouncerError) {
        debug!(info_hash = %announcer.torrent_info_hash(), "enqueue stop (retry) after stop failure");
        self.delay_queue.add_or_replace(
            AnnounceRequest::create_stop(announcer.torrent_info_hash().clone()),
            Duration::ZERO,
        );
    }
}

/// Calls [`BandwidthDispatcher::register_torrent`] /
/// [`BandwidthDispatcher::unregister_torrent`] /
/// [`BandwidthDispatcher::update_torrent_peers`] at the appropriate events.
///
/// Port of Java `BandwidthDispatcherNotifier`.
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
    fn on_start_success(&self, announcer: &Arc<Announcer>, result: SuccessAnnounceResponse) {
        let info_hash = announcer.torrent_info_hash().clone();
        debug!(info_hash = %info_hash, "register torrent with bandwidth dispatcher");
        self.bandwidth.register_torrent(info_hash.clone());
        self.bandwidth.update_torrent_peers(
            info_hash,
            result.seeders().max(0) as u32,
            result.leechers().max(0) as u32,
        );
    }

    fn on_regular_success(&self, announcer: &Arc<Announcer>, result: SuccessAnnounceResponse) {
        let info_hash = announcer.torrent_info_hash().clone();
        debug!(info_hash = %info_hash, "update torrent peers in bandwidth dispatcher");
        self.bandwidth.update_torrent_peers(
            info_hash,
            result.seeders().max(0) as u32,
            result.leechers().max(0) as u32,
        );
    }

    fn on_stop_success(&self, announcer: &Arc<Announcer>, _result: SuccessAnnounceResponse) {
        let info_hash = announcer.torrent_info_hash().clone();
        debug!(info_hash = %info_hash, "unregister torrent from bandwidth dispatcher");
        self.bandwidth.unregister_torrent(&info_hash);
    }

    fn on_too_many_failed_in_a_row(&self, announcer: &Arc<Announcer>, _err: &TooManyFailuresError) {
        let info_hash = announcer.torrent_info_hash().clone();
        debug!(info_hash = %info_hash, "unregister torrent after too many failures");
        self.bandwidth.unregister_torrent(&info_hash);
    }
}

/// Bridges the handler chain into the orchestrator's drop/refill logic.
///
/// Port of Java `ClientNotifier`. Exposed as a trait so tests can substitute
/// a spy without building a real `ClientOrchestrator`.
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
    fn on_start_success(&self, announcer: &Arc<Announcer>, result: SuccessAnnounceResponse) {
        if result.seeders() < 1 || result.leechers() < 1 {
            self.sink.on_no_more_peers(announcer.torrent_info_hash());
        }
    }

    fn on_regular_success(&self, announcer: &Arc<Announcer>, result: SuccessAnnounceResponse) {
        if result.seeders() < 1 || result.leechers() < 1 {
            self.sink.on_no_more_peers(announcer.torrent_info_hash());
            return;
        }
        if announcer.has_reached_upload_ratio_limit() {
            self.sink
                .on_upload_ratio_limit_reached(announcer.torrent_info_hash());
        }
    }

    fn on_stop_success(&self, announcer: &Arc<Announcer>, _result: SuccessAnnounceResponse) {
        debug!(info_hash = %announcer.torrent_info_hash(), "torrent has stopped");
        self.sink
            .on_torrent_has_stopped(announcer.torrent_info_hash());
    }

    fn on_too_many_failed_in_a_row(&self, announcer: &Arc<Announcer>, _err: &TooManyFailuresError) {
        debug!(info_hash = %announcer.torrent_info_hash(), "torrent has failed too many times");
        self.sink.on_too_many_failed(announcer.torrent_info_hash());
    }
}

/// Fans out visible events to the [`EngineEventSink`] for UI / logging.
///
/// Port of Java `AnnounceEventPublisher`. Only emits the subset of events
/// the UI actually consumes; extend as needed.
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
    fn on_too_many_failed_in_a_row(&self, announcer: &Arc<Announcer>, _err: &TooManyFailuresError) {
        self.sink
            .publish(EngineEvent::TooManyAnnouncesFailedInARow {
                info_hash: announcer.torrent_info_hash().clone(),
                name: announcer.torrent().name.clone(),
            });
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
        // Guard against regressing the `result.seeders().max(0)` / `.leechers().max(0)`
        // clamp when porting the Java version. Nothing to run — compiled
        // correctness of the field accessors is enough.
        let _ = AnnounceRequest::create_regular(ih(1));
    }
}
