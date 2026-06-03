//! Announce-request plumbing: the value object + the data accessor.
//!
//! Port of:
//! - `org.araymond.joal.core.ttorrent.client.announcer.request.AnnounceRequest`
//! - `org.araymond.joal.core.ttorrent.client.announcer.request.AnnounceDataAccessor`
//!
//! The Rust `AnnounceRequest` is an immutable value (`InfoHash + RequestEvent`);
//! it does not hold a reference to the [`Announcer`][super::state::Announcer]
//! itself because the Rust caller already has that handle. The
//! [`AnnounceDataAccessor`] is the shared glue that combines the runtime
//! [`BitTorrentClient`][crate::client::BitTorrentClient] + the
//! [`BandwidthDispatcher`][crate::bandwidth::BandwidthDispatcher] +
//! [`ConnectionHandler`][crate::client::ConnectionHandler] into the final
//! announce URL query string.

use std::sync::Arc;

use crate::bandwidth::BandwidthDispatcher;
use crate::client::{BitTorrentClient, ConnectionHandler, RequestEvent};
use crate::torrent::InfoHash;

use super::error::AnnouncerError;

/// Value object describing a single pending announce.
///
/// Matches Java `AnnounceRequest`. Keeping it `Clone + Copy-like` makes it
/// trivial to queue multiple events per torrent without re-allocating.
#[derive(Debug, Clone)]
pub struct AnnounceRequest {
    info_hash: InfoHash,
    event: RequestEvent,
}

impl AnnounceRequest {
    /// Construct a `STARTED` request — the first announce JOAL sends after
    /// picking up a new torrent.
    #[must_use]
    pub fn create_start(info_hash: InfoHash) -> Self {
        Self {
            info_hash,
            event: RequestEvent::Started,
        }
    }

    /// Construct a regular periodic announce (`event=` omitted on the wire).
    #[must_use]
    pub fn create_regular(info_hash: InfoHash) -> Self {
        Self {
            info_hash,
            event: RequestEvent::None,
        }
    }

    /// Construct a `STOPPED` request — fired once when a torrent leaves the
    /// seeding pool.
    #[must_use]
    pub fn create_stop(info_hash: InfoHash) -> Self {
        Self {
            info_hash,
            event: RequestEvent::Stopped,
        }
    }

    /// Sibling request that replaces the current event with `STOPPED`.
    #[must_use]
    pub fn to_stop(&self) -> Self {
        Self::create_stop(self.info_hash.clone())
    }

    #[must_use]
    pub fn info_hash(&self) -> &InfoHash {
        &self.info_hash
    }

    #[must_use]
    pub fn event(&self) -> RequestEvent {
        self.event
    }
}

/// Shared accessor that knows how to turn an `(InfoHash, RequestEvent)` pair
/// into a byte-compatible tracker query string + header list.
///
/// Mirrors Java `AnnounceDataAccessor`. Kept cheap to clone via `Arc`s so a
/// single instance can be shared across an entire announcer pool.
#[derive(Debug, Clone)]
pub struct AnnounceDataAccessor {
    client: Arc<BitTorrentClient>,
    bandwidth: Arc<BandwidthDispatcher>,
    connection: Arc<ConnectionHandler>,
}

impl AnnounceDataAccessor {
    #[must_use]
    pub fn new(
        client: Arc<BitTorrentClient>,
        bandwidth: Arc<BandwidthDispatcher>,
        connection: Arc<ConnectionHandler>,
    ) -> Self {
        Self {
            client,
            bandwidth,
            connection,
        }
    }

    /// Build the full announce URL query string.
    ///
    /// This is the Rust twin of
    /// `AnnounceDataAccessor.getHttpRequestQueryForTorrent(infoHash, event)`.
    /// All placeholders (`{infohash}`, `{peerid}`, `{key}`, `{event}`,
    /// `{uploaded}`, `{downloaded}`, `{left}`, `{port}`, `{numwant}`,
    /// `{ip}`, `{ipv6}`) are resolved here in the exact order used by
    /// `BitTorrentClient::create_request_query`, which itself was written
    /// to match the Java version line-for-line.
    pub fn http_request_query_for_torrent(
        &self,
        info_hash: &InfoHash,
        event: RequestEvent,
    ) -> Result<String, AnnouncerError> {
        let stats = self.bandwidth.get_seed_stat_for_torrent(info_hash);
        self.client
            .create_request_query(event, info_hash, &stats, &self.connection)
            .map_err(|e| AnnouncerError::Http(format!("failed to build announce query: {e}")))
    }

    /// The resolved HTTP headers for every announce request. Sent verbatim
    /// on the wire.
    #[must_use]
    pub fn http_headers_for_torrent(&self) -> &[(String, String)] {
        self.client.headers()
    }

    /// Current `uploaded` counter for the torrent, used by
    /// [`Announcer`][super::state::Announcer] to measure progress towards
    /// `uploadRatioTarget`.
    #[must_use]
    pub fn uploaded(&self, info_hash: &InfoHash) -> u64 {
        self.bandwidth
            .get_seed_stat_for_torrent(info_hash)
            .uploaded()
    }
}
