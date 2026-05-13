use joal_core::snapshot::{EngineSnapshot, TorrentStatus};

use super::DeleteConfirmation;
use super::status_bar::format_speed;

pub fn show(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    pending_delete: &mut Option<DeleteConfirmation>,
) {
    if snapshot.torrents.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("No torrents loaded — add .torrent files to your torrents/ folder");
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
        .column(egui_extras::Column::remainder().at_least(150.0)) // Name
        .column(egui_extras::Column::auto().at_least(80.0)) // Hash
        .column(egui_extras::Column::auto().at_least(80.0)) // Speed
        .column(egui_extras::Column::auto().at_least(80.0)) // Uploaded
        .column(egui_extras::Column::auto().at_least(60.0)) // Seeders
        .column(egui_extras::Column::auto().at_least(60.0)) // Leechers
        .column(egui_extras::Column::auto().at_least(60.0)) // Interval
        .column(egui_extras::Column::auto().at_least(60.0)) // Status
        .column(egui_extras::Column::auto().at_least(50.0)) // Actions
        .header(text_height + 4.0, |mut header| {
            header.col(|ui| {
                ui.strong("Name");
            });
            header.col(|ui| {
                ui.strong("Hash");
            });
            header.col(|ui| {
                ui.strong("Speed");
            });
            header.col(|ui| {
                ui.strong("Uploaded");
            });
            header.col(|ui| {
                ui.strong("Seeders");
            });
            header.col(|ui| {
                ui.strong("Leechers");
            });
            header.col(|ui| {
                ui.strong("Interval");
            });
            header.col(|ui| {
                ui.strong("Status");
            });
            header.col(|ui| {
                ui.strong("");
            });
        })
        .body(|body| {
            body.rows(text_height + 2.0, snapshot.torrents.len(), |mut row| {
                let t = &snapshot.torrents[row.index()];
                row.col(|ui| {
                    ui.label(&t.name);
                });
                row.col(|ui| {
                    ui.label(&t.info_hash.to_string()[..8]);
                });
                row.col(|ui| {
                    ui.label(format_speed(t.current_speed_bps));
                });
                row.col(|ui| {
                    ui.label(format_bytes(t.uploaded_bytes));
                });
                row.col(|ui| {
                    ui.label(opt_u32(t.last_known_seeders));
                });
                row.col(|ui| {
                    ui.label(opt_u32(t.last_known_leechers));
                });
                row.col(|ui| {
                    ui.label(opt_interval(t.last_known_interval));
                });
                row.col(|ui| {
                    status_label(ui, t);
                });
                row.col(|ui| {
                    if ui
                        .button(
                            egui::RichText::new("X").color(egui::Color32::from_rgb(200, 60, 60)),
                        )
                        .clicked()
                    {
                        *pending_delete = Some(DeleteConfirmation {
                            info_hash: t.info_hash.clone(),
                            name: t.name.clone(),
                        });
                    }
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

fn opt_interval(val: Option<u32>) -> String {
    val.map_or_else(|| "\u{2014}".to_owned(), |v| format!("{v}s"))
}
