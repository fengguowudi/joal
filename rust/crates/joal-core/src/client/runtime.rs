//! Runtime helpers used by the emulated client.
//!
//! S7 moves [`TorrentSeedStats`][crate::bandwidth::TorrentSeedStats] into its
//! Java-native home (`core::bandwidth`). What remains here is the local
//! seeding endpoint that
//! [`BitTorrentClient::create_request_query`][super::BitTorrentClient::create_request_query]
//! reads at announce time — the `(port, ip_address)` pair owned by the
//! announcer (Java counterpart:
//! `org.araymond.joal.core.ttorrent.client.ConnectionHandler`).
//!
//! # Port allocation
//!
//! Java `ConnectionHandler` scans the `[49152, 65534]` ephemeral / dynamic
//! port range and binds the first port that accepts a listen socket. The Rust
//! side does the same via [`std::net::TcpListener::bind("0.0.0.0:0")`][std::net::TcpListener]
//! when the caller uses [`ConnectionHandler::with_ephemeral_port`] — the OS
//! picks a port inside its ephemeral range. For determinism the port can also
//! be fixed via [`ConnectionHandler::new`].
//!
//! # IP address
//!
//! Java resolves the public IP by polling third-party HTTP "what-is-my-ip"
//! providers. That is an online-only behaviour and not safe to pull in as a
//! dependency for a library that must compile + test offline. S8 therefore
//! exposes the IP as an `Option<IpAddr>`: `None` renders `{ip}` / `{ipv6}`
//! as empty (Java `ConnectionHandler.getIpAddress()` can likewise return
//! `null` before `start()` runs, and the `BitTorrentClient` query builder
//! strips the `&ip={ip}` fragment when that happens). Upstream callers that
//! want a detected public IP can set it explicitly after construction.

use std::io;
use std::net::{IpAddr, TcpListener};

/// Local seeding endpoint (port + optional bind address).
///
/// Cheap to clone: both fields are `Copy`. The IP is optional because the
/// Java reference also allows a `null` IP before the background resolver
/// thread populates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionHandler {
    port: u16,
    ip_address: Option<IpAddr>,
}

impl ConnectionHandler {
    /// Build a handler with a fixed port and IP.
    ///
    /// Used by deterministic tests and by any caller that pre-resolves the
    /// public endpoint out of band.
    #[must_use]
    pub const fn new(port: u16, ip_address: IpAddr) -> Self {
        Self {
            port,
            ip_address: Some(ip_address),
        }
    }

    /// Build a handler with a fixed port and no advertised IP. `{ip}` /
    /// `{ipv6}` placeholders will be stripped from announce URLs in that case.
    #[must_use]
    pub const fn with_port_only(port: u16) -> Self {
        Self {
            port,
            ip_address: None,
        }
    }

    /// Bind a random ephemeral-range port and return a handler that uses it.
    ///
    /// This mirrors Java `ConnectionHandler.bindToPort()` in spirit: the OS
    /// picks an available ephemeral port. The listener is immediately
    /// dropped (S8 does not yet implement peer-wire; the port number is all
    /// the tracker cares about) but the port is recorded for the announce
    /// URL builder.
    pub fn with_ephemeral_port() -> io::Result<Self> {
        // Java scans 49152..=65534; we defer to the OS kernel which, on
        // every platform JOAL targets (Linux, Windows, macOS), picks from
        // the standard ephemeral range. The observable tracker-visible
        // effect is identical.
        let listener = TcpListener::bind(("0.0.0.0", 0))?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            port,
            ip_address: None,
        })
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Advertised IP for the `{ip}` / `{ipv6}` placeholders, if any.
    ///
    /// `None` is valid — the announce URL builder drops the placeholder
    /// entirely in that case, matching Java's `null`-handling.
    #[must_use]
    pub const fn ip_address(&self) -> Option<IpAddr> {
        self.ip_address
    }

    /// Attach or replace the advertised IP. Intended for upstream callers
    /// that resolve the public IP asynchronously after construction.
    pub fn set_ip_address(&mut self, ip: Option<IpAddr>) {
        self.ip_address = ip;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn constructors_populate_fields() {
        let h = ConnectionHandler::new(1234, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(h.port(), 1234);
        assert_eq!(h.ip_address(), Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));

        let h = ConnectionHandler::with_port_only(4321);
        assert_eq!(h.port(), 4321);
        assert!(h.ip_address().is_none());
    }

    #[test]
    fn ephemeral_port_is_nonzero() {
        let h = ConnectionHandler::with_ephemeral_port().expect("bind ephemeral port");
        assert!(h.port() > 0);
        assert!(h.ip_address().is_none());
    }

    #[test]
    fn set_ip_address_updates_field() {
        let mut h = ConnectionHandler::with_port_only(1);
        h.set_ip_address(Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))));
        assert_eq!(
            h.ip_address(),
            Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)))
        );
    }
}
