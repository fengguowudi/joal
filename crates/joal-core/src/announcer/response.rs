//! Tracker response parsing.
//!
//! Combines Java
//! `org.araymond.joal.core.ttorrent.client.announcer.request.SuccessAnnounceResponse`
//! with the bencode parsing that used to live in
//! `com.turn.ttorrent.common.protocol.http.HTTPTrackerMessage.parse` +
//! `TrackerResponseHandler`. The happy path returns a
//! [`SuccessAnnounceResponse`], while a tracker-reported `failure reason` is
//! promoted to a typed [`AnnouncerError::TrackerReported`].

use crate::bencode::{self, Value};

use super::error::AnnouncerError;

/// Subset of the BEP-3 tracker response that JOAL actually uses.
///
/// Java exposes `interval`, `seeders` (= `complete - 1`, clamped at 0), and
/// `leechers` (= `incomplete`). Keep identical semantics here so the
/// `Announcer` state machine reads the same numbers the Java one would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuccessAnnounceResponse {
    interval: i32,
    seeders: i32,
    leechers: i32,
}

impl SuccessAnnounceResponse {
    #[must_use]
    pub const fn new(interval: i32, seeders: i32, leechers: i32) -> Self {
        Self {
            interval,
            seeders,
            leechers,
        }
    }

    /// Recommended seconds between announces. Honoured verbatim.
    #[must_use]
    pub const fn interval(&self) -> i32 {
        self.interval
    }

    /// Count of other seeders. Java subtracts 1 to exclude self:
    /// `max(0, complete - 1)`. This field already has that adjustment
    /// applied — callers see the Java-compatible number.
    #[must_use]
    pub const fn seeders(&self) -> i32 {
        self.seeders
    }

    /// Count of leechers (`incomplete` in BEP-3).
    #[must_use]
    pub const fn leechers(&self) -> i32 {
        self.leechers
    }

    /// Parse a raw bencode tracker response.
    ///
    /// * If the top-level dict contains a `failure reason`, returns
    ///   [`AnnouncerError::TrackerReported`] with the tracker URI filled in
    ///   by the caller via [`Self::parse_with_uri`].
    /// * Missing required fields (`interval`, `complete`, `incomplete`)
    ///   surface as [`AnnouncerError::IncompleteResponse`].
    pub fn parse(bytes: &[u8]) -> Result<Self, AnnouncerError> {
        Self::parse_with_uri(bytes, "<unknown>")
    }

    /// Same as [`Self::parse`] but attaches `tracker_uri` to
    /// [`AnnouncerError::TrackerReported`] for log clarity.
    pub fn parse_with_uri(bytes: &[u8], tracker_uri: &str) -> Result<Self, AnnouncerError> {
        let value = bencode::parse_lenient(bytes)?;

        if let Some(reason) = value.get("failure reason").and_then(Value::as_bytes) {
            let reason = String::from_utf8_lossy(reason).into_owned();
            return Err(AnnouncerError::TrackerReported {
                uri: tracker_uri.to_owned(),
                reason,
            });
        }

        let interval = value
            .get("interval")
            .and_then(Value::as_int)
            .ok_or(AnnouncerError::IncompleteResponse("interval"))?;
        let complete = value
            .get("complete")
            .and_then(Value::as_int)
            .ok_or(AnnouncerError::IncompleteResponse("complete"))?;
        let incomplete = value
            .get("incomplete")
            .and_then(Value::as_int)
            .ok_or(AnnouncerError::IncompleteResponse("incomplete"))?;

        // Java: `Math.max(0, complete - 1)` — we are ourselves one of the
        // seeders, subtract that out.
        let seeders = i32::try_from(complete.saturating_sub(1))
            .unwrap_or(i32::MAX)
            .max(0);
        let leechers = i32::try_from(incomplete).unwrap_or(i32::MAX).max(0);
        let interval = i32::try_from(interval).unwrap_or(i32::MAX).max(0);

        Ok(Self::new(interval, seeders, leechers))
    }
}
