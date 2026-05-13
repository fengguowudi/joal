use std::collections::VecDeque;
use std::time::Instant;

use super::LogEntry;

pub fn show(
    ui: &mut egui::Ui,
    log_buffer: &VecDeque<LogEntry>,
    auto_scroll: &mut bool,
    started_at: Instant,
) {
    ui.horizontal(|ui| {
        ui.strong("Log");
        ui.separator();
        ui.checkbox(auto_scroll, "Auto-scroll");
        ui.label(format!("({} entries)", log_buffer.len()));
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
                ui.label(format!("[{h:02}:{m:02}:{s:02}] {}", entry.message));
            }
        });
}
