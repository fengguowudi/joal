//! Torrent domain: `InfoHash`, `MockedTorrent`, filesystem watcher.
//!
//! Mirrors Java `org.araymond.joal.core.torrent.*` — the minimum a fake
//! seeder needs: parse a `.torrent` file, compute its `info_hash` (SHA-1 of
//! the raw `info` dict bytes, BEP-3), extract the announce URLs and the
//! fields required to validate that `pieces × piece_length` matches the
//! advertised size. The [`watcher`] submodule adds the async [`notify`]-based
//! hot-reload behaviour ported from Java `TorrentFileProvider`.
//!
//! # Equality semantics
//!
//! Java's `InfoHash` is compared by its 20-byte hash. Rust ditto: `PartialEq`
//! / `Hash` are derived from the raw bytes. This keeps torrents deduplicated
//! in a `HashMap` keyed on `InfoHash` identically to the Java side.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use sha1::{Digest, Sha1};
use tokio::io;

use crate::bencode::{self, BencodeError, Value};

pub mod state;
pub mod watcher;

pub use state::{StateStoreError, TorrentFlags, TorrentStateStore};
pub use watcher::{NoMoreTorrentsError, TorrentFileChangeAware, TorrentFileProvider};

/// The 20-byte SHA-1 of a torrent's `info` dictionary (BEP-3 `info_hash`).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct InfoHash([u8; 20]);

impl InfoHash {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Lowercase hex, 40 chars. This is what trackers and UIs display.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(40);
        for b in self.0 {
            s.push(nibble(b >> 4));
            s.push(nibble(b & 0x0f));
        }
        s
    }
}

const fn nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + n - 10) as char,
        _ => '?',
    }
}

impl fmt::Debug for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("InfoHash").field(&self.to_hex()).finish()
    }
}

impl fmt::Display for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Parsed `.torrent` — the subset JOAL actually needs.
#[derive(Debug, Clone)]
pub struct MockedTorrent {
    pub info_hash: InfoHash,
    pub name: String,
    pub total_size: u64,
    pub piece_length: u64,
    pub piece_count: usize,
    /// Primary announce URL (`announce` key). Always present in valid torrents.
    pub announce: String,
    /// `announce-list` (BEP-12) flattened tiers. Empty if the torrent has none.
    pub announce_tiers: Vec<Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum TorrentError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("bencode error: {0}")]
    Bencode(#[from] BencodeError),
    #[error("missing required key `{0}`")]
    MissingKey(&'static str),
    #[error("key `{key}` has the wrong type (expected {expected})")]
    WrongType {
        key: &'static str,
        expected: &'static str,
    },
    #[error("torrent size does not match piece count × piece length")]
    SizeMismatch,
    #[error("`pieces` byte string length ({len}) is not a multiple of 20")]
    InvalidPiecesLength { len: usize },
    #[error("torrent `name` is not valid UTF-8")]
    NonUtf8Name,
    #[error("announce url is not valid UTF-8")]
    NonUtf8Announce,
}

impl MockedTorrent {
    pub async fn from_file(path: impl AsRef<Path>) -> Result<Self, TorrentError> {
        let bytes = tokio::fs::read(path).await?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TorrentError> {
        let top = bencode::parse(bytes)?;
        let info_raw = bencode::extract_info_dict_bytes(bytes)?;
        let info = top_info_dict(&top)?;
        let piece_length = parse_piece_length(info)?;
        let piece_count = parse_piece_count(info)?;
        let total_size = compute_total_size(info)?;
        validate_piece_capacity(piece_count, piece_length, total_size)?;

        Ok(Self {
            info_hash: info_hash_from_raw(info_raw),
            name: parse_name(info)?,
            total_size,
            piece_length,
            piece_count,
            announce: parse_announce(&top)?,
            announce_tiers: parse_announce_tiers(&top),
        })
    }
}

fn info_hash_from_raw(info_raw: &[u8]) -> InfoHash {
    let mut hasher = Sha1::new();
    hasher.update(info_raw);
    let digest = hasher.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest);
    InfoHash::from_bytes(out)
}

fn top_info_dict(top: &Value) -> Result<&BTreeMap<Vec<u8>, Value>, TorrentError> {
    top.get("info")
        .ok_or(TorrentError::MissingKey("info"))?
        .as_dict()
        .ok_or(TorrentError::WrongType {
            key: "info",
            expected: "dict",
        })
}

fn parse_name(info: &BTreeMap<Vec<u8>, Value>) -> Result<String, TorrentError> {
    let name = required_bytes(info, b"name".as_slice(), "info.name")?;
    std::str::from_utf8(name)
        .map(str::to_owned)
        .map_err(|_| TorrentError::NonUtf8Name)
}

fn parse_piece_length(info: &BTreeMap<Vec<u8>, Value>) -> Result<u64, TorrentError> {
    let piece_length = required_int(info, b"piece length".as_slice(), "info.piece length")?;
    if piece_length <= 0 {
        return Err(TorrentError::WrongType {
            key: "info.piece length",
            expected: "positive integer",
        });
    }
    Ok(piece_length as u64)
}

fn parse_piece_count(info: &BTreeMap<Vec<u8>, Value>) -> Result<usize, TorrentError> {
    let pieces = required_bytes(info, b"pieces".as_slice(), "info.pieces")?;
    if !pieces.len().is_multiple_of(20) {
        return Err(TorrentError::InvalidPiecesLength { len: pieces.len() });
    }
    Ok(pieces.len() / 20)
}

fn validate_piece_capacity(
    piece_count: usize,
    piece_length: u64,
    total_size: u64,
) -> Result<(), TorrentError> {
    let capacity_bytes = (piece_count as u64).saturating_mul(piece_length);
    if capacity_bytes < total_size {
        Err(TorrentError::SizeMismatch)
    } else {
        Ok(())
    }
}

fn parse_announce(top: &Value) -> Result<String, TorrentError> {
    let announce = top
        .get("announce")
        .and_then(Value::as_bytes)
        .ok_or(TorrentError::MissingKey("announce"))?;
    std::str::from_utf8(announce)
        .map(str::to_owned)
        .map_err(|_| TorrentError::NonUtf8Announce)
}

fn parse_announce_tiers(top: &Value) -> Vec<Vec<String>> {
    top.get("announce-list")
        .and_then(Value::as_list)
        .map(announce_tiers_from_value)
        .unwrap_or_default()
}

fn announce_tiers_from_value(tiers: &[Value]) -> Vec<Vec<String>> {
    tiers
        .iter()
        .filter_map(Value::as_list)
        .map(announce_tier_urls)
        .filter(|tier| !tier.is_empty())
        .collect()
}

fn announce_tier_urls(urls: &[Value]) -> Vec<String> {
    urls.iter()
        .filter_map(Value::as_bytes)
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::to_owned)
        .collect()
}

fn required_bytes<'a>(
    dict: &'a BTreeMap<Vec<u8>, Value>,
    key: &[u8],
    error_key: &'static str,
) -> Result<&'a [u8], TorrentError> {
    dict.get(key)
        .and_then(Value::as_bytes)
        .ok_or(TorrentError::MissingKey(error_key))
}

fn required_int(
    dict: &BTreeMap<Vec<u8>, Value>,
    key: &[u8],
    error_key: &'static str,
) -> Result<i64, TorrentError> {
    dict.get(key)
        .and_then(Value::as_int)
        .ok_or(TorrentError::MissingKey(error_key))
}

fn compute_total_size(info: &BTreeMap<Vec<u8>, Value>) -> Result<u64, TorrentError> {
    if let Some(files) = info.get(&b"files"[..].to_vec()).and_then(Value::as_list) {
        let mut total: u64 = 0;
        for entry in files {
            let entry = entry.as_dict().ok_or(TorrentError::WrongType {
                key: "info.files[]",
                expected: "dict",
            })?;
            let length = entry
                .get(&b"length"[..].to_vec())
                .and_then(Value::as_int)
                .ok_or(TorrentError::MissingKey("info.files[].length"))?;
            if length < 0 {
                return Err(TorrentError::WrongType {
                    key: "info.files[].length",
                    expected: "non-negative integer",
                });
            }
            total = total
                .checked_add(length as u64)
                .ok_or(TorrentError::SizeMismatch)?;
        }
        Ok(total)
    } else if let Some(length) = info.get(&b"length"[..].to_vec()).and_then(Value::as_int) {
        if length < 0 {
            return Err(TorrentError::WrongType {
                key: "info.length",
                expected: "non-negative integer",
            });
        }
        Ok(length as u64)
    } else {
        Err(TorrentError::MissingKey("info.length or info.files"))
    }
}
