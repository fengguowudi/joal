use std::hash::Hash;

use egui::{Color32, CornerRadius, Frame, Label, Margin, RichText, Stroke, Ui};

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
    style.visuals = egui::Visuals::light();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.text_edit_width = 160.0;
    style.spacing.window_margin = Margin::same(12);

    let visuals = &mut style.visuals;
    visuals.override_text_color = Some(text_primary());
    visuals.weak_text_color = Some(text_secondary());
    visuals.hyperlink_color = accent_text();
    visuals.selection.bg_fill = Color32::from_rgb(210, 226, 255);
    visuals.selection.stroke = Stroke::new(1.0, accent_text());
    visuals.faint_bg_color = Color32::from_rgb(245, 247, 251);
    visuals.extreme_bg_color = surface();
    visuals.text_edit_bg_color = Some(surface());
    visuals.code_bg_color = Color32::from_rgb(244, 247, 251);
    visuals.warn_fg_color = tone_colors(Tone::Warning).fg;
    visuals.error_fg_color = tone_colors(Tone::Danger).fg;
    visuals.window_corner_radius = CornerRadius::same(5);
    visuals.menu_corner_radius = CornerRadius::same(5);
    visuals.window_fill = surface();
    visuals.window_stroke = Stroke::new(1.0, border());
    visuals.panel_fill = app_background();
    visuals.popup_shadow.blur = 12;
    visuals.window_shadow.blur = 12;

    visuals.widgets.noninteractive.bg_fill = panel_fill();
    visuals.widgets.noninteractive.weak_bg_fill = panel_fill();
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border());
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(5);
    visuals.widgets.noninteractive.fg_stroke.color = text_primary();

    visuals.widgets.inactive.bg_fill = surface();
    visuals.widgets.inactive.weak_bg_fill = panel_fill();
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, border());
    visuals.widgets.inactive.corner_radius = CornerRadius::same(5);
    visuals.widgets.inactive.fg_stroke.color = text_primary();

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(247, 249, 252);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(240, 244, 250);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, border_strong());
    visuals.widgets.hovered.corner_radius = CornerRadius::same(5);
    visuals.widgets.hovered.fg_stroke.color = text_primary();

    visuals.widgets.active.bg_fill = Color32::from_rgb(232, 239, 252);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(232, 239, 252);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(165, 189, 236));
    visuals.widgets.active.corner_radius = CornerRadius::same(5);
    visuals.widgets.active.fg_stroke.color = text_primary();

    visuals.widgets.open.bg_fill = Color32::from_rgb(236, 242, 252);
    visuals.widgets.open.weak_bg_fill = Color32::from_rgb(236, 242, 252);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, Color32::from_rgb(165, 189, 236));
    visuals.widgets.open.corner_radius = CornerRadius::same(5);
    visuals.widgets.open.fg_stroke.color = text_primary();

    ctx.set_global_style(style);
}

pub(super) fn text_primary() -> Color32 {
    Color32::from_rgb(56, 58, 66)
}

pub(super) fn text_secondary() -> Color32 {
    Color32::from_rgb(109, 115, 127)
}

pub(super) fn border() -> Color32 {
    Color32::from_rgb(211, 218, 229)
}

pub(super) fn border_strong() -> Color32 {
    Color32::from_rgb(193, 202, 215)
}

pub(super) fn app_background() -> Color32 {
    Color32::from_rgb(244, 247, 250)
}

pub(super) fn panel_fill() -> Color32 {
    Color32::from_rgb(249, 250, 252)
}

pub(super) fn surface() -> Color32 {
    Color32::from_rgb(255, 255, 255)
}

fn accent_text() -> Color32 {
    Color32::from_rgb(64, 120, 242)
}

pub(super) fn panel_frame() -> Frame {
    Frame::new()
        .fill(surface())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(12, 10))
}

pub(super) fn inset_frame() -> Frame {
    Frame::new()
        .fill(panel_fill())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(12, 10))
}

pub(super) fn tone_frame(tone: Tone) -> Frame {
    let colors = tone_colors(tone);
    Frame::new()
        .fill(colors.bg)
        .stroke(Stroke::new(1.0, colors.stroke))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(12, 10))
}

pub(super) fn badge(ui: &mut Ui, id: impl Hash, text: &str, tone: Tone) {
    let colors = tone_colors(tone);
    ui.push_id(id, |ui| {
        Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::new(1.0, colors.stroke))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_max_width(220.0);
                ui.add(
                    Label::new(RichText::new(text).color(colors.fg).strong().small()).truncate(),
                );
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
            .corner_radius(CornerRadius::same(5))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_max_width(240.0);
                ui.horizontal(|ui| {
                    if !label.is_empty() {
                        ui.add(
                            Label::new(RichText::new(label).small().color(text_secondary()))
                                .truncate(),
                        );
                    }
                    ui.add(
                        Label::new(RichText::new(value).strong().color(
                            if matches!(tone, Tone::Neutral) {
                                text_primary()
                            } else {
                                colors.fg
                            },
                        ))
                        .truncate(),
                    );
                });
            });
    });
}

pub(super) fn tone_colors(tone: Tone) -> ToneColors {
    match tone {
        Tone::Neutral => ToneColors {
            fg: text_primary(),
            bg: Color32::from_rgb(249, 250, 252),
            stroke: border(),
        },
        Tone::Accent => ToneColors {
            fg: Color32::from_rgb(52, 100, 204),
            bg: Color32::from_rgb(236, 242, 255),
            stroke: Color32::from_rgb(179, 198, 241),
        },
        Tone::Info => ToneColors {
            fg: Color32::from_rgb(29, 112, 171),
            bg: Color32::from_rgb(234, 245, 252),
            stroke: Color32::from_rgb(184, 214, 235),
        },
        Tone::Success => ToneColors {
            fg: Color32::from_rgb(69, 137, 68),
            bg: Color32::from_rgb(236, 247, 235),
            stroke: Color32::from_rgb(190, 224, 187),
        },
        Tone::Warning => ToneColors {
            fg: Color32::from_rgb(156, 107, 20),
            bg: Color32::from_rgb(255, 246, 226),
            stroke: Color32::from_rgb(237, 217, 173),
        },
        Tone::Danger => ToneColors {
            fg: Color32::from_rgb(191, 78, 65),
            bg: Color32::from_rgb(255, 239, 236),
            stroke: Color32::from_rgb(238, 197, 191),
        },
    }
}
