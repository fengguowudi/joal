//! Application configuration + `joal-conf/` folder layout.
//!
//! Mirrors Java `org.araymond.joal.core.config.*` but also captures the
//! folder conventions that the Java side scattered across
//! `SeedManager.JoalFoldersPath`.
//!
//! ## File layout
//!
//! ```text
//! <joal_conf>/
//! ├── config.json            # AppConfiguration
//! ├── clients/               # *.client files (emulated BitTorrent clients)
//! └── torrents/              # *.torrent files + archived/ subfolder
//! ```
//!
//! The on-disk format of `config.json` is kept byte-compatible with the Java
//! version so existing `joal-conf/` directories can be reused verbatim.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io;

/// Sentinel used in the Java side: `-1.0` means "no target ratio, seed forever".
pub const UPLOAD_RATIO_TARGET_DISABLED: f32 = -1.0;

/// Application configuration persisted as `joal-conf/config.json`.
///
/// Field names match the Java `AppConfiguration` JSON mapping **exactly** —
/// do not rename without a migration plan for existing users. Unknown keys
/// are tolerated (Java uses `@JsonIgnoreProperties(ignoreUnknown = true)`)
/// to keep forward-compatibility with future JOAL versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfiguration {
    #[serde(rename = "minUploadRate")]
    pub min_upload_rate: u64,

    #[serde(rename = "maxUploadRate")]
    pub max_upload_rate: u64,

    /// Lower bound (kB/s) for the simulated **download** speed. `0/0` disables
    /// the download faker entirely — matching the historical behaviour where
    /// `downloaded` and `left` were always reported as `0` to the tracker.
    /// Optional in `config.json` for backward compatibility.
    #[serde(rename = "minDownloadRate", default)]
    pub min_download_rate: u64,

    /// Upper bound (kB/s) for the simulated download speed. See [`min_download_rate`].
    #[serde(rename = "maxDownloadRate", default)]
    pub max_download_rate: u64,

    #[serde(rename = "simultaneousSeed")]
    pub simultaneous_seed: u32,

    /// Filename of the `.client` file inside `joal-conf/clients/`.
    pub client: String,

    #[serde(rename = "keepTorrentWithZeroLeechers")]
    pub keep_torrent_with_zero_leechers: bool,

    /// `-1.0` disables the target. Java treats `null` the same as `-1.0`.
    #[serde(rename = "uploadRatioTarget", default = "default_ratio_target")]
    pub upload_ratio_target: f32,

    /// Optional HTTP proxy host for tracker announces and IP fetching.
    /// Mirrors Java's `http.proxyHost` system property.
    #[serde(rename = "proxyHost", default, skip_serializing_if = "Option::is_none")]
    pub proxy_host: Option<String>,

    /// Optional HTTP proxy port. Only used when `proxy_host` is set.
    /// Mirrors Java's `http.proxyPort` system property.
    #[serde(rename = "proxyPort", default, skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
}

fn default_ratio_target() -> f32 {
    UPLOAD_RATIO_TARGET_DISABLED
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    NotFound(PathBuf),
    #[error("io error reading {path}: {source}")]
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
    #[error("invalid configuration: {0}")]
    Invalid(&'static str),
}

impl AppConfiguration {
    /// Port of Java `AppConfiguration.validate()`. Kept identical in spirit
    /// so existing configs keep their meaning.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // `min_upload_rate` is unsigned so the `< 0` Java check is implicit.
        if self.max_upload_rate < self.min_upload_rate {
            return Err(ConfigError::Invalid(
                "maxUploadRate must be greater than or equal to minUploadRate",
            ));
        }
        if self.max_download_rate < self.min_download_rate {
            return Err(ConfigError::Invalid(
                "maxDownloadRate must be greater than or equal to minDownloadRate",
            ));
        }
        if self.simultaneous_seed < 1 {
            return Err(ConfigError::Invalid(
                "simultaneousSeed must be greater than 0",
            ));
        }
        if self.client.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "client is required, no file name given",
            ));
        }
        if self.upload_ratio_target < 0.0
            && self.upload_ratio_target != UPLOAD_RATIO_TARGET_DISABLED
        {
            return Err(ConfigError::Invalid(
                "uploadRatioTarget must be greater than 0 (or equal to -1)",
            ));
        }
        Ok(())
    }

    /// Returns the proxy URL if both host and port are configured.
    #[must_use]
    pub fn proxy_url(&self) -> Option<String> {
        match (&self.proxy_host, self.proxy_port) {
            (Some(host), Some(port)) if !host.trim().is_empty() => {
                Some(format!("http://{host}:{port}"))
            }
            _ => None,
        }
    }
}

/// The three directories that make up a `joal-conf/`.
///
/// Java equivalent: `SeedManager.JoalFoldersPath`.
#[derive(Debug, Clone)]
pub struct JoalFolders {
    pub conf_root: PathBuf,
    pub clients_dir: PathBuf,
    pub torrents_dir: PathBuf,
    pub torrents_archive_dir: PathBuf,
}

impl JoalFolders {
    #[must_use]
    pub fn new(conf_root: impl Into<PathBuf>) -> Self {
        let conf_root = conf_root.into();
        let clients_dir = conf_root.join("clients");
        let torrents_dir = conf_root.join("torrents");
        let torrents_archive_dir = torrents_dir.join("archived");
        Self {
            conf_root,
            clients_dir,
            torrents_dir,
            torrents_archive_dir,
        }
    }

    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.conf_root.join("config.json")
    }
}

/// Loads `config.json`, validates it, and returns both the parsed config and
/// the folder layout it lives in. Emulates Java
/// `JoalConfigProvider.init() + loadConfiguration()`.
pub async fn load(
    joal_conf_root: impl AsRef<Path>,
) -> Result<(AppConfiguration, JoalFolders), ConfigError> {
    let folders = JoalFolders::new(joal_conf_root.as_ref());
    let path = folders.config_file();

    let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            ConfigError::NotFound(path.clone())
        } else {
            ConfigError::Io {
                path: path.clone(),
                source: e,
            }
        }
    })?;
    if !metadata.is_file() {
        return Err(ConfigError::NotFound(path));
    }

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
    let config: AppConfiguration =
        serde_json::from_slice(&bytes).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;

    config.validate()?;
    Ok((config, folders))
}

/// List all `.client` filenames in the `clients/` directory.
pub async fn list_client_files(folders: &JoalFolders) -> Result<Vec<String>, ConfigError> {
    let mut entries = tokio::fs::read_dir(&folders.clients_dir)
        .await
        .map_err(|source| ConfigError::Io {
            path: folders.clients_dir.clone(),
            source,
        })?;
    let mut names = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "client")
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Serialize + write `config.json` atomically (write-to-temp, then rename).
pub async fn save(folders: &JoalFolders, config: &AppConfiguration) -> Result<(), ConfigError> {
    config.validate()?;
    let path = folders.config_file();
    let tmp = path.with_extension("json.tmp");

    let pretty = serde_json::to_vec_pretty(config).map_err(|source| ConfigError::Parse {
        path: path.clone(),
        source,
    })?;
    tokio::fs::write(&tmp, &pretty)
        .await
        .map_err(|source| ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
    Ok(())
}
