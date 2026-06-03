mod config_panel;
mod config_state;
pub mod i18n;
mod log_panel;
mod speed_chart;
mod status_bar;
mod theme;
mod torrent_table;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use joal_core::config::{AppConfiguration, JoalFolders};
use joal_core::events::EngineEvent;
use joal_core::snapshot::EngineSnapshot;
use joal_core::torrent::InfoHash;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::warn;

use crate::{EngineCommand, EngineResponse};
use config_state::{ConfigEditState, ConfigNotice, ConfigValidationIssue};
use i18n::{Language, tr};

const LOG_BUFFER_CAPACITY: usize = 500;
const SPEED_HISTORY_CAPACITY: usize = 300;

pub fn configure_visuals(ctx: &egui::Context) {
    theme::apply(ctx);
}

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
    config_validation_errors: Vec<ConfigValidationIssue>,
    config_operation_error: Option<String>,
    config_notice: Option<ConfigNotice>,
    config_apply_in_progress: bool,
    available_clients: Vec<String>,
    clients_requested: bool,
    table_state: torrent_table::TableState,

    // Delete confirmation
    pending_delete: Option<DeleteConfirmation>,

    // Language
    language: Language,
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
            config_validation_errors: Vec::new(),
            config_operation_error: None,
            config_notice: None,
            config_apply_in_progress: false,
            available_clients: Vec::new(),
            clients_requested: false,
            table_state: torrent_table::TableState::default(),
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
                            self.config_validation_errors.clear();
                            self.config_operation_error = None;
                            self.config_apply_in_progress = false;
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
                EngineResponse::ConfigApplied => {
                    self.config_validation_errors.clear();
                    self.config_operation_error = None;
                    self.config_notice = Some(ConfigNotice::SavedAndRestarted);
                    self.config_apply_in_progress = false;
                }
                EngineResponse::Error(msg) => {
                    self.log_buffer.push_back(LogEntry {
                        timestamp: Instant::now(),
                        message: format!("[error] {msg}"),
                    });
                    if self.log_buffer.len() > LOG_BUFFER_CAPACITY {
                        self.log_buffer.pop_front();
                    }
                    if self.config_apply_in_progress {
                        self.config_apply_in_progress = false;
                        self.config_notice = None;
                        self.config_operation_error = Some(msg);
                    }
                }
            }
        }
    }

    fn send_command(&self, cmd: EngineCommand) {
        if let Err(error) = self.cmd_tx.try_send(cmd) {
            warn!(%error, "failed to enqueue UI command");
        }
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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = tr(self.language);
        self.draw_top_panel(ui, t);
        self.draw_footer(ui, t);
        self.draw_telemetry_panel(ui, t);
        self.draw_config_window(ui, t);
        self.draw_delete_confirmation(ui, t);
        self.draw_central_table(ui, t);
    }
}

impl JoalApp {
    fn draw_top_panel(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        egui::Panel::top("top_panel")
            .frame(theme::strip_frame(theme::surface()))
            .show_inside(ui, |ui| {
                self.draw_top_status_row(ui, t);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                torrent_table::toolbar(ui, &self.current_snapshot, &mut self.table_state, t);
            });
    }

    fn draw_footer(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        egui::Panel::bottom("footer_status")
            .frame(theme::strip_frame(theme::surface()))
            .show_inside(ui, |ui| {
                status_bar::bottom_bar(
                    ui,
                    &self.current_snapshot,
                    self.started_at,
                    self.engine_running,
                    t,
                );
            });
    }

    fn draw_telemetry_panel(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        egui::Panel::bottom("telemetry_panel")
            .resizable(true)
            .default_size(150.0)
            .min_size(110.0)
            .max_size(320.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::app_background())
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show_inside(ui, |ui| {
                self.draw_log_panel(ui, t);
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    speed_chart::show(ui, &self.speed_history, t);
                });
            });
    }

    fn draw_log_panel(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        egui::Panel::right("log_panel")
            .resizable(true)
            .default_size(420.0)
            .min_size(260.0)
            .show_inside(ui, |ui| {
                log_panel::show(
                    ui,
                    &self.log_buffer,
                    &mut self.log_auto_scroll,
                    self.started_at,
                    t,
                );
            });
    }

    fn draw_config_window(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        let mut show_config_window = self.show_config_panel;
        if show_config_window {
            let response = self.show_config_window(ui, &mut show_config_window, t);
            self.handle_config_window_response(response);
        }
        self.show_config_panel = show_config_window;
    }

    fn show_config_window(
        &mut self,
        ui: &mut egui::Ui,
        show_config_window: &mut bool,
        t: &i18n::Tr,
    ) -> Option<config_panel::ConfigPanelAction> {
        egui::Window::new(t.configuration)
            .id(egui::Id::new("config_window"))
            .open(show_config_window)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(420.0, 560.0))
            .min_width(360.0)
            .anchor(egui::Align2::RIGHT_TOP, [-16.0, 64.0])
            .frame(theme::window_frame())
            .show(ui.ctx(), |ui| {
                config_panel::show(
                    ui,
                    &mut self.config_edit,
                    config_panel::ConfigPanelView {
                        validation_errors: &self.config_validation_errors,
                        operation_error: self.config_operation_error.as_deref(),
                        notice: self.config_notice,
                        apply_in_progress: self.config_apply_in_progress,
                        available_clients: &self.available_clients,
                        t,
                    },
                )
            })
            .and_then(|r| r.inner)
    }

    fn handle_config_window_response(&mut self, response: Option<config_panel::ConfigPanelAction>) {
        let Some(inner) = response else { return };
        if inner.edited {
            self.config_validation_errors.clear();
            self.config_operation_error = None;
            self.config_notice = None;
        }
        if inner.apply_requested {
            self.apply_edited_config();
        }
    }

    fn apply_edited_config(&mut self) {
        match self.config_edit.validated_config(&self.available_clients) {
            Ok(config) => {
                self.config_validation_errors.clear();
                self.config_operation_error = None;
                self.config_notice = None;
                self.config_apply_in_progress = true;
                self.send_command(EngineCommand::ApplyConfig(config));
            }
            Err(errors) => {
                self.config_validation_errors = errors;
                self.config_operation_error = None;
                self.config_notice = None;
                self.config_apply_in_progress = false;
            }
        }
    }

    fn draw_delete_confirmation(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        let mut close_dialog = false;
        let mut do_delete: Option<InfoHash> = None;
        if let Some(confirm) = &self.pending_delete {
            show_delete_window(ui, t, confirm, &mut close_dialog, &mut do_delete);
        }
        if let Some(hash) = do_delete {
            self.send_command(EngineCommand::DeleteTorrent(hash));
        }
        if close_dialog {
            self.pending_delete = None;
        }
    }

    fn draw_central_table(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::app_background())
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show_inside(ui, |ui| {
                theme::panel_frame().show(ui, |ui| {
                    torrent_table::show(
                        ui,
                        &mut self.current_snapshot,
                        &mut self.pending_delete,
                        &self.cmd_tx,
                        &mut self.table_state,
                        t,
                    );
                });
            });
    }
}

impl JoalApp {
    /// First row of the top panel: status badges/metrics on the left, action
    /// buttons + language toggle pushed to the right edge. Keeping these in a
    /// single row is what frees up the vertical space the central table
    /// claims.
    fn draw_top_status_row(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        ui.horizontal(|ui| {
            status_bar::top_bar_status(ui, &self.current_snapshot, self.engine_running, t);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.draw_language_toggle(ui);
                self.draw_engine_toggle(ui, t);
                self.draw_config_toggle(ui, t);
                self.draw_announce_button(ui, t);
                self.draw_add_torrent_button(ui, t);
            });
        });
    }

    fn draw_language_toggle(&mut self, ui: &mut egui::Ui) {
        if theme::secondary_button(
            ui,
            "language_toggle_button",
            self.language.toggle().label(),
            egui::vec2(64.0, 30.0),
        )
        .clicked()
        {
            self.language = self.language.toggle();
        }
    }

    fn draw_engine_toggle(&self, ui: &mut egui::Ui, t: &i18n::Tr) {
        let label = if self.engine_running { t.stop } else { t.start };
        let tone = if self.engine_running {
            theme::Tone::Danger
        } else {
            theme::Tone::Success
        };
        if theme::tone_button(
            ui,
            "engine_toggle_button",
            label,
            tone,
            egui::vec2(86.0, 30.0),
            true,
        )
        .clicked()
        {
            self.send_command(if self.engine_running {
                EngineCommand::Stop
            } else {
                EngineCommand::Start
            });
        }
    }

    fn draw_config_toggle(&mut self, ui: &mut egui::Ui, t: &i18n::Tr) {
        let label = if self.show_config_panel {
            t.hide_config
        } else {
            t.config
        };
        if theme::tone_button(
            ui,
            "config_panel_toggle_button",
            label,
            theme::Tone::Accent,
            egui::vec2(104.0, 30.0),
            self.show_config_panel,
        )
        .clicked()
        {
            self.show_config_panel = !self.show_config_panel;
            if self.show_config_panel {
                self.send_command(EngineCommand::ListClients);
            }
        }
    }

    fn draw_announce_button(&self, ui: &mut egui::Ui, t: &i18n::Tr) {
        if theme::secondary_button_enabled(
            ui,
            "announce_all_button",
            t.announce_all_now,
            egui::vec2(140.0, 30.0),
            self.engine_running,
        )
        .clicked()
        {
            self.send_command(EngineCommand::AnnounceAllNow);
        }
    }

    fn draw_add_torrent_button(&self, ui: &mut egui::Ui, t: &i18n::Tr) {
        if !theme::primary_button(
            ui,
            "add_torrent_button",
            t.add_torrent,
            egui::vec2(118.0, 30.0),
        )
        .clicked()
        {
            return;
        }
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("Torrent files", &["torrent"])
            .pick_files()
        {
            for path in paths {
                self.send_command(EngineCommand::AddTorrent(path));
            }
        }
    }
}

fn show_delete_window(
    ui: &mut egui::Ui,
    t: &i18n::Tr,
    confirm: &DeleteConfirmation,
    close_dialog: &mut bool,
    do_delete: &mut Option<InfoHash>,
) {
    let name = confirm.name.clone();
    let hash = confirm.info_hash.clone();
    egui::Window::new(t.confirm_delete)
        .id(egui::Id::new("delete_confirmation_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(theme::window_frame())
        .show(ui.ctx(), |ui| {
            ui.label(format!("{} \"{name}\"?", t.delete_prompt));
            ui.label(t.delete_hint);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::tone_button(
                    ui,
                    "confirm_delete_button",
                    t.delete,
                    theme::Tone::Danger,
                    egui::vec2(96.0, 30.0),
                    true,
                )
                .clicked()
                {
                    *do_delete = Some(hash.clone());
                    *close_dialog = true;
                }
                if theme::secondary_button(
                    ui,
                    "cancel_delete_button",
                    t.cancel,
                    egui::vec2(80.0, 30.0),
                )
                .clicked()
                {
                    *close_dialog = true;
                }
            });
        });
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
                "Config loaded - client: {}, upload: {}-{} kB/s, download: {}-{} kB/s",
                config.client,
                config.min_upload_rate,
                config.max_upload_rate,
                config.min_download_rate,
                config.max_download_rate,
            )
        }
    }
}
