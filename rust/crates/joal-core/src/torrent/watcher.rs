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
            let _ = tx.blocking_send(res);
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
                let _ = handle.await;
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

        let Some(name) = file.file_name() else {
            warn!(path = %file.display(), "archive target has no file name");
            return;
        };
        let target = self.archive_dir.join(name);
        if let Err(e) = tokio::fs::rename(file, &target).await {
            // Java's `Files.move(..., REPLACE_EXISTING)` overwrites. On
            // Windows, `tokio::fs::rename` fails if the target exists, so we
            // delete-then-rename as a fallback. A raw `copy + remove` would
            // also work; `remove + rename` is cheaper.
            if target.exists() {
                if let Err(del_err) = tokio::fs::remove_file(&target).await {
                    error!(
                        source = %file.display(),
                        target = %target.display(),
                        error = %del_err,
                        "failed to archive file: could not remove existing target"
                    );
                    return;
                }
                if let Err(move_err) = tokio::fs::rename(file, &target).await {
                    error!(
                        source = %file.display(),
                        target = %target.display(),
                        error = %move_err,
                        "failed to archive file after removing existing target"
                    );
                    return;
                }
            } else {
                error!(
                    source = %file.display(),
                    target = %target.display(),
                    error = %e,
                    "failed to archive file, remains in folder"
                );
                return;
            }
        }
        info!(source = %file.display(), "successfully moved file to archive folder");
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
        if !file.exists() {
            return;
        }
        let Some(name) = file.file_name() else {
            return;
        };
        let target = self.archive_dir.join(name);
        if let Err(e) = tokio::fs::rename(file, &target).await {
            if target.exists() {
                let _ = tokio::fs::remove_file(&target).await;
                if let Err(move_err) = tokio::fs::rename(file, &target).await {
                    error!(
                        source = %file.display(),
                        error = %move_err,
                        "failed to archive malformed file"
                    );
                    return;
                }
            } else {
                error!(source = %file.display(), error = %e, "failed to archive malformed file");
                return;
            }
        }
        info!(source = %file.display(), "archived malformed torrent file");
    }
}

fn path_is_torrent_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "torrent")
}

fn notify_to_io(err: &notify::Error) -> io::Error {
    io::Error::other(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct CountingListener {
        added: AtomicUsize,
        removed: AtomicUsize,
    }

    impl CountingListener {
        fn new() -> Self {
            Self {
                added: AtomicUsize::new(0),
                removed: AtomicUsize::new(0),
            }
        }
    }

    impl TorrentFileChangeAware for CountingListener {
        fn on_torrent_file_added(&self, _torrent: &MockedTorrent) {
            self.added.fetch_add(1, Ordering::SeqCst);
        }
        fn on_torrent_file_removed(&self, _torrent: &MockedTorrent) {
            self.removed.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Build a valid minimal single-file torrent file with a deterministic
    /// info hash controlled by the `tag` byte.
    fn build_torrent_bytes(tag: u8) -> Vec<u8> {
        let mut pieces = vec![0u8; 20];
        pieces[0] = tag;
        let mut info = Vec::new();
        info.push(b'd');
        info.extend_from_slice(b"6:lengthi10e");
        info.extend_from_slice(b"4:name8:test.bin");
        info.extend_from_slice(b"12:piece lengthi10e");
        info.extend_from_slice(b"6:pieces20:");
        info.extend_from_slice(&pieces);
        info.push(b'e');

        let mut torrent = Vec::new();
        torrent.push(b'd');
        torrent.extend_from_slice(b"8:announce13:http://x/y/za");
        torrent.extend_from_slice(b"4:info");
        torrent.extend_from_slice(&info);
        torrent.push(b'e');
        torrent
    }

    fn joal_folders(tmp: &tempfile::TempDir) -> JoalFolders {
        let folders = JoalFolders::new(tmp.path());
        std::fs::create_dir_all(&folders.torrents_dir).unwrap();
        folders
    }

    #[tokio::test]
    async fn startup_scan_picks_up_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = joal_folders(&tmp);
        let path = folders.torrents_dir.join("sample.torrent");
        tokio::fs::write(&path, build_torrent_bytes(1))
            .await
            .unwrap();

        let provider = TorrentFileProvider::new(&folders).unwrap();
        let listener = Arc::new(CountingListener::new());
        provider
            .register_listener(listener.clone() as Arc<dyn TorrentFileChangeAware>)
            .await;
        provider.start().await.unwrap();

        assert_eq!(listener.added.load(Ordering::SeqCst), 1);
        assert_eq!(provider.torrent_count().await, 1);
        provider.stop().await;
    }

    #[tokio::test]
    async fn get_torrent_not_in_excludes_listed_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = joal_folders(&tmp);
        let a_path = folders.torrents_dir.join("a.torrent");
        let b_path = folders.torrents_dir.join("b.torrent");
        tokio::fs::write(&a_path, build_torrent_bytes(1))
            .await
            .unwrap();
        tokio::fs::write(&b_path, build_torrent_bytes(2))
            .await
            .unwrap();

        let provider = TorrentFileProvider::new(&folders).unwrap();
        provider.start().await.unwrap();
        assert_eq!(provider.torrent_count().await, 2);

        let all = provider.torrent_files().await;
        let hash_a = all
            .iter()
            .find(|t| t.name == "test.bin")
            .unwrap()
            .info_hash
            .clone();
        let mut exclude = HashSet::new();
        exclude.insert(hash_a.clone());
        // At least one of the two torrents has a hash distinct from hash_a.
        let chosen = provider.get_torrent_not_in(&exclude).await.unwrap();
        assert_ne!(chosen.info_hash, hash_a);

        // If we exclude all, it fails.
        let mut exclude_all = HashSet::new();
        for t in &all {
            exclude_all.insert(t.info_hash.clone());
        }
        assert!(provider.get_torrent_not_in(&exclude_all).await.is_err());
        provider.stop().await;
    }

    #[tokio::test]
    async fn archive_moves_file_and_removes_from_catalogue() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = joal_folders(&tmp);
        let path = folders.torrents_dir.join("doomed.torrent");
        tokio::fs::write(&path, build_torrent_bytes(1))
            .await
            .unwrap();

        let provider = TorrentFileProvider::new(&folders).unwrap();
        provider.start().await.unwrap();
        let initial = provider.torrent_files().await;
        let hash = initial[0].info_hash.clone();

        provider.move_to_archive_folder(&hash).await;
        // File moved out of torrents/, archive/ now contains it.
        assert!(!path.exists());
        assert!(folders.torrents_archive_dir.join("doomed.torrent").exists());
        provider.stop().await;
    }

    #[tokio::test]
    async fn malformed_file_is_archived_instead_of_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = joal_folders(&tmp);
        let path = folders.torrents_dir.join("broken.torrent");
        tokio::fs::write(&path, b"not a torrent").await.unwrap();

        let provider = TorrentFileProvider::new(&folders).unwrap();
        provider.start().await.unwrap();
        assert_eq!(provider.torrent_count().await, 0);
        // Wait a very short time (startup scan is synchronous but the archive
        // call is async).
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!path.exists());
        assert!(folders.torrents_archive_dir.join("broken.torrent").exists());
        provider.stop().await;
    }

    #[tokio::test]
    async fn watcher_picks_up_file_dropped_after_start() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = joal_folders(&tmp);

        let provider = TorrentFileProvider::new(&folders).unwrap();
        let listener = Arc::new(CountingListener::new());
        provider
            .register_listener(listener.clone() as Arc<dyn TorrentFileChangeAware>)
            .await;
        provider.start().await.unwrap();
        assert_eq!(provider.torrent_count().await, 0);

        let path = folders.torrents_dir.join("late.torrent");
        tokio::fs::write(&path, build_torrent_bytes(1))
            .await
            .unwrap();

        // Wait for the watcher to observe the creation. `notify` on Windows
        // emits events fairly promptly but not instantly — poll with a
        // deadline so the test is not flaky on slow runners.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if provider.torrent_count().await >= 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "watcher did not observe new file within timeout"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(listener.added.load(Ordering::SeqCst) >= 1);
        provider.stop().await;
    }
}
