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
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{self, MissedTickBehavior};
use tracing::debug;

use crate::bandwidth::peers::Peers;
use crate::bandwidth::random_speed::{DownloadSpeedProvider, RandomSpeedProvider};
use crate::bandwidth::speed::Speed;
use crate::bandwidth::stats::{DownloadEdge, TorrentSeedStats};
use crate::bandwidth::weight::{PeersAwareWeightCalculator, WeightHolder};
use crate::snapshot::MergerPoke;
use crate::torrent::InfoHash;

/// Java `TWENTY_MINS_MS = MINUTES.toMillis(20)` — how often the global
/// bandwidth budget is re-sampled from [`RandomSpeedProvider`].
#[allow(clippy::duration_suboptimal_units)]
pub const DEFAULT_BANDWIDTH_REFRESH_INTERVAL: Duration = Duration::from_secs(20 * 60);

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
    /// Total size in bytes per torrent, used to cap simulated `downloaded`.
    total_sizes: HashMap<InfoHash, u64>,
    /// Per-torrent slice of the global *upload* budget.
    speed_map: HashMap<InfoHash, Speed>,
    /// Per-torrent slice of the global *download* budget. Only torrents
    /// whose `downloaded < total_size` get a non-zero allocation.
    download_speed_map: HashMap<InfoHash, Speed>,
    random_speed_provider: RandomSpeedProvider,
    download_speed_provider: DownloadSpeedProvider,
    tick_counter: u64,
    ticks_per_refresh: u64,
    poke: Option<mpsc::Sender<MergerPoke>>,
}

impl Inner {
    fn recompute_speeds(&mut self) {
        let total = self.weight_holder.total_weight();

        // Upload budget: every registered torrent participates.
        let global_up = self.random_speed_provider.current_speed();
        let weight_holder = &self.weight_holder;
        for (info_hash, speed) in &mut self.speed_map {
            let weight = weight_holder.weight_for(info_hash);
            let assigned = if total == 0.0 {
                0
            } else {
                #[allow(clippy::cast_precision_loss)]
                let global = global_up as f64;
                (global * weight / total) as u64
            };
            speed.set_bytes_per_second(assigned);
        }

        // Download budget: only torrents that still have bytes to "download"
        // (downloaded < total_size) participate. Their weights get re-summed
        // so the budget is not silently lost on completed torrents.
        let global_down = if self.download_speed_provider.is_enabled() {
            self.download_speed_provider.current_speed()
        } else {
            0
        };
        let stats = &self.torrents_seed_stats;
        let totals = &self.total_sizes;
        let active_total: f64 = self
            .download_speed_map
            .keys()
            .filter(|h| {
                let st = stats.get(*h).copied().unwrap_or_default();
                let cap = totals.get(*h).copied().unwrap_or(0);
                cap > 0 && st.downloaded() < cap
            })
            .map(|h| weight_holder.weight_for(h))
            .sum();
        for (info_hash, speed) in &mut self.download_speed_map {
            let st = stats.get(info_hash).copied().unwrap_or_default();
            let cap = totals.get(info_hash).copied().unwrap_or(0);
            let assigned = if global_down == 0 || active_total == 0.0 || cap == 0 || st.downloaded() >= cap {
                0
            } else {
                #[allow(clippy::cast_precision_loss)]
                let global = global_down as f64;
                let weight = weight_holder.weight_for(info_hash);
                (global * weight / active_total) as u64
            };
            speed.set_bytes_per_second(assigned);
        }

        if let Some(sender) = self.poke.as_ref() {
            // try_send is intentional: the merger task rebuilds the whole
            // snapshot on every wake-up, so a dropped poke when
            // the queue is full is safe — it collapses into the next one.
            let _ = sender.try_send(MergerPoke::SpeedRecomputed);
        }
    }

    fn accumulate_traffic(&mut self, tick_ms: u64) {
        let mut completed: Vec<InfoHash> = Vec::new();
        let speed_map = &self.speed_map;
        let download_speed_map = &self.download_speed_map;
        let totals = &self.total_sizes;
        for (info_hash, stats) in &mut self.torrents_seed_stats {
            // Upload — same math as before.
            let up_bps = speed_map.get(info_hash).map_or(0, Speed::bytes_per_second);
            let up_bytes = up_bps.saturating_mul(tick_ms) / 1000;
            stats.add_uploaded(up_bytes);

            // Download — only credit if the faker is actually allocating.
            let dl_bps = download_speed_map
                .get(info_hash)
                .map_or(0, Speed::bytes_per_second);
            if dl_bps == 0 {
                continue;
            }
            let cap = totals.get(info_hash).copied().unwrap_or(0);
            if cap == 0 {
                continue;
            }
            let dl_bytes = dl_bps.saturating_mul(tick_ms) / 1000;
            if matches!(
                stats.add_downloaded(dl_bytes, cap),
                DownloadEdge::JustCompleted
            ) {
                completed.push(info_hash.clone());
            }
        }

        if !completed.is_empty() {
            // Recompute so the freshly-completed torrents stop drawing from
            // the download budget (their weight is removed from the active
            // sum next call).
            self.recompute_speeds();
            if let Some(sender) = self.poke.as_ref() {
                for h in completed {
                    let _ = sender.try_send(MergerPoke::TorrentCompleted(h));
                }
            }
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
    task: Mutex<Option<JoinHandle<()>>>,
}

impl BandwidthDispatcher {
    /// Build a dispatcher using the default 20-minute refresh cadence.
    #[must_use]
    pub fn new(
        tick_period: Duration,
        random_speed_provider: RandomSpeedProvider,
        download_speed_provider: DownloadSpeedProvider,
    ) -> Self {
        Self::with_refresh_interval(
            tick_period,
            random_speed_provider,
            download_speed_provider,
            DEFAULT_BANDWIDTH_REFRESH_INTERVAL,
        )
    }

    /// Explicit refresh cadence — useful for tests that want to verify the
    /// 20-minute boundary without running for 20 minutes.
    #[must_use]
    pub fn with_refresh_interval(
        tick_period: Duration,
        random_speed_provider: RandomSpeedProvider,
        download_speed_provider: DownloadSpeedProvider,
        refresh_interval: Duration,
    ) -> Self {
        let ticks_per_refresh = compute_ticks_per_refresh(tick_period, refresh_interval);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                weight_holder: WeightHolder::new(PeersAwareWeightCalculator::new()),
                torrents_seed_stats: HashMap::new(),
                total_sizes: HashMap::new(),
                speed_map: HashMap::new(),
                download_speed_map: HashMap::new(),
                random_speed_provider,
                download_speed_provider,
                tick_counter: 0,
                ticks_per_refresh,
                poke: None,
            })),
            tick_period,
            task: Mutex::new(None),
        }
    }

    /// Wire the merger-task mailbox used by [`SeedManager`][crate::seed_manager].
    ///
    /// Passing `None` clears the hook (tests + library-only consumers that
    /// don't run a merger). The dispatcher never blocks on the channel —
    /// sends are `try_send`; a full queue is silently dropped because the
    /// next poke will re-merge the latest state anyway.
    pub fn set_merger_poke(&self, poke: Option<mpsc::Sender<MergerPoke>>) {
        self.with_lock(|inner| inner.poke = poke);
    }

    pub fn register_torrent(
        &self,
        info_hash: InfoHash,
        total_size: u64,
        initial_completed: bool,
    ) {
        debug!(
            info_hash = %info_hash.to_hex(),
            total_size, initial_completed,
            "registering torrent with bandwidth dispatcher",
        );
        self.with_lock(|inner| {
            let stats = if initial_completed {
                TorrentSeedStats::completed(total_size)
            } else {
                TorrentSeedStats::fresh(total_size)
            };
            inner.torrents_seed_stats.insert(info_hash.clone(), stats);
            inner.total_sizes.insert(info_hash.clone(), total_size);
            inner.speed_map.insert(info_hash.clone(), Speed::new(0));
            inner.download_speed_map.insert(info_hash, Speed::new(0));
        });
    }

    pub fn unregister_torrent(&self, info_hash: &InfoHash) {
        debug!(info_hash = %info_hash.to_hex(), "unregistering torrent from bandwidth dispatcher");
        self.with_lock(|inner| {
            inner.weight_holder.remove(info_hash);
            inner.torrents_seed_stats.remove(info_hash);
            inner.total_sizes.remove(info_hash);
            inner.speed_map.remove(info_hash);
            inner.download_speed_map.remove(info_hash);
            inner.recompute_speeds();
        });
    }

    /// Force-mark a torrent as already finished (downloaded == total_size).
    /// Triggered from the UI checkbox at runtime. Returns whether the change
    /// actually flipped the completed flag — callers (the orchestrator) can
    /// use that to decide whether to fire an `event=completed` announce.
    pub fn force_initial_completed(&self, info_hash: &InfoHash, completed: bool) -> bool {
        self.with_lock(|inner| {
            let cap = inner.total_sizes.get(info_hash).copied().unwrap_or(0);
            let Some(stats) = inner.torrents_seed_stats.get_mut(info_hash) else {
                return false;
            };
            let was_done = stats.left() == 0 && stats.downloaded() == cap && cap > 0;
            if completed {
                stats.force_completed(cap);
            } else {
                stats.reset_download(cap);
            }
            inner.recompute_speeds();
            completed && !was_done
        })
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

    /// Per-torrent slice of the *download* budget, mirroring [`Self::speed_map`].
    /// Always returns 0 entries when the download faker is disabled (0/0).
    #[must_use]
    pub fn download_speed_map(&self) -> HashMap<InfoHash, Speed> {
        self.with_lock(|inner| inner.download_speed_map.clone())
    }

    /// Hot-update the speed-provider limits from a (possibly reloaded)
    /// configuration. Caller is expected to call [`Self::refresh_current_bandwidth`]
    /// afterwards if it wants the new bounds to take effect immediately.
    pub fn update_limits(&self, config: &crate::config::AppConfiguration) {
        self.with_lock(|inner| {
            inner.random_speed_provider.update_limits(config);
            inner.download_speed_provider.update_limits(config);
        });
    }

    /// Java `refreshCurrentBandwidth` — re-sample and recompute immediately.
    /// Used by the 20-minute tick boundary and exposed for tests.
    pub fn refresh_current_bandwidth(&self) {
        self.with_lock(|inner| {
            inner.random_speed_provider.refresh();
            inner.download_speed_provider.refresh();
            inner.recompute_speeds();
        });
    }

    /// Spawn the background scheduler. The task runs until [`Self::stop`] is
    /// called or the dispatcher is dropped.
    pub fn start(&self) -> Result<(), BandwidthError> {
        let mut task_slot = self.task.lock().unwrap_or_else(PoisonError::into_inner);
        if task_slot.is_some() {
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
        *task_slot = Some(handle);
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), BandwidthError> {
        let handle = {
            let mut task_slot = self.task.lock().unwrap_or_else(PoisonError::into_inner);
            task_slot.take().ok_or(BandwidthError::NotRunning)?
        };
        handle.abort();
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
        let running = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        f.debug_struct("BandwidthDispatcher")
            .field("tick_period", &self.tick_period)
            .field("running", &running)
            .finish_non_exhaustive()
    }
}

impl Drop for BandwidthDispatcher {
    fn drop(&mut self) {
        let task = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = task {
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
        guard.download_speed_provider.refresh();
        guard.recompute_speeds();
        debug!("bandwidth dispatcher refreshed global bandwidth");
    }
    guard.accumulate_traffic(tick_ms);
    if let Some(sender) = guard.poke.as_ref() {
        // try_send: a dropped poke collapses into the next one because
        // the merger always rebuilds from current state. On a 20-min
        // boundary tick this is the second send (recompute_speeds
        // already pushed one); the merger coalesces, so don't dedupe.
        let _ = sender.try_send(MergerPoke::SpeedRecomputed);
    }
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

    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::bandwidth::random_speed::{
        DownloadSpeedProvider, RandomSpeedProvider, RandomSpeedSource,
    };
    use crate::config::AppConfiguration;

    fn cfg(min_kib: u64, max_kib: u64) -> AppConfiguration {
        AppConfiguration {
            min_upload_rate: min_kib,
            max_upload_rate: max_kib,
            min_download_rate: 0,
            max_download_rate: 0,
            simultaneous_seed: 1,
            client: "x.client".into(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
            proxy_host: None,
            proxy_port: None,
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
        let dl = DownloadSpeedProvider::with_source(&cfg(100, 200), Box::new(Constant(0)));
        BandwidthDispatcher::new(Duration::from_secs(1), provider, dl)
    }

    /// Build a dispatcher whose download faker is enabled and returns
    /// `dl_bytes_per_sec` from a constant source. Caller still controls the
    /// upload constant via `up_bytes_per_sec`.
    fn dispatcher_with_download(up_bytes_per_sec: u64, dl_bytes_per_sec: u64) -> BandwidthDispatcher {
        let upload =
            RandomSpeedProvider::with_source(&cfg(100, 200), Box::new(Constant(up_bytes_per_sec)));
        let download = DownloadSpeedProvider::with_source(
            &cfg_with_download(50, 100),
            Box::new(Constant(dl_bytes_per_sec)),
        );
        BandwidthDispatcher::new(Duration::from_secs(1), upload, download)
    }

    fn cfg_with_download(min_dl: u64, max_dl: u64) -> AppConfiguration {
        AppConfiguration {
            min_upload_rate: 0,
            max_upload_rate: 0,
            min_download_rate: min_dl,
            max_download_rate: max_dl,
            simultaneous_seed: 1,
            client: "x.client".into(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
            proxy_host: None,
            proxy_port: None,
        }
    }

    #[test]
    fn register_inserts_zero_speed_and_default_stats() {
        let d = dispatcher(1_000);
        let h = hash(1);
        d.register_torrent(h.clone(), 0, false);
        assert_eq!(d.speed_map().get(&h).copied(), Some(Speed::new(0)));
        assert_eq!(d.get_seed_stat_for_torrent(&h).uploaded(), 0);
    }

    #[test]
    fn weights_split_global_speed_proportionally() {
        let d = dispatcher(1_000_000);
        let a = hash(1);
        let b = hash(2);
        d.register_torrent(a.clone(), 0, false);
        d.register_torrent(b.clone(), 0, false);
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
        d.register_torrent(a.clone(), 0, false);
        // No update_torrent_peers — weight_holder is empty, total_weight = 0.
        assert_eq!(d.speed_map().get(&a).unwrap().bytes_per_second(), 0);
    }

    #[test]
    fn unregister_recomputes_for_remaining_torrents() {
        let d = dispatcher(1_000_000);
        let a = hash(1);
        let b = hash(2);
        d.register_torrent(a.clone(), 0, false);
        d.register_torrent(b.clone(), 0, false);
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
        d.register_torrent(a.clone(), 0, false);
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
        d.register_torrent(a.clone(), 0, false);
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
        let dl = DownloadSpeedProvider::with_source(&cfg(100, 200), Box::new(Constant(0)));
        let d = BandwidthDispatcher::with_refresh_interval(
            Duration::from_secs(1),
            provider,
            dl,
            Duration::from_secs(2), // refresh every 2 ticks
        );
        let a = hash(1);
        d.register_torrent(a.clone(), 0, false);
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

    #[tokio::test]
    async fn merger_poke_fires_on_each_recompute() {
        let d = dispatcher(1_000_000);
        let (tx, mut rx) = mpsc::channel(16);
        d.set_merger_poke(Some(tx));

        let a = hash(1);
        d.register_torrent(a.clone(), 0, false); // no recompute on bare register
        d.update_torrent_peers(a.clone(), 10, 10); // recompute #1
        d.refresh_current_bandwidth(); // recompute #2
        d.unregister_torrent(&a); // recompute #3

        let mut pokes = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            pokes.push(msg);
        }
        assert_eq!(pokes.len(), 3);
        assert!(
            pokes
                .iter()
                .all(|p| matches!(p, MergerPoke::SpeedRecomputed))
        );
    }

    #[tokio::test]
    async fn tick_pokes_merger_for_live_uploaded_refresh() {
        // Default 20-min refresh interval -> 1200 ticks per refresh; three
        // ticks below cannot cross the boundary, so each poke is from the
        // post-accumulate path, not the boundary recompute.
        let d = dispatcher(500_000);
        let a = hash(1);
        d.register_torrent(a.clone(), 0, false);
        d.update_torrent_peers(a.clone(), 10, 10);

        let (tx, mut rx) = mpsc::channel(16);
        d.set_merger_poke(Some(tx));

        // Drain anything that snuck in between channel attach and now.
        while rx.try_recv().is_ok() {}

        d.tick_once_for_test();
        d.tick_once_for_test();
        d.tick_once_for_test();

        let mut pokes = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            pokes.push(msg);
        }
        assert_eq!(pokes.len(), 3, "expected one poke per tick");
        assert!(
            pokes
                .iter()
                .all(|p| matches!(p, MergerPoke::SpeedRecomputed))
        );
        // Sanity: uploaded actually advanced this entire time.
        assert_eq!(d.get_seed_stat_for_torrent(&a).uploaded(), 1_500_000);
    }

    #[test]
    fn tick_without_poke_subscriber_does_not_panic() {
        let d = dispatcher(500_000);
        let a = hash(1);
        d.register_torrent(a.clone(), 0, false);
        d.update_torrent_peers(a.clone(), 10, 10);
        // No set_merger_poke call: inner.poke stays None.

        d.tick_once_for_test();
        d.tick_once_for_test();

        assert_eq!(d.get_seed_stat_for_torrent(&a).uploaded(), 1_000_000);
    }

    #[test]
    fn register_initial_completed_starts_at_total_size() {
        let d = dispatcher_with_download(0, 0); // upload off, download enabled bounds
        let h = hash(0xaa);
        d.register_torrent(h.clone(), 5_000, true);
        let stats = d.get_seed_stat_for_torrent(&h);
        assert_eq!(stats.downloaded(), 5_000);
        assert_eq!(stats.left(), 0);
        assert!(stats.is_completed());
    }

    #[test]
    fn accumulate_traffic_credits_download_and_caps() {
        let d = dispatcher_with_download(0, 200_000);
        let a = hash(1);
        d.register_torrent(a.clone(), 500_000, false);
        d.update_torrent_peers(a.clone(), 10, 10);

        // 200_000 B/s * 1s = 200_000 per tick. After 2 ticks => 400_000.
        d.tick_once_for_test();
        assert_eq!(d.get_seed_stat_for_torrent(&a).downloaded(), 200_000);
        assert_eq!(d.get_seed_stat_for_torrent(&a).left(), 300_000);
        d.tick_once_for_test();
        assert_eq!(d.get_seed_stat_for_torrent(&a).downloaded(), 400_000);

        // Tick 3 should overshoot by 100_000 — must clamp at 500_000.
        d.tick_once_for_test();
        assert_eq!(d.get_seed_stat_for_torrent(&a).downloaded(), 500_000);
        assert_eq!(d.get_seed_stat_for_torrent(&a).left(), 0);
    }

    #[tokio::test]
    async fn completion_emits_torrent_completed_poke_exactly_once() {
        let d = dispatcher_with_download(0, 1_000_000);
        let a = hash(1);
        d.register_torrent(a.clone(), 1_000_000, false);
        d.update_torrent_peers(a.clone(), 10, 10);

        let (tx, mut rx) = mpsc::channel(16);
        d.set_merger_poke(Some(tx));
        while rx.try_recv().is_ok() {}

        // Tick #1: caps exactly at total_size -> JustCompleted -> 1 poke.
        d.tick_once_for_test();
        // Tick #2: AlreadyCompleted -> no extra TorrentCompleted poke.
        d.tick_once_for_test();

        let mut completed_pokes = 0;
        while let Ok(p) = rx.try_recv() {
            if let MergerPoke::TorrentCompleted(h) = p {
                assert_eq!(h, a);
                completed_pokes += 1;
            }
        }
        assert_eq!(completed_pokes, 1);
    }

    #[test]
    fn force_initial_completed_returns_true_only_when_flipping_to_done() {
        let d = dispatcher_with_download(0, 0);
        let a = hash(1);
        d.register_torrent(a.clone(), 1_000, false);

        assert!(d.force_initial_completed(&a, true));
        // Already completed: should not report a fresh transition.
        assert!(!d.force_initial_completed(&a, true));

        // Reset to not-completed; subsequent flip-to-true must report true.
        assert!(!d.force_initial_completed(&a, false));
        assert_eq!(d.get_seed_stat_for_torrent(&a).downloaded(), 0);
        assert!(d.force_initial_completed(&a, true));
    }

    #[tokio::test]
    async fn double_start_returns_error() {
        let d = dispatcher(1_000);
        d.start().expect("first start");
        assert!(matches!(d.start(), Err(BandwidthError::AlreadyRunning)));
        d.stop().await.expect("stop");
    }

    #[tokio::test]
    async fn stop_without_start_returns_error() {
        let d = dispatcher(1_000);
        assert!(matches!(d.stop().await, Err(BandwidthError::NotRunning)));
    }

    #[tokio::test]
    async fn start_and_stop_completes_cleanly() {
        let d = dispatcher(100_000);
        d.start().expect("start");
        d.stop().await.expect("stop");
        let task = d.task.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(task.is_none());
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
