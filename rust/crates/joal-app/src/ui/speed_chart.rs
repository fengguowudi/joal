use std::collections::VecDeque;

use egui_plot::{Line, Plot, PlotPoints};

pub fn show(ui: &mut egui::Ui, speed_history: &VecDeque<(f64, f64)>) {
    if speed_history.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("Waiting for speed data...");
        });
        return;
    }

    let points: Vec<[f64; 2]> = speed_history
        .iter()
        .map(|&(t, speed)| [t, speed / 1024.0])
        .collect();

    let series: PlotPoints<'_> = points.into();
    let line = Line::new("Upload (KB/s)", series);

    Plot::new("speed_chart")
        .height(ui.available_height())
        .width(ui.available_width())
        .x_axis_label("Time (s)")
        .y_axis_label("KB/s")
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .show_axes(true)
        .show(ui, |plot_ui| {
            plot_ui.line(line);
        });
}
