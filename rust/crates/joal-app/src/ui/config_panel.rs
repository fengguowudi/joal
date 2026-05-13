use tokio::sync::mpsc;

use super::ConfigEditState;
use crate::EngineCommand;

pub fn show(
    ui: &mut egui::Ui,
    state: &mut ConfigEditState,
    available_clients: &[String],
    cmd_tx: &mpsc::Sender<EngineCommand>,
) {
    ui.heading("Configuration");
    ui.add_space(8.0);

    egui::Grid::new("config_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Min Upload Rate (kB/s):");
            ui.text_edit_singleline(&mut state.min_upload_rate);
            ui.end_row();

            ui.label("Max Upload Rate (kB/s):");
            ui.text_edit_singleline(&mut state.max_upload_rate);
            ui.end_row();

            ui.label("Simultaneous Seed:");
            ui.text_edit_singleline(&mut state.simultaneous_seed);
            ui.end_row();

            ui.label("Upload Ratio Target:");
            ui.text_edit_singleline(&mut state.upload_ratio_target);
            ui.end_row();

            ui.label("Client:");
            egui::ComboBox::from_id_salt("client_combo")
                .selected_text(&state.selected_client)
                .show_ui(ui, |ui| {
                    for client in available_clients {
                        ui.selectable_value(&mut state.selected_client, client.clone(), client);
                    }
                });
            ui.end_row();

            ui.label("Keep zero-leecher torrents:");
            ui.checkbox(&mut state.keep_torrent_with_zero_leechers, "");
            ui.end_row();
        });

    ui.add_space(12.0);
    ui.label(
        egui::RichText::new("Tip: -1.0 ratio target = seed forever")
            .small()
            .weak(),
    );
    ui.add_space(8.0);

    if ui.button("Save & Restart").clicked()
        && let Some(config) = state.to_config()
    {
        let _ = cmd_tx.try_send(EngineCommand::SaveConfig(config));
        // Restart: stop then start
        let _ = cmd_tx.try_send(EngineCommand::Stop);
        let _ = cmd_tx.try_send(EngineCommand::Start);
    }
}
