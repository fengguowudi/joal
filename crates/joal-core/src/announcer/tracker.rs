//! HTTP tracker client: URI rotation + `reqwest`-based announce round-trip.
//!
//! Port of the Java pair:
//! - `org.araymond.joal.core.ttorrent.client.announcer.tracker.TrackerClientUriProvider`
//! - `org.araymond.joal.core.ttorrent.client.announcer.tracker.TrackerClient`
//!
//! Unlike Java (which uses Apache `HttpClient` through `ttorrent-core`), the
//! Rust side uses [`reqwest`] with the crate-wide `rustls-tls` + `gzip`
//! features configured at the workspace level. The tracker URL is built
//! byte-for-byte the same way Java does:
//!
//! ```text
//! base_uri + ('?' if base_uri has no '?' else '&') + query
//! ```
//!
//! The response bytes flow through [`SuccessAnnounceResponse::parse_with_uri`]
//! which combines bencode parsing and the BEP-3 tracker-error handling.

use std::sync::Mutex;
use std::time::Duration;

use reqwest::Client;
use reqwest::header::{HOST, HeaderMap, HeaderName, HeaderValue};
use tracing::{debug, warn};

use super::error::{AnnouncerError, NoMoreUriAvailableError};
use super::response::SuccessAnnounceResponse;

/// Cycles through the HTTP(S) tracker URIs for one torrent.
///
/// Java uses `Iterators.cycle(...)` on a `List<URI>` that has already been
/// filtered to http/https schemes. Rust keeps a plain [`Vec<String>`] + a
/// current-index cursor, which is behind a [`Mutex`] because a
/// [`TrackerClient`] is shared across announce tasks that may advance the
/// cursor concurrently.
///
/// URIs are stored as raw strings (not parsed [`reqwest::Url`]) because the
/// tracker path needs to round-trip the original `?`-or-not shape byte-for-
/// byte into the final announce URL — re-serializing through a URL parser
/// would risk normalising the host/path in ways Java's `URI#toString()`
/// does not.
pub struct TrackerClientUriProvider {
    inner: Mutex<UriCursor>,
}

struct UriCursor {
    uris: Vec<String>,
    current: usize,
}

impl TrackerClientUriProvider {
    /// Construct from already-filtered HTTP(S) URI strings.
    ///
    /// Mirrors Java's `TrackerClientUriProvider` constructor: filter to
    /// schemes starting with `http` and fail fast on an empty list.
    pub fn new(uris: Vec<String>) -> Result<Self, AnnouncerError> {
        let uris: Vec<String> = uris
            .into_iter()
            .filter(|u| {
                let lower = u.to_ascii_lowercase();
                lower.starts_with("http://") || lower.starts_with("https://")
            })
            .collect();
        if uris.is_empty() {
            return Err(AnnouncerError::NoUrisConfigured);
        }
        Ok(Self {
            inner: Mutex::new(UriCursor { uris, current: 0 }),
        })
    }

    /// Currently selected URI. Infallible — the cursor is non-empty by
    /// construction and [`Self::delete_current_and_move_to_next`] errors
    /// out before it would drain the list.
    pub fn current(&self) -> String {
        let cursor = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cursor.uris[cursor.current].clone()
    }

    /// Move to the next URI. Cycles back to the start when the end is
    /// reached — matches Java `Iterators.cycle` semantics.
    pub fn move_to_next(&self) -> Result<String, AnnouncerError> {
        let mut cursor = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cursor.uris.is_empty() {
            return Err(AnnouncerError::NoMoreUri(NoMoreUriAvailableError::new(
                "No more valid tracker URIs left",
            )));
        }
        cursor.current = (cursor.current + 1) % cursor.uris.len();
        Ok(cursor.uris[cursor.current].clone())
    }

    /// Delete the currently selected URI and advance. If this empties the
    /// list the call fails with [`AnnouncerError::NoMoreUri`].
    pub fn delete_current_and_move_to_next(&self) -> Result<String, AnnouncerError> {
        let mut cursor = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cursor.uris.is_empty() {
            return Err(AnnouncerError::NoMoreUri(NoMoreUriAvailableError::new(
                "No more valid tracker URIs left",
            )));
        }
        let current = cursor.current;
        cursor.uris.remove(current);
        if cursor.uris.is_empty() {
            return Err(AnnouncerError::NoMoreUri(NoMoreUriAvailableError::new(
                "No more valid tracker URIs left",
            )));
        }
        if cursor.current >= cursor.uris.len() {
            cursor.current = 0;
        }
        Ok(cursor.uris[cursor.current].clone())
    }

    /// Snapshot of remaining URI count. Useful for tests and metrics.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .uris
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for TrackerClientUriProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cursor = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.debug_struct("TrackerClientUriProvider")
            .field("current", &cursor.current)
            .field("uri_count", &cursor.uris.len())
            .finish()
    }
}

/// Default per-request timeout. Java relies on Apache HttpClient defaults —
/// Rust makes the value explicit so the announce loop can't hang forever.
pub const DEFAULT_ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(30);

/// Async HTTP tracker client with URI fallback.
///
/// Wraps a [`reqwest::Client`] + a [`TrackerClientUriProvider`].
pub struct TrackerClient {
    uri_provider: TrackerClientUriProvider,
    http: Client,
}

impl std::fmt::Debug for TrackerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackerClient")
            .field("uri_provider", &self.uri_provider)
            .finish_non_exhaustive()
    }
}

impl TrackerClient {
    /// Construct with a default `reqwest::Client` (gzip, rustls, 30s
    /// timeout).
    pub fn new(uri_provider: TrackerClientUriProvider) -> Result<Self, AnnouncerError> {
        let http = Client::builder()
            .timeout(DEFAULT_ANNOUNCE_TIMEOUT)
            .gzip(true)
            .build()?;
        Ok(Self { uri_provider, http })
    }

    /// Build with a caller-supplied `reqwest::Client`. Useful when multiple
    /// announcers should share a connection pool.
    #[must_use]
    pub fn with_http_client(uri_provider: TrackerClientUriProvider, http: Client) -> Self {
        Self { uri_provider, http }
    }

    /// Snapshot accessor — mostly useful for tests and debug logs.
    #[must_use]
    pub fn uri_provider(&self) -> &TrackerClientUriProvider {
        &self.uri_provider
    }

    /// Send one announce and decode its response.
    ///
    /// `headers` come from [`BitTorrentClient::headers`][crate::client::BitTorrentClient::headers]
    /// and are forwarded to the tracker verbatim. On failure the current
    /// tracker URI is advanced (Java: `TrackerClientUriProvider.moveToNext`)
    /// so the caller's retry sees a different tracker.
    pub async fn announce(
        &self,
        request_query: &str,
        headers: &[(String, String)],
    ) -> Result<SuccessAnnounceResponse, AnnouncerError> {
        let base_uri = self.uri_provider.current();
        match self.attempt_once(&base_uri, request_query, headers).await {
            Ok(resp) => Ok(resp),
            Err(err) => {
                // Advance the cursor so the next announce attempts a new
                // tracker; mirrors Java's catch block in TrackerClient.
                if let Err(rotate_err) = self.uri_provider.move_to_next() {
                    warn!(
                        tracker = %base_uri,
                        cause = %err,
                        rotate_error = %rotate_err,
                        "announce failed and no more tracker URIs are available"
                    );
                    return Err(AnnouncerError::NoMoreUri(NoMoreUriAvailableError::new(
                        "No more valid tracker for torrent",
                    )));
                }
                Err(err)
            }
        }
    }

    async fn attempt_once(
        &self,
        base_uri: &str,
        request_query: &str,
        headers: &[(String, String)],
    ) -> Result<SuccessAnnounceResponse, AnnouncerError> {
        let separator = if base_uri.contains('?') { '&' } else { '?' };
        let full_url = format!("{base_uri}{separator}{request_query}");
        debug!(tracker = %base_uri, "sending announce");

        let mut header_map = HeaderMap::new();
        if let Some(host) = host_header_value(base_uri)
            && let Ok(hv) = HeaderValue::from_str(&host)
        {
            header_map.insert(HOST, hv);
        }
        for (name, value) in headers {
            let lower = name.to_ascii_lowercase();
            // Java's tracker client forwards every header the `.client` file
            // declares, but `reqwest` rejects `Host` in HeaderMap (it's set
            // by the client). It also manages `Content-Length` etc. itself.
            // Drop the ones reqwest owns; others pass through unchanged.
            if lower == "host" || lower == "content-length" {
                continue;
            }
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_str(value) else {
                continue;
            };
            header_map.insert(name, value);
        }

        let response = self.http.get(&full_url).headers(header_map).send().await?;

        if response.status().is_server_error() || response.status().is_client_error() {
            warn!(
                tracker = %base_uri,
                status = response.status().as_u16(),
                "tracker returned a non-success HTTP status"
            );
        }

        let body = response.bytes().await?;
        SuccessAnnounceResponse::parse_with_uri(&body, base_uri)
    }
}

/// Extract a `Host:` header value (`host` or `host:port`) from a tracker
/// URI. Handles the default-port case the same way Java does — if the URI
/// carries an explicit port, include it; otherwise omit the `:port` part.
fn host_header_value(uri: &str) -> Option<String> {
    let without_scheme = uri.split_once("://").map_or(uri, |(_, rest)| rest);
    let authority_end = without_scheme
        .find(['/', '?', '#'])
        .unwrap_or(without_scheme.len());
    let authority = &without_scheme[..authority_end];
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_rejects_empty_list() {
        assert!(matches!(
            TrackerClientUriProvider::new(Vec::new()).unwrap_err(),
            AnnouncerError::NoUrisConfigured,
        ));
    }

    #[test]
    fn provider_filters_non_http_schemes() {
        let err = TrackerClientUriProvider::new(vec![
            "udp://tracker.example:6969".to_owned(),
            "wss://tracker.example/ws".to_owned(),
        ])
        .unwrap_err();
        assert!(matches!(err, AnnouncerError::NoUrisConfigured));
    }

    #[test]
    fn provider_cycles_through_uris() {
        let provider = TrackerClientUriProvider::new(vec![
            "http://a/".to_owned(),
            "http://b/".to_owned(),
            "http://c/".to_owned(),
        ])
        .unwrap();
        assert_eq!(provider.current(), "http://a/");
        assert_eq!(provider.move_to_next().unwrap(), "http://b/");
        assert_eq!(provider.move_to_next().unwrap(), "http://c/");
        // Cycle.
        assert_eq!(provider.move_to_next().unwrap(), "http://a/");
    }

    #[test]
    fn provider_exhausts_after_all_deletes() {
        let provider =
            TrackerClientUriProvider::new(vec!["http://a/".to_owned(), "http://b/".to_owned()])
                .unwrap();
        // Delete current (a); survivor is b.
        assert_eq!(
            provider.delete_current_and_move_to_next().unwrap(),
            "http://b/"
        );
        // Delete b; now empty.
        assert!(matches!(
            provider.delete_current_and_move_to_next().unwrap_err(),
            AnnouncerError::NoMoreUri(_),
        ));
    }

    #[test]
    fn host_header_value_no_port() {
        assert_eq!(
            host_header_value("http://tracker.example/announce"),
            Some("tracker.example".to_owned())
        );
    }

    #[test]
    fn host_header_value_with_port() {
        assert_eq!(
            host_header_value("http://tracker.example:8080/announce?x=1"),
            Some("tracker.example:8080".to_owned())
        );
    }

    #[test]
    fn host_header_value_without_scheme() {
        assert_eq!(
            host_header_value("tracker.example/announce"),
            Some("tracker.example".to_owned())
        );
    }
}
