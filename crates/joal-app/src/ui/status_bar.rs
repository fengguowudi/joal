use joal_core::snapshot::EngineSnapshot;

use super::{i18n::Tr, theme};

pub fn top_bar(ui: &mut egui::Ui, snapshot: &EngineSnapshot, engine_running: bool, t: &Tr) {
    let attention_count = snapshot
        .torrents
        .iter()
        .filter(|torrent| torrent.consecutive_fails > 0 || torrent.last_known_leechers == Some(0))
        .count();
    let zero_leecher_count = snapshot
        .torrents
        .iter()
        .filter(|torrent| torrent.last_known_leechers == Some(0))
        .count();

    ui.horizontal_wrapped(|ui| {
        theme::badge(
            ui,
            "engine_state",
            if engine_running { t.running } else { t.stopped },
            if engine_running {
                theme::Tone::Success
            } else {
                theme::Tone::Danger
            },
        );
        theme::metric(
            ui,
            "active_client",
            t.client,
            &snapshot.active_client_filename,
            theme::Tone::Neutral,
        );
        theme::metric(
            ui,
            "global_upload_speed",
            "▲",
            format_speed(snapshot.global_upload_speed_bps),
            theme::Tone::Accent,
        );
        theme::metric(
            ui,
            "global_download_speed",
            "▼",
            format_speed(snapshot.global_download_speed_bps),
            theme::Tone::Info,
        );
        theme::metric(
            ui,
            "torrent_count",
            t.torrents,
            snapshot.torrents.len(),
            theme::Tone::Neutral,
        );
        theme::metric(
            ui,
            "attention_count",
            t.attention,
            attention_count,
            if attention_count > 0 {
                theme::Tone::Warning
            } else {
                theme::Tone::Success
            },
        );
        theme::metric(
            ui,
            "zero_leechers_count",
            t.zero_leechers,
            zero_leecher_count,
            if zero_leecher_count > 0 {
                theme::Tone::Warning
            } else {
                theme::Tone::Neutral
            },
        );
    });
}

pub fn bottom_bar(ui: &mut egui::Ui, started_at: std::time::Instant, engine_running: bool, t: &Tr) {
    ui.horizontal_wrapped(|ui| {
        let elapsed = started_at.elapsed().as_secs();
        let h = elapsed / 3600;
        let m = (elapsed % 3600) / 60;
        let s = elapsed % 60;

        theme::badge(
            ui,
            "engine_status_footer",
            if engine_running { t.running } else { t.stopped },
            if engine_running {
                theme::Tone::Success
            } else {
                theme::Tone::Danger
            },
        );
        theme::metric(
            ui,
            "uptime_footer",
            t.uptime,
            format!("{h:02}:{m:02}:{s:02}"),
            theme::Tone::Neutral,
        );
    });
}

pub fn format_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1_048_576 {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1_048_576.0)
    } else if bytes_per_sec >= 1024 {
        format!("{:.1} KB/s", bytes_per_sec as f64 / 1024.0)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}
