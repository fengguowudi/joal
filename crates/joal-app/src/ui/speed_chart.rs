use std::collections::VecDeque;

use egui_plot::{Line, Plot, PlotPoints};

use super::{i18n::Tr, theme};

pub fn show(ui: &mut egui::Ui, speed_history: &VecDeque<(f64, f64)>, t: &Tr) {
    theme::panel_frame().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(t.upload_kbs)
                    .strong()
                    .color(theme::text_primary()),
            );
            if let Some((_, latest_speed)) = speed_history.back() {
                theme::metric(
                    ui,
                    "latest_upload_speed",
                    "",
                    format!("{:.1} KB/s", latest_speed / 1024.0),
                    theme::Tone::Accent,
                );
            }
        });
        ui.add_space(8.0);

        if speed_history.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new(t.waiting_for_speed_data).color(theme::text_secondary()),
                );
            });
            return;
        }

        let points: Vec<[f64; 2]> = speed_history
            .iter()
            .map(|&(t, speed)| [t, speed / 1024.0])
            .collect();

        let series: PlotPoints<'_> = points.into();
        let line =
            Line::new(t.upload_kbs, series).color(theme::tone_colors(theme::Tone::Accent).fg);

        Plot::new("speed_chart")
            .height(ui.available_height())
            .width(ui.available_width())
            .x_axis_label(t.time_s)
            .y_axis_label("KB/s")
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show_axes(true)
            .show(ui, |plot_ui| {
                plot_ui.line(line);
            });
    });
}
