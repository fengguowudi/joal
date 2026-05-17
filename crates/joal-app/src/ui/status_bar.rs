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

    // Single-row state strip: left segment shows the engine status and the
    // primary throughput/torrent-count metrics; right segment pushes the
    // attention counters and the active client filename to the far edge so the
    // strip is edge-to-edge instead of left-clumped.
    theme::panel_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
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

            // Right-anchored segment: lay items out right-to-left so they
            // attach to the strip's trailing edge. Order in code is the order
            // visually right-to-left, so the active client filename ends up at
            // the far right of the strip.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.push_id("active_client_strip", |ui| {
                    ui.horizontal(|ui| {
                        // The wrapping `active_client_strip` push_id is not
                        // enough on its own — the filename text mutates between
                        // frames (different clients) and even the localized
                        // "Client" label changes width when the language is
                        // toggled, so each inner label gets its own static
                        // push_id key for stable multi-pass id derivation.
                        ui.push_id("active_client_label", |ui| {
                            ui.label(
                                egui::RichText::new(t.client)
                                    .small()
                                    .color(theme::text_secondary()),
                            );
                        });
                        ui.push_id("active_client_filename", |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&snapshot.active_client_filename)
                                        .strong()
                                        .color(theme::text_primary()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&snapshot.active_client_filename);
                        });
                    });
                });
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
            });
        });
    });
}

pub fn bottom_bar(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    started_at: std::time::Instant,
    engine_running: bool,
    t: &Tr,
) {
    theme::panel_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
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

            // Right-anchored footer telemetry: mirrors the top bar's "▲" speed
            // and torrent-count metrics so the bottom strip is balanced and the
            // user can read the same headline numbers at a glance from either
            // edge of the workspace.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::metric(
                    ui,
                    "torrent_count_footer",
                    t.torrents,
                    snapshot.torrents.len(),
                    theme::Tone::Neutral,
                );
                theme::metric(
                    ui,
                    "global_upload_speed_footer",
                    "▲",
                    format_speed(snapshot.global_upload_speed_bps),
                    theme::Tone::Accent,
                );
            });
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
