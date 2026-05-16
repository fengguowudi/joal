//! Async announcer executor.
//!
//! Port of Java
//! `org.araymond.joal.core.ttorrent.client.announcer.request.AnnouncerExecutor`.
//! Where Java uses a bounded `ThreadPoolExecutor`, the Rust side uses
//! [`tokio::spawn`] and tracks running tasks in a [`Mutex`]-guarded map
//! keyed on [`InfoHash`]. There is no explicit pool size — tokio's work-
//! stealing runtime scales announce tasks across all worker threads
//! naturally.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::announcer::{AnnounceRequest, Announcer, AnnouncerError};
use crate::client::RequestEvent;
use crate::orchestrator::response_handlers::AnnounceOutcome;
use crate::torrent::InfoHash;

/// Matches Java's `awaitTermination(10, SECONDS)` for outstanding announces.
pub const AWAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Response callback trait. Implementations are the glue between the
/// executor and the `AnnounceResponseHandlerChain`. Mirrors Java
/// `AnnounceResponseCallback`.
pub trait AnnounceResponseCallback: Send + Sync {
    fn on_will_announce(&self, event: RequestEvent, announcer: &Arc<Announcer>);
    fn on_announce_result(
        &self,
        event: RequestEvent,
        announcer: &Arc<Announcer>,
        outcome: &AnnounceOutcome,
    );
}

/// Async-friendly executor that dispatches announces to a handler chain.
pub struct AnnouncerExecutor {
    callback: Arc<dyn AnnounceResponseCallback>,
    control: Arc<dyn OrchestratorControl>,
    running: Arc<Mutex<HashMap<InfoHash, RunningTask>>>,
}

/// Strategy used to look up the [`Announcer`] for a given request and
/// receive lifecycle notifications from the handler chain.
///
/// Combines the former `AnnouncerResolver` and `ClientNotificationSink`
/// traits into a single seam that the orchestrator implements.
pub trait OrchestratorControl: Send + Sync {
    fn resolve_announcer(&self, info_hash: &InfoHash) -> Option<Arc<Announcer>>;
    fn on_too_many_failed(&self, info_hash: &InfoHash);
    fn on_upload_ratio_limit_reached(&self, info_hash: &InfoHash);
    fn on_no_more_peers(&self, info_hash: &InfoHash);
    fn on_torrent_has_stopped(&self, info_hash: &InfoHash);
}

struct RunningTask {
    announcer: Arc<Announcer>,
    handle: JoinHandle<()>,
}

impl AnnouncerExecutor {
    #[must_use]
    pub fn new(
        callback: Arc<dyn AnnounceResponseCallback>,
        control: Arc<dyn OrchestratorControl>,
    ) -> Self {
        Self {
            callback,
            control,
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Dispatch `request` to the background runtime.
    #[allow(clippy::needless_pass_by_value)]
    pub fn execute(&self, request: AnnounceRequest) {
        let info_hash = request.info_hash().clone();
        let mut event = request.event();
        let Some(announcer) = self.control.resolve_announcer(&info_hash) else {
            debug!(info_hash = %info_hash, "execute: announcer not resolvable, skipping");
            return;
        };
        // Sticky `pending_completed` swap: if the simulated download crossed
        // total_size between the previous announce and now, the merger has
        // flipped the bit on the announcer. Promote a regular `None`
        // announce to `Completed` exactly once (the bit is consumed).
        if matches!(event, RequestEvent::None) && announcer.take_pending_completed() {
            event = RequestEvent::Completed;
        }
        let callback = Arc::clone(&self.callback);
        let announcer_task = Arc::clone(&announcer);
        let running = Arc::clone(&self.running);
        let key = info_hash.clone();
        let handle = tokio::spawn(async move {
            callback.on_will_announce(event, &announcer_task);
            let outcome = match announcer_task.announce(event).await {
                Ok(result) => AnnounceOutcome::Success(result),
                Err(AnnouncerError::TooManyFailures(err)) => AnnounceOutcome::TooManyFailures(err),
                Err(err) => AnnounceOutcome::Failure(err),
            };
            callback.on_announce_result(event, &announcer_task, &outcome);
            let mut running = running.lock().unwrap_or_else(PoisonError::into_inner);
            running.remove(&key);
        });
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        // If a task was already running for this torrent, abort it. The Java
        // `currentlyRunning.put(...)` silently overwrites; the Rust side must
        // proactively cancel the previous task to avoid double-announces.
        if let Some(existing) = running.insert(info_hash, RunningTask { announcer, handle }) {
            existing.handle.abort();
        }
    }

    /// Cancel a running announce by info-hash. Returns the [`Announcer`] if
    /// one was running, matching Java's `deny`.
    pub fn deny(&self, info_hash: &InfoHash) -> Option<Arc<Announcer>> {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        running.remove(info_hash).map(|task| {
            task.handle.abort();
            task.announcer
        })
    }

    /// Cancel every running announce. Mirrors Java's `denyAll`.
    pub fn deny_all(&self) -> Vec<Arc<Announcer>> {
        let running = {
            let mut g = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut *g)
        };
        let mut announcers = Vec::with_capacity(running.len());
        for (_, task) in running {
            task.handle.abort();
            announcers.push(task.announcer);
        }
        announcers
    }

    /// Wait for in-flight announces to complete. Matches Java's
    /// `awaitForRunningTasks`: 10-second ceiling, warn on timeout.
    pub async fn await_running_tasks(&self) {
        let handles: Vec<JoinHandle<()>> = {
            let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            running.drain().map(|(_, task)| task.handle).collect()
        };
        if handles.is_empty() {
            return;
        }
        let joined = async {
            for h in handles {
                let _ = h.await;
            }
        };
        if let Ok(()) = timeout(AWAIT_TIMEOUT, joined).await {
            debug!("all announcer tasks have completed");
        } else {
            warn!("AnnouncerExecutor timed out after {AWAIT_TIMEOUT:?}");
        }
    }

    /// Current number of in-flight tasks. Mainly for tests.
    pub fn running_count(&self) -> usize {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

impl std::fmt::Debug for AnnouncerExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnouncerExecutor")
            .field("running", &self.running_count())
            .finish_non_exhaustive()
    }
}
