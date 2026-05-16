//! Per-torrent persistent UI state, stored in `joal-conf/torrent_state.json`.
//!
//! This is a Rust-only addition (no Java equivalent): the UI lets the user
//! mark a torrent as "initial completed" so the bandwidth dispatcher will
//! seed it as if the download already finished. The state must survive
//! restarts, so it lives next to `config.json` rather than inside it
//! (`config.json` stays purely global).
//!
//! Schema (stable, additive only):
//! ```json
//! {
//!   "<info_hash_hex>": { "initialCompleted": true },
//!   ...
//! }
//! ```
//! Missing file == empty map. Unknown keys are tolerated for forward-compat.
//! Entries that match the default (`initialCompleted: false`) are pruned on
//! save to keep the file small.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tokio::io;
use tracing::warn;

use crate::config::JoalFolders;
use crate::torrent::InfoHash;

/// On-disk per-torrent flags. Field naming is camelCase to match the rest of
/// the JOAL JSON files. New fields **must** be `#[serde(default)]` and
/// optional in semantics so older state files keep loading.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentFlags {
    /// User asked the torrent to start with `downloaded == total_size`.
    #[serde(rename = "initialCompleted", default)]
    pub initial_completed: bool,
}

impl TorrentFlags {
    const fn is_default(self) -> bool {
        !self.initial_completed
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StateStoreError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Concurrent in-memory cache backed by an atomic JSON file.
///
/// Reads (`flags_for`, `is_initial_completed`) take a read lock; writes
/// (`set_initial_completed`) take a write lock and synchronously persist
/// before returning so the UI's "checkbox click → restart" flow sees the
/// updated state on the next load.
#[derive(Debug)]
pub struct TorrentStateStore {
    path: PathBuf,
    inner: RwLock<HashMap<String, TorrentFlags>>,
}

impl TorrentStateStore {
    /// Load `joal-conf/torrent_state.json`. Missing file returns an empty
    /// store (not an error). Malformed JSON is logged and the store starts
    /// empty so a corrupted file never blocks the engine from starting.
    pub async fn load(folders: &JoalFolders) -> Arc<Self> {
        let path = folders.conf_root.join("torrent_state.json");
        let map = match tokio::fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<HashMap<String, TorrentFlags>>(&bytes) {
                Ok(m) => m,
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "torrent_state.json is malformed; starting with an empty store",
                    );
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "could not read torrent_state.json; starting with an empty store",
                );
                HashMap::new()
            }
        };
        Arc::new(Self {
            path,
            inner: RwLock::new(map),
        })
    }
    /// Snapshot of the flags for a given torrent (default if absent).
    /// Synchronous: callable from announce-side hooks that don't have an
    /// `await` available.
    pub fn flags_for(&self, info_hash: &InfoHash) -> TorrentFlags {
        let guard = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.get(&info_hash.to_hex()).copied().unwrap_or_default()
    }

    pub fn is_initial_completed(&self, info_hash: &InfoHash) -> bool {
        self.flags_for(info_hash).initial_completed
    }

    /// Update the flag and persist atomically. Errors are returned to the
    /// caller (UI layer logs them) but the in-memory cache is updated either
    /// way so the running engine reflects the user's intent.
    pub async fn set_initial_completed(
        &self,
        info_hash: &InfoHash,
        completed: bool,
    ) -> Result<(), StateStoreError> {
        let key = info_hash.to_hex();
        let snapshot = {
            let mut guard = self
                .inner
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = guard.entry(key).or_default();
            entry.initial_completed = completed;
            // Prune defaults to keep the file small.
            guard.retain(|_, flags| !flags.is_default());
            guard.clone()
        };
        self.persist(&snapshot).await
    }

    /// Drop a torrent's entry entirely (used when a torrent is removed).
    pub async fn forget(&self, info_hash: &InfoHash) -> Result<(), StateStoreError> {
        let snapshot = {
            let mut guard = self
                .inner
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.remove(&info_hash.to_hex());
            guard.clone()
        };
        self.persist(&snapshot).await
    }

    async fn persist(&self, map: &HashMap<String, TorrentFlags>) -> Result<(), StateStoreError> {
        let bytes = serde_json::to_vec_pretty(map).map_err(|source| StateStoreError::Parse {
            path: self.path.clone(),
            source,
        })?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|source| StateStoreError::Io {
                path: tmp.clone(),
                source,
            })?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .map_err(|source| StateStoreError::Io {
                path: self.path.clone(),
                source,
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> InfoHash {
        InfoHash::from_bytes([byte; 20])
    }

    #[tokio::test]
    async fn missing_file_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = JoalFolders::new(tmp.path());
        let store = TorrentStateStore::load(&folders).await;
        assert!(!store.is_initial_completed(&hash(0xaa)));
    }

    #[tokio::test]
    async fn set_then_reload_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = JoalFolders::new(tmp.path());
        let store = TorrentStateStore::load(&folders).await;
        let h = hash(0x11);
        store.set_initial_completed(&h, true).await.unwrap();

        // Drop and reload from disk.
        drop(store);
        let store = TorrentStateStore::load(&folders).await;
        assert!(store.is_initial_completed(&h));
    }

    #[tokio::test]
    async fn unset_prunes_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = JoalFolders::new(tmp.path());
        let store = TorrentStateStore::load(&folders).await;
        let h = hash(0x22);
        store.set_initial_completed(&h, true).await.unwrap();
        store.set_initial_completed(&h, false).await.unwrap();

        // File should now contain `{}` (no default rows).
        let bytes = tokio::fs::read(folders.conf_root.join("torrent_state.json"))
            .await
            .unwrap();
        let parsed: HashMap<String, TorrentFlags> = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.is_empty());
    }

    #[tokio::test]
    async fn malformed_file_falls_back_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let folders = JoalFolders::new(tmp.path());
        tokio::fs::write(
            folders.conf_root.join("torrent_state.json"),
            b"this is not json",
        )
        .await
        .unwrap();
        let store = TorrentStateStore::load(&folders).await;
        assert!(!store.is_initial_completed(&hash(0x33)));
    }
}
