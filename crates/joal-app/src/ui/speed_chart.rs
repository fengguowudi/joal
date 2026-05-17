use std::collections::VecDeque;

use egui_plot::{Line, Plot, PlotPoints};

use super::{i18n::Tr, theme};

pub fn show(ui: &mut egui::Ui, speed_history: &VecDeque<(f64, f64)>, t: &Tr) {
    // No outer panel frame — the chart sits directly on the surrounding
    // background. The header (label + latest speed) takes one compact row.
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::Label::new(
            egui::RichText::new(t.upload_kbs)
                .strong()
                .color(theme::text_primary()),
        ));
        if let Some((_, latest_speed)) = speed_history.back() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::metric(
                    ui,
                    "latest_upload_speed",
                    "",
                    format!("{:.1} KB/s", latest_speed / 1024.0),
                    theme::Tone::Accent,
                );
            });
        }
    });
    ui.add_space(4.0);

    if speed_history.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new(t.waiting_for_speed_data)
                    .small()
                    .color(theme::text_tertiary()),
            );
        });
        return;
    }

    let accent = theme::primary_color();

    let points: Vec<[f64; 2]> = speed_history
        .iter()
        .map(|&(t, speed)| [t, speed / 1024.0])
        .collect();
    let series: PlotPoints<'_> = points.into();

    // `Line::fill(y_ref)` shades the area between the curve and the y=ref
    // baseline using the line's own color, with `fill_alpha` for transparency
    // — that's the "area chart under a thick blue line" look.
    let line = Line::new(t.upload_kbs, series)
        .color(accent)
        .width(2.2)
        .fill(0.0)
        .fill_alpha(0.18);

    Plot::new("speed_chart")
        .height(ui.available_height().max(80.0))
        .width(ui.available_width())
        .show_axes([false, true])
        .show_grid([false, true])
        .grid_color(theme::divider_color())
        .show_background(false)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .show(ui, |plot_ui| {
            plot_ui.line(line);
        });
}
