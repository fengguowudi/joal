use super::i18n::Tr;
use super::{ConfigEditState, ConfigNotice, ConfigValidationIssue, theme};

pub(super) struct ConfigPanelAction {
    pub apply_requested: bool,
    pub edited: bool,
}

#[derive(Clone, Copy)]
pub(super) struct ConfigPanelView<'a> {
    pub validation_errors: &'a [ConfigValidationIssue],
    pub operation_error: Option<&'a str>,
    pub notice: Option<ConfigNotice>,
    pub apply_in_progress: bool,
    pub available_clients: &'a [String],
    pub t: &'a Tr,
}

pub fn show(
    ui: &mut egui::Ui,
    state: &mut ConfigEditState,
    view: ConfigPanelView<'_>,
) -> ConfigPanelAction {
    let t = view.t;
    let mut edited = false;
    let accent = theme::tone_colors(theme::Tone::Accent);

    ui.heading(
        egui::RichText::new(t.configuration)
            .strong()
            .color(theme::text_primary()),
    );
    ui.add_space(8.0);
    theme::inset_frame().show(ui, |ui| {
        edited |= show_config_grid(ui, state, view.available_clients, t);
    });
    ui.add_space(10.0);
    theme::inset_frame().show(ui, |ui| {
        ui.label(
            egui::RichText::new(t.proxy_optional)
                .strong()
                .color(theme::text_primary()),
        );
        ui.add_space(6.0);
        edited |= show_proxy_grid(ui, state, t);
    });
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(t.tip_ratio)
            .small()
            .color(theme::text_secondary()),
    );
    ui.add_space(8.0);
    show_feedback(ui, &view);

    let apply_requested = ui
        .push_id("config_apply_button", |ui| {
            ui.add_enabled(
                !view.apply_in_progress,
                egui::Button::new(
                    egui::RichText::new(t.save_and_restart)
                        .strong()
                        .color(accent.fg),
                )
                .fill(accent.bg)
                .stroke(egui::Stroke::new(1.0, accent.stroke))
                .corner_radius(egui::CornerRadius::same(5))
                .min_size(egui::vec2(ui.available_width().max(180.0), 34.0)),
            )
        })
        .inner
        .clicked();

    ConfigPanelAction {
        apply_requested,
        edited,
    }
}

fn show_config_grid(
    ui: &mut egui::Ui,
    state: &mut ConfigEditState,
    available_clients: &[String],
    t: &Tr,
) -> bool {
    let mut edited = false;
    egui::Grid::new("config_grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            field_label(ui, t.min_upload_rate);
            edited |= config_text_field(ui, "config_min_upload_rate", &mut state.min_upload_rate)
                .changed();
            ui.end_row();

            field_label(ui, t.max_upload_rate);
            edited |= config_text_field(ui, "config_max_upload_rate", &mut state.max_upload_rate)
                .changed();
            ui.end_row();

            field_label(ui, t.min_download_rate);
            edited |=
                config_text_field(ui, "config_min_download_rate", &mut state.min_download_rate)
                    .changed();
            ui.end_row();

            field_label(ui, t.max_download_rate);
            edited |=
                config_text_field(ui, "config_max_download_rate", &mut state.max_download_rate)
                    .changed();
            ui.end_row();

            field_label(ui, t.simultaneous_seed);
            edited |=
                config_text_field(ui, "config_simultaneous_seed", &mut state.simultaneous_seed)
                    .changed();
            ui.end_row();

            field_label(ui, t.upload_ratio_target);
            edited |= config_text_field(
                ui,
                "config_upload_ratio_target",
                &mut state.upload_ratio_target,
            )
            .changed();
            ui.end_row();

            field_label(ui, t.client_label);
            egui::ComboBox::from_id_salt("client_combo")
                .width(178.0)
                .truncate()
                .selected_text(&state.selected_client)
                .show_ui(ui, |ui| {
                    for client in available_clients {
                        edited |= ui
                            .selectable_value(&mut state.selected_client, client.clone(), client)
                            .changed();
                    }
                });
            ui.end_row();

            field_label(ui, t.keep_zero_leecher);
            edited |= ui
                .push_id("config_keep_zero_leecher", |ui| {
                    ui.checkbox(&mut state.keep_torrent_with_zero_leechers, "")
                })
                .inner
                .changed();
            ui.end_row();
        });
    edited
}

fn show_proxy_grid(ui: &mut egui::Ui, state: &mut ConfigEditState, t: &Tr) -> bool {
    let mut edited = false;
    egui::Grid::new("proxy_grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            field_label(ui, t.proxy_host);
            edited |= config_text_field(ui, "config_proxy_host", &mut state.proxy_host).changed();
            ui.end_row();

            field_label(ui, t.proxy_port);
            edited |= config_text_field(ui, "config_proxy_port", &mut state.proxy_port).changed();
            ui.end_row();
        });
    edited
}

fn show_feedback(ui: &mut egui::Ui, view: &ConfigPanelView<'_>) {
    let t = view.t;
    if !view.validation_errors.is_empty() {
        let danger = theme::tone_colors(theme::Tone::Danger);
        theme::tone_frame(theme::Tone::Danger).show(ui, |ui| {
            ui.label(
                egui::RichText::new(t.config_validation_errors)
                    .strong()
                    .color(danger.fg),
            );
            ui.add_space(4.0);
            for issue in view.validation_errors {
                ui.label(egui::RichText::new(format!("• {}", issue.message(t))).color(danger.fg));
            }
        });
        ui.add_space(8.0);
    }

    if let Some(message) = view.operation_error {
        let danger = theme::tone_colors(theme::Tone::Danger);
        theme::tone_frame(theme::Tone::Danger).show(ui, |ui| {
            ui.label(egui::RichText::new(message).strong().color(danger.fg));
        });
        ui.add_space(8.0);
    }

    if let Some(notice) = view.notice {
        let success = theme::tone_colors(theme::Tone::Success);
        theme::tone_frame(theme::Tone::Success).show(ui, |ui| {
            ui.label(
                egui::RichText::new(notice.message(t))
                    .strong()
                    .color(success.fg),
            );
        });
        ui.add_space(8.0);
    }

    if view.apply_in_progress {
        let info = theme::tone_colors(theme::Tone::Info);
        theme::tone_frame(theme::Tone::Info).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new(t.config_apply_in_progress).color(info.fg));
            });
        });
        ui.add_space(8.0);
    }
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::text_secondary()));
}

fn config_text_field<'a>(
    ui: &mut egui::Ui,
    id: &'static str,
    value: &'a mut String,
) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .id_salt(id)
            .desired_width(148.0),
    )
}
