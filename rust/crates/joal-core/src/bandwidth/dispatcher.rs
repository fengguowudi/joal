//! Periodic bandwidth dispatcher.
//!
//! Port of Java `org.araymond.joal.core.bandwith.BandwidthDispatcher`.
//!
//! On every tick the dispatcher:
//! 1. counts against a 20-minute refresh boundary and, when the boundary is
//!    reached, re-samples the global upload budget via
//!    [`RandomSpeedProvider::refresh`] and reshards it across torrents;
//! 2. credits each registered torrent with `current_speed * tick_ms / 1000`
//!    into its [`TorrentSeedStats::uploaded`] counter.
//!
//! # Concurrency model
//!
//! Java guards state with a `ReentrantReadWriteLock`; this port uses a plain
//! `std::sync::Mutex` because every critical section is a cheap map
//! operation, never crosses `.await`, and the stats hot-path (`get_seed_stat_for_torrent`)
//! only ever needs a short read-and-copy. A tokio task driven by
//! `tokio::time::interval` replaces the bespoke `Thread.sleep` loop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use thiserror::Error;
use tokio::task::JoinHandle;
use tokio::time::{self, MissedTickBehavior};
use tracing::debug;

use crate::bandwidth::peers::Peers;
use crate::bandwidth::random_speed::RandomSpeedProvider;
use crate::bandwidth::speed::Speed;
use crate::bandwidth::stats::TorrentSeedStats;
use crate::bandwidth::weight::{PeersAwareWeightCalculator, WeightHolder};
use crate::torrent::InfoHash;

/// Java `TWENTY_MINS_MS = MINUTES.toMillis(20)` — how often the global
/// bandwidth budget is re-sampled from [`RandomSpeedProvider`].
#[allow(clippy::duration_suboptimal_units)]
pub const DEFAULT_BANDWIDTH_REFRESH_INTERVAL: Duration = Duration::from_secs(20 * 60);

/// Fired after every speed recomputation (torrent register / unregister,
/// peer-count update, or a 20-minute global refresh).
///
/// Port of Java `SpeedChangedListener`. Listeners are invoked synchronously
/// with the dispatcher's mutex held — do not call back into the dispatcher
/// from inside the callback.
pub trait SpeedChangedListener: Send + Sync {
    fn speeds_has_changed(&self, speeds: &HashMap<InfoHash, Speed>);
}

/// Lifecycle errors for [`BandwidthDispatcher::start`] / [`BandwidthDispatcher::stop`].
#[derive(Debug, Error)]
pub enum BandwidthError {
    #[error("bandwidth dispatcher is already running")]
    AlreadyRunning,
    #[error("bandwidth dispatcher is not running")]
    NotRunning,
}

struct Inner {
    weight_holder: WeightHolder<InfoHash>,
    torrents_seed_stats: HashMap<InfoHash, TorrentSeedStats>,
    speed_map: HashMap<InfoHash, Speed>,
    random_speed_provider: RandomSpeedProvider,
    tick_counter: u64,
    ticks_per_refresh: u64,
    listener: Option<Arc<dyn SpeedChangedListener>>,
}

impl Inner {
    fn recompute_speeds(&mut self) {
        let total = self.weight_holder.total_weight();
        let current_global = self.random_speed_provider.current_speed();
        let weight_holder = &self.weight_holder;
        for (info_hash, speed) in &mut self.speed_map {
            let weight = weight_holder.weight_for(info_hash);
            let assigned = if total == 0.0 {
                0
            } else {
                #[allow(clippy::cast_precision_loss)]
                let global = current_global as f64;
                (global * weight / total) as u64
            };
            speed.set_bytes_per_second(assigned);
        }

        if let Some(listener) = self.listener.as_ref() {
            let snapshot = self.speed_map.clone();
            listener.speeds_has_changed(&snapshot);
        }
    }

    fn accumulate_uploaded(&mut self, tick_ms: u64) {
        let speed_map = &self.speed_map;
        for (info_hash, stats) in &mut self.torrents_seed_stats {
            let bytes_per_sec = speed_map.get(info_hash).map_or(0, Speed::bytes_per_second);
            let tick_bytes = bytes_per_sec.saturating_mul(tick_ms) / 1000;
            stats.add_uploaded(tick_bytes);
        }
    }
}

/// Handle for the bandwidth dispatcher task.
///
/// The handle is deliberately not `Clone`: lifecycle (start/stop) is
/// single-owner, while read-only observers should use the snapshot methods
/// (`speed_map`, `get_seed_stat_for_torrent`), which take `&self`.
pub struct BandwidthDispatcher {
    inner: Arc<Mutex<Inner>>,
    tick_period: Duration,
    task: Option<JoinHandle<()>>,
}

impl BandwidthDispatcher {
    /// Build a dispatcher using the default 20-minute refresh cadence.
    #[must_use]
    pub fn new(tick_period: Duration, random_speed_provider: RandomSpeedProvider) -> Self {
        Self::with_refresh_interval(
            tick_period,
            random_speed_provider,
            DEFAULT_BANDWIDTH_REFRESH_INTERVAL,
        )
    }

    /// Explicit refresh cadence — useful for tests that want to verify the
    /// 20-minute boundary without running for 20 minutes.
    #[must_use]
    pub fn with_refresh_interval(
        tick_period: Duration,
        random_speed_provider: RandomSpeedProvider,
        refresh_interval: Duration,
    ) -> Self {
        let ticks_per_refresh = compute_ticks_per_refresh(tick_period, refresh_interval);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                weight_holder: WeightHolder::new(PeersAwareWeightCalculator::new()),
                torrents_seed_stats: HashMap::new(),
                speed_map: HashMap::new(),
                random_speed_provider,
                tick_counter: 0,
                ticks_per_refresh,
                listener: None,
            })),
            tick_period,
            task: None,
        }
    }

    pub fn set_speed_listener(&self, listener: Arc<dyn SpeedChangedListener>) {
        self.with_lock(|inner| inner.listener = Some(listener));
    }

    pub fn register_torrent(&self, info_hash: InfoHash) {
        debug!(info_hash = %info_hash.to_hex(), "registering torrent with bandwidth dispatcher");
        self.with_lock(|inner| {
            inner
                .torrents_seed_stats
                .insert(info_hash.clone(), TorrentSeedStats::default());
            inner.speed_map.insert(info_hash, Speed::new(0));
        });
    }

    pub fn unregister_torrent(&self, info_hash: &InfoHash) {
        debug!(info_hash = %info_hash.to_hex(), "unregistering torrent from bandwidth dispatcher");
        self.with_lock(|inner| {
            inner.weight_holder.remove(info_hash);
            inner.torrents_seed_stats.remove(info_hash);
            inner.speed_map.remove(info_hash);
            inner.recompute_speeds();
        });
    }

    pub fn update_torrent_peers(&self, info_hash: InfoHash, seeders: u32, leechers: u32) {
        debug!(
            info_hash = %info_hash.to_hex(),
            seeders, leechers,
            "updating torrent peers",
        );
        self.with_lock(|inner| {
            inner
                .weight_holder
                .add_or_update(info_hash, &Peers::new(seeders, leechers));
            inner.recompute_speeds();
        });
    }

    #[must_use]
    pub fn get_seed_stat_for_torrent(&self, info_hash: &InfoHash) -> TorrentSeedStats {
        self.with_lock(|inner| {
            inner
                .torrents_seed_stats
                .get(info_hash)
                .copied()
                .unwrap_or_default()
        })
    }

    #[must_use]
    pub fn speed_map(&self) -> HashMap<InfoHash, Speed> {
        self.with_lock(|inner| inner.speed_map.clone())
    }

    /// Java `refreshCurrentBandwidth` — re-sample and recompute immediately.
    /// Used by the 20-minute tick boundary and exposed for tests.
    pub fn refresh_current_bandwidth(&self) {
        self.with_lock(|inner| {
            inner.random_speed_provider.refresh();
            inner.recompute_speeds();
        });
    }

    /// Spawn the background scheduler. The task runs until [`Self::stop`] is
    /// called or the dispatcher is dropped.
    pub fn start(&mut self) -> Result<(), BandwidthError> {
        if self.task.is_some() {
            return Err(BandwidthError::AlreadyRunning);
        }
        let inner = Arc::clone(&self.inner);
        let tick_period = self.tick_period;
        let tick_ms = u64::try_from(tick_period.as_millis()).unwrap_or(u64::MAX);
        let handle = tokio::spawn(async move {
            let mut interval = time::interval(tick_period);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Java starts with `MILLISECONDS.sleep(threadPauseIntervalMs)`
            // — i.e. the first action is a wait, not an immediate tick. The
            // first `interval.tick()` fires immediately so we drop it.
            interval.tick().await;
            loop {
                interval.tick().await;
                on_tick(&inner, tick_ms);
            }
        });
        self.task = Some(handle);
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), BandwidthError> {
        let handle = self.task.take().ok_or(BandwidthError::NotRunning)?;
        handle.abort();
        // JoinError on abort is expected; any other variant means the task
        // panicked — we surface nothing and return cleanly either way.
        let _ = handle.await;
        Ok(())
    }

    /// Run exactly one dispatcher tick synchronously. Reserved for tests
    /// (deterministic scheduling beats `tokio::time::pause` for pure-state
    /// assertions).
    #[doc(hidden)]
    pub fn tick_once_for_test(&self) {
        let tick_ms = u64::try_from(self.tick_period.as_millis()).unwrap_or(u64::MAX);
        on_tick(&self.inner, tick_ms);
    }

    fn with_lock<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }
}

impl std::fmt::Debug for BandwidthDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BandwidthDispatcher")
            .field("tick_period", &self.tick_period)
            .field("running", &self.task.is_some())
            .finish_non_exhaustive()
    }
}

impl Drop for BandwidthDispatcher {
    fn drop(&mut self) {
        if let Some(handle) = self.task.take() {
            handle.abort();
        }
    }
}

fn on_tick(inner: &Mutex<Inner>, tick_ms: u64) {
    let mut guard = inner.lock().unwrap_or_else(PoisonError::into_inner);
    guard.tick_counter = guard.tick_counter.wrapping_add(1);
    if guard.tick_counter >= guard.ticks_per_refresh {
        guard.tick_counter = 0;
        guard.random_speed_provider.refresh();
        guard.recompute_speeds();
        debug!("bandwidth dispatcher refreshed global bandwidth");
    }
    guard.accumulate_uploaded(tick_ms);
}

fn compute_ticks_per_refresh(tick_period: Duration, refresh_interval: Duration) -> u64 {
    let tick_ms = u64::try_from(tick_period.as_millis()).unwrap_or(u64::MAX);
    let refresh_ms = u64::try_from(refresh_interval.as_millis()).unwrap_or(u64::MAX);
    if tick_ms == 0 {
        return 1;
    }
    (refresh_ms / tick_ms).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::bandwidth::random_speed::{RandomSpeedProvider, RandomSpeedSource};
    use crate::config::AppConfiguration;

    fn cfg(min_kib: u64, max_kib: u64) -> AppConfiguration {
        AppConfiguration {
            min_upload_rate: min_kib,
            max_upload_rate: max_kib,
            simultaneous_seed: 1,
            client: "x.client".into(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
        }
    }

    #[derive(Debug)]
    struct Constant(u64);
    impl RandomSpeedSource for Constant {
        fn sample(&self, _min: u64, _max: u64) -> u64 {
            self.0
        }
    }

    fn hash(byte: u8) -> InfoHash {
        InfoHash::from_bytes([byte; 20])
    }

    fn dispatcher(bytes_per_sec: u64) -> BandwidthDispatcher {
        let provider =
            RandomSpeedProvider::with_source(&cfg(100, 200), Box::new(Constant(bytes_per_sec)));
        BandwidthDispatcher::new(Duration::from_secs(1), provider)
    }

    #[test]
    fn register_inserts_zero_speed_and_default_stats() {
        let d = dispatcher(1_000);
        let h = hash(1);
        d.register_torrent(h.clone());
        assert_eq!(d.speed_map().get(&h).copied(), Some(Speed::new(0)));
        assert_eq!(d.get_seed_stat_for_torrent(&h).uploaded(), 0);
    }

    #[test]
    fn weights_split_global_speed_proportionally() {
        let d = dispatcher(1_000_000);
        let a = hash(1);
        let b = hash(2);
        d.register_torrent(a.clone());
        d.register_torrent(b.clone());
        d.update_torrent_peers(a.clone(), 10, 10);
        d.update_torrent_peers(b.clone(), 10, 30);

        // Weights (from PeersAwareWeightCalculator): a -> 250.0, b -> 1687.5.
        let total_weight = 250.0 + 1687.5;
        let expected_a = (1_000_000.0 * 250.0 / total_weight) as u64;
        let expected_b = (1_000_000.0 * 1687.5 / total_weight) as u64;
        let speeds = d.speed_map();
        assert_eq!(speeds.get(&a).unwrap().bytes_per_second(), expected_a);
        assert_eq!(speeds.get(&b).unwrap().bytes_per_second(), expected_b);
    }

    #[test]
    fn zero_total_weight_gives_zero_speed() {
        let d = dispatcher(1_000_000);
        let a = hash(1);
        d.register_torrent(a.clone());
        // No update_torrent_peers — weight_holder is empty, total_weight = 0.
        assert_eq!(d.speed_map().get(&a).unwrap().bytes_per_second(), 0);
    }

    #[test]
    fn unregister_recomputes_for_remaining_torrents() {
        let d = dispatcher(1_000_000);
        let a = hash(1);
        let b = hash(2);
        d.register_torrent(a.clone());
        d.register_torrent(b.clone());
        d.update_torrent_peers(a.clone(), 10, 10);
        d.update_torrent_peers(b.clone(), 10, 30);
        d.unregister_torrent(&a);

        let speeds = d.speed_map();
        assert!(!speeds.contains_key(&a));
        // `b` now absorbs the entire budget (truncation to integer).
        assert_eq!(speeds.get(&b).unwrap().bytes_per_second(), 1_000_000);
    }

    #[test]
    fn tick_accumulates_uploaded_using_current_speed() {
        let d = dispatcher(500_000);
        let a = hash(1);
        d.register_torrent(a.clone());
        d.update_torrent_peers(a.clone(), 10, 10); // 100% of budget (only torrent).

        // tick_period = 1000ms, speed = 500_000 B/s -> 500_000 B per tick.
        d.tick_once_for_test();
        assert_eq!(d.get_seed_stat_for_torrent(&a).uploaded(), 500_000);
        d.tick_once_for_test();
        assert_eq!(d.get_seed_stat_for_torrent(&a).uploaded(), 1_000_000);
    }

    #[test]
    fn zero_weight_torrents_accumulate_nothing() {
        let d = dispatcher(500_000);
        let a = hash(1);
        d.register_torrent(a.clone());
        // No peer update -> weight_for = 0 -> speed = 0 -> uploaded stays 0.
        d.tick_once_for_test();
        d.tick_once_for_test();
        assert_eq!(d.get_seed_stat_for_torrent(&a).uploaded(), 0);
    }

    #[test]
    fn tick_counter_triggers_refresh_at_boundary() {
        #[derive(Debug)]
        struct Seq {
            idx: AtomicUsize,
            values: Vec<u64>,
        }
        impl RandomSpeedSource for Seq {
            fn sample(&self, _: u64, _: u64) -> u64 {
                let i = self.idx.fetch_add(1, Ordering::SeqCst);
                *self
                    .values
                    .get(i)
                    .unwrap_or_else(|| self.values.last().expect("non-empty"))
            }
        }

        let source = Box::new(Seq {
            idx: AtomicUsize::new(0),
            values: vec![1_000, 2_000, 3_000],
        });
        let provider = RandomSpeedProvider::with_source(&cfg(100, 200), source);
        let d = BandwidthDispatcher::with_refresh_interval(
            Duration::from_secs(1),
            provider,
            Duration::from_secs(2), // refresh every 2 ticks
        );
        let a = hash(1);
        d.register_torrent(a.clone());
        d.update_torrent_peers(a.clone(), 10, 10);

        // Initial provider refresh during construction pulled the first value.
        assert_eq!(d.speed_map().get(&a).unwrap().bytes_per_second(), 1_000);

        d.tick_once_for_test(); // counter=1 < 2, no refresh
        assert_eq!(d.speed_map().get(&a).unwrap().bytes_per_second(), 1_000);

        d.tick_once_for_test(); // counter=2, refresh -> value 2_000
        assert_eq!(d.speed_map().get(&a).unwrap().bytes_per_second(), 2_000);

        d.tick_once_for_test(); // counter=1 again
        assert_eq!(d.speed_map().get(&a).unwrap().bytes_per_second(), 2_000);

        d.tick_once_for_test(); // counter=2, refresh -> value 3_000
        assert_eq!(d.speed_map().get(&a).unwrap().bytes_per_second(), 3_000);
    }

    #[test]
    fn speed_listener_is_fired_on_each_recompute() {
        #[derive(Default)]
        struct Counter {
            snapshots: StdMutex<Vec<HashMap<InfoHash, Speed>>>,
        }
        impl SpeedChangedListener for Counter {
            fn speeds_has_changed(&self, speeds: &HashMap<InfoHash, Speed>) {
                self.snapshots
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(speeds.clone());
            }
        }

        let d = dispatcher(1_000_000);
        let counter = Arc::new(Counter::default());
        d.set_speed_listener(counter.clone());

        let a = hash(1);
        d.register_torrent(a.clone()); // no recompute on bare register
        d.update_torrent_peers(a.clone(), 10, 10); // recompute #1
        d.refresh_current_bandwidth(); // recompute #2
        d.unregister_torrent(&a); // recompute #3

        let snapshots = counter.snapshots.lock().unwrap();
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].len(), 1);
        assert_eq!(snapshots[2].len(), 0);
    }

    #[tokio::test]
    async fn double_start_returns_error() {
        let mut d = dispatcher(1_000);
        d.start().expect("first start");
        assert!(matches!(d.start(), Err(BandwidthError::AlreadyRunning)));
        d.stop().await.expect("stop");
    }

    #[tokio::test]
    async fn stop_without_start_returns_error() {
        let mut d = dispatcher(1_000);
        assert!(matches!(d.stop().await, Err(BandwidthError::NotRunning)));
    }

    #[tokio::test]
    async fn start_and_stop_completes_cleanly() {
        let mut d = dispatcher(100_000);
        d.start().expect("start");
        d.stop().await.expect("stop");
        assert!(d.task.is_none());
    }

    #[test]
    #[allow(clippy::duration_suboptimal_units)]
    fn compute_ticks_per_refresh_matches_java_division() {
        // Java: TWENTY_MINS_MS (1_200_000) / threadPauseIntervalMs (1000) = 1200.
        assert_eq!(
            compute_ticks_per_refresh(Duration::from_secs(1), Duration::from_secs(20 * 60)),
            1200,
        );
        assert_eq!(
            compute_ticks_per_refresh(Duration::from_millis(500), Duration::from_secs(10)),
            20,
        );
        // Tick >= refresh: clamp to 1 to avoid zero-divide "never refresh" bug.
        assert_eq!(
            compute_ticks_per_refresh(Duration::from_secs(30), Duration::from_secs(10)),
            1,
        );
        // Zero tick period: defensive default.
        assert_eq!(
            compute_ticks_per_refresh(Duration::from_secs(0), Duration::from_secs(10)),
            1,
        );
    }
}
