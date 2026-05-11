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

use std::fmt;
use std::path::Path;

use sha1::{Digest, Sha1};
use tokio::io;

use crate::bencode::{self, BencodeError, Value};

pub mod watcher;

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

        let info_hash = {
            let mut hasher = Sha1::new();
            hasher.update(info_raw);
            let digest = hasher.finalize();
            let mut out = [0u8; 20];
            out.copy_from_slice(&digest);
            InfoHash::from_bytes(out)
        };

        let info = top.get("info").ok_or(TorrentError::MissingKey("info"))?;
        let info = info.as_dict().ok_or(TorrentError::WrongType {
            key: "info",
            expected: "dict",
        })?;

        let name = info
            .get(&b"name"[..].to_vec())
            .and_then(Value::as_bytes)
            .ok_or(TorrentError::MissingKey("info.name"))?;
        let name = std::str::from_utf8(name)
            .map_err(|_| TorrentError::NonUtf8Name)?
            .to_owned();

        let piece_length = info
            .get(&b"piece length"[..].to_vec())
            .and_then(Value::as_int)
            .ok_or(TorrentError::MissingKey("info.piece length"))?;
        if piece_length <= 0 {
            return Err(TorrentError::WrongType {
                key: "info.piece length",
                expected: "positive integer",
            });
        }
        let piece_length = piece_length as u64;

        let pieces = info
            .get(&b"pieces"[..].to_vec())
            .and_then(Value::as_bytes)
            .ok_or(TorrentError::MissingKey("info.pieces"))?;
        if !pieces.len().is_multiple_of(20) {
            return Err(TorrentError::InvalidPiecesLength { len: pieces.len() });
        }
        let piece_count = pieces.len() / 20;

        let total_size = compute_total_size(info)?;

        let capacity_bytes = (piece_count as u64).saturating_mul(piece_length);
        if capacity_bytes < total_size {
            return Err(TorrentError::SizeMismatch);
        }

        let announce = top
            .get("announce")
            .and_then(Value::as_bytes)
            .ok_or(TorrentError::MissingKey("announce"))?;
        let announce = std::str::from_utf8(announce)
            .map_err(|_| TorrentError::NonUtf8Announce)?
            .to_owned();

        let announce_tiers = top
            .get("announce-list")
            .and_then(Value::as_list)
            .map(|tiers| {
                tiers
                    .iter()
                    .filter_map(|tier| {
                        tier.as_list().map(|urls| {
                            urls.iter()
                                .filter_map(|u| {
                                    u.as_bytes()
                                        .and_then(|b| std::str::from_utf8(b).ok())
                                        .map(str::to_owned)
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(Self {
            info_hash,
            name,
            total_size,
            piece_length,
            piece_count,
            announce,
            announce_tiers,
        })
    }
}

fn compute_total_size(
    info: &std::collections::BTreeMap<Vec<u8>, Value>,
) -> Result<u64, TorrentError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_hash_hex_is_40_lowercase_chars() {
        let bytes: [u8; 20] = [
            0x00, 0xff, 0xab, 0xcd, 0xef, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        ];
        let h = InfoHash::from_bytes(bytes);
        let hex = h.to_hex();
        assert_eq!(hex.len(), 40);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(hex.chars().all(|c| !c.is_ascii_uppercase()));
        assert!(hex.starts_with("00ffabcdef"));
    }

    #[test]
    fn single_file_torrent_parses() {
        // Minimal single-file torrent: 20 bytes total, 10-byte pieces (2 pieces).
        let pieces = vec![0u8; 40]; // 2 * 20-byte SHA-1
        let mut info = Vec::new();
        info.push(b'd');
        info.extend_from_slice(b"6:lengthi20e");
        info.extend_from_slice(b"4:name4:file");
        info.extend_from_slice(b"12:piece lengthi10e");
        info.extend_from_slice(b"6:pieces40:");
        info.extend_from_slice(&pieces);
        info.push(b'e');

        let mut torrent = Vec::new();
        torrent.push(b'd');
        torrent.extend_from_slice(b"8:announce13:http://x/y/za");
        torrent.extend_from_slice(b"4:info");
        torrent.extend_from_slice(&info);
        torrent.push(b'e');

        let mt = MockedTorrent::from_bytes(&torrent).unwrap();
        assert_eq!(mt.name, "file");
        assert_eq!(mt.total_size, 20);
        assert_eq!(mt.piece_length, 10);
        assert_eq!(mt.piece_count, 2);
        assert_eq!(mt.announce, "http://x/y/za");
        assert!(mt.announce_tiers.is_empty());
    }

    #[test]
    fn rejects_torrent_with_insufficient_pieces() {
        // size=100 but only 1 piece of 10 bytes → 10 < 100 → reject.
        let pieces = vec![0u8; 20];
        let mut info = Vec::new();
        info.push(b'd');
        info.extend_from_slice(b"6:lengthi100e");
        info.extend_from_slice(b"4:name4:file");
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

        assert!(matches!(
            MockedTorrent::from_bytes(&torrent),
            Err(TorrentError::SizeMismatch)
        ));
    }

    #[test]
    fn multi_file_sizes_accumulate() {
        let pieces = vec![0u8; 40];
        let mut info = Vec::new();
        info.push(b'd');
        // "files" key must come before "name" lexicographically; BTreeMap ensures order.
        info.extend_from_slice(b"5:filesl");
        info.extend_from_slice(b"d6:lengthi7e4:pathl1:ae");
        info.extend_from_slice(b"ed6:lengthi13e4:pathl1:be");
        info.extend_from_slice(b"ee");
        info.extend_from_slice(b"4:name3:pkg");
        info.extend_from_slice(b"12:piece lengthi10e");
        info.extend_from_slice(b"6:pieces40:");
        info.extend_from_slice(&pieces);
        info.push(b'e');

        let mut torrent = Vec::new();
        torrent.push(b'd');
        torrent.extend_from_slice(b"8:announce13:http://x/y/za");
        torrent.extend_from_slice(b"4:info");
        torrent.extend_from_slice(&info);
        torrent.push(b'e');

        let mt = MockedTorrent::from_bytes(&torrent).unwrap();
        assert_eq!(mt.total_size, 20);
        assert_eq!(mt.name, "pkg");
    }
}
