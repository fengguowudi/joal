//! Seeding orchestrator.
//!
//! Port of Java `org.araymond.joal.core.ttorrent.client.Client` (+
//! `ClientBuilder`). Ties everything together:
//!
//! 1. Subscribes to [`TorrentFileProvider`] changes — when a new torrent is
//!    added and we have capacity, schedule a `started` announce; when a
//!    torrent is removed, schedule a `stopped` announce.
//! 2. Owns a [`DelayQueue<AnnounceRequest>`] + a background tick task that
//!    drains it every [`ORCHESTRATOR_TICK`] and dispatches ready entries to
//!    the [`AnnouncerExecutor`].
//! 3. Owns the `currently_seeding_announcers` list, exposed to UI consumers
//!    via read-only [`AnnouncerFacade`] references.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{self, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::announcer::{AnnounceRequest, Announcer, AnnouncerFacade};
use crate::bandwidth::BandwidthDispatcher;
use crate::client::RequestEvent;
use crate::config::AppConfiguration;
use crate::events::EngineEventSink;
use crate::snapshot::MergerPoke;
use crate::torrent::{
    InfoHash, MockedTorrent, NoMoreTorrentsError, TorrentFileChangeAware, TorrentFileProvider,
};
use crate::ttorrent_client::announcer_executor::{AnnouncerExecutor, OrchestratorControl};
use crate::ttorrent_client::announcer_factory::AnnouncerFactory;
use crate::ttorrent_client::delay_queue::DelayQueue;
use crate::ttorrent_client::response_handlers::{
    AnnounceEventPublisher, AnnounceReEnqueuer, AnnounceResponseHandlerChain,
    BandwidthDispatcherNotifier, ClientNotifier, MergerPokeNotifier,
};

/// Period between delay-queue drain attempts. Matches Java's
/// `MILLISECONDS.sleep(1000)` in `Client.start()`.
pub const ORCHESTRATOR_TICK: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("orchestrator is already running")]
    AlreadyRunning,
    #[error("orchestrator is not running")]
    NotRunning,
}

/// The heart of the seeding engine.
pub struct ClientOrchestrator {
    shared: Arc<SharedState>,
    executor: Arc<AnnouncerExecutor>,
    delay_queue: Arc<DelayQueue<AnnounceRequest>>,
    task: Mutex<Option<JoinHandle<()>>>,
    listener: Mutex<Option<Arc<dyn TorrentFileChangeAware>>>,
}

struct SharedState {
    app_config: AppConfiguration,
    torrent_provider: Arc<TorrentFileProvider>,
    announcer_factory: AnnouncerFactory,
    announcers: Mutex<Vec<Arc<Announcer>>>,
    stopping: Arc<std::sync::atomic::AtomicBool>,
    events: Arc<dyn EngineEventSink>,
    delay_queue: Arc<DelayQueue<AnnounceRequest>>,
    executor: Mutex<Option<Arc<AnnouncerExecutor>>>,
    weak_self: std::sync::OnceLock<std::sync::Weak<Self>>,
}

impl ClientOrchestrator {
    /// Build an orchestrator from its collaborators. The caller is
    /// responsible for starting the [`BandwidthDispatcher`] and the
    /// [`TorrentFileProvider`] before calling [`ClientOrchestrator::start`].
    ///
    /// `merger_poke`, when `Some`, gets a `MergerPoke::AnnouncerUpdated` on
    /// every announce round-trip (success, fail, too-many-failures, stop).
    /// [`SeedManager`][crate::seed_manager] passes its merger-task mailbox;
    /// stand-alone integration tests can pass `None`.
    pub fn new(
        app_config: AppConfiguration,
        torrent_provider: Arc<TorrentFileProvider>,
        bandwidth: Arc<BandwidthDispatcher>,
        announcer_factory: AnnouncerFactory,
        events: &Arc<dyn EngineEventSink>,
        merger_poke: Option<mpsc::Sender<MergerPoke>>,
    ) -> Arc<Self> {
        let delay_queue = Arc::new(DelayQueue::<AnnounceRequest>::new());
        let shared = Arc::new(SharedState {
            app_config,
            torrent_provider,
            announcer_factory,
            announcers: Mutex::new(Vec::new()),
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            events: Arc::clone(events),
            delay_queue: Arc::clone(&delay_queue),
            executor: Mutex::new(None),
            weak_self: std::sync::OnceLock::new(),
        });
        let _ = shared.weak_self.set(Arc::downgrade(&shared));

        // Build the response-handler chain: order matters — publisher first
        // for UX parity with Java.
        let mut chain = AnnounceResponseHandlerChain::new();
        chain.append(Arc::new(AnnounceEventPublisher::new(Arc::clone(events))));
        chain.append(Arc::new(AnnounceReEnqueuer::new(Arc::clone(&delay_queue))));
        chain.append(Arc::new(BandwidthDispatcherNotifier::new(bandwidth)));
        // ClientNotifier comes last among the behaviour-bearing handlers: its
        // callbacks must see the fully-wired state.
        let control: Arc<dyn OrchestratorControl> = Arc::clone(&shared) as _;
        chain.append(Arc::new(ClientNotifier::new(Arc::clone(&control))));
        // The merger poke is strictly a snapshot trigger — it runs after every
        // other handler so the facade fields it triggers on already reflect
        // the new announce's side-effects.
        if let Some(poke) = merger_poke {
            chain.append(Arc::new(MergerPokeNotifier::new(poke)));
        }

        let callback = chain.into_callback();
        let executor = Arc::new(AnnouncerExecutor::new(callback, control));

        {
            let mut slot = shared
                .executor
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            *slot = Some(Arc::clone(&executor));
        }

        Arc::new(Self {
            shared,
            executor,
            delay_queue,
            task: Mutex::new(None),
            listener: Mutex::new(None),
        })
    }

    /// Start the orchestrator loop. Must be called from inside a tokio
    /// runtime. Returns [`ClientError::AlreadyRunning`] on double-start.
    pub async fn start(self: &Arc<Self>) -> Result<(), ClientError> {
        {
            let mut task = self.task.lock().unwrap_or_else(PoisonError::into_inner);
            if task.is_some() {
                return Err(ClientError::AlreadyRunning);
            }
            // Mark-as-running *before* spawning so concurrent starts fail fast.
            *task = Some(tokio::spawn(orchestrator_loop(
                Arc::clone(&self.shared),
                Arc::clone(&self.executor),
                Arc::clone(&self.delay_queue),
            )));
        }
        self.shared
            .stopping
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // Seed the pool up to `simultaneous_seed`. Java does this in `start()`
        // before kicking off the orchestrator thread.
        for _ in 0..self.shared.app_config.simultaneous_seed {
            if self.shared.add_torrent_from_directory().await.is_err() {
                break;
            }
        }
        let listener = Arc::new(TorrentChangeAdapter {
            shared: Arc::clone(&self.shared),
        }) as Arc<dyn TorrentFileChangeAware>;
        self.shared
            .torrent_provider
            .register_listener(Arc::clone(&listener))
            .await;
        {
            let mut slot = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
            *slot = Some(listener);
        }
        info!("client orchestrator started");
        Ok(())
    }

    /// Stop the orchestrator and wait for in-flight announces to finish.
    ///
    /// On the way out every still-pending non-START request is turned into a
    /// STOPPED announce and executed. Mirror of Java `Client.stop()`.
    pub async fn stop(self: &Arc<Self>) -> Result<(), ClientError> {
        self.shared
            .stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);

        // Drop the listener *before* killing the task so no torrent-add event
        // sneaks into the queue mid-shutdown.
        let listener = {
            let mut slot = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
            slot.take()
        };
        if let Some(listener) = listener {
            self.shared
                .torrent_provider
                .unregister_listener(&listener)
                .await;
        }

        let handle = {
            let mut task = self.task.lock().unwrap_or_else(PoisonError::into_inner);
            task.take()
        };
        let Some(handle) = handle else {
            return Err(ClientError::NotRunning);
        };
        handle.abort();
        let _ = handle.await;

        // Turn every queued non-start request into a stop and submit it.
        for pending in self.delay_queue.drain_all() {
            if pending.event() == RequestEvent::Started {
                // Java drops pending starts: no stop event is meaningful
                // when we never actually started.
                continue;
            }
            self.executor.execute(pending.to_stop());
        }
        // Also schedule explicit stops for every live announcer (Java's
        // `denyAll()` + `awaitForRunningTasks` does the same sweep via the
        // `Client.stop()` path).
        let live = self.announcers_snapshot();
        for announcer in live {
            self.executor.execute(AnnounceRequest::create_stop(
                announcer.torrent_info_hash().clone(),
            ));
        }
        self.executor.await_running_tasks().await;
        info!("client orchestrator stopped");
        Ok(())
    }

    /// Read-only snapshot of live announcers. Used by UI consumers.
    #[must_use]
    pub fn announcers_snapshot(&self) -> Vec<Arc<Announcer>> {
        let guard = self
            .shared
            .announcers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.clone()
    }

    /// Read-only view of live announcers typed as [`AnnouncerFacade`]
    /// handles, matching Java `getCurrentlySeedingAnnouncers()`.
    #[must_use]
    pub fn seeding_announcer_facades(&self) -> Vec<Arc<dyn AnnouncerFacade>> {
        self.announcers_snapshot()
            .into_iter()
            .map(|a| a as Arc<dyn AnnouncerFacade>)
            .collect()
    }

    /// Access the delay queue. Mostly for tests.
    #[must_use]
    pub fn delay_queue(&self) -> &Arc<DelayQueue<AnnounceRequest>> {
        &self.delay_queue
    }

    /// Access the executor. Mostly for tests.
    #[must_use]
    pub fn executor(&self) -> &Arc<AnnouncerExecutor> {
        &self.executor
    }
}

impl std::fmt::Debug for ClientOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientOrchestrator")
            .field("announcers", &self.announcers_snapshot().len())
            .finish_non_exhaustive()
    }
}

impl SharedState {
    fn self_arc(&self) -> Arc<Self> {
        self.weak_self
            .get()
            .expect("weak_self not initialized")
            .upgrade()
            .expect("SharedState dropped while still in use")
    }

    async fn add_torrent_from_directory(&self) -> Result<(), NoMoreTorrentsError> {
        let excluded: HashSet<InfoHash> = {
            let guard = self
                .announcers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard
                .iter()
                .map(|a| a.torrent_info_hash().clone())
                .collect()
        };
        let torrent = self.torrent_provider.get_torrent_not_in(&excluded).await?;
        self.add_torrent(torrent);
        Ok(())
    }

    fn add_torrent(&self, torrent: MockedTorrent) {
        let info_hash = torrent.info_hash.clone();
        match self.announcer_factory.create(torrent) {
            Ok(announcer) => {
                {
                    let mut guard = self
                        .announcers
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    // Dedup by info-hash (Java's HashSet semantics).
                    guard.retain(|existing| existing.torrent_info_hash() != &info_hash);
                    guard.push(Arc::clone(&announcer));
                }
                self.delay_queue
                    .add_or_replace(AnnounceRequest::create_start(info_hash), Duration::ZERO);
            }
            Err(e) => {
                warn!(info_hash = %info_hash, error = %e, "failed to build announcer");
            }
        }
    }

    fn remove_announcer(&self, info_hash: &InfoHash) -> Option<Arc<Announcer>> {
        let mut guard = self
            .announcers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard
            .iter()
            .position(|a| a.torrent_info_hash() == info_hash)
            .map(|pos| guard.remove(pos))
    }

    fn is_stopping(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }
}

async fn orchestrator_loop(
    shared: Arc<SharedState>,
    executor: Arc<AnnouncerExecutor>,
    delay_queue: Arc<DelayQueue<AnnounceRequest>>,
) {
    let mut ticker = time::interval(ORCHESTRATOR_TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Java's first action is a 1s sleep (`MILLISECONDS.sleep(1000)` inside
    // the loop). `tokio::time::interval`'s first tick fires immediately —
    // drop it so semantics match.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        if shared.is_stopping() {
            break;
        }
        let ready = delay_queue.get_availables();
        for req in ready {
            executor.execute(req);
        }
    }
    debug!("orchestrator loop exited");
}

// ─────────────────────────────────────────────────────────────────────────
// OrchestratorControl implementation on SharedState.
// ─────────────────────────────────────────────────────────────────────────

impl OrchestratorControl for SharedState {
    fn resolve_announcer(&self, info_hash: &InfoHash) -> Option<Arc<Announcer>> {
        let guard = self
            .announcers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard
            .iter()
            .find(|a| a.torrent_info_hash() == info_hash)
            .cloned()
    }

    fn on_too_many_failed(&self, info_hash: &InfoHash) {
        if self.is_stopping() {
            self.remove_announcer(info_hash);
            return;
        }
        self.remove_announcer(info_hash);
        let shared = self.self_arc();
        let info_hash = info_hash.clone();
        tokio::spawn(async move {
            shared
                .torrent_provider
                .move_to_archive_folder(&info_hash)
                .await;
            let _ = shared.add_torrent_from_directory().await;
        });
    }

    fn on_upload_ratio_limit_reached(&self, info_hash: &InfoHash) {
        info!(info_hash = %info_hash, "upload ratio reached, archiving torrent");
        let shared = self.self_arc();
        let info_hash = info_hash.clone();
        tokio::spawn(async move {
            shared
                .torrent_provider
                .move_to_archive_folder(&info_hash)
                .await;
        });
    }

    fn on_no_more_peers(&self, info_hash: &InfoHash) {
        if self.app_config.keep_torrent_with_zero_leechers {
            return;
        }
        let shared = self.self_arc();
        let info_hash = info_hash.clone();
        tokio::spawn(async move {
            shared
                .torrent_provider
                .move_to_archive_folder(&info_hash)
                .await;
        });
    }

    fn on_torrent_has_stopped(&self, info_hash: &InfoHash) {
        if self.is_stopping() {
            self.remove_announcer(info_hash);
            return;
        }
        let shared = self.self_arc();
        let stopped = info_hash.clone();
        let executor = self
            .executor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(Arc::clone);
        tokio::spawn(async move {
            let _ = shared.add_torrent_from_directory().await;
            shared.remove_announcer(&stopped);
            if let Some(exec) = executor {
                exec.deny(&stopped);
            }
        });
    }
}

struct TorrentChangeAdapter {
    shared: Arc<SharedState>,
}

impl TorrentFileChangeAware for TorrentChangeAdapter {
    fn on_torrent_file_added(&self, torrent: &MockedTorrent) {
        self.shared
            .events
            .publish(crate::events::EngineEvent::TorrentFileAdded {
                info_hash: torrent.info_hash.clone(),
                name: torrent.name.clone(),
                total_size: torrent.total_size,
            });
        if self.shared.is_stopping() {
            return;
        }
        let seeding_count = {
            let guard = self
                .shared
                .announcers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.len() as u32
        };
        if seeding_count < self.shared.app_config.simultaneous_seed {
            self.shared.add_torrent(torrent.clone());
        }
    }

    fn on_torrent_file_removed(&self, torrent: &MockedTorrent) {
        self.shared
            .events
            .publish(crate::events::EngineEvent::TorrentFileDeleted {
                info_hash: torrent.info_hash.clone(),
                name: torrent.name.clone(),
            });
        let info_hash = torrent.info_hash.clone();
        let has_live = {
            let guard = self
                .shared
                .announcers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.iter().any(|a| a.torrent_info_hash() == &info_hash)
        };
        if has_live {
            self.shared.delay_queue.add_or_replace(
                AnnounceRequest::create_stop(info_hash),
                Duration::from_secs(1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_tick_is_one_second() {
        assert_eq!(ORCHESTRATOR_TICK, Duration::from_secs(1));
    }
}
