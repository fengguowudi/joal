use joal_core::snapshot::{EngineSnapshot, TorrentStatus};
use tokio::sync::mpsc;

use super::DeleteConfirmation;
use super::i18n::Tr;
use super::status_bar::format_speed;
use crate::EngineCommand;

#[allow(clippy::too_many_lines)]
pub fn show(
    ui: &mut egui::Ui,
    snapshot: &mut EngineSnapshot,
    pending_delete: &mut Option<DeleteConfirmation>,
    cmd_tx: &mpsc::Sender<EngineCommand>,
    t: &Tr,
) {
    if snapshot.torrents.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(t.no_torrents);
        });
        return;
    }

    let text_height = egui::TextStyle::Body
        .resolve(ui.style())
        .size
        .max(ui.spacing().interact_size.y);

    let available_height = ui.available_height();

    egui_extras::TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .min_scrolled_height(available_height)
        .max_scroll_height(available_height)
        .column(egui_extras::Column::remainder().at_least(180.0).clip(true)) // Name
        .column(egui_extras::Column::auto().at_least(70.0)) // Hash
        .column(egui_extras::Column::auto().at_least(70.0)) // Upload speed
        .column(egui_extras::Column::auto().at_least(80.0)) // Uploaded
        .column(egui_extras::Column::auto().at_least(70.0)) // Download speed
        .column(egui_extras::Column::auto().at_least(80.0)) // Downloaded
        .column(egui_extras::Column::auto().at_least(70.0)) // Progress
        .column(egui_extras::Column::auto().at_least(60.0)) // Seeders
        .column(egui_extras::Column::auto().at_least(60.0)) // Leechers
        .column(egui_extras::Column::auto().at_least(70.0)) // Status
        .column(egui_extras::Column::auto().at_least(80.0)) // Actions
        .header(text_height + 4.0, |mut header| {
            header.col(|ui| {
                ui.strong(t.col_name);
            });
            header.col(|ui| {
                ui.strong(t.col_hash);
            });
            header.col(|ui| {
                ui.strong(t.col_speed);
            });
            header.col(|ui| {
                ui.strong(t.col_uploaded);
            });
            header.col(|ui| {
                ui.strong(t.col_dl_speed);
            });
            header.col(|ui| {
                ui.strong(t.col_downloaded);
            });
            header.col(|ui| {
                ui.strong(t.col_progress);
            });
            header.col(|ui| {
                ui.strong(t.col_seeders);
            });
            header.col(|ui| {
                ui.strong(t.col_leechers);
            });
            header.col(|ui| {
                ui.strong(t.col_status);
            });
            header.col(|ui| {
                ui.strong(t.col_actions);
            });
        })
        .body(|body| {
            body.rows(text_height + 2.0, snapshot.torrents.len(), |mut row| {
                let torrent = &mut snapshot.torrents[row.index()];
                row.col(|ui| {
                    ui.add(egui::Label::new(torrent.name.as_str()).truncate());
                });
                row.col(|ui| {
                    let hash = torrent.info_hash.to_string();
                    ui.label(&hash[..8]);
                });
                row.col(|ui| {
                    ui.label(format_speed(torrent.current_speed_bps));
                });
                row.col(|ui| {
                    ui.label(format_bytes(torrent.uploaded_bytes));
                });
                row.col(|ui| {
                    ui.label(format_speed(torrent.current_download_speed_bps));
                });
                row.col(|ui| {
                    ui.label(format_bytes(torrent.downloaded_bytes));
                });
                row.col(|ui| {
                    ui.label(progress_text(torrent));
                });
                row.col(|ui| {
                    ui.label(opt_u32(torrent.last_known_seeders));
                });
                row.col(|ui| {
                    ui.label(opt_u32(torrent.last_known_leechers));
                });
                row.col(|ui| {
                    status_label(ui, torrent);
                });
                row.col(|ui| {
                    ui.horizontal(|ui| {
                        let response = ui
                            .checkbox(&mut torrent.initial_completed, "")
                            .on_hover_text(t.mark_completed_tooltip);
                        if response.changed() {
                            let _ = cmd_tx.try_send(EngineCommand::SetTorrentInitialCompleted {
                                info_hash: torrent.info_hash.clone(),
                                completed: torrent.initial_completed,
                            });
                        }
                        if ui
                            .button(
                                egui::RichText::new("X")
                                    .color(egui::Color32::from_rgb(200, 60, 60)),
                            )
                            .clicked()
                        {
                            *pending_delete = Some(DeleteConfirmation {
                                info_hash: torrent.info_hash.clone(),
                                name: torrent.name.clone(),
                            });
                        }
                    });
                });
            });
        });
}

fn status_label(ui: &mut egui::Ui, t: &TorrentStatus) {
    if t.consecutive_fails > 3 {
        ui.colored_label(egui::Color32::from_rgb(220, 50, 50), "ERROR");
    } else if t.consecutive_fails > 0 {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 50),
            format!("WARN({})", t.consecutive_fails),
        );
    } else {
        ui.colored_label(egui::Color32::from_rgb(80, 200, 80), "OK");
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn opt_u32(val: Option<u32>) -> String {
    val.map_or_else(|| "\u{2014}".to_owned(), |v| v.to_string())
}

fn progress_text(torrent: &TorrentStatus) -> String {
    if torrent.total_size == 0 {
        return "100.0%".to_owned();
    }
    let progress = torrent.downloaded_bytes as f64 * 100.0 / torrent.total_size as f64;
    format!("{progress:.1}%")
}
