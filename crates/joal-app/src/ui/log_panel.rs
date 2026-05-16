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
        theme::metric(
            ui,
            "log_entry_count",
            "",
            format!("{} {}", log_buffer.len(), t.entries),
            theme::Tone::Neutral,
        );
    });

    let available = ui.available_height().max(60.0);
    egui::ScrollArea::vertical()
        .max_height(available)
        .stick_to_bottom(*auto_scroll)
        .show(ui, |ui| {
            for entry in log_buffer {
                let elapsed = entry.timestamp.duration_since(started_at).as_secs();
                let h = elapsed / 3600;
                let m = (elapsed % 3600) / 60;
                let s = elapsed % 60;
                ui.label(
                    egui::RichText::new(format!("[{h:02}:{m:02}:{s:02}] {}", entry.message))
                        .monospace()
                        .small()
                        .color(theme::text_secondary()),
                );
            }
        });
}
