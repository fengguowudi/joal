use std::hash::Hash;

use egui::{Button, Color32, CornerRadius, Frame, Label, Margin, Response, RichText, Stroke, Ui};

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
    /// Kept for API stability — visual edges are intentionally transparent so
    /// content blocks lean on background contrast instead of 1px borders.
    #[allow(dead_code)]
    pub stroke: Color32,
}

pub(super) fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.visuals = egui::Visuals::light();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.text_edit_width = 160.0;
    style.spacing.window_margin = Margin::same(12);

    let visuals = &mut style.visuals;
    visuals.override_text_color = Some(text_primary());
    visuals.weak_text_color = Some(text_tertiary());
    visuals.hyperlink_color = primary_color();
    visuals.selection.bg_fill = Color32::from_rgb(219, 234, 254);
    visuals.selection.stroke = Stroke::new(1.0, primary_color());
    visuals.faint_bg_color = panel_fill();
    visuals.extreme_bg_color = surface();
    visuals.text_edit_bg_color = Some(surface());
    visuals.code_bg_color = panel_fill();
    visuals.warn_fg_color = tone_colors(Tone::Warning).fg;
    visuals.error_fg_color = tone_colors(Tone::Danger).fg;
    visuals.window_corner_radius = CornerRadius::same(CR_PANEL);
    visuals.menu_corner_radius = CornerRadius::same(CR_INSET);
    visuals.window_fill = surface();
    visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(230, 235, 241));
    visuals.panel_fill = app_background();
    visuals.popup_shadow.blur = 16;
    visuals.window_shadow.blur = 16;
    visuals.window_shadow.color = Color32::from_black_alpha(20);

    visuals.widgets.noninteractive.bg_fill = panel_fill();
    visuals.widgets.noninteractive.weak_bg_fill = panel_fill();
    visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(CR_INSET);
    visuals.widgets.noninteractive.fg_stroke.color = text_primary();

    visuals.widgets.inactive.bg_fill = surface();
    visuals.widgets.inactive.weak_bg_fill = panel_fill();
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, soft_border());
    visuals.widgets.inactive.corner_radius = CornerRadius::same(CR_INSET);
    visuals.widgets.inactive.fg_stroke.color = text_primary();

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(241, 245, 249);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(241, 245, 249);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(203, 213, 225));
    visuals.widgets.hovered.corner_radius = CornerRadius::same(CR_INSET);
    visuals.widgets.hovered.fg_stroke.color = text_primary();

    visuals.widgets.active.bg_fill = Color32::from_rgb(226, 232, 240);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(226, 232, 240);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(203, 213, 225));
    visuals.widgets.active.corner_radius = CornerRadius::same(CR_INSET);
    visuals.widgets.active.fg_stroke.color = text_primary();

    visuals.widgets.open.bg_fill = Color32::from_rgb(241, 245, 249);
    visuals.widgets.open.weak_bg_fill = Color32::from_rgb(241, 245, 249);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, Color32::from_rgb(203, 213, 225));
    visuals.widgets.open.corner_radius = CornerRadius::same(CR_INSET);
    visuals.widgets.open.fg_stroke.color = text_primary();

    ctx.set_global_style(style);
}

// Unified corner radii. badge < inset/widget < panel.
pub(super) const CR_BADGE: u8 = 4;
pub(super) const CR_INSET: u8 = 6;
pub(super) const CR_PANEL: u8 = 8;

/// Near-black, reserved for strong data (torrent names, "100.0%", primary
/// numbers).
pub(super) fn text_primary() -> Color32 {
    Color32::from_rgb(17, 24, 39)
}

/// Mid-gray, used for normal body text, table headers, and field labels.
pub(super) fn text_secondary() -> Color32 {
    Color32::from_rgb(107, 114, 128)
}

/// Light-gray, used for auxiliary metadata (timestamps, "Interval ...",
/// client filenames).
pub(super) fn text_tertiary() -> Color32 {
    Color32::from_rgb(156, 163, 175)
}

pub(super) fn soft_border() -> Color32 {
    Color32::from_rgb(226, 232, 240)
}

pub(super) fn divider_color() -> Color32 {
    Color32::from_rgb(243, 244, 246)
}

pub(super) fn app_background() -> Color32 {
    Color32::from_rgb(244, 246, 248)
}

pub(super) fn panel_fill() -> Color32 {
    Color32::from_rgb(249, 250, 251)
}

pub(super) fn surface() -> Color32 {
    Color32::from_rgb(255, 255, 255)
}

/// Primary accent color used for the primary button, hyperlinks, and selection
/// highlights. Matches `Tone::Accent.fg`.
pub(super) fn primary_color() -> Color32 {
    Color32::from_rgb(37, 99, 235)
}

pub(super) fn panel_frame() -> Frame {
    Frame::new()
        .fill(surface())
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(CR_PANEL))
        .inner_margin(Margin::symmetric(16, 16))
}

pub(super) fn inset_frame() -> Frame {
    Frame::new()
        .fill(panel_fill())
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(CR_INSET))
        .inner_margin(Margin::symmetric(12, 10))
}

pub(super) fn tone_frame(tone: Tone) -> Frame {
    let colors = tone_colors(tone);
    Frame::new()
        .fill(colors.bg)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(CR_INSET))
        .inner_margin(Margin::symmetric(12, 10))
}

pub(super) fn badge(ui: &mut Ui, id: impl Hash, text: &str, tone: Tone) {
    let colors = tone_colors(tone);
    ui.push_id(id, |ui| {
        Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(CR_BADGE))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.push_id("badge_label", |ui| {
                    ui.add(
                        Label::new(RichText::new(text).color(colors.fg).strong().small())
                            .truncate(),
                    );
                });
            });
    });
}

pub(super) fn metric(ui: &mut Ui, id: impl Hash, label: &str, value: impl ToString, tone: Tone) {
    let colors = tone_colors(tone);
    let value = value.to_string();
    ui.push_id(id, |ui| {
        Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(CR_INSET))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if !label.is_empty() {
                        ui.push_id("metric_label", |ui| {
                            ui.add(
                                Label::new(RichText::new(label).small().color(text_secondary()))
                                    .truncate(),
                            );
                        });
                    }
                    ui.push_id("metric_value", |ui| {
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
    });
}

pub(super) fn tone_colors(tone: Tone) -> ToneColors {
    match tone {
        Tone::Neutral => ToneColors {
            fg: text_primary(),
            bg: Color32::from_rgb(244, 246, 250),
            stroke: Color32::TRANSPARENT,
        },
        Tone::Accent => ToneColors {
            fg: Color32::from_rgb(37, 99, 235),
            bg: Color32::from_rgb(239, 246, 255),
            stroke: Color32::TRANSPARENT,
        },
        Tone::Info => ToneColors {
            fg: Color32::from_rgb(14, 116, 144),
            bg: Color32::from_rgb(236, 253, 255),
            stroke: Color32::TRANSPARENT,
        },
        Tone::Success => ToneColors {
            fg: Color32::from_rgb(22, 163, 74),
            bg: Color32::from_rgb(240, 253, 244),
            stroke: Color32::TRANSPARENT,
        },
        Tone::Warning => ToneColors {
            fg: Color32::from_rgb(217, 119, 6),
            bg: Color32::from_rgb(255, 251, 235),
            stroke: Color32::TRANSPARENT,
        },
        Tone::Danger => ToneColors {
            fg: Color32::from_rgb(220, 38, 38),
            bg: Color32::from_rgb(254, 242, 242),
            stroke: Color32::TRANSPARENT,
        },
    }
}

/// Render a high-emphasis primary button (solid accent fill, white text, no
/// border). Use sparingly — typically one per surface, on the single action
/// you most want the user to click. Other actions should fall back to the
/// default light-gray secondary button.
pub(super) fn primary_button(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    min_size: egui::Vec2,
) -> Response {
    primary_button_enabled(ui, id, text, min_size, true)
}

/// Same as [`primary_button`] but lets the caller gate the enabled state. We
/// keep the visual structure (push_id + `Button::min_size`) identical to the
/// always-on path so toggling enabled does not destabilize widget ids across
/// multi-pass frames.
pub(super) fn primary_button_enabled(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    min_size: egui::Vec2,
    enabled: bool,
) -> Response {
    ui.push_id(id, |ui| {
        let bg = primary_color();
        let bg_hover = Color32::from_rgb(29, 78, 216);
        let bg_active = Color32::from_rgb(30, 64, 175);
        let label = RichText::new(text).strong().color(Color32::WHITE);
        let button = Button::new(label)
            .truncate()
            .fill(bg)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(CR_INSET))
            .min_size(min_size);

        // Override widget visuals inside this scope so hover/active also stay
        // on a single primary hue (no light-gray hover bleed-through).
        let mut visuals = ui.visuals().clone();
        visuals.widgets.inactive.bg_fill = bg;
        visuals.widgets.inactive.weak_bg_fill = bg;
        visuals.widgets.inactive.bg_stroke = Stroke::NONE;
        visuals.widgets.inactive.fg_stroke.color = Color32::WHITE;
        visuals.widgets.hovered.bg_fill = bg_hover;
        visuals.widgets.hovered.weak_bg_fill = bg_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::NONE;
        visuals.widgets.hovered.fg_stroke.color = Color32::WHITE;
        visuals.widgets.active.bg_fill = bg_active;
        visuals.widgets.active.weak_bg_fill = bg_active;
        visuals.widgets.active.bg_stroke = Stroke::NONE;
        visuals.widgets.active.fg_stroke.color = Color32::WHITE;

        ui.scope(|ui| {
            ui.ctx().set_visuals(visuals);
            ui.add_enabled(enabled, button)
        })
        .inner
    })
    .inner
}

/// Render a secondary (default) button — white surface, soft gray border,
/// near-black text. Pairs with [`primary_button`] so the toolbar has a
/// clear visual hierarchy.
pub(super) fn secondary_button(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    min_size: egui::Vec2,
) -> Response {
    secondary_button_enabled(ui, id, text, min_size, true)
}

pub(super) fn secondary_button_enabled(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    min_size: egui::Vec2,
    enabled: bool,
) -> Response {
    ui.push_id(id, |ui| {
        let label = RichText::new(text).strong().color(text_primary());
        ui.add_enabled(
            enabled,
            Button::new(label)
                .truncate()
                .fill(surface())
                .stroke(Stroke::new(1.0, soft_border()))
                .corner_radius(CornerRadius::same(CR_INSET))
                .min_size(min_size),
        )
    })
    .inner
}

/// Render a secondary button tinted with a tone color (used for engine
/// start/stop toggle and other secondary actions that still benefit from a
/// semantic hue but should not compete with the primary button).
pub(super) fn tone_button(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    tone: Tone,
    min_size: egui::Vec2,
    selected: bool,
) -> Response {
    tone_button_enabled(ui, id, text, tone, min_size, selected, true)
}

pub(super) fn tone_button_enabled(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    tone: Tone,
    min_size: egui::Vec2,
    selected: bool,
    enabled: bool,
) -> Response {
    ui.push_id(id, |ui| {
        let colors = tone_colors(tone);
        // Soft tinted background with strong colored text; no border.
        let bg = if selected { colors.bg } else { surface() };
        let fg = if selected { colors.fg } else { text_primary() };
        let stroke = if selected {
            Stroke::NONE
        } else {
            Stroke::new(1.0, soft_border())
        };
        ui.add_enabled(
            enabled,
            Button::new(RichText::new(text).strong().color(fg))
                .truncate()
                .fill(bg)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(CR_INSET))
                .selected(selected)
                .frame_when_inactive(true)
                .min_size(min_size),
        )
    })
    .inner
}
