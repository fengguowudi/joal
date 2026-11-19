use joal_core::snapshot::TorrentStatus;

use super::{i18n::Tr, theme::Tone};

/// Unified torrent health, ordered from healthy to most severe for sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Health {
    Healthy,
    PendingAnnounce,
    ZeroLeechers,
    TrackerWarning,
    TrackerError,
}

impl Health {
    pub fn of(torrent: &TorrentStatus) -> Self {
        if torrent.consecutive_fails > 3 {
            Self::TrackerError
        } else if torrent.consecutive_fails > 0 {
            Self::TrackerWarning
        } else if torrent.last_known_leechers == Some(0) {
            Self::ZeroLeechers
        } else if torrent.last_announced_at.is_none() {
            Self::PendingAnnounce
        } else {
            Self::Healthy
        }
    }

    pub fn needs_attention(self) -> bool {
        self != Self::Healthy
    }

    pub fn tone(self) -> Tone {
        match self {
            Self::Healthy => Tone::Success,
            Self::PendingAnnounce => Tone::Info,
            Self::ZeroLeechers | Self::TrackerWarning => Tone::Warning,
            Self::TrackerError => Tone::Danger,
        }
    }

    pub fn label(self, t: &Tr) -> &'static str {
        match self {
            Self::Healthy => t.status_healthy,
            Self::PendingAnnounce => t.status_pending_announce,
            Self::ZeroLeechers => t.status_zero_leechers,
            Self::TrackerWarning => t.status_tracker_warning,
            Self::TrackerError => t.status_tracker_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use joal_core::torrent::InfoHash;

    use super::Health;
    use crate::ui::{
        i18n::{Language, tr},
        theme::Tone,
    };

    fn announced_torrent() -> joal_core::snapshot::TorrentStatus {
        joal_core::snapshot::TorrentStatus {
            info_hash: InfoHash::from_bytes([0; 20]),
            name: "health-fixture".to_owned(),
            total_size: 1,
            uploaded_bytes: 0,
            downloaded_bytes: 0,
            left_bytes: 1,
            current_speed_bps: 0,
            current_download_speed_bps: 0,
            initial_completed: false,
            last_known_interval: Some(1_800),
            last_known_seeders: Some(1),
            last_known_leechers: Some(1),
            consecutive_fails: 0,
            last_announced_at: Some(Instant::now()),
        }
    }

    #[test]
    fn classifies_all_five_branches_with_metadata_and_attention() {
        let translations = tr(Language::English);

        let healthy = announced_torrent();

        let mut pending = announced_torrent();
        pending.last_announced_at = None;

        let mut zero_leechers = announced_torrent();
        zero_leechers.last_known_leechers = Some(0);
        zero_leechers.last_announced_at = None;

        let mut warning = announced_torrent();
        warning.consecutive_fails = 1;
        warning.last_known_leechers = Some(0);
        warning.last_announced_at = None;

        let mut error = announced_torrent();
        error.consecutive_fails = 4;
        error.last_known_leechers = Some(0);
        error.last_announced_at = None;

        let cases = [
            (
                healthy,
                Health::Healthy,
                Tone::Success,
                translations.status_healthy,
                false,
            ),
            (
                pending,
                Health::PendingAnnounce,
                Tone::Info,
                translations.status_pending_announce,
                true,
            ),
            (
                zero_leechers,
                Health::ZeroLeechers,
                Tone::Warning,
                translations.status_zero_leechers,
                true,
            ),
            (
                warning,
                Health::TrackerWarning,
                Tone::Warning,
                translations.status_tracker_warning,
                true,
            ),
            (
                error,
                Health::TrackerError,
                Tone::Danger,
                translations.status_tracker_error,
                true,
            ),
        ];

        for (torrent, expected, tone, label, needs_attention) in &cases {
            let actual = Health::of(torrent);
            assert_eq!(actual, *expected);
            assert_eq!(actual.tone(), *tone);
            assert_eq!(actual.label(translations), *label);
            assert_eq!(actual.needs_attention(), *needs_attention);
        }
    }
}
