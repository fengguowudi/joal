//! `.client` file discovery + loader.
//!
//! Port of Java `org.araymond.joal.core.client.emulated.BitTorrentClientProvider`.
//! The Java side caches a single `BitTorrentClient` instance behind a
//! `javax.inject.Provider`; the Rust side is stateless — callers hold onto
//! the returned `BitTorrentClient` themselves.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::client::bit_torrent_client::BitTorrentClient;
use crate::client::config::BitTorrentClientConfig;
use crate::client::error::ClientError;

/// Scans a `clients/` directory for `.client` files and builds runtime
/// `BitTorrentClient` instances from them.
#[derive(Debug, Clone)]
pub struct BitTorrentClientProvider {
    clients_dir: PathBuf,
}

impl BitTorrentClientProvider {
    #[must_use]
    pub fn new(clients_dir: impl Into<PathBuf>) -> Self {
        Self {
            clients_dir: clients_dir.into(),
        }
    }

    /// Directory scanned by this provider.
    #[must_use]
    pub fn clients_dir(&self) -> &Path {
        &self.clients_dir
    }

    /// List the `*.client` file names (not full paths) under
    /// [`clients_dir`](Self::clients_dir), sorted with the semantic-version
    /// comparator used by the Java UI.
    pub async fn list_client_files(&self) -> Result<Vec<String>, ClientError> {
        let mut entries = tokio::fs::read_dir(&self.clients_dir).await.map_err(|e| {
            ClientError::Integrity(format!(
                "Failed to list .client files in [{}]: {e}",
                self.clients_dir.display()
            ))
        })?;

        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            ClientError::Integrity(format!(
                "Failed to iterate .client files in [{}]: {e}",
                self.clients_dir.display()
            ))
        })? {
            let file_type = entry.file_type().await.map_err(|e| {
                ClientError::Integrity(format!("Failed to stat {}: {e}", entry.path().display()))
            })?;
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".client") {
                names.push(name);
            }
        }

        names.sort_by(|a, b| compare_semver_filenames(a, b));
        Ok(names)
    }

    /// Load a single `.client` file by name (relative to the clients dir)
    /// and turn it into a runtime [`BitTorrentClient`].
    pub async fn load(&self, file_name: &str) -> Result<BitTorrentClient, ClientError> {
        let path = self.clients_dir.join(file_name);
        let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
            ClientError::Integrity(format!(
                "BitTorrent client configuration file [{}] not found: {e}",
                path.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(ClientError::Integrity(format!(
                "BitTorrent client configuration file [{}] not found",
                path.display()
            )));
        }

        let contents = tokio::fs::read_to_string(&path).await.map_err(|e| {
            ClientError::Integrity(format!(
                "Failed to read .client file [{}]: {e}",
                path.display()
            ))
        })?;
        let config = BitTorrentClientConfig::try_from(contents.as_str())?;
        BitTorrentClient::new(config)
    }
}

/// Port of Java `SemanticVersionFilenameComparator`.
///
/// Compares two `<client-name>-<version>.client` file names. `_` in the
/// version segment is treated as `.` (Java's build-number convention, e.g.
/// `utorrent-3.5.0_43916.client`). Client names are compared case-insensitive;
/// when they differ the comparator falls back to natural string order. Missing
/// trailing version segments are treated as `0`.
///
/// Malformed version segments (non-numeric) degrade to a plain `str::cmp` so
/// that a single broken file name cannot poison an entire directory listing.
fn compare_semver_filenames(a: &str, b: &str) -> Ordering {
    let a_norm = strip_client_ext(a).replace('_', ".");
    let b_norm = strip_client_ext(b).replace('_', ".");

    let (Some(a_dash), Some(b_dash)) = (a_norm.rfind('-'), b_norm.rfind('-')) else {
        return a.cmp(b);
    };
    let a_name = &a_norm[..a_dash];
    let b_name = &b_norm[..b_dash];

    if !a_name.eq_ignore_ascii_case(b_name) {
        return a.cmp(b);
    }

    let a_version = &a_norm[a_dash + 1..];
    let b_version = &b_norm[b_dash + 1..];
    let a_parts: Vec<&str> = a_version.split('.').collect();
    let b_parts: Vec<&str> = b_version.split('.').collect();
    let len = a_parts.len().max(b_parts.len());

    for i in 0..len {
        let ap = a_parts.get(i).copied().unwrap_or("0");
        let bp = b_parts.get(i).copied().unwrap_or("0");
        let (Ok(ai), Ok(bi)) = (ap.parse::<u64>(), bp.parse::<u64>()) else {
            tracing::warn!(
                "semver compare: non-numeric version segment between {a:?} and {b:?}, falling back to lexical order"
            );
            return a.cmp(b);
        };
        match ai.cmp(&bi) {
            Ordering::Equal => {}
            non_equal => return non_equal,
        }
    }
    Ordering::Equal
}

fn strip_client_ext(name: &str) -> String {
    name.strip_suffix(".client").unwrap_or(name).to_owned()
}
