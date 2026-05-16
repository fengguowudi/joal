use std::hash::Hash;

use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Neutral,
    Accent,
    Info,
    Success,
    Warning,
    Danger,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ToneColors {
    pub fg: Color32,
    pub bg: Color32,
    pub stroke: Color32,
}

pub(super) fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.visuals = egui::Visuals::dark();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.text_edit_width = 160.0;
    style.spacing.window_margin = Margin::same(12);

    let visuals = &mut style.visuals;
    visuals.override_text_color = Some(text_primary());
    visuals.weak_text_color = Some(text_secondary());
    visuals.hyperlink_color = tone_colors(Tone::Accent).fg;
    visuals.selection.bg_fill = Color32::from_rgb(60, 102, 178);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(235, 243, 255));
    visuals.faint_bg_color = Color32::from_rgb(22, 29, 37);
    visuals.extreme_bg_color = Color32::from_rgb(10, 14, 18);
    visuals.text_edit_bg_color = Some(Color32::from_rgb(18, 24, 31));
    visuals.code_bg_color = Color32::from_rgb(18, 24, 31);
    visuals.warn_fg_color = tone_colors(Tone::Warning).fg;
    visuals.error_fg_color = tone_colors(Tone::Danger).fg;
    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.menu_corner_radius = CornerRadius::same(6);
    visuals.window_fill = Color32::from_rgb(17, 23, 30);
    visuals.window_stroke = Stroke::new(1.0, border());
    visuals.panel_fill = Color32::from_rgb(12, 17, 22);
    visuals.popup_shadow.blur = 16;
    visuals.window_shadow.blur = 16;

    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(17, 23, 30);
    visuals.widgets.noninteractive.weak_bg_fill = Color32::from_rgb(17, 23, 30);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border());
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(6);
    visuals.widgets.noninteractive.fg_stroke.color = text_primary();

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(22, 29, 37);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(22, 29, 37);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, border());
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.inactive.fg_stroke.color = text_primary();

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(29, 39, 50);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(29, 39, 50);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(74, 95, 120));
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.fg_stroke.color = text_primary();

    visuals.widgets.active.bg_fill = Color32::from_rgb(38, 64, 106);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(38, 64, 106);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(86, 126, 194));
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.fg_stroke.color = Color32::from_rgb(244, 248, 255);

    visuals.widgets.open.bg_fill = Color32::from_rgb(28, 46, 76);
    visuals.widgets.open.weak_bg_fill = Color32::from_rgb(28, 46, 76);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, Color32::from_rgb(86, 126, 194));
    visuals.widgets.open.corner_radius = CornerRadius::same(6);
    visuals.widgets.open.fg_stroke.color = Color32::from_rgb(244, 248, 255);

    ctx.set_global_style(style);
}

pub(super) fn text_primary() -> Color32 {
    Color32::from_rgb(233, 239, 246)
}

pub(super) fn text_secondary() -> Color32 {
    Color32::from_rgb(154, 165, 180)
}

pub(super) fn border() -> Color32 {
    Color32::from_rgb(46, 58, 73)
}

pub(super) fn panel_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgb(16, 22, 28))
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 8))
}

pub(super) fn inset_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgb(19, 26, 33))
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 8))
}

pub(super) fn tone_frame(tone: Tone) -> Frame {
    let colors = tone_colors(tone);
    Frame::new()
        .fill(colors.bg)
        .stroke(Stroke::new(1.0, colors.stroke))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 8))
}

pub(super) fn badge(ui: &mut Ui, id: impl Hash, text: &str, tone: Tone) {
    let colors = tone_colors(tone);
    ui.push_id(id, |ui| {
        Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::new(1.0, colors.stroke))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.label(RichText::new(text).color(colors.fg).strong().small());
            });
    });
}

pub(super) fn metric(ui: &mut Ui, id: impl Hash, label: &str, value: impl ToString, tone: Tone) {
    let colors = tone_colors(tone);
    let value = value.to_string();
    ui.push_id(id, |ui| {
        Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::new(1.0, colors.stroke))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if !label.is_empty() {
                        ui.label(RichText::new(label).small().color(text_secondary()));
                    }
                    ui.label(RichText::new(value).strong().color(
                        if matches!(tone, Tone::Neutral) {
                            text_primary()
                        } else {
                            colors.fg
                        },
                    ));
                });
            });
    });
}

pub(super) fn tone_colors(tone: Tone) -> ToneColors {
    match tone {
        Tone::Neutral => ToneColors {
            fg: text_primary(),
            bg: Color32::from_rgb(18, 24, 31),
            stroke: border(),
        },
        Tone::Accent => ToneColors {
            fg: Color32::from_rgb(143, 187, 255),
            bg: Color32::from_rgb(27, 47, 78),
            stroke: Color32::from_rgb(72, 108, 164),
        },
        Tone::Info => ToneColors {
            fg: Color32::from_rgb(134, 204, 236),
            bg: Color32::from_rgb(23, 53, 68),
            stroke: Color32::from_rgb(54, 107, 130),
        },
        Tone::Success => ToneColors {
            fg: Color32::from_rgb(133, 220, 183),
            bg: Color32::from_rgb(22, 58, 47),
            stroke: Color32::from_rgb(51, 113, 90),
        },
        Tone::Warning => ToneColors {
            fg: Color32::from_rgb(238, 196, 103),
            bg: Color32::from_rgb(71, 55, 24),
            stroke: Color32::from_rgb(128, 100, 43),
        },
        Tone::Danger => ToneColors {
            fg: Color32::from_rgb(244, 157, 170),
            bg: Color32::from_rgb(79, 37, 45),
            stroke: Color32::from_rgb(140, 72, 84),
        },
    }
}
