use std::time::Instant;

use joal_core::snapshot::EngineSnapshot;

use crate::engine::EnginePhase;

use super::format::{format_bytes, format_speed};
use super::{i18n::Tr, theme};

pub fn top_bar_status(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    phase: EnginePhase,
    download_enabled: bool,
    t: &Tr,
) {
    let (label, tone) = match phase {
        EnginePhase::Starting | EnginePhase::Stopping => (
            if phase == EnginePhase::Starting {
                t.starting
            } else {
                t.stopping
            },
            theme::Tone::Info,
        ),
        EnginePhase::Running => (t.running, theme::Tone::Success),
        EnginePhase::Stopped => (t.stopped, theme::Tone::Neutral),
        EnginePhase::Failed => (t.failed, theme::Tone::Danger),
    };
    theme::engine_badge(ui, "engine_state", label, tone);
    if matches!(phase, EnginePhase::Starting | EnginePhase::Stopping) {
        ui.spinner();
    }
    speed_readout(ui, true, snapshot.global_upload_speed_bps);
    if download_enabled {
        speed_readout(ui, false, snapshot.global_download_speed_bps);
    }
}

fn speed_readout(ui: &mut egui::Ui, upload: bool, bytes_per_sec: u64) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        let center = rect.center();
        let points = if upload {
            vec![
                center + egui::vec2(-4.0, 3.0),
                center + egui::vec2(4.0, 3.0),
                center + egui::vec2(0.0, -4.0),
            ]
        } else {
            vec![
                center + egui::vec2(-4.0, -3.0),
                center + egui::vec2(4.0, -3.0),
                center + egui::vec2(0.0, 4.0),
            ]
        };
        let color = if upload {
            theme::primary_color()
        } else {
            theme::tone_colors(theme::Tone::Info).fg
        };
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
        ui.label(egui::RichText::new(format_speed(bytes_per_sec)).color(theme::text_primary()));
    });
}

pub fn bottom_bar(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    started_at: Instant,
    total_uploaded: u64,
    t: &Tr,
) {
    ui.horizontal(|ui| {
        let elapsed = started_at.elapsed().as_secs();
        let uptime = format!(
            "{:02}:{:02}:{:02}",
            elapsed / 3600,
            (elapsed % 3600) / 60,
            elapsed % 60
        );
        ui.label(
            egui::RichText::new(format!(
                "{} {} · {} {uptime}",
                t.client, snapshot.active_client_filename, t.uptime
            ))
            .small()
            .color(theme::text_secondary()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} {}",
                    t.uploaded_total,
                    format_bytes(total_uploaded)
                ))
                .color(theme::text_primary()),
            );
        });
    });
}
