use super::config_state::{ConfigField, ConfigPanelState, ConfigValidationIssue};
use super::i18n::Tr;
use super::theme;

pub(super) struct ConfigPanelAction {
    pub apply_requested: bool,
    pub edited: bool,
}

pub fn show(ui: &mut egui::Ui, state: &mut ConfigPanelState, t: &Tr) -> ConfigPanelAction {
    let mut edited = false;
    theme::inset_frame().show(ui, |ui| {
        edited |= show_config_grid(ui, state, t);
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

    if state.dirty() && !state.applying {
        let hint = if state.only_rate_fields_edited() {
            t.config_apply_live
        } else {
            t.config_apply_restart
        };
        ui.label(
            egui::RichText::new(hint)
                .small()
                .color(theme::text_secondary()),
        );
    }
    ui.add_space(8.0);
    show_feedback(ui, state, t);

    let response = ui
        .vertical_centered(|ui| {
            theme::primary_button_enabled(
                ui,
                "config_apply_button",
                t.save,
                egui::vec2(240.0, 36.0),
                state.dirty() && !state.applying,
            )
        })
        .inner;
    if !state.dirty() {
        response.clone().on_disabled_hover_text(t.config_no_changes);
    }

    ConfigPanelAction {
        apply_requested: response.clicked(),
        edited,
    }
}

#[allow(clippy::too_many_lines)]
fn show_config_grid(ui: &mut egui::Ui, state: &mut ConfigPanelState, t: &Tr) -> bool {
    let mut edited = false;
    let issues = &state.issues;
    let edit = &mut state.edit;
    egui::Grid::new("config_grid")
        .num_columns(2)
        .min_col_width(120.0)
        .spacing([24.0, 12.0])
        .show(ui, |ui| {
            field_label(ui, t.min_upload_rate);
            edited |= config_text_field(
                ui,
                "config_min_upload_rate",
                &mut edit.min_upload_rate,
                ConfigField::MinUploadRate,
                issues,
                t,
            )
            .changed();
            ui.end_row();

            field_label(ui, t.max_upload_rate);
            edited |= config_text_field(
                ui,
                "config_max_upload_rate",
                &mut edit.max_upload_rate,
                ConfigField::MaxUploadRate,
                issues,
                t,
            )
            .changed();
            ui.end_row();

            field_label(ui, t.min_download_rate);
            edited |= config_text_field(
                ui,
                "config_min_download_rate",
                &mut edit.min_download_rate,
                ConfigField::MinDownloadRate,
                issues,
                t,
            )
            .changed();
            ui.end_row();

            field_label(ui, t.max_download_rate);
            edited |= config_text_field(
                ui,
                "config_max_download_rate",
                &mut edit.max_download_rate,
                ConfigField::MaxDownloadRate,
                issues,
                t,
            )
            .changed();
            ui.end_row();

            field_label(ui, t.simultaneous_seed);
            edited |= config_text_field(
                ui,
                "config_simultaneous_seed",
                &mut edit.simultaneous_seed,
                ConfigField::SimultaneousSeed,
                issues,
                t,
            )
            .changed();
            ui.end_row();

            field_label(ui, t.upload_ratio_target);
            edited |= config_text_field(
                ui,
                "config_upload_ratio_target",
                &mut edit.upload_ratio_target,
                ConfigField::UploadRatioTarget,
                issues,
                t,
            )
            .changed();
            ui.end_row();

            field_label(ui, t.client_label);
            ui.vertical(|ui| {
                let response = egui::ComboBox::from_id_salt("client_combo")
                    .width(148.0)
                    .truncate()
                    .selected_text(&edit.selected_client)
                    .show_ui(ui, |ui| {
                        for client in &state.clients {
                            edited |= ui
                                .selectable_value(&mut edit.selected_client, client.clone(), client)
                                .changed();
                        }
                    });
                theme::focus_ring(ui, &response.response, theme::CR_INSET);
                inline_error(ui, ConfigField::Client, issues, t);
            });
            ui.end_row();

            ui.label("");
            edited |= ui
                .push_id("config_keep_zero_leecher", |ui| {
                    ui.checkbox(
                        &mut edit.keep_torrent_with_zero_leechers,
                        egui::RichText::new(t.keep_zero_leecher).color(theme::text_primary()),
                    )
                })
                .inner
                .changed();
            ui.end_row();
        });
    edited
}

fn show_proxy_grid(ui: &mut egui::Ui, state: &mut ConfigPanelState, t: &Tr) -> bool {
    let mut edited = false;
    let issues = &state.issues;
    let edit = &mut state.edit;
    egui::Grid::new("proxy_grid")
        .num_columns(2)
        .min_col_width(120.0)
        .spacing([24.0, 12.0])
        .show(ui, |ui| {
            field_label(ui, t.proxy_host);
            edited |= config_text_field(
                ui,
                "config_proxy_host",
                &mut edit.proxy_host,
                ConfigField::ProxyHost,
                issues,
                t,
            )
            .changed();
            ui.end_row();

            field_label(ui, t.proxy_port);
            edited |= config_text_field(
                ui,
                "config_proxy_port",
                &mut edit.proxy_port,
                ConfigField::ProxyPort,
                issues,
                t,
            )
            .changed();
            ui.end_row();
        });
    edited
}

fn show_feedback(ui: &mut egui::Ui, state: &ConfigPanelState, t: &Tr) {
    let has_unexpected = state
        .issues
        .iter()
        .any(|issue| matches!(issue, ConfigValidationIssue::Unexpected(_)));
    if has_unexpected {
        theme::tone_frame(theme::Tone::Danger).show(ui, |ui| {
            for issue in &state.issues {
                if matches!(issue, ConfigValidationIssue::Unexpected(_)) {
                    ui.label(
                        egui::RichText::new(issue.message(t))
                            .color(theme::tone_colors(theme::Tone::Danger).fg),
                    );
                }
            }
        });
        ui.add_space(8.0);
    }
    if let Some(message) = &state.operation_error {
        theme::tone_frame(theme::Tone::Danger).show(ui, |ui| {
            ui.label(
                egui::RichText::new(message)
                    .strong()
                    .color(theme::tone_colors(theme::Tone::Danger).fg),
            );
        });
        ui.add_space(8.0);
    }
    if let Some(notice) = state.notice {
        theme::tone_frame(theme::Tone::Success).show(ui, |ui| {
            ui.label(
                egui::RichText::new(notice.message(t))
                    .strong()
                    .color(theme::tone_colors(theme::Tone::Success).fg),
            );
        });
        ui.add_space(8.0);
    }
    if state.applying {
        theme::tone_frame(theme::Tone::Info).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new(t.config_apply_in_progress)
                        .color(theme::tone_colors(theme::Tone::Info).fg),
                );
            });
        });
        ui.add_space(8.0);
    }
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::text_primary()));
}

#[allow(clippy::too_many_arguments)]
fn config_text_field(
    ui: &mut egui::Ui,
    id: &'static str,
    value: &mut String,
    field: ConfigField,
    issues: &[ConfigValidationIssue],
    t: &Tr,
) -> egui::Response {
    ui.vertical(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(value)
                .id_salt(id)
                .desired_width(148.0),
        );
        theme::focus_ring(ui, &response, theme::CR_INSET);
        inline_error(ui, field, issues, t);
        response
    })
    .inner
}

fn inline_error(ui: &mut egui::Ui, field: ConfigField, issues: &[ConfigValidationIssue], t: &Tr) {
    if let Some(issue) = issues.iter().find(|issue| issue.field() == Some(field)) {
        ui.label(
            egui::RichText::new(issue.message(t))
                .small()
                .color(theme::tone_colors(theme::Tone::Danger).fg),
        );
    }
}
