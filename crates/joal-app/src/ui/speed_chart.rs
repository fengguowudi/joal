use std::collections::VecDeque;

use egui_plot::{Corner, Legend, Line, LineStyle, Plot, PlotPoints};

use super::format::format_speed_kb;
use super::telemetry::SpeedSample;
use super::{i18n::Tr, theme};

pub fn show(
    ui: &mut egui::Ui,
    speed_history: &VecDeque<SpeedSample>,
    download_enabled: bool,
    t: &Tr,
) {
    ui.label(
        egui::RichText::new(t.speed_history_title)
            .strong()
            .color(theme::text_primary()),
    );
    ui.add_space(4.0);
    if speed_history.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new(t.waiting_for_speed_data)
                    .small()
                    .color(theme::text_secondary()),
            );
        });
        return;
    }

    let upload_points: PlotPoints<'_> = speed_history
        .iter()
        .map(|sample| [sample.t, sample.up / 1024.0])
        .collect::<Vec<_>>()
        .into();
    let upload = Line::new(t.upload_kbs, upload_points)
        .color(theme::primary_color())
        .width(2.2_f32)
        .fill(0.0_f32)
        .fill_alpha(0.15_f32);
    let download_points: PlotPoints<'_> = speed_history
        .iter()
        .map(|sample| [sample.t, sample.down / 1024.0])
        .collect::<Vec<_>>()
        .into();
    let download = Line::new(t.download_kbs, download_points)
        .color(theme::tone_colors(theme::Tone::Info).fg)
        .width(2.0_f32)
        .style(LineStyle::Dashed { length: 6.0 });

    let plot = Plot::new("speed_chart")
        .height(ui.available_height().max(80.0))
        .width(ui.available_width())
        .include_y(0.0)
        .show_axes([false, true])
        .show_grid([false, true])
        .grid_color(theme::divider_color())
        .show_background(false)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .y_axis_formatter(|mark, _| format_speed_kb(mark.value))
        .label_formatter(|name, point| format!("{name}\n{}", format_speed_kb(point.y)));
    let plot = if download_enabled {
        plot.legend(Legend::default().position(Corner::LeftTop))
    } else {
        plot
    };
    plot.show(ui, |plot_ui| {
        plot_ui.line(upload);
        if download_enabled {
            plot_ui.line(download);
        }
    });
}
