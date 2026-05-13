//! Per-frame, UI-shaped projection of engine state.
//!
//! Events (see [`crate::events::EngineEvent`]) describe *transitions* — "a
//! torrent was added", "seeding started". They intentionally carry no live
//! bookkeeping: a UI that arrives mid-session after a `TorrentFileAdded`
//! would otherwise need to re-play the event log to reconstruct the current
//! upload counters.
//!
//! [`EngineSnapshot`] closes that gap: it is a cheaply cloneable projection
//! of *state*, published on every meaningful change via a
//! [`tokio::sync::watch`] channel. Consumers (CLI logger, egui `ViewModel`,
//! headless test) always see the most recent frame and can drop historical
//! frames freely.
//!
//! The snapshot is *coherent*: the [`SeedManager`][crate::seed_manager]
//! merger task joins the orchestrator (announcer facades) with the bandwidth
//! dispatcher (per-torrent upload counters + speeds) in one critical section
//! before publishing. Consumers never observe half-joined state.

use std::time::Instant;

use crate::torrent::InfoHash;

/// One frame of the engine's externally-visible state.
///
/// Cloning is `O(torrent_count)` — about 200 bytes per torrent on 64-bit
/// targets — which is the cost the [`tokio::sync::watch`] channel pays on
/// every publish. Sized for 10–100 torrents in the common case.
#[derive(Clone, Debug, Default)]
pub struct EngineSnapshot {
    /// Filename of the active `.client` (e.g. `qbittorrent-4.5.0.client`).
    /// Empty on the sentinel default frame published before the first merge.
    pub active_client_filename: String,

    /// Sum of per-torrent `current_speed_bps`. Provided as a convenience so
    /// a status HUD doesn't have to re-sum the list every frame.
    pub global_upload_speed_bps: u64,

    /// One entry per live announcer, ordered as returned by the orchestrator
    /// (insertion order).
    pub torrents: Vec<TorrentStatus>,
}

/// Live state for a single seeding torrent.
#[derive(Clone, Debug)]
pub struct TorrentStatus {
    pub info_hash: InfoHash,
    pub name: String,
    pub total_size: u64,

    /// Accumulated tracker-visible `uploaded` bytes — from
    /// `BandwidthDispatcher::get_seed_stat_for_torrent`.
    pub uploaded_bytes: u64,
    /// Latest per-torrent allocation from the bandwidth dispatcher.
    pub current_speed_bps: u64,

    /// Tracker-reported `interval` seconds from the most recent announce.
    /// `None` before the first successful announce.
    pub last_known_interval: Option<u32>,
    /// Tracker-reported seeder count (already self-excluded: `max(0, complete - 1)`).
    pub last_known_seeders: Option<u32>,
    pub last_known_leechers: Option<u32>,
    /// Consecutive-failure counter; crosses [`MAX_CONSECUTIVE_FAILURES`][crate::announcer::MAX_CONSECUTIVE_FAILURES]
    /// just before the announcer is dropped from the pool.
    pub consecutive_fails: u32,
    /// Monotonic timestamp of the most recent announce attempt.
    pub last_announced_at: Option<Instant>,
}

/// Internal wake-up token sent to the [`SeedManager`][crate::seed_manager]
/// merger task. One variant per non-event trigger source.
///
/// `broadcast::Receiver<EngineEvent>` already covers every transition-style
/// signal; `MergerPoke` fills the two state-only gaps that have no matching
/// engine event:
///
/// * [`MergerPoke::SpeedRecomputed`] — bandwidth dispatcher finished a
///   speed-map recomputation (torrent register / unregister / peer update /
///   20-minute refresh). Replaces the orphan `SpeedChangedListener` trait.
/// * [`MergerPoke::AnnouncerUpdated`] — an announcer's facade fields changed
///   after a successful/failed announce (interval, seeder/leecher counts,
///   consecutive-fails counter, `last_announced_at`). Transition-level
///   success/failure is *also* on the event bus, but the live facade is
///   where the snapshot pulls its per-torrent fields from, so we poke
///   whenever those fields move.
#[derive(Clone, Debug)]
pub enum MergerPoke {
    /// Bandwidth dispatcher recomputed per-torrent speeds.
    SpeedRecomputed,
    /// An announcer updated its facade after an announce round-trip.
    AnnouncerUpdated,
}
