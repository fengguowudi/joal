use joal_core::snapshot::EngineSnapshot;

pub fn top_bar(ui: &mut egui::Ui, snapshot: &EngineSnapshot, engine_running: bool) {
    ui.horizontal(|ui| {
        // Engine status indicator
        if engine_running {
            ui.colored_label(egui::Color32::from_rgb(80, 200, 80), "\u{25CF}");
        } else {
            ui.colored_label(egui::Color32::from_rgb(200, 60, 60), "\u{25CF}");
        }
        ui.separator();
        ui.label(
            egui::RichText::new(format!("Client: {}", snapshot.active_client_filename)).strong(),
        );
        ui.separator();
        ui.label(format!(
            "Upload: {}",
            format_speed(snapshot.global_upload_speed_bps)
        ));
        ui.separator();
        ui.label(format!("Torrents: {}", snapshot.torrents.len()));
    });
}

pub fn bottom_bar(ui: &mut egui::Ui, started_at: std::time::Instant, engine_running: bool) {
    ui.horizontal(|ui| {
        let elapsed = started_at.elapsed().as_secs();
        let h = elapsed / 3600;
        let m = (elapsed % 3600) / 60;
        let s = elapsed % 60;

        if engine_running {
            ui.colored_label(egui::Color32::from_rgb(80, 200, 80), "\u{25CF}");
            ui.label("Running");
        } else {
            ui.colored_label(egui::Color32::from_rgb(200, 60, 60), "\u{25CF}");
            ui.label("Stopped");
        }
        ui.separator();
        ui.label(format!("Uptime: {h:02}:{m:02}:{s:02}"));
    });
}

pub fn format_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1_048_576 {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1_048_576.0)
    } else if bytes_per_sec >= 1024 {
        format!("{:.1} KB/s", bytes_per_sec as f64 / 1024.0)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}
