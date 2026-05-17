use std::collections::VecDeque;
use std::time::Instant;

use super::LogEntry;
use super::{i18n::Tr, theme};

pub fn show(
    ui: &mut egui::Ui,
    log_buffer: &VecDeque<LogEntry>,
    auto_scroll: &mut bool,
    started_at: Instant,
    t: &Tr,
) {
    // No outer panel_frame here — the panel that owns this widget supplies the
    // surface; the log itself is rendered as plain text on top of it.
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(t.log)
                .strong()
                .color(theme::text_primary()),
        );
        let auto_scroll_response = ui.push_id("log_auto_scroll_toggle", |ui| {
            ui.checkbox(auto_scroll, t.auto_scroll)
        });
        auto_scroll_response.inner.on_hover_text(t.auto_scroll);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme::metric(
                ui,
                "log_entry_count",
                "",
                &format!("{} {}", log_buffer.len(), t.entries),
                theme::Tone::Neutral,
            );
        });
    });

    ui.add_space(4.0);
    let available = ui.available_height().max(60.0);
    egui::ScrollArea::vertical()
        .max_height(available)
        .stick_to_bottom(*auto_scroll)
        .show(ui, |ui| {
            // Slightly wider line spacing so the log feels easier to scan.
            ui.spacing_mut().item_spacing.y = 3.0;
            for entry in log_buffer {
                let elapsed = entry.timestamp.duration_since(started_at).as_secs();
                let h = elapsed / 3600;
                let m = (elapsed % 3600) / 60;
                let s = elapsed % 60;
                let message = format!("[{h:02}:{m:02}:{s:02}] {}", entry.message);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&message)
                            .monospace()
                            .small()
                            .color(theme::text_tertiary()),
                    )
                    .truncate(),
                )
                .on_hover_text(message);
            }
        });
}
