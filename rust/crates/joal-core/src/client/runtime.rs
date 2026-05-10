//! Runtime helpers used by the emulated client.
//!
//! S7 moves [`TorrentSeedStats`][crate::bandwidth::TorrentSeedStats] into its
//! Java-native home (`core::bandwidth`). What remains here is the local
//! seeding endpoint that
//! [`BitTorrentClient::create_request_query`][super::BitTorrentClient::create_request_query]
//! reads at announce time — the `(port, ip_address)` pair that the announcer
//! will own in S8/S9 (Java counterpart:
//! `org.araymond.joal.core.ttorrent.client.ConnectionHandler`).

use std::net::IpAddr;

/// Local seeding endpoint (port + bind address).
///
/// Minimal placeholder for S8/S9; full behaviour comes later.
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
