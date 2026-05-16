use super::i18n::Tr;
use super::{ConfigEditState, ConfigNotice, ConfigValidationIssue};

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

    ui.heading(t.configuration);
    ui.add_space(8.0);
    edited |= show_config_grid(ui, state, view.available_clients, t);
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(egui::RichText::new(t.proxy_optional).strong());
    ui.add_space(4.0);
    edited |= show_proxy_grid(ui, state, t);
    ui.add_space(12.0);
    ui.label(egui::RichText::new(t.tip_ratio).small().weak());
    ui.add_space(8.0);
    show_feedback(ui, &view);

    let apply_requested = ui
        .add_enabled(
            !view.apply_in_progress,
            egui::Button::new(t.save_and_restart),
        )
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
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label(t.min_upload_rate);
            edited |= ui
                .text_edit_singleline(&mut state.min_upload_rate)
                .changed();
            ui.end_row();

            ui.label(t.max_upload_rate);
            edited |= ui
                .text_edit_singleline(&mut state.max_upload_rate)
                .changed();
            ui.end_row();

            ui.label(t.min_download_rate);
            edited |= ui
                .text_edit_singleline(&mut state.min_download_rate)
                .changed();
            ui.end_row();

            ui.label(t.max_download_rate);
            edited |= ui
                .text_edit_singleline(&mut state.max_download_rate)
                .changed();
            ui.end_row();

            ui.label(t.simultaneous_seed);
            edited |= ui
                .text_edit_singleline(&mut state.simultaneous_seed)
                .changed();
            ui.end_row();

            ui.label(t.upload_ratio_target);
            edited |= ui
                .text_edit_singleline(&mut state.upload_ratio_target)
                .changed();
            ui.end_row();

            ui.label(t.client_label);
            egui::ComboBox::from_id_salt("client_combo")
                .selected_text(&state.selected_client)
                .show_ui(ui, |ui| {
                    for client in available_clients {
                        edited |= ui
                            .selectable_value(&mut state.selected_client, client.clone(), client)
                            .changed();
                    }
                });
            ui.end_row();

            ui.label(t.keep_zero_leecher);
            edited |= ui
                .checkbox(&mut state.keep_torrent_with_zero_leechers, "")
                .changed();
            ui.end_row();
        });
    edited
}

fn show_proxy_grid(ui: &mut egui::Ui, state: &mut ConfigEditState, t: &Tr) -> bool {
    let mut edited = false;
    egui::Grid::new("proxy_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label(t.proxy_host);
            edited |= ui.text_edit_singleline(&mut state.proxy_host).changed();
            ui.end_row();

            ui.label(t.proxy_port);
            edited |= ui.text_edit_singleline(&mut state.proxy_port).changed();
            ui.end_row();
        });
    edited
}

fn show_feedback(ui: &mut egui::Ui, view: &ConfigPanelView<'_>) {
    let t = view.t;
    if !view.validation_errors.is_empty() {
        ui.group(|ui| {
            ui.colored_label(ui.visuals().error_fg_color, t.config_validation_errors);
            ui.add_space(4.0);
            for issue in view.validation_errors {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("• {}", issue.message(t)),
                );
            }
        });
        ui.add_space(8.0);
    }

    if let Some(message) = view.operation_error {
        ui.colored_label(ui.visuals().error_fg_color, message);
        ui.add_space(8.0);
    }

    if let Some(notice) = view.notice {
        ui.colored_label(egui::Color32::from_rgb(56, 142, 60), notice.message(t));
        ui.add_space(8.0);
    }

    if view.apply_in_progress {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(t.config_apply_in_progress);
        });
        ui.add_space(8.0);
    }
}
