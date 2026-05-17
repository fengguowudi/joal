//! Bandwidth dispatcher + per-torrent weight allocation.
//!
//! Port of Java `org.araymond.joal.core.bandwith` (intentionally kept spelled
//! "bandwith" on the Java side; Rust normalises to the conventional spelling
//! here). The dispatcher periodically accumulates tracker-visible `uploaded`
//! counters per torrent based on a globally-sampled upload speed split across
//! torrents by a [peers-aware weight][`PeersAwareWeightCalculator`].
//!
//! ## Thread model
//!
//! Java uses `ReentrantReadWriteLock` + a bespoke `Thread` driven by
//! `Thread.sleep(threadPauseIntervalMs)`. This Rust port keeps the same
//! observable semantics (tick cadence, stats accumulated as
//! `speed * tick_ms / 1000`) but replaces the thread with a
//! `tokio::time::interval` loop and the RW-lock with a `std::sync::Mutex`
//! guarding the [`Inner`][dispatcher::Inner] state.
//!
//! `std::sync::Mutex` is fine here because every critical section is bounded,
//! does not cross `.await`, and is entered by cheap map operations. Using a
//! plain mutex lets us expose synchronous `get_seed_stats` / `speeds_snapshot`
//! methods that the announcer hot path (S8) can call without paying for a
//! message round-trip.
//!
//! ## Divergence from Java
//!
//! * `Peers::leechers_ratio` uses `f32` precision (Java float) so the weight
//!   values match exactly for the canonical test cases (e.g. `Peers(1, 1)`
//!   weight = `25.0`, `Peers(2000, 2000)` weight = `50000.0`).
//! * Weight allocation is `global_speed * weight_for_torrent / total_weight`
//!   truncated to an integer, matching Java's `(long)` cast.
//! * Stats accumulation uses `current_speed * tick_ms / 1000` — tick-driven,
//!   not wall-clock, deliberately so tests can drive the dispatcher without
//!   sleeping.
//! * The global upload/download speeds evolve via a **per-tick reflected
//!   random walk** ([`random_speed::RandomSpeedProvider::step`]) instead of
//!   Java's "re-sample every 20 minutes and hold flat in between". Long-term
//!   mean is preserved, but the instantaneous value is no longer frozen
//!   between tracker announces.

pub mod dispatcher;
pub mod peers;
pub mod random_speed;
pub mod speed;
pub mod stats;
pub mod weight;

pub use dispatcher::{BandwidthDispatcher, BandwidthError};
pub use peers::Peers;
pub use random_speed::{DownloadSpeedProvider, RandomSpeedProvider, RandomSpeedSource};
pub use speed::Speed;
pub use stats::{DownloadEdge, TorrentSeedStats};
pub use weight::{PeersAwareWeightCalculator, WeightHolder};
