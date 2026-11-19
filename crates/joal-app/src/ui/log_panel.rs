use std::collections::VecDeque;

use super::telemetry::{LogEntry, LogLevel};
use super::{i18n::Tr, theme};

pub fn show(
    ui: &mut egui::Ui,
    logs: &VecDeque<LogEntry>,
    auto_scroll: &mut bool,
    problems_only: &mut bool,
    t: &Tr,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(t.log)
                .strong()
                .color(theme::text_primary()),
        );
        ui.checkbox(auto_scroll, t.auto_scroll);
        if theme::tone_button(
            ui,
            "log_problems_only",
            t.problems_only,
            theme::Tone::Warning,
            egui::vec2(92.0, 26.0),
            *problems_only,
        )
        .clicked()
        {
            *problems_only = !*problems_only;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{} {}", logs.len(), t.entries))
                    .small()
                    .color(theme::text_secondary()),
            );
        });
    });
    ui.add_space(4.0);

    let filtered = problems_only.then(|| {
        logs.iter()
            .enumerate()
            .filter(|(_, entry)| entry.level() >= LogLevel::Warning)
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    });
    let row_count = filtered.as_ref().map_or(logs.len(), Vec::len);
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    let available = ui.available_height().max(60.0);
    egui::ScrollArea::vertical()
        .max_height(available)
        .stick_to_bottom(*auto_scroll)
        .show_rows(ui, row_height, row_count, |ui, range| {
            ui.spacing_mut().item_spacing.y = 3.0;
            for row_index in range {
                let index = filtered
                    .as_ref()
                    .map_or(row_index, |indices| indices[row_index]);
                let Some(entry) = logs.get(index) else {
                    continue;
                };
                ui.push_id(("log_row", row_index), |ui| {
                    ui.horizontal(|ui| {
                        let elapsed = entry.at.as_secs();
                        let timestamp = format!(
                            "{:02}:{:02}:{:02}",
                            elapsed / 3600,
                            (elapsed % 3600) / 60,
                            elapsed % 60
                        );
                        ui.label(
                            egui::RichText::new(timestamp)
                                .monospace()
                                .color(theme::text_tertiary()),
                        );
                        let level = entry.level();
                        if level != LogLevel::Info {
                            let color = match level {
                                LogLevel::Success => theme::tone_colors(theme::Tone::Success).fg,
                                LogLevel::Warning => theme::tone_colors(theme::Tone::Warning).fg,
                                LogLevel::Error => theme::tone_colors(theme::Tone::Danger).fg,
                                LogLevel::Info => theme::text_secondary(),
                            };
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 3.0, color);
                        }
                        let message = entry.text(t);
                        let color = if level == LogLevel::Error {
                            theme::tone_colors(theme::Tone::Danger).fg
                        } else {
                            theme::text_secondary()
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&message).monospace().color(color),
                            )
                            .truncate(),
                        )
                        .on_hover_ui(|ui| {
                            ui.label(&message);
                        });
                    });
                });
            }
        });
}
