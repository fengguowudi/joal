use std::hash::Hash;

use egui::{
    Button, Color32, CornerRadius,
    FontFamily::{Monospace, Proportional},
    FontId, Frame, Label, Margin, Response, RichText, Shadow, Stroke, TextStyle, Ui,
};

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
}

pub(super) fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.visuals = egui::Visuals::light();
    style.text_styles = [
        (TextStyle::Small, FontId::new(12.0, Proportional)),
        (TextStyle::Body, FontId::new(14.0, Proportional)),
        (TextStyle::Button, FontId::new(14.0, Proportional)),
        (TextStyle::Heading, FontId::new(18.0, Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
    ]
    .into();
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
    visuals.selection.stroke = Stroke::new(1.0_f32, primary_color());
    visuals.faint_bg_color = panel_fill();
    visuals.extreme_bg_color = surface();
    visuals.text_edit_bg_color = Some(surface());
    visuals.code_bg_color = panel_fill();
    visuals.warn_fg_color = tone_colors(Tone::Warning).fg;
    visuals.error_fg_color = tone_colors(Tone::Danger).fg;
    visuals.window_corner_radius = CornerRadius::same(CR_PANEL);
    visuals.menu_corner_radius = CornerRadius::same(CR_INSET);
    visuals.window_fill = surface();
    visuals.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(230, 235, 241));
    visuals.panel_fill = app_background();
    visuals.popup_shadow.blur = 16;
    visuals.window_shadow.blur = 16;
    visuals.window_shadow.color = Color32::from_black_alpha(20);

    visuals.widgets.noninteractive.bg_fill = panel_fill();
    visuals.widgets.noninteractive.weak_bg_fill = panel_fill();
    visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(CR_INSET);
    // Pin every widget state's foreground so TextEdit content and ComboBox
    // items stay legible on the light surfaces.
    visuals.widgets.noninteractive.fg_stroke.color = text_primary();

    visuals.widgets.inactive.bg_fill = surface();
    visuals.widgets.inactive.weak_bg_fill = panel_fill();
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, soft_border());
    visuals.widgets.inactive.corner_radius = CornerRadius::same(CR_INSET);
    visuals.widgets.inactive.fg_stroke.color = text_primary();

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(241, 245, 249);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(241, 245, 249);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(203, 213, 225));
    visuals.widgets.hovered.corner_radius = CornerRadius::same(CR_INSET);
    visuals.widgets.hovered.fg_stroke.color = text_primary();

    visuals.widgets.active.bg_fill = Color32::from_rgb(226, 232, 240);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(226, 232, 240);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(203, 213, 225));
    visuals.widgets.active.corner_radius = CornerRadius::same(CR_INSET);
    visuals.widgets.active.fg_stroke.color = text_primary();

    visuals.widgets.open.bg_fill = Color32::from_rgb(241, 245, 249);
    visuals.widgets.open.weak_bg_fill = Color32::from_rgb(241, 245, 249);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(203, 213, 225));
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

/// Mid-gray used for muted/secondary text (hint copy, metric labels, "no
/// torrents" placeholders, column subtitles). Previously this returned
/// slate-500 (`#6B7280`), but CJK body-size glyphs at slate-500 on a pure
/// white `surface()` fill read as washed-out / "white-on-white" even though
/// the WCAG contrast number nominally passed. Bumping to slate-600
/// (`#475569`) keeps the muted-vs-primary visual hierarchy intact while
/// crossing the perceived-weight threshold for legibility on `#FFFFFF`.
pub(super) fn text_secondary() -> Color32 {
    Color32::from_rgb(71, 85, 105)
}

/// Slate-500 auxiliary metadata text. Use on white or `panel_fill()`, where
/// it meets the contrast target; do not use it on `app_background()`.
pub(super) fn text_tertiary() -> Color32 {
    Color32::from_rgb(100, 116, 139)
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

/// Frame for the floating config `egui::Window`. We override egui's default
/// `Frame::window` so the surface fill and stroke come from our explicit theme
/// values rather than whatever the theme-derived defaults produce — earlier
/// rounds shipped a state where the window rendered white-on-white in some
/// configurations, and pinning the frame at the call site removes that whole
/// failure mode regardless of which theme path egui resolves.
///
/// Mirrors `panel_frame()`'s surface (white + `CR_PANEL` rounding) so the
/// floating window reads as the same kind of "card" as the central torrent
/// table panel, but adds a 1px `soft_border` stroke and a soft drop shadow so
/// it visually lifts off the page when overlaid on top of the central panel.
pub(super) fn window_frame() -> Frame {
    Frame::new()
        .fill(surface())
        .stroke(Stroke::new(1.0_f32, soft_border()))
        .corner_radius(CornerRadius::same(CR_PANEL))
        .inner_margin(Margin::same(12))
        .shadow(Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(20),
        })
}

/// Edge-to-edge strip frame: square corners, configurable fill. Used for the
/// top status strip and bottom footer that act as visual dividers between
/// content regions. Rounded corners here would make the strips look like
/// chiclets rather than dividers — interactive controls (buttons, the config
/// window) keep their rounding via their own helpers.
pub(super) fn strip_frame(fill: Color32) -> Frame {
    Frame::new()
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::ZERO)
        .inner_margin(Margin::symmetric(16, 10))
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

/// Larger, status-strip-grade badge with a painted status dot and a body-size
/// label. Painting the dot avoids font-dependent glyph metrics and baselines.
pub(super) fn engine_badge(ui: &mut Ui, id: impl Hash, text: &str, tone: Tone) {
    let colors = tone_colors(tone);
    ui.push_id(id, |ui| {
        Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(CR_INSET))
            .inner_margin(Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.push_id("engine_badge_dot", |ui| {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(dot_rect.center(), 4.0, colors.fg);
                    });
                    ui.push_id("engine_badge_label", |ui| {
                        ui.add(
                            Label::new(RichText::new(text).color(colors.fg).strong()).truncate(),
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
        },
        Tone::Accent => ToneColors {
            fg: Color32::from_rgb(37, 99, 235),
            bg: Color32::from_rgb(239, 246, 255),
        },
        Tone::Info => ToneColors {
            fg: Color32::from_rgb(14, 116, 144),
            bg: Color32::from_rgb(236, 253, 255),
        },
        Tone::Success => ToneColors {
            fg: Color32::from_rgb(21, 128, 61),
            bg: Color32::from_rgb(240, 253, 244),
        },
        Tone::Warning => ToneColors {
            fg: Color32::from_rgb(180, 83, 9),
            bg: Color32::from_rgb(255, 251, 235),
        },
        Tone::Danger => ToneColors {
            fg: Color32::from_rgb(185, 28, 28),
            bg: Color32::from_rgb(254, 242, 242),
        },
    }
}

#[derive(Clone, Copy)]
struct ButtonPalette {
    rest: Color32,
    hover: Color32,
    press: Color32,
    fg: Color32,
    border: Stroke,
}

fn secondary_palette() -> ButtonPalette {
    ButtonPalette {
        rest: surface(),
        hover: Color32::from_rgb(241, 245, 249),
        press: Color32::from_rgb(226, 232, 240),
        fg: text_primary(),
        border: Stroke::new(1.0_f32, soft_border()),
    }
}

/// Linear channel interpolation used by selected tone buttons for their hover
/// and pressed fills.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    fn channel(a: u8, b: u8, t: f32) -> u8 {
        (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8
    }

    let t = t.clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied(
        channel(a.r(), b.r(), t),
        channel(a.g(), b.g(), t),
        channel(a.b(), b.b(), t),
        channel(a.a(), b.a(), t),
    )
}

/// Draw a 2px keyboard-focus indicator without adding a border to the button
/// itself. Other focusable controls can reuse this helper with their radius.
pub(super) fn focus_ring(ui: &Ui, response: &Response, radius: u8) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            CornerRadius::same(radius + 2),
            Stroke::new(2.0_f32, primary_color()),
            egui::StrokeKind::Outside,
        );
    }
}

fn themed_button(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    min_size: egui::Vec2,
    enabled: bool,
    palette: ButtonPalette,
) -> Response {
    ui.push_id(id, |ui| {
        // A scoped style override keeps state colors local to this button. In
        // particular, primary white foregrounds never leak into later widgets.
        ui.scope(|ui| {
            let widgets = &mut ui.visuals_mut().widgets;
            for (state, fill) in [
                (&mut widgets.inactive, palette.rest),
                (&mut widgets.hovered, palette.hover),
                (&mut widgets.active, palette.press),
            ] {
                state.weak_bg_fill = fill;
                state.bg_fill = fill;
                state.bg_stroke = palette.border;
                state.fg_stroke.color = palette.fg;
                state.expansion = 0.0;
            }

            let response = ui.add_enabled(
                enabled,
                Button::new(RichText::new(text).color(palette.fg))
                    .truncate()
                    .corner_radius(CornerRadius::same(CR_INSET))
                    .min_size(min_size),
            );
            focus_ring(ui, &response, CR_INSET);
            response
        })
        .inner
    })
    .inner
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

/// Same as [`primary_button`] but lets the caller gate the enabled state.
pub(super) fn primary_button_enabled(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    min_size: egui::Vec2,
    enabled: bool,
) -> Response {
    themed_button(
        ui,
        id,
        text,
        min_size,
        enabled,
        ButtonPalette {
            rest: primary_color(),
            hover: Color32::from_rgb(29, 78, 216),
            press: Color32::from_rgb(30, 64, 175),
            fg: Color32::WHITE,
            border: Stroke::NONE,
        },
    )
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
    themed_button(ui, id, text, min_size, enabled, secondary_palette())
}

/// Borderless secondary button for compact inline actions. It retains the
/// standard secondary hover/press feedback and the common focus ring.
pub(super) fn quiet_secondary_button(
    ui: &mut Ui,
    id: impl Hash,
    text: &str,
    min_size: egui::Vec2,
) -> Response {
    themed_button(
        ui,
        id,
        text,
        min_size,
        true,
        ButtonPalette {
            rest: Color32::TRANSPARENT,
            hover: Color32::from_rgb(241, 245, 249),
            press: Color32::from_rgb(226, 232, 240),
            fg: text_secondary(),
            border: Stroke::NONE,
        },
    )
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
    let palette = if selected {
        let colors = tone_colors(tone);
        ButtonPalette {
            rest: colors.bg,
            hover: mix(colors.bg, colors.fg, 0.08),
            press: mix(colors.bg, colors.fg, 0.16),
            fg: colors.fg,
            border: Stroke::NONE,
        }
    } else {
        secondary_palette()
    };
    themed_button(ui, id, text, min_size, enabled, palette)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_helpers_do_not_overlap_in_right_to_left_layout() {
        let ctx = egui::Context::default();
        let mut rects = Vec::new();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                rects.push(secondary_button(ui, "one", "EN", egui::vec2(56.0, 30.0)).rect);
                rects.push(
                    tone_button(
                        ui,
                        "two",
                        "Stop",
                        Tone::Danger,
                        egui::vec2(96.0, 30.0),
                        true,
                    )
                    .rect,
                );
                rects.push(primary_button(ui, "three", "Add", egui::vec2(124.0, 30.0)).rect);
            });
        });
        assert!(
            rects.windows(2).all(|pair| !pair[0].intersects(pair[1])),
            "{rects:?}"
        );
        assert!(
            rects[0].max.x > rects[1].max.x && rects[1].max.x > rects[2].max.x,
            "{rects:?}"
        );
    }

    #[test]
    fn action_group_fits_a_960px_surface() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960.0, 600.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut rects = Vec::new();
        let _ = ctx.run_ui(input, |ui| {
            egui::Panel::top("test_top").show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Running  0 B/s");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (id, label, width) in [
                            ("language", "EN", 56.0),
                            ("engine", "Stop", 96.0),
                            ("config", "Config", 96.0),
                            ("announce", "Announce Now", 132.0),
                            ("add", "Add Torrent", 124.0),
                        ] {
                            rects.push(
                                secondary_button(ui, id, label, egui::vec2(width, 30.0)).rect,
                            );
                        }
                    });
                });
            });
        });
        assert!(
            rects.iter().all(|rect| screen.contains_rect(*rect)),
            "{rects:?}"
        );
        assert!(
            rects.windows(2).all(|pair| !pair[0].intersects(pair[1])),
            "{rects:?}"
        );
    }
}
