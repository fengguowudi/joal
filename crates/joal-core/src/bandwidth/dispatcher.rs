//! Periodic bandwidth dispatcher.
//!
//! Originally a port of Java `org.araymond.joal.core.bandwith.BandwidthDispatcher`,
//! which re-sampled the global upload budget once every 20 minutes and kept
//! the value frozen in between. The Rust port replaces that with a
//! **per-tick reflected random walk** ([`RandomSpeedProvider::step`]) so the
//! global speed evolves continuously — there are no more long flat plateaus
//! between refreshes.
//!
//! On every tick the dispatcher:
//! 1. advances the upload + download speed providers by one walk step;
//! 2. resplits the global budgets across registered torrents via
//!    [`Inner::recompute_speeds`];
//! 3. credits each torrent with `current_speed * tick_ms / 1000` into its
//!    [`TorrentSeedStats::uploaded`] / `downloaded` counters.
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
use tokio::task::{JoinError, JoinHandle};
use tokio::time::{self, MissedTickBehavior};
use tracing::{debug, warn};

use crate::bandwidth::peers::Peers;
use crate::bandwidth::random_speed::{DownloadSpeedProvider, RandomSpeedProvider};
use crate::bandwidth::speed::Speed;
use crate::bandwidth::stats::{DownloadEdge, TorrentSeedStats};
use crate::bandwidth::weight::{PeersAwareWeightCalculator, WeightHolder};
use crate::snapshot::MergerPoke;
use crate::torrent::InfoHash;

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
            let assigned =
                if global_down == 0 || active_total == 0.0 || cap == 0 || st.downloaded() >= cap {
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
            if sender.try_send(MergerPoke::SpeedRecomputed).is_err() {
                warn!("merger poke channel is full or closed; speed recompute will be coalesced");
            }
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
                    if sender.try_send(MergerPoke::TorrentCompleted(h)).is_err() {
                        warn!(
                            "merger poke channel is full or closed; torrent completion will be coalesced"
                        );
                    }
                }
            }
        }
    }
}

/// Batched read-only state snapshot for callers that need speeds and stats for
/// many torrents at once.
#[derive(Clone, Debug, Default)]
pub struct BandwidthSnapshot {
    pub speeds: HashMap<InfoHash, Speed>,
    pub download_speeds: HashMap<InfoHash, Speed>,
    pub stats: HashMap<InfoHash, TorrentSeedStats>,
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
    /// Build a dispatcher. The global upload/download speeds are evolved by a
    /// per-tick reflected random walk; there is no longer a periodic full
    /// re-sample (Java's 20-minute boundary). Call
    /// [`Self::refresh_current_bandwidth`] to force a manual full re-sample.
    #[must_use]
    pub fn new(
        tick_period: Duration,
        random_speed_provider: RandomSpeedProvider,
        download_speed_provider: DownloadSpeedProvider,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                weight_holder: WeightHolder::new(PeersAwareWeightCalculator::new()),
                torrents_seed_stats: HashMap::new(),
                total_sizes: HashMap::new(),
                speed_map: HashMap::new(),
                download_speed_map: HashMap::new(),
                random_speed_provider,
                download_speed_provider,
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

    pub fn register_torrent(&self, info_hash: InfoHash, total_size: u64, initial_completed: bool) {
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

    /// Single-lock snapshot of every per-torrent speed/stat map.
    #[must_use]
    pub fn snapshot(&self) -> BandwidthSnapshot {
        self.with_lock(|inner| BandwidthSnapshot {
            speeds: inner.speed_map.clone(),
            download_speeds: inner.download_speed_map.clone(),
            stats: inner.torrents_seed_stats.clone(),
        })
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

    /// Force a full re-sample of both speed providers and recompute the
    /// per-torrent budget split. The walk continues from the freshly-sampled
    /// position. Useful for tests and for "jump to a new random starting
    /// point" semantics (e.g. after a major config change).
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
        if let Err(error) = handle.await
            && !is_expected_abort(&error)
        {
            debug!(%error, "bandwidth dispatcher task ended during stop");
        }
        Ok(())
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

fn is_expected_abort(error: &JoinError) -> bool {
    error.is_cancelled()
}

fn on_tick(inner: &Mutex<Inner>, tick_ms: u64) {
    let mut guard = inner.lock().unwrap_or_else(PoisonError::into_inner);
    guard.random_speed_provider.step();
    guard.download_speed_provider.step();
    // recompute_speeds already emits a SpeedRecomputed poke when a sender
    // is attached, so no trailing poke is needed — the merger picks up the
    // updated `uploaded` from the same lock window.
    guard.recompute_speeds();
    guard.accumulate_traffic(tick_ms);
}
