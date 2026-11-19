use std::time::Duration;

use joal_core::snapshot::TorrentStatus;

use super::i18n::Tr;

pub(super) fn short_hash(torrent: &TorrentStatus) -> String {
    let hash = torrent.info_hash.to_string();
    format!("{}...", &hash[..12])
}

pub(super) fn last_announce_text(torrent: &TorrentStatus, t: &Tr) -> String {
    match torrent.last_announced_at {
        Some(last_announced_at) => format_duration(last_announced_at.elapsed()),
        None => t.status_never.to_owned(),
    }
}

pub(super) fn interval_text(torrent: &TorrentStatus, t: &Tr) -> String {
    match torrent.last_known_interval {
        Some(interval) => format!(
            "{} {}",
            t.status_interval,
            format_duration(Duration::from_secs(interval.into()))
        ),
        None => format!("{} {}", t.status_interval, t.status_never),
    }
}

pub(super) fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    if total_secs < 60 {
        format!("{total_secs}s")
    } else if total_secs < 3600 {
        format!("{}m {}s", total_secs / 60, total_secs % 60)
    } else {
        format!("{}h {}m", total_secs / 3600, (total_secs % 3600) / 60)
    }
}

pub(super) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub(super) fn format_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1_048_576 {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1_048_576.0)
    } else if bytes_per_sec >= 1024 {
        format!("{:.1} KB/s", bytes_per_sec as f64 / 1024.0)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}

pub(super) fn format_speed_kb(kilobytes_per_sec: f64) -> String {
    if kilobytes_per_sec.abs() >= 1024.0 {
        format!("{:.1} MB/s", kilobytes_per_sec / 1024.0)
    } else {
        format!("{kilobytes_per_sec:.1} KB/s")
    }
}

pub(super) fn opt_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| value.to_string())
}

pub(super) fn progress_fraction(torrent: &TorrentStatus) -> f64 {
    if torrent.total_size == 0 {
        1.0
    } else {
        (torrent.downloaded_bytes as f64 / torrent.total_size as f64).clamp(0.0, 1.0)
    }
}

pub(super) fn progress_text(torrent: &TorrentStatus) -> String {
    format!("{:.1}%", progress_fraction(torrent) * 100.0)
}
