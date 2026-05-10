//! Errors produced by the `client` module.
//!
//! Mirrors the Java `TorrentClientConfigIntegrityException` and related
//! `IllegalArgumentException` sites in `emulated/*.java`. Keeping these as a
//! single `thiserror` enum so library consumers can match on specific
//! integrity failures without pulling in `anyhow`.

use thiserror::Error;

/// Errors raised while validating / constructing emulated-client state.
#[derive(Debug, Error)]
pub enum ClientError {
    /// A `.client` file (or an in-memory equivalent) is internally
    /// inconsistent: generator disabled but referenced in the query string,
    /// empty required field, invalid bounds, etc.
    #[error("torrent client config integrity error: {0}")]
    Integrity(String),

    /// The regex supplied to a generator cannot be compiled by `rand_regex`.
    #[error("invalid regex pattern: {0}")]
    InvalidRegex(String),

    /// A generator produced an output that cannot be expressed as UTF-8
    /// (for byte-level regex classes). JOAL only emits ASCII patterns so
    /// this should never fire in normal usage; treat it as a hard error.
    #[error("generator produced non-UTF-8 bytes: {0}")]
    NonUtf8Output(String),
}
