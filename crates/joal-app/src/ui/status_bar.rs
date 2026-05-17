use joal_core::snapshot::EngineSnapshot;

use super::{i18n::Tr, theme};

/// Render the top status strip: engine badge + upload/download/torrent count
/// metrics on the left.
///
/// Buttons are intentionally NOT rendered here — `mod.rs` adds them in the
/// same row using a `right_to_left` layout so the top strip stays a single
/// height-efficient line. The function does NOT call `ui.horizontal()` —
/// callers wrap it in whatever layout they need (the main app embeds it in a
/// horizontal layout shared with action buttons).
pub fn top_bar_status(ui: &mut egui::Ui, snapshot: &EngineSnapshot, engine_running: bool, t: &Tr) {
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

    // Engine status uses the larger `engine_badge` (colored status dot + label
    // at body-text size) so it visually anchors next to the upload/download
    // metrics rather than reading as a tiny pill.
    theme::engine_badge(
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
        "global_upload_speed",
        "▲",
        &format_speed(snapshot.global_upload_speed_bps),
        theme::Tone::Accent,
    );
    theme::metric(
        ui,
        "global_download_speed",
        "▼",
        &format_speed(snapshot.global_download_speed_bps),
        theme::Tone::Info,
    );
    theme::metric(
        ui,
        "torrent_count",
        t.torrents,
        &snapshot.torrents.len(),
        theme::Tone::Neutral,
    );
    theme::metric(
        ui,
        "attention_count",
        t.attention,
        &attention_count,
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
        &zero_leecher_count,
        if zero_leecher_count > 0 {
            theme::Tone::Warning
        } else {
            theme::Tone::Neutral
        },
    );
}

/// Standalone top-bar wrapper that takes care of the `horizontal()` layout —
/// used by the benchmark harness which doesn't share the row with anything
/// else. Production UI calls `top_bar_status` directly inside its own
/// horizontal row so it can pack action buttons alongside.
#[cfg(test)]
pub fn top_bar(ui: &mut egui::Ui, snapshot: &EngineSnapshot, engine_running: bool, t: &Tr) {
    ui.horizontal(|ui| {
        top_bar_status(ui, snapshot, engine_running, t);
    });
}

pub fn bottom_bar(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    started_at: std::time::Instant,
    engine_running: bool,
    t: &Tr,
) {
    ui.horizontal(|ui| {
        let elapsed = started_at.elapsed().as_secs();
        let h = elapsed / 3600;
        let m = (elapsed % 3600) / 60;
        let s = elapsed % 60;

        theme::engine_badge(
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
            &format!("{h:02}:{m:02}:{s:02}"),
            theme::Tone::Neutral,
        );

        // Right-anchored footer telemetry: mirrors the top bar's "▲" speed and
        // torrent-count metrics so the bottom strip is balanced and the user
        // can read the same headline numbers at a glance from either edge of
        // the workspace. Push to the right edge using a right-to-left layout.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Active client filename — light gray, low emphasis, sits at the
            // far right corner like a "powered by" tag.
            ui.push_id("active_client_footer", |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&snapshot.active_client_filename)
                            .small()
                            .color(theme::text_tertiary()),
                    )
                    .truncate(),
                )
                .on_hover_text(&snapshot.active_client_filename);
            });
            theme::metric(
                ui,
                "torrent_count_footer",
                t.torrents,
                &snapshot.torrents.len(),
                theme::Tone::Neutral,
            );
            theme::metric(
                ui,
                "global_upload_speed_footer",
                "▲",
                &format_speed(snapshot.global_upload_speed_bps),
                theme::Tone::Accent,
            );
        });
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
