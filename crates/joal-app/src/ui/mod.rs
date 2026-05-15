mod config_panel;
pub mod i18n;
mod log_panel;
mod speed_chart;
mod status_bar;
mod torrent_table;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use joal_core::config::{AppConfiguration, JoalFolders};
use joal_core::events::EngineEvent;
use joal_core::snapshot::EngineSnapshot;
use joal_core::torrent::InfoHash;
use tokio::sync::{broadcast, mpsc, watch};

use crate::{EngineCommand, EngineResponse};
use i18n::{Language, tr};

const LOG_BUFFER_CAPACITY: usize = 500;
const SPEED_HISTORY_CAPACITY: usize = 300;

pub struct LogEntry {
    pub timestamp: Instant,
    pub message: String,
}

/// State for the delete-confirmation dialog.
struct DeleteConfirmation {
    info_hash: InfoHash,
    name: String,
}

#[allow(clippy::struct_excessive_bools)]
pub struct JoalApp {
    snapshot_rx: watch::Receiver<EngineSnapshot>,
    events_rx: broadcast::Receiver<EngineEvent>,
    started_at: Instant,
    current_snapshot: EngineSnapshot,
    log_buffer: VecDeque<LogEntry>,
    speed_history: VecDeque<(f64, f64)>,
    log_auto_scroll: bool,

    // Command/response channels
    cmd_tx: mpsc::Sender<EngineCommand>,
    resp_rx: mpsc::Receiver<EngineResponse>,
    #[allow(dead_code)]
    folders: Arc<JoalFolders>,

    // Engine state tracking
    engine_running: bool,

    // Config editor state
    show_config_panel: bool,
    config_edit: ConfigEditState,
    available_clients: Vec<String>,
    clients_requested: bool,

    // Delete confirmation
    pending_delete: Option<DeleteConfirmation>,

    // Language
    language: Language,
}

/// Editable config fields mirroring AppConfiguration.
struct ConfigEditState {
    min_upload_rate: String,
    max_upload_rate: String,
    simultaneous_seed: String,
    upload_ratio_target: String,
    selected_client: String,
    keep_torrent_with_zero_leechers: bool,
    proxy_host: String,
    proxy_port: String,
}

impl ConfigEditState {
    fn from_snapshot(snapshot: &EngineSnapshot, config: Option<&AppConfiguration>) -> Self {
        if let Some(cfg) = config {
            Self {
                min_upload_rate: cfg.min_upload_rate.to_string(),
                max_upload_rate: cfg.max_upload_rate.to_string(),
                simultaneous_seed: cfg.simultaneous_seed.to_string(),
                upload_ratio_target: format!("{:.1}", cfg.upload_ratio_target),
                selected_client: cfg.client.clone(),
                keep_torrent_with_zero_leechers: cfg.keep_torrent_with_zero_leechers,
                proxy_host: cfg.proxy_host.clone().unwrap_or_default(),
                proxy_port: cfg.proxy_port.map_or_else(String::new, |p| p.to_string()),
            }
        } else {
            Self {
                min_upload_rate: "30".to_owned(),
                max_upload_rate: "170".to_owned(),
                simultaneous_seed: "5".to_owned(),
                upload_ratio_target: "-1.0".to_owned(),
                selected_client: snapshot.active_client_filename.clone(),
                keep_torrent_with_zero_leechers: true,
                proxy_host: String::new(),
                proxy_port: String::new(),
            }
        }
    }

    fn to_config(&self) -> Option<AppConfiguration> {
        let min_upload_rate = self.min_upload_rate.parse::<u64>().ok()?;
        let max_upload_rate = self.max_upload_rate.parse::<u64>().ok()?;
        let simultaneous_seed = self.simultaneous_seed.parse::<u32>().ok()?;
        let upload_ratio_target = self.upload_ratio_target.parse::<f32>().ok()?;
        let proxy_host = if self.proxy_host.trim().is_empty() {
            None
        } else {
            Some(self.proxy_host.trim().to_owned())
        };
        let proxy_port = if self.proxy_port.trim().is_empty() {
            None
        } else {
            self.proxy_port.trim().parse::<u16>().ok()
        };
        Some(AppConfiguration {
            min_upload_rate,
            max_upload_rate,
            simultaneous_seed,
            client: self.selected_client.clone(),
            keep_torrent_with_zero_leechers: self.keep_torrent_with_zero_leechers,
            upload_ratio_target,
            proxy_host,
            proxy_port,
        })
    }
}

impl JoalApp {
    pub fn new(
        snapshot_rx: watch::Receiver<EngineSnapshot>,
        events_rx: broadcast::Receiver<EngineEvent>,
        started_at: Instant,
        cmd_tx: mpsc::Sender<EngineCommand>,
        resp_rx: mpsc::Receiver<EngineResponse>,
        folders: Arc<JoalFolders>,
    ) -> Self {
        let current_snapshot = snapshot_rx.borrow().clone();
        // Load initial config from disk so the config panel shows real values.
        let initial_config = load_config_sync(&folders);
        let config_edit =
            ConfigEditState::from_snapshot(&current_snapshot, initial_config.as_ref());
        Self {
            snapshot_rx,
            events_rx,
            started_at,
            current_snapshot,
            log_buffer: VecDeque::with_capacity(LOG_BUFFER_CAPACITY),
            speed_history: VecDeque::with_capacity(SPEED_HISTORY_CAPACITY),
            log_auto_scroll: true,
            cmd_tx,
            resp_rx,
            folders,
            engine_running: true,
            show_config_panel: false,
            config_edit,
            available_clients: Vec::new(),
            clients_requested: false,
            pending_delete: None,
            language: Language::default(),
        }
    }

    fn poll_snapshot(&mut self) -> bool {
        if self.snapshot_rx.has_changed().unwrap_or(false) {
            self.current_snapshot = self.snapshot_rx.borrow_and_update().clone();
            let elapsed = self.started_at.elapsed().as_secs_f64();
            let speed = self.current_snapshot.global_upload_speed_bps as f64;
            self.speed_history.push_back((elapsed, speed));
            if self.speed_history.len() > SPEED_HISTORY_CAPACITY {
                self.speed_history.pop_front();
            }
            true
        } else {
            false
        }
    }

    fn drain_events(&mut self) {
        loop {
            match self.events_rx.try_recv() {
                Ok(event) => {
                    // Track engine state from events
                    match &event {
                        EngineEvent::GlobalSeedStarted { .. } => {
                            self.engine_running = true;
                        }
                        EngineEvent::GlobalSeedStopped => {
                            self.engine_running = false;
                        }
                        EngineEvent::ConfigLoaded { config } => {
                            self.config_edit = ConfigEditState::from_snapshot(
                                &self.current_snapshot,
                                Some(config),
                            );
                        }
                        _ => {}
                    }
                    let message = format_event(&event);
                    self.log_buffer.push_back(LogEntry {
                        timestamp: Instant::now(),
                        message,
                    });
                    if self.log_buffer.len() > LOG_BUFFER_CAPACITY {
                        self.log_buffer.pop_front();
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    self.log_buffer.push_back(LogEntry {
                        timestamp: Instant::now(),
                        message: format!("[warning] event bus lagged, {n} events dropped"),
                    });
                    if self.log_buffer.len() > LOG_BUFFER_CAPACITY {
                        self.log_buffer.pop_front();
                    }
                }
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => break,
            }
        }
    }

    fn drain_responses(&mut self) {
        while let Ok(resp) = self.resp_rx.try_recv() {
            match resp {
                EngineResponse::ClientList(clients) => {
                    self.available_clients = clients;
                }
                EngineResponse::Stopped => {
                    self.engine_running = false;
                }
                EngineResponse::Started {
                    snapshot_rx,
                    events_rx,
                } => {
                    self.snapshot_rx = snapshot_rx;
                    self.events_rx = events_rx;
                    self.engine_running = true;
                }
                EngineResponse::Error(msg) => {
                    self.log_buffer.push_back(LogEntry {
                        timestamp: Instant::now(),
                        message: format!("[error] {msg}"),
                    });
                    if self.log_buffer.len() > LOG_BUFFER_CAPACITY {
                        self.log_buffer.pop_front();
                    }
                }
            }
        }
    }

    fn send_command(&self, cmd: EngineCommand) {
        let _ = self.cmd_tx.try_send(cmd);
    }
}

impl eframe::App for JoalApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let changed = self.poll_snapshot();
        self.drain_events();
        self.drain_responses();

        // Request client list once
        if !self.clients_requested {
            self.clients_requested = true;
            self.send_command(EngineCommand::ListClients);
        }

        if changed {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    #[allow(clippy::too_many_lines)]
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = tr(self.language);

        egui::Panel::top("top_bar").show_inside(ui, |ui| {
            status_bar::top_bar(ui, &self.current_snapshot, self.engine_running, t);
            ui.horizontal(|ui| {
                // Start/Stop button
                if self.engine_running {
                    if ui.button(t.stop).clicked() {
                        self.send_command(EngineCommand::Stop);
                    }
                } else if ui.button(t.start).clicked() {
                    self.send_command(EngineCommand::Start);
                }
                ui.separator();
                // Config panel toggle
                if ui
                    .button(if self.show_config_panel {
                        t.hide_config
                    } else {
                        t.config
                    })
                    .clicked()
                {
                    self.show_config_panel = !self.show_config_panel;
                    if self.show_config_panel {
                        self.send_command(EngineCommand::ListClients);
                    }
                }
                ui.separator();
                // Add torrent button
                if ui.button(t.add_torrent).clicked()
                    && let Some(paths) = rfd::FileDialog::new()
                        .add_filter("Torrent files", &["torrent"])
                        .pick_files()
                {
                    for path in paths {
                        self.send_command(EngineCommand::AddTorrent(path));
                    }
                }
                ui.separator();
                // Language toggle
                if ui.button(self.language.toggle().label()).clicked() {
                    self.language = self.language.toggle();
                }
            });
        });

        egui::Panel::bottom("bottom_bar").show_inside(ui, |ui| {
            status_bar::bottom_bar(ui, self.started_at, self.engine_running, t);
        });

        // Config side panel
        if self.show_config_panel {
            egui::Panel::right("config_panel")
                .default_size(280.0)
                .show_inside(ui, |ui| {
                    config_panel::show(
                        ui,
                        &mut self.config_edit,
                        &self.available_clients,
                        &self.cmd_tx,
                        t,
                    );
                });
        }

        // Delete confirmation dialog
        let mut close_dialog = false;
        let mut do_delete: Option<InfoHash> = None;
        if let Some(confirm) = &self.pending_delete {
            let name = confirm.name.clone();
            let hash = confirm.info_hash.clone();
            egui::Window::new(t.confirm_delete)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(format!("{} \"{name}\"?", t.delete_prompt));
                    ui.label(t.delete_hint);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t.delete).clicked() {
                            do_delete = Some(hash.clone());
                            close_dialog = true;
                        }
                        if ui.button(t.cancel).clicked() {
                            close_dialog = true;
                        }
                    });
                });
        }
        if let Some(hash) = do_delete {
            self.send_command(EngineCommand::DeleteTorrent(hash));
        }
        if close_dialog {
            self.pending_delete = None;
        }

        // Central content
        egui::CentralPanel::default_margins().show_inside(ui, |ui| {
            let available = ui.available_height();
            let table_height = (available * 0.70).max(200.0);
            let chart_height = (available * 0.20).max(100.0);
            let log_height = (available * 0.10).max(80.0);

            ui.allocate_ui(egui::vec2(ui.available_width(), table_height), |ui| {
                torrent_table::show(ui, &self.current_snapshot, &mut self.pending_delete, t);
            });

            ui.separator();

            ui.allocate_ui(egui::vec2(ui.available_width(), chart_height), |ui| {
                speed_chart::show(ui, &self.speed_history, t);
            });

            ui.separator();

            ui.allocate_ui(egui::vec2(ui.available_width(), log_height), |ui| {
                log_panel::show(
                    ui,
                    &self.log_buffer,
                    &mut self.log_auto_scroll,
                    self.started_at,
                    t,
                );
            });
        });
    }
}

/// Load `config.json` synchronously for UI initialization. Returns `None` on
/// any I/O or parse error — the config panel will fall back to defaults.
fn load_config_sync(folders: &JoalFolders) -> Option<AppConfiguration> {
    let path = folders.config_file();
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn format_event(event: &EngineEvent) -> String {
    match event {
        EngineEvent::GlobalSeedStarted { client_name } => {
            format!("Seeding started - client: {client_name}")
        }
        EngineEvent::GlobalSeedStopped => "Seeding stopped".to_owned(),
        EngineEvent::TorrentFileAdded { name, .. } => {
            format!("Torrent added: {name}")
        }
        EngineEvent::TorrentFileDeleted { name, .. } => {
            format!("Torrent removed: {name}")
        }
        EngineEvent::FailedToAddTorrentFile { name, reason } => {
            format!("Failed to add torrent {name}: {reason}")
        }
        EngineEvent::TooManyAnnouncesFailedInARow { name, .. } => {
            format!("Too many failures: {name}")
        }
        EngineEvent::AnnounceStarted {
            name, tracker_url, ..
        } => {
            format!("Announcing: {name} -> {tracker_url}")
        }
        EngineEvent::AnnounceSucceeded {
            name,
            seeders,
            leechers,
            interval,
            ..
        } => {
            format!("Announce OK: {name} (S:{seeders} L:{leechers} I:{interval}s)")
        }
        EngineEvent::AnnounceFailed { name, error, .. } => {
            format!("Announce FAILED: {name} - {error}")
        }
        EngineEvent::ConfigLoaded { config } => {
            format!(
                "Config loaded - client: {}, speed: {}-{} kB/s",
                config.client, config.min_upload_rate, config.max_upload_rate,
            )
        }
    }
}
