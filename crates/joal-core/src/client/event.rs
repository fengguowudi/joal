//! Tracker announce `event` values.
//!
//! Mirrors `com.turn.ttorrent.common.protocol.TrackerMessage.AnnounceRequestMessage.RequestEvent`
//! as used throughout JOAL. The string forms (`started`, `stopped`,
//! `completed`, empty for `NONE`) are part of the HTTP tracker wire format
//! and must stay byte-compatible.

/// Announce request event, corresponding to the BEP-3 `event` query-string
/// parameter. Used by numwant providers, peer-id/key refresh policies, and
/// by the `BitTorrentClient` query builder (S5+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestEvent {
    /// Regular periodic announce; the event key is dropped from the URL.
    None,
    Started,
    Stopped,
    Completed,
}

impl RequestEvent {
    /// String form used in the tracker `event=` URL parameter. Java
    /// `RequestEvent.getEventName()` returns an empty string for `NONE`.
    #[must_use]
    pub const fn event_name(self) -> &'static str {
        match self {
            RequestEvent::None => "",
            RequestEvent::Started => "started",
            RequestEvent::Stopped => "stopped",
            RequestEvent::Completed => "completed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_match_bep3_wire_format() {
        assert_eq!(RequestEvent::None.event_name(), "");
        assert_eq!(RequestEvent::Started.event_name(), "started");
        assert_eq!(RequestEvent::Stopped.event_name(), "stopped");
        assert_eq!(RequestEvent::Completed.event_name(), "completed");
    }
}
