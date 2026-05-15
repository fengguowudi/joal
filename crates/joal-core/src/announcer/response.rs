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

#[cfg(test)]
mod tests {
    use super::*;

    fn bencode_dict(items: &[(&str, Value)]) -> Vec<u8> {
        // Writes a bencode dict in the ordering the caller provides.
        // All dict keys used by tracker responses are ASCII and the three
        // relevant ones already sort lexicographically when passed in the
        // order `(complete, incomplete, interval)`, so callers can rely on
        // that for valid payloads.
        let mut out = Vec::new();
        out.push(b'd');
        for (k, v) in items {
            write_bytes(&mut out, k.as_bytes());
            write_value(&mut out, v);
        }
        out.push(b'e');
        out
    }

    fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(bytes.len().to_string().as_bytes());
        out.push(b':');
        out.extend_from_slice(bytes);
    }

    fn write_value(out: &mut Vec<u8>, value: &Value) {
        match value {
            Value::Integer(i) => {
                out.push(b'i');
                out.extend_from_slice(i.to_string().as_bytes());
                out.push(b'e');
            }
            Value::ByteString(b) => write_bytes(out, b),
            _ => panic!("test helper does not need list/dict values"),
        }
    }

    #[test]
    fn parses_canonical_happy_path() {
        let payload = bencode_dict(&[
            ("complete", Value::Integer(12)),
            ("incomplete", Value::Integer(3)),
            ("interval", Value::Integer(1800)),
        ]);
        let resp = SuccessAnnounceResponse::parse(&payload).unwrap();
        // Java subtracts one for self.
        assert_eq!(resp.seeders(), 11);
        assert_eq!(resp.leechers(), 3);
        assert_eq!(resp.interval(), 1800);
    }

    #[test]
    fn clamps_seeders_to_zero_when_tracker_only_knows_us() {
        let payload = bencode_dict(&[
            ("complete", Value::Integer(1)),
            ("incomplete", Value::Integer(0)),
            ("interval", Value::Integer(30)),
        ]);
        let resp = SuccessAnnounceResponse::parse(&payload).unwrap();
        assert_eq!(resp.seeders(), 0);
    }

    #[test]
    fn failure_reason_turns_into_tracker_reported_error() {
        let payload = bencode_dict(&[(
            "failure reason",
            Value::ByteString(b"torrent not registered".to_vec()),
        )]);
        let err = SuccessAnnounceResponse::parse_with_uri(&payload, "http://tracker/announce")
            .unwrap_err();
        match err {
            AnnouncerError::TrackerReported { uri, reason } => {
                assert_eq!(uri, "http://tracker/announce");
                assert_eq!(reason, "torrent not registered");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn missing_interval_is_incomplete_response() {
        let payload = bencode_dict(&[
            ("complete", Value::Integer(1)),
            ("incomplete", Value::Integer(2)),
        ]);
        let err = SuccessAnnounceResponse::parse(&payload).unwrap_err();
        assert!(matches!(
            err,
            AnnouncerError::IncompleteResponse("interval")
        ));
    }

    #[test]
    fn invalid_bencode_surfaces_as_invalid_response() {
        let err = SuccessAnnounceResponse::parse(b"not bencode").unwrap_err();
        assert!(matches!(err, AnnouncerError::InvalidResponse(_)));
    }
}
