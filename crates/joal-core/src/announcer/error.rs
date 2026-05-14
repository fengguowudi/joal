//! Errors surfaced by the announcer layer.
//!
//! The Java codebase splits announcer failures across three types:
//!
//! - `AnnounceException` (from `ttorrent-core`) — a catch-all wrapper for
//!   HTTP, protocol, and tracker-reported failures
//! - `NoMoreUriAvailableException` — every URI in the torrent's
//!   `announce-list` has been exhausted
//! - `TooManyAnnouncesFailedInARowException` — the hard failure threshold
//!   (5 consecutive failures, see [`MAX_CONSECUTIVE_FAILURES`]) has been
//!   crossed
//!
//! Rust side keeps the same taxonomy but expresses it as a single
//! [`AnnouncerError`] enum plus two sentinel unit errors for the checked
//! "recoverable" failures. Matching on the enum lets callers differentiate
//! transient tracker errors (retry after `interval`) from terminal ones
//! (surface to the UI / stop announcing) without relying on exception
//! instance-of checks.
//!
//! [`MAX_CONSECUTIVE_FAILURES`]: crate::announcer::state::MAX_CONSECUTIVE_FAILURES

use std::io;

use thiserror::Error;

use crate::bencode::BencodeError;
use crate::torrent::InfoHash;

/// Every URI in the torrent's `announce-list` has been tried and removed or
/// rotated past.
///
/// Java counterpart: `NoMoreUriAvailableException`.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct NoMoreUriAvailableError {
    pub message: String,
}

impl NoMoreUriAvailableError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// The announcer has failed [`MAX_CONSECUTIVE_FAILURES`][crate::announcer::state::MAX_CONSECUTIVE_FAILURES]
/// times in a row — the caller is expected to stop scheduling announces for
/// the torrent until the user intervenes.
///
/// Java counterpart: `TooManyAnnouncesFailedInARowException`.
#[derive(Debug, Error)]
#[error("announcer for torrent {info_hash} failed {consecutive_fails} times in a row")]
pub struct TooManyFailuresError {
    pub info_hash: InfoHash,
    pub consecutive_fails: u32,
}

/// Unified announcer failure type. Matches the observable behaviour of the
/// Java `AnnounceException` + `NoMoreUriAvailableException` pair.
#[derive(Debug, Error)]
pub enum AnnouncerError {
    /// The HTTP layer failed (DNS, TCP, TLS, timeout, connection aborted).
    ///
    /// Java: `AnnounceException("Failed to announce: error or connection aborted", e)`.
    #[error("announce http error: {0}")]
    Http(String),

    /// The tracker returned an HTTP response but its body could not be parsed
    /// as bencode.
    ///
    /// Java: `AnnounceException("Error reading tracker response!", ioe)`.
    #[error("tracker response was not valid bencode: {0}")]
    InvalidResponse(#[from] BencodeError),

    /// The tracker responded with a bencode-encoded `failure reason`
    /// (BEP-3). Java treats this as an `ErrorMessage` and throws
    /// `AnnounceException` with the reason.
    #[error("tracker {uri} reported failure: {reason}")]
    TrackerReported { uri: String, reason: String },

    /// The tracker's response was syntactically bencode but was missing a
    /// field required by BEP-3 (`interval`, `complete`, `incomplete`) and
    /// did not carry a `failure reason` either. Java surfaces this through
    /// `HTTPTrackerMessage.parse` validation.
    #[error("tracker response is incomplete: missing `{0}`")]
    IncompleteResponse(&'static str),

    /// The announcer has no more tracker URIs to try.
    #[error(transparent)]
    NoMoreUri(#[from] NoMoreUriAvailableError),

    /// The torrent's announce list contained no usable HTTP/HTTPS URIs at
    /// construction time. Java throws `NoMoreUriAvailableException` from the
    /// `TrackerClientUriProvider` constructor in that case.
    #[error("no valid http trackers provided")]
    NoUrisConfigured,

    /// Hard threshold crossed — the announcer has failed
    /// [`MAX_CONSECUTIVE_FAILURES`][crate::announcer::state::MAX_CONSECUTIVE_FAILURES]
    /// times in a row for this torrent.
    #[error(transparent)]
    TooManyFailures(#[from] TooManyFailuresError),

    /// Underlying `std::io` error (wraps e.g. socket binding failures
    /// produced by the announcer support code).
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

impl From<reqwest::Error> for AnnouncerError {
    fn from(err: reqwest::Error) -> Self {
        // reqwest's `Error::to_string()` is already well-structured
        // ("error sending request for url (...)" / "error decoding response body" / etc.).
        AnnouncerError::Http(err.to_string())
    }
}
