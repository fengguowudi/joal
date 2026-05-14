//! Shared fixtures + Java-compatibility golden samples.
//!
//! Kept deliberately small; this crate exposes repo-embedded sample `.client`
//! files that integration tests can deserialize without reaching outside the
//! Cargo workspace root at runtime.

#![forbid(unsafe_code)]

/// Raw contents of `resources/clients/qbittorrent-4.5.0.client`.
#[must_use]
pub fn sample_client_file() -> &'static str {
    include_str!("../../../resources/clients/qbittorrent-4.5.0.client")
}
