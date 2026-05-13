//! Engine event bus.
//!
//! Rust counterpart of Java's `core/events/*` + the scattered
//! `ApplicationEventPublisher` usage. There is no Spring-style DI in the Rust
//! side, so the engine uses a single [`EngineEvent`] enum plus an
//! [`EngineEventSink`] trait; the default implementation wraps
//! [`tokio::sync::broadcast`] so UI / logging / persistence consumers can
//! subscribe in parallel.
//!
//! # Design notes
//!
//! - Events are **data-only**. Consumers MUST NOT mutate engine state from
//!   inside an event handler — the Java `CoreEventListener` comment applies
//!   verbatim. If a consumer needs to trigger behaviour, it should publish a
//!   new event.
//! - The channel is lossy by design: a slow subscriber that drops events
//!   would never wedge the announcer hot path.
//! - [`EngineEvent`] is `Clone` so the broadcast channel can fan out. All
//!   payloads avoid holding references to engine-internal state (they use
//!   owned `InfoHash`, `String`, etc.).

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::torrent::InfoHash;

/// Distinct observable events emitted by the engine.
///
/// The naming mirrors Java's `core/events/<family>/<Name>Event`. Where Java
/// carries the whole `Announcer` reference, the Rust event carries only the
/// observable fields — this keeps the bus decoupled from the orchestrator's
/// internal state.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// Seeding has started. Emitted after the orchestrator has built the
    /// bandwidth dispatcher and kicked off the first torrents.
    GlobalSeedStarted {
        /// Human-readable name of the active emulated client (e.g.
        /// `"qBittorrent/4.5.0"`). Derived from the `.client` User-Agent.
        client_name: String,
    },

    /// Seeding has stopped. Emitted once all STOPPED announces have either
    /// completed or timed out.
    GlobalSeedStopped,

    /// A `.torrent` file has been successfully added to the catalogue.
    TorrentFileAdded {
        info_hash: InfoHash,
        name: String,
        total_size: u64,
    },

    /// A `.torrent` file has been removed (user delete, archive, or failure).
    TorrentFileDeleted { info_hash: InfoHash, name: String },

    /// Adding / saving a `.torrent` failed.
    FailedToAddTorrentFile { name: String, reason: String },

    /// An announcer has crossed the 5-consecutive-failure threshold and will
    /// be dropped from the seeding pool.
    TooManyAnnouncesFailedInARow { info_hash: InfoHash, name: String },

    /// Config has just been reloaded from disk. Carries the new settings —
    /// consumers can snapshot without touching the config provider.
    ConfigLoaded {
        config: crate::config::AppConfiguration,
    },
}

/// Publish endpoint for [`EngineEvent`].
///
/// Kept as a trait so the UI layer can swap in a richer sink (e.g. a tokio
/// mpsc wired straight into `egui::Context::request_repaint`). Default
/// implementation is [`BroadcastSink`].
pub trait EngineEventSink: Send + Sync {
    fn publish(&self, event: EngineEvent);
}

/// Discards every event. Used by headless tests that don't care about the
/// bus and by the seed-manager when no subscriber has been wired yet.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSink;

impl EngineEventSink for NoopSink {
    fn publish(&self, _event: EngineEvent) {}
}

/// Default sink: a [`tokio::sync::broadcast`] channel.
///
/// Subscribers receive every event published after their
/// [`BroadcastSink::subscribe`] call; older events are dropped per the
/// broadcast channel's ring-buffer semantics.
#[derive(Debug, Clone)]
pub struct BroadcastSink {
    sender: broadcast::Sender<EngineEvent>,
}

impl BroadcastSink {
    /// Build a sink with the given channel capacity. A capacity of at least
    /// 64 is a safe default for the engine's bursty publish patterns.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to every future event. Each subscriber gets its own cursor.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.sender.subscribe()
    }
}

impl Default for BroadcastSink {
    fn default() -> Self {
        Self::new(256)
    }
}

impl EngineEventSink for BroadcastSink {
    fn publish(&self, event: EngineEvent) {
        // `send` only fails when there are no active receivers; that is a
        // legitimate quiescent state — drop the event.
        let _ = self.sender.send(event);
    }
}

/// Convenience helper used by the orchestrator / handlers. Takes ownership of
/// an `Arc<dyn EngineEventSink>` so callers don't have to clone themselves.
pub fn publish(sink: &Arc<dyn EngineEventSink>, event: EngineEvent) {
    sink.publish(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcast_sink_fans_out_to_every_subscriber() {
        let sink = BroadcastSink::new(8);
        let mut rx1 = sink.subscribe();
        let mut rx2 = sink.subscribe();
        sink.publish(EngineEvent::GlobalSeedStopped);

        match rx1.recv().await.unwrap() {
            EngineEvent::GlobalSeedStopped => {}
            other => panic!("rx1: {other:?}"),
        }
        match rx2.recv().await.unwrap() {
            EngineEvent::GlobalSeedStopped => {}
            other => panic!("rx2: {other:?}"),
        }
    }

    #[test]
    fn noop_sink_silently_discards() {
        let sink = NoopSink;
        sink.publish(EngineEvent::GlobalSeedStopped);
    }
}
