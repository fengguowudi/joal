//! Builds [`Announcer`] instances from parsed `.torrent` files.
//!
//! Port of Java `org.araymond.joal.core.ttorrent.client.announcer.AnnouncerFactory`.
//! The Rust version accepts the shared building blocks once and produces a
//! fully-wired announcer per torrent: it chooses the tracker URIs (preferring
//! `announce-list` tiers when non-empty), hands them off to a
//! [`TrackerClient`], and threads the shared
//! [`AnnounceDataAccessor`][crate::announcer::AnnounceDataAccessor].

use std::sync::Arc;

use reqwest::Client;
use tracing::warn;

use crate::announcer::{
    AnnounceDataAccessor, Announcer, AnnouncerError, TrackerClient, TrackerClientUriProvider,
};
use crate::torrent::MockedTorrent;

/// Shared factory. Hold one of these on the orchestrator and use it to build
/// every announcer the seeding pool needs.
#[derive(Clone)]
pub struct AnnouncerFactory {
    data_accessor: AnnounceDataAccessor,
    http: Client,
    upload_ratio_target: f32,
}

impl AnnouncerFactory {
    #[must_use]
    pub fn new(
        data_accessor: AnnounceDataAccessor,
        http: Client,
        upload_ratio_target: f32,
    ) -> Self {
        Self {
            data_accessor,
            http,
            upload_ratio_target,
        }
    }

    /// Build an announcer for `torrent`. Returns [`AnnouncerError::NoUrisConfigured`]
    /// when every tracker URI filters out (non-http/https).
    pub fn create(&self, torrent: MockedTorrent) -> Result<Arc<Announcer>, AnnouncerError> {
        let uris = collect_tracker_uris(&torrent);
        if uris.is_empty() {
            warn!(
                info_hash = %torrent.info_hash,
                name = %torrent.name,
                "no usable http(s) trackers in torrent"
            );
            return Err(AnnouncerError::NoUrisConfigured);
        }
        let uri_provider = TrackerClientUriProvider::new(uris)?;
        let tracker_client = TrackerClient::with_http_client(uri_provider, self.http.clone());
        Ok(Arc::new(Announcer::new(
            torrent,
            tracker_client,
            self.data_accessor.clone(),
            self.upload_ratio_target,
        )))
    }

    #[must_use]
    pub fn data_accessor(&self) -> &AnnounceDataAccessor {
        &self.data_accessor
    }
}

impl std::fmt::Debug for AnnouncerFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnouncerFactory")
            .field("upload_ratio_target", &self.upload_ratio_target)
            .finish_non_exhaustive()
    }
}

/// Collect the tracker URIs used by an announcer. Prefers `announce-list`
/// tiers (BEP-12) and falls back to the primary `announce`.
fn collect_tracker_uris(torrent: &MockedTorrent) -> Vec<String> {
    if torrent.announce_tiers.is_empty() {
        return vec![torrent.announce.clone()];
    }
    let mut out = Vec::new();
    for tier in &torrent.announce_tiers {
        out.extend(tier.iter().cloned());
    }
    if out.is_empty() {
        out.push(torrent.announce.clone());
    }
    out
}
