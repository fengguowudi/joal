//! Minimal placeholder for S7/S9; full behavior comes later.
//!
//! S6 needs a concrete shape for the two values that feed into
//! [`BitTorrentClient::create_request_query`](super::BitTorrentClient::create_request_query):
//!
//! - [`TorrentSeedStats`] is the running upload/download/left counter that the
//!   bandwidth dispatcher will fill in S7. Java counterpart:
//!   `org.araymond.joal.core.bandwith.TorrentSeedStats`.
//! - [`ConnectionHandler`] is the `(port, ip_address)` pair that the announcer
//!   will own in S8/S9. Java counterpart:
//!   `org.araymond.joal.core.ttorrent.client.ConnectionHandler`.
//!
//! Both types here are kept intentionally tiny — they expose the exact surface
//! `create_request_query` reads and nothing more. The seed-manager and
//! bandwidth modules will extend them (or wrap them) when they come online.

use std::net::IpAddr;

/// Running torrent statistics for the announce query.
///
/// Minimal placeholder for S7/S9; full behavior comes later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TorrentSeedStats {
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
}

impl TorrentSeedStats {
    #[must_use]
    pub const fn new(uploaded: u64, downloaded: u64, left: u64) -> Self {
        Self {
            uploaded,
            downloaded,
            left,
        }
    }
}

/// Local seeding endpoint (port + bind address).
///
/// Minimal placeholder for S7/S9; full behavior comes later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionHandler {
    port: u16,
    ip_address: IpAddr,
}

impl ConnectionHandler {
    #[must_use]
    pub const fn new(port: u16, ip_address: IpAddr) -> Self {
        Self { port, ip_address }
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub const fn ip_address(&self) -> IpAddr {
        self.ip_address
    }
}
