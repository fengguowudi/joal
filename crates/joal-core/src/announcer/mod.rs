//! Tracker HTTP announcer.
//!
//! Port of Java `org.araymond.joal.core.ttorrent.client.announcer.*` — builds
//! announce URLs that are **byte-compatible** with `ttorrent-core 1.5` so a
//! tracker cannot distinguish a Rust JOAL from a Java JOAL by URL shape
//! alone, sends them over async HTTP via [`reqwest`], parses bencode tracker
//! responses, tracks consecutive failure counts, and rotates through the
//! torrent's `announce-list` tiers when a tracker fails.
//!
//! ## Module map
//!
//! | Module | Java counterpart |
//! |--------|------------------|
//! | [`error`] | `AnnounceException` / `NoMoreUriAvailableException` / `TooManyAnnouncesFailedInARowException` |
//! | [`request`] | `AnnounceRequest` + `AnnounceDataAccessor` |
//! | [`response`] | `SuccessAnnounceResponse` + `TrackerResponseHandler` |
//! | [`tracker`] | `TrackerClient` + `TrackerClientUriProvider` |
//! | [`state`] | `Announcer` + `AnnouncerSnapshot` |
//!
//! ## Byte-level compatibility invariants
//!
//! 1. The announce URL query string comes from
//!    [`BitTorrentClient::create_request_query`][crate::client::BitTorrentClient::create_request_query]
//!    — S8 does **not** re-implement that logic.
//! 2. The leading `?` / `&` separator between the announce URI and the query
//!    matches Java `TrackerClient.makeCallAndGetResponseAsByteBuffer`: use
//!    `&` when the base URI already contains `?`, `?` otherwise.
//! 3. Headers come from the `.client` file via
//!    [`BitTorrentClient::headers`][crate::client::BitTorrentClient::headers]
//!    and are sent verbatim. The tracker host header is computed on the fly
//!    (host + `:port` when the URI is non-default).
//! 4. `seeders` is reported as `max(0, complete - 1)` exactly like Java —
//!    the comment in `TrackerClient.java:58` calls this out.
//! 5. Failure threshold is **5 in a row**, matching Java's hard-coded `>= 5`
//!    comparison in `Announcer.announce(...)`.

pub mod error;
pub mod request;
pub mod response;
pub mod state;
pub mod tracker;

pub use error::{AnnouncerError, NoMoreUriAvailableError, TooManyFailuresError};
pub use request::{AnnounceDataAccessor, AnnounceRequest};
pub use response::SuccessAnnounceResponse;
pub use state::{Announcer, AnnouncerSnapshot, MAX_CONSECUTIVE_FAILURES};
pub use tracker::{TrackerClient, TrackerClientUriProvider};
