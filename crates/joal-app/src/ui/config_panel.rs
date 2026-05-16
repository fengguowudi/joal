use tokio::sync::mpsc;

use super::ConfigEditState;
use super::i18n::Tr;
use crate::EngineCommand;

pub fn show(
    ui: &mut egui::Ui,
    state: &mut ConfigEditState,
    available_clients: &[String],
    cmd_tx: &mpsc::Sender<EngineCommand>,
    t: &Tr,
) {
    ui.heading(t.configuration);
    ui.add_space(8.0);

    egui::Grid::new("config_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label(t.min_upload_rate);
            ui.text_edit_singleline(&mut state.min_upload_rate);
            ui.end_row();

            ui.label(t.max_upload_rate);
            ui.text_edit_singleline(&mut state.max_upload_rate);
            ui.end_row();

            ui.label(t.min_download_rate);
            ui.text_edit_singleline(&mut state.min_download_rate);
            ui.end_row();

            ui.label(t.max_download_rate);
            ui.text_edit_singleline(&mut state.max_download_rate);
            ui.end_row();

            ui.label(t.simultaneous_seed);
            ui.text_edit_singleline(&mut state.simultaneous_seed);
            ui.end_row();

            ui.label(t.upload_ratio_target);
            ui.text_edit_singleline(&mut state.upload_ratio_target);
            ui.end_row();

            ui.label(t.client_label);
            egui::ComboBox::from_id_salt("client_combo")
                .selected_text(&state.selected_client)
                .show_ui(ui, |ui| {
                    for client in available_clients {
                        ui.selectable_value(&mut state.selected_client, client.clone(), client);
                    }
                });
            ui.end_row();

            ui.label(t.keep_zero_leecher);
            ui.checkbox(&mut state.keep_torrent_with_zero_leechers, "");
            ui.end_row();
        });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(egui::RichText::new(t.proxy_optional).strong());
    ui.add_space(4.0);

    egui::Grid::new("proxy_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label(t.proxy_host);
            ui.text_edit_singleline(&mut state.proxy_host);
            ui.end_row();

            ui.label(t.proxy_port);
            ui.text_edit_singleline(&mut state.proxy_port);
            ui.end_row();
        });

    ui.add_space(12.0);
    ui.label(egui::RichText::new(t.tip_ratio).small().weak());
    ui.add_space(8.0);

    if ui.button(t.save_and_restart).clicked()
        && let Some(config) = state.to_config()
    {
        let _ = cmd_tx.try_send(EngineCommand::SaveConfig(config));
        let _ = cmd_tx.try_send(EngineCommand::Stop);
        let _ = cmd_tx.try_send(EngineCommand::Start);
    }
}
