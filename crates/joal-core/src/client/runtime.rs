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
//! providers and refreshes every 90 minutes. The Rust side implements the same
//! behaviour via [`fetch_public_ip`] which tries multiple providers in random
//! order. The [`SeedManager`][crate::seed_manager] spawns a background task
//! that periodically refreshes the IP.

use std::io;
use std::net::{IpAddr, TcpListener};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rand::seq::SliceRandom;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// IP provider URLs matching Java's `ConnectionHandler.IP_PROVIDERS`.
const IP_PROVIDERS: &[&str] = &[
    "http://whatismyip.akamai.com",
    "http://ipecho.net/plain",
    "http://ip.tyk.nu/",
    "http://l2.io/ip",
    "http://ident.me/",
    "http://icanhazip.com/",
    "https://api.ipify.org",
    "https://ipinfo.io/ip",
    "https://checkip.amazonaws.com",
];

/// How often to refresh the public IP. Java uses 90 minutes.
#[allow(clippy::duration_suboptimal_units)]
pub const IP_REFRESH_INTERVAL: Duration = Duration::from_secs(90 * 60);

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

/// Fetch the public IP from one of the well-known providers.
/// Tries providers in random order until one succeeds.
/// Optionally uses a proxy for the HTTP requests.
pub async fn fetch_public_ip(proxy_url: Option<&str>) -> Option<IpAddr> {
    let mut providers: Vec<&str> = IP_PROVIDERS.to_vec();
    providers.shuffle(&mut rand::thread_rng());

    let mut client_builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/65.0.3325.181 Safari/537.36");

    if let Some(url) = proxy_url
        && let Ok(proxy) = reqwest::Proxy::all(url)
    {
        client_builder = client_builder.proxy(proxy);
    }

    let client = match client_builder.build() {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "failed to build HTTP client for IP fetch");
            return None;
        }
    };

    for provider in providers {
        info!(target: "joal_core::connection_handler", provider, "fetching IP");
        match client.get(provider).send().await {
            Ok(resp) => {
                if let Ok(body) = resp.text().await {
                    let trimmed = body.trim();
                    if let Ok(ip) = trimmed.parse::<IpAddr>() {
                        info!(
                            target: "joal_core::connection_handler",
                            ip = %ip,
                            "successfully fetched public IP"
                        );
                        return Some(ip);
                    }
                }
            }
            Err(e) => {
                warn!(
                    target: "joal_core::connection_handler",
                    provider,
                    error = %e,
                    "failed to fetch IP from provider"
                );
            }
        }
    }
    None
}

/// Spawns a background task that periodically fetches the public IP and
/// updates the shared `ConnectionHandler`. Returns the task handle.
pub fn spawn_ip_refresher(
    connection: Arc<Mutex<ConnectionHandler>>,
    proxy_url: Option<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let ip = fetch_public_ip(proxy_url.as_deref()).await;
            {
                let mut handler = connection.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(addr) = ip {
                    handler.set_ip_address(Some(addr));
                }
            }
            tokio::time::sleep(IP_REFRESH_INTERVAL).await;
        }
    })
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
