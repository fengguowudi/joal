//! Filesystem watcher for the `torrents/` directory.
//!
//! Port of Java `org.araymond.joal.core.torrent.watcher.TorrentFileProvider`
//! (plus its commons-io `TorrentFileWatcher` sidekick). The Rust side uses the
//! cross-platform [`notify`] crate in async mode: a [`RecommendedWatcher`]
//! feeds filesystem events into a [`tokio::sync::mpsc`] channel that a
//! dedicated watcher task drains and dispatches.
//!
//! ## Concurrency model
//!
//! Java stores the torrent map in `Collections.synchronizedMap(new HashMap<>())`
//! and always copies into a fresh `ArrayList<>` before iterating. The Rust
//! port keeps the same discipline: a [`tokio::sync::RwLock`] around a
//! [`HashMap<PathBuf, MockedTorrent>`]. Every read that streams over the map
//! (`get_torrent_not_in`, `torrent_files`) clones the values out first so
//! listener callbacks can freely mutate the map without deadlocking.
//!
//! ## Startup semantics
//!
//! Java's commons-io watcher fires `onFileCreate` for every file present at
//! start-up. `notify` does not — the Rust side therefore performs an explicit
//! startup scan in [`TorrentFileProvider::start`] to match the observable
//! behaviour (existing `.torrent` files appear on listeners as adds).

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::config::JoalFolders;
use crate::torrent::{InfoHash, MockedTorrent};

/// Listener invoked when torrent files are added or removed on disk.
///
/// Mirror of Java `TorrentFileChangeAware`. Implementations must be cheap
/// (they are called with the provider's map-write lock dropped but the
/// caller's tokio task still waiting on them) and MUST NOT panic — any panic
/// would abort the watcher task.
pub trait TorrentFileChangeAware: Send + Sync {
    fn on_torrent_file_added(&self, torrent: &MockedTorrent);
    fn on_torrent_file_removed(&self, torrent: &MockedTorrent);
}

/// Raised by [`TorrentFileProvider::get_torrent_not_in`] when every known
/// torrent is already being seeded. Mirror of Java
/// `NoMoreTorrentsFileAvailableException`.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct NoMoreTorrentsError(pub String);

impl NoMoreTorrentsError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

struct Listeners {
    items: Vec<Arc<dyn TorrentFileChangeAware>>,
}

impl Listeners {
    const fn new() -> Self {
        Self { items: Vec::new() }
    }

    fn add(&mut self, listener: Arc<dyn TorrentFileChangeAware>) {
        // Deduplicate by pointer identity, matching Java HashSet semantics.
        if !self
            .items
            .iter()
            .any(|existing| Arc::ptr_eq(existing, &listener))
        {
            self.items.push(listener);
        }
    }

    fn remove(&mut self, listener: &Arc<dyn TorrentFileChangeAware>) {
        self.items.retain(|l| !Arc::ptr_eq(l, listener));
    }

    fn snapshot(&self) -> Vec<Arc<dyn TorrentFileChangeAware>> {
        self.items.clone()
    }
}

/// Hot-reloading torrent file catalogue.
///
/// Cheap to clone via [`Arc`]; all state is kept behind async locks so
/// multiple listeners can query the provider concurrently.
pub struct TorrentFileProvider {
    torrents_dir: PathBuf,
    archive_dir: PathBuf,
    torrent_files: Arc<RwLock<HashMap<PathBuf, MockedTorrent>>>,
    listeners: Arc<RwLock<Listeners>>,
    task: Mutex<Option<JoinHandle<()>>>,
    // Holding the RecommendedWatcher keeps the native watch active. Dropping
    // it stops the background thread.
    watcher: Mutex<Option<RecommendedWatcher>>,
}

impl std::fmt::Debug for TorrentFileProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TorrentFileProvider")
            .field("torrents_dir", &self.torrents_dir)
            .field("archive_dir", &self.archive_dir)
            .finish_non_exhaustive()
    }
}

impl TorrentFileProvider {
    /// Build a provider rooted at `joal_folders.torrents_dir`.
    ///
    /// Mirrors Java `TorrentFileProvider(JoalFoldersPath)` + `init()`:
    /// the torrents directory must already exist (fail fast) and the archive
    /// subdirectory is created on demand.
    pub fn new(joal_folders: &JoalFolders) -> io::Result<Arc<Self>> {
        let torrents_dir = joal_folders.torrents_dir.clone();
        let archive_dir = joal_folders.torrents_archive_dir.clone();

        if !torrents_dir.is_dir() {
            error!(dir = %torrents_dir.display(), "torrents directory does not exist");
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Torrent folder [{}] not found", torrents_dir.display()),
            ));
        }

        if archive_dir.exists() && !archive_dir.is_dir() {
            error!(dir = %archive_dir.display(), "archive path exists but is not a directory");
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Archive folder exists, but is not a directory",
            ));
        }
        if !archive_dir.exists() {
            std::fs::create_dir_all(&archive_dir)?;
        }

        Ok(Arc::new(Self {
            torrents_dir,
            archive_dir,
            torrent_files: Arc::new(RwLock::new(HashMap::new())),
            listeners: Arc::new(RwLock::new(Listeners::new())),
            task: Mutex::new(None),
            watcher: Mutex::new(None),
        }))
    }

    /// Perform the initial directory scan and start the watcher task.
    ///
    /// `start()` is idempotent with respect to an already-running task: a
    /// second call warns and returns early, matching Java's single-shot
    /// semantics (the Java watcher is `stop()`-only, never restart).
    pub async fn start(self: &Arc<Self>) -> io::Result<()> {
        {
            let task_guard = self.task.lock().await;
            if task_guard.is_some() {
                warn!("TorrentFileProvider::start called but watcher is already running");
                return Ok(());
            }
        }

        // 1) Startup scan: replay every existing `.torrent` file as a synthetic
        //    create event so listeners see the initial state.
        let mut entries = tokio::fs::read_dir(&self.torrents_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path_is_torrent_file(&path) {
                continue;
            }
            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to stat entry during scan");
                    continue;
                }
            };
            if !metadata.is_file() {
                continue;
            }
            self.on_file_create(&path).await;
        }

        // 2) Wire up notify + a tokio channel + a dispatcher task.
        let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(64);
        let mut watcher = notify::recommended_watcher(move |res| {
            // `tx.blocking_send` would deadlock when the tokio runtime only
            // has one worker thread on Windows. Use `try_send` and drop on
            // overflow: lost events are reasserted by the filesystem on
            // subsequent re-scans (we don't get that, but a debounce error
            // is still better than a hung thread).
            if let Err(error) = tx.try_send(res) {
                warn!(%error, "watcher event channel is full or closed; notify event coalesced");
            }
        })
        .map_err(|err| notify_to_io(&err))?;
        watcher
            .watch(&self.torrents_dir, RecursiveMode::NonRecursive)
            .map_err(|err| notify_to_io(&err))?;

        let provider = Arc::clone(self);
        let task = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    Ok(ev) => provider.handle_event(ev).await,
                    Err(e) => warn!(error = %e, "notify error"),
                }
            }
            debug!("watcher dispatch task exiting (channel closed)");
        });

        {
            let mut task_guard = self.task.lock().await;
            *task_guard = Some(task);
        }
        {
            let mut watcher_guard = self.watcher.lock().await;
            *watcher_guard = Some(watcher);
        }
        info!(dir = %self.torrents_dir.display(), "torrent watcher started");
        Ok(())
    }

    /// Stop the watcher task and clear the in-memory catalogue. Mirror of
    /// Java `TorrentFileProvider.stop()`.
    pub async fn stop(self: &Arc<Self>) {
        // Drop the RecommendedWatcher first: its background thread stops and
        // its Sender ends, which closes the channel and lets the dispatch
        // task exit cleanly.
        {
            let mut watcher_guard = self.watcher.lock().await;
            *watcher_guard = None;
        }
        {
            let mut task_guard = self.task.lock().await;
            if let Some(handle) = task_guard.take() {
                handle.abort();
                if let Err(error) = handle.await {
                    debug!(%error, "torrent watcher task aborted during stop");
                }
            }
        }
        self.torrent_files.write().await.clear();
        info!("torrent watcher stopped");
    }

    /// Directory scanned by this provider.
    #[must_use]
    pub fn torrents_dir(&self) -> &Path {
        &self.torrents_dir
    }

    /// Archive directory under the torrents root.
    #[must_use]
    pub fn archive_dir(&self) -> &Path {
        &self.archive_dir
    }

    /// Subscribe to add/remove notifications.
    pub async fn register_listener(&self, listener: Arc<dyn TorrentFileChangeAware>) {
        let mut listeners = self.listeners.write().await;
        listeners.add(listener);
    }

    /// Unsubscribe a previously-registered listener. No-op if not present.
    pub async fn unregister_listener(&self, listener: &Arc<dyn TorrentFileChangeAware>) {
        let mut listeners = self.listeners.write().await;
        listeners.remove(listener);
    }

    /// Snapshot of every loaded torrent. Safe to iterate without holding a
    /// lock (the returned `Vec` is a clone).
    pub async fn torrent_files(&self) -> Vec<MockedTorrent> {
        self.torrent_files.read().await.values().cloned().collect()
    }

    /// Number of loaded torrents.
    pub async fn torrent_count(&self) -> usize {
        self.torrent_files.read().await.len()
    }

    /// Pick any torrent whose info-hash is not in `exclude`. Mirror of Java
    /// `getTorrentNotIn`: the Java side shuffles the candidate list then
    /// calls `findAny`, so the caller should not assume a deterministic
    /// ordering. Tests that need determinism should seed `exclude` such that
    /// only one candidate remains.
    pub async fn get_torrent_not_in(
        &self,
        exclude: &HashSet<InfoHash>,
    ) -> Result<MockedTorrent, NoMoreTorrentsError> {
        let torrents = self.torrent_files.read().await;
        // Not shuffling here: Rust side picks the first qualifying torrent,
        // which callers treat as arbitrary order. If hot-spot bias becomes
        // an issue we can switch to `rand::seq::SliceRandom::choose`.
        for torrent in torrents.values() {
            if !exclude.contains(&torrent.info_hash) {
                return Ok(torrent.clone());
            }
        }
        Err(NoMoreTorrentsError::new("No more torrent files available"))
    }

    /// Move a torrent identified by info-hash to the archive folder. Mirror
    /// of Java `moveToArchiveFolder(InfoHash)`: if the torrent is not
    /// tracked, logs a warning and returns.
    pub async fn move_to_archive_folder(&self, info_hash: &InfoHash) {
        let path = {
            let torrents = self.torrent_files.read().await;
            torrents
                .iter()
                .find(|(_, t)| t.info_hash == *info_hash)
                .map(|(p, _)| p.clone())
        };
        if let Some(path) = path {
            self.archive_file(&path).await;
        } else {
            warn!(
                info_hash = %info_hash,
                "cannot move torrent to archive folder. Torrent file is not registered"
            );
        }
    }

    async fn archive_file(&self, file: &Path) {
        if !file.exists() {
            return;
        }
        // Remove the torrent from the catalogue first, matching Java's
        // `onFileDelete -> moveToArchiveFolder` ordering.
        self.on_file_delete(file).await;
        self.archive_path(file, "successfully moved file to archive folder")
            .await;
    }

    async fn handle_event(self: &Arc<Self>, event: Event) {
        for path in event.paths.iter().filter(|p| path_is_torrent_file(p)) {
            match event.kind {
                EventKind::Create(_) => self.on_file_create(path).await,
                EventKind::Modify(_) => self.on_file_change(path).await,
                EventKind::Remove(_) => self.on_file_delete(path).await,
                _ => {}
            }
        }
    }

    async fn on_file_create(&self, file: &Path) {
        info!(path = %file.display(), "torrent file addition detected");
        match MockedTorrent::from_file(file).await {
            Ok(torrent) => {
                {
                    let mut torrents = self.torrent_files.write().await;
                    torrents.insert(file.to_path_buf(), torrent.clone());
                }
                let listeners = self.listeners.read().await.snapshot();
                for listener in listeners {
                    listener.on_torrent_file_added(&torrent);
                }
            }
            Err(e) => {
                // Java catches IOException | NoSuchAlgorithmException with a
                // `warn`, and every other exception with an `error`. The Rust
                // enum groups transport + parse errors together, so we log
                // at `warn` for all of them — failure to read a dropped file
                // is recoverable (we archive it). The task MUST NOT die.
                warn!(
                    path = %file.display(),
                    error = %e,
                    "failed to read torrent file, moving to archive folder"
                );
                self.move_path_to_archive(file).await;
            }
        }
    }

    async fn on_file_change(&self, file: &Path) {
        info!(path = %file.display(), "torrent file change detected, hot reloading");
        self.on_file_delete(file).await;
        self.on_file_create(file).await;
    }

    async fn on_file_delete(&self, file: &Path) {
        let removed = {
            let mut torrents = self.torrent_files.write().await;
            torrents.remove(file)
        };
        if let Some(torrent) = removed {
            info!(path = %file.display(), "torrent file deletion detected");
            let listeners = self.listeners.read().await.snapshot();
            for listener in listeners {
                listener.on_torrent_file_removed(&torrent);
            }
        }
    }

    /// Archive a raw file path (used by the watcher when a parse fails before
    /// the file enters the map).
    async fn move_path_to_archive(&self, file: &Path) {
        self.archive_path(file, "archived malformed torrent file")
            .await;
    }

    /// Shared archive move: validate the path, build the archive target under
    /// `archive_dir`, and rename with overwrite. `success_msg` is the caller's
    /// success log line. All error paths are logged inside
    /// `rename_with_overwrite`, so a failed move returns quietly here.
    async fn archive_path(&self, file: &Path, success_msg: &str) {
        if !file.exists() {
            return;
        }
        let Some(name) = file.file_name() else {
            warn!(path = %file.display(), "archive target has no file name");
            return;
        };
        let target = self.archive_dir.join(name);
        if rename_with_overwrite(file, &target).await.is_err() {
            return;
        }
        info!(source = %file.display(), message = success_msg, "archived file");
    }
}

fn path_is_torrent_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "torrent")
}

fn notify_to_io(err: &notify::Error) -> io::Error {
    io::Error::other(err.to_string())
}

/// Move `src` to `target`, overwriting if `target` already exists.
///
/// Java uses `Files.move(..., REPLACE_EXISTING)`. `tokio::fs::rename` fails
/// if the target exists on Windows, so we fall back to delete-then-rename.
/// All error paths are logged; returns `Err` so the caller can skip the
/// success-log path.
async fn rename_with_overwrite(src: &Path, target: &Path) -> io::Result<()> {
    match tokio::fs::rename(src, target).await {
        Ok(()) => Ok(()),
        Err(e) => {
            if !target.exists() {
                error!(
                    source = %src.display(),
                    target = %target.display(),
                    error = %e,
                    "failed to archive file, remains in folder"
                );
                return Err(e);
            }
            if let Err(del_err) = tokio::fs::remove_file(target).await {
                error!(
                    source = %src.display(),
                    target = %target.display(),
                    error = %del_err,
                    "failed to archive file: could not remove existing target"
                );
                return Err(del_err);
            }
            if let Err(move_err) = tokio::fs::rename(src, target).await {
                error!(
                    source = %src.display(),
                    target = %target.display(),
                    error = %move_err,
                    "failed to archive file after removing existing target"
                );
                return Err(move_err);
            }
            Ok(())
        }
    }
}
