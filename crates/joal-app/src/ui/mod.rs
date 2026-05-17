#[cfg(test)]
mod benchmark;
mod config_panel;
pub mod i18n;
mod log_panel;
mod speed_chart;
mod status_bar;
mod theme;
mod torrent_table;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use joal_core::config::{AppConfiguration, ConfigError, JoalFolders, UPLOAD_RATIO_TARGET_DISABLED};
use joal_core::events::EngineEvent;
use joal_core::snapshot::EngineSnapshot;
use joal_core::torrent::InfoHash;
use tokio::sync::{broadcast, mpsc, watch};

use crate::{EngineCommand, EngineResponse};
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

/// Editable config fields mirroring AppConfiguration.
struct ConfigEditState {
    min_upload_rate: String,
    max_upload_rate: String,
    min_download_rate: String,
    max_download_rate: String,
    simultaneous_seed: String,
    upload_ratio_target: String,
    selected_client: String,
    keep_torrent_with_zero_leechers: bool,
    proxy_host: String,
    proxy_port: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigField {
    MinUploadRate,
    MaxUploadRate,
    MinDownloadRate,
    MaxDownloadRate,
    SimultaneousSeed,
    UploadRatioTarget,
}

impl ConfigField {
    fn label(self, t: &i18n::Tr) -> &str {
        match self {
            Self::MinUploadRate => t.min_upload_rate,
            Self::MaxUploadRate => t.max_upload_rate,
            Self::MinDownloadRate => t.min_download_rate,
            Self::MaxDownloadRate => t.max_download_rate,
            Self::SimultaneousSeed => t.simultaneous_seed,
            Self::UploadRatioTarget => t.upload_ratio_target,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigValidationIssue {
    InvalidNumber(ConfigField),
    InvalidPort,
    ClientRequired,
    ClientUnavailable,
    ProxyPairRequired,
    UploadRateRange,
    DownloadRateRange,
    SimultaneousSeedTooLow,
    UploadRatioTargetInvalid,
    Unexpected(String),
}

impl ConfigValidationIssue {
    fn message(&self, t: &i18n::Tr) -> String {
        match self {
            Self::InvalidNumber(field) => format!("{} {}", field.label(t), t.config_invalid_number),
            Self::InvalidPort => format!("{} {}", t.proxy_port, t.config_invalid_port),
            Self::ClientRequired => t.config_client_required.to_owned(),
            Self::ClientUnavailable => t.config_client_unavailable.to_owned(),
            Self::ProxyPairRequired => t.config_proxy_pair_required.to_owned(),
            Self::UploadRateRange => t.config_upload_rate_range.to_owned(),
            Self::DownloadRateRange => t.config_download_rate_range.to_owned(),
            Self::SimultaneousSeedTooLow => t.config_simultaneous_seed_positive.to_owned(),
            Self::UploadRatioTargetInvalid => t.config_upload_ratio_invalid.to_owned(),
            Self::Unexpected(message) => message.clone(),
        }
    }

    fn from_config_error(error: ConfigError) -> Self {
        match error {
            ConfigError::Invalid(
                "maxUploadRate must be greater than or equal to minUploadRate",
            ) => Self::UploadRateRange,
            ConfigError::Invalid(
                "maxDownloadRate must be greater than or equal to minDownloadRate",
            ) => Self::DownloadRateRange,
            ConfigError::Invalid("simultaneousSeed must be greater than 0") => {
                Self::SimultaneousSeedTooLow
            }
            ConfigError::Invalid("client is required, no file name given") => Self::ClientRequired,
            ConfigError::Invalid("uploadRatioTarget must be greater than 0 (or equal to -1)") => {
                Self::UploadRatioTargetInvalid
            }
            other => Self::Unexpected(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigNotice {
    SavedAndRestarted,
}

impl ConfigNotice {
    fn message(self, t: &i18n::Tr) -> &str {
        match self {
            Self::SavedAndRestarted => t.config_saved_restarted,
        }
    }
}

struct ParsedNumericConfig {
    min_upload_rate: u64,
    max_upload_rate: u64,
    min_download_rate: u64,
    max_download_rate: u64,
    simultaneous_seed: u32,
    upload_ratio_target: f32,
}

struct ParsedProxyConfig {
    host: Option<String>,
    port: Option<u16>,
}

impl ConfigEditState {
    fn from_snapshot(snapshot: &EngineSnapshot, config: Option<&AppConfiguration>) -> Self {
        if let Some(cfg) = config {
            Self {
                min_upload_rate: cfg.min_upload_rate.to_string(),
                max_upload_rate: cfg.max_upload_rate.to_string(),
                min_download_rate: cfg.min_download_rate.to_string(),
                max_download_rate: cfg.max_download_rate.to_string(),
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
                min_download_rate: "0".to_owned(),
                max_download_rate: "0".to_owned(),
                simultaneous_seed: "5".to_owned(),
                upload_ratio_target: "-1.0".to_owned(),
                selected_client: snapshot.active_client_filename.clone(),
                keep_torrent_with_zero_leechers: true,
                proxy_host: String::new(),
                proxy_port: String::new(),
            }
        }
    }

    fn validated_config(
        &self,
        available_clients: &[String],
    ) -> Result<AppConfiguration, Vec<ConfigValidationIssue>> {
        let mut errors = Vec::new();

        let selected_client = self.validate_client_selection(available_clients, &mut errors);
        let proxy = self.validate_proxy_settings(&mut errors);
        let Some(numbers) = self.parse_numeric_config(&mut errors) else {
            return Err(errors);
        };

        validate_numeric_ranges(&numbers, &mut errors);

        let config = AppConfiguration {
            min_upload_rate: numbers.min_upload_rate,
            max_upload_rate: numbers.max_upload_rate,
            min_download_rate: numbers.min_download_rate,
            max_download_rate: numbers.max_download_rate,
            simultaneous_seed: numbers.simultaneous_seed,
            client: selected_client,
            keep_torrent_with_zero_leechers: self.keep_torrent_with_zero_leechers,
            upload_ratio_target: numbers.upload_ratio_target,
            proxy_host: proxy.host,
            proxy_port: proxy.port,
        };

        if let Err(error) = config.validate() {
            push_config_error(&mut errors, ConfigValidationIssue::from_config_error(error));
        }

        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors)
        }
    }

    fn validate_client_selection(
        &self,
        available_clients: &[String],
        errors: &mut Vec<ConfigValidationIssue>,
    ) -> String {
        let selected_client = self.selected_client.trim().to_owned();
        if selected_client.is_empty() {
            errors.push(ConfigValidationIssue::ClientRequired);
        } else if !available_clients
            .iter()
            .any(|client| client == &selected_client)
        {
            errors.push(ConfigValidationIssue::ClientUnavailable);
        }
        selected_client
    }

    fn validate_proxy_settings(
        &self,
        errors: &mut Vec<ConfigValidationIssue>,
    ) -> ParsedProxyConfig {
        let proxy_host = self.proxy_host.trim().to_owned();
        let proxy_port_text = self.proxy_port.trim();
        let has_proxy_host = !proxy_host.is_empty();
        let has_proxy_port = !proxy_port_text.is_empty();
        if has_proxy_host != has_proxy_port {
            errors.push(ConfigValidationIssue::ProxyPairRequired);
        }

        let port = if has_proxy_port {
            match proxy_port_text.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => {
                    errors.push(ConfigValidationIssue::InvalidPort);
                    None
                }
            }
        } else {
            None
        };

        ParsedProxyConfig {
            host: has_proxy_host.then_some(proxy_host),
            port,
        }
    }

    fn parse_numeric_config(
        &self,
        errors: &mut Vec<ConfigValidationIssue>,
    ) -> Option<ParsedNumericConfig> {
        let min_upload_rate =
            parse_config_value::<u64>(&self.min_upload_rate, ConfigField::MinUploadRate, errors);
        let max_upload_rate =
            parse_config_value::<u64>(&self.max_upload_rate, ConfigField::MaxUploadRate, errors);
        let min_download_rate = parse_config_value::<u64>(
            &self.min_download_rate,
            ConfigField::MinDownloadRate,
            errors,
        );
        let max_download_rate = parse_config_value::<u64>(
            &self.max_download_rate,
            ConfigField::MaxDownloadRate,
            errors,
        );
        let simultaneous_seed = parse_config_value::<u32>(
            &self.simultaneous_seed,
            ConfigField::SimultaneousSeed,
            errors,
        );
        let upload_ratio_target = parse_config_value::<f32>(
            &self.upload_ratio_target,
            ConfigField::UploadRatioTarget,
            errors,
        );

        let (
            Some(min_upload_rate),
            Some(max_upload_rate),
            Some(min_download_rate),
            Some(max_download_rate),
            Some(simultaneous_seed),
            Some(upload_ratio_target),
        ) = (
            min_upload_rate,
            max_upload_rate,
            min_download_rate,
            max_download_rate,
            simultaneous_seed,
            upload_ratio_target,
        )
        else {
            return None;
        };

        Some(ParsedNumericConfig {
            min_upload_rate,
            max_upload_rate,
            min_download_rate,
            max_download_rate,
            simultaneous_seed,
            upload_ratio_target,
        })
    }
}

fn validate_numeric_ranges(numbers: &ParsedNumericConfig, errors: &mut Vec<ConfigValidationIssue>) {
    if numbers.max_upload_rate < numbers.min_upload_rate {
        push_config_error(errors, ConfigValidationIssue::UploadRateRange);
    }
    if numbers.max_download_rate < numbers.min_download_rate {
        push_config_error(errors, ConfigValidationIssue::DownloadRateRange);
    }
    if numbers.simultaneous_seed < 1 {
        push_config_error(errors, ConfigValidationIssue::SimultaneousSeedTooLow);
    }
    if numbers.upload_ratio_target < 0.0
        && numbers.upload_ratio_target != UPLOAD_RATIO_TARGET_DISABLED
    {
        push_config_error(errors, ConfigValidationIssue::UploadRatioTargetInvalid);
    }
}

fn parse_config_value<T>(
    value: &str,
    field: ConfigField,
    errors: &mut Vec<ConfigValidationIssue>,
) -> Option<T>
where
    T: std::str::FromStr,
{
    if let Ok(parsed) = value.trim().parse::<T>() {
        Some(parsed)
    } else {
        errors.push(ConfigValidationIssue::InvalidNumber(field));
        None
    }
}

fn push_config_error(errors: &mut Vec<ConfigValidationIssue>, issue: ConfigValidationIssue) {
    if !errors.contains(&issue) {
        errors.push(issue);
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

        // === TOP PANEL: status row + thin table toolbar ===
        // The top panel is sized by content so the central table can keep
        // ~70 % of the vertical space. Two compact rows live here:
        //   1) status badges/metrics on the left, action buttons pushed to the
        //      right edge with a right-to-left layout
        //   2) the table toolbar (search + filter + visible count)
        //
        // Uses `theme::strip_frame(...)` (square corners, edge-to-edge fill)
        // because this is a horizontal divider strip, not a card. Rounded
        // corners on a panel that spans the full window width would render as
        // chiclets pinned to the top edge — interactive controls (buttons,
        // the config window) keep their own rounding via their helpers.
        egui::Panel::top("top_panel")
            .frame(theme::strip_frame(theme::surface()))
            .show_inside(ui, |ui| {
                self.draw_top_status_row(ui, t);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                torrent_table::toolbar(ui, &self.current_snapshot, &mut self.table_state, t);
            });

        // === BOTTOM FOOTER: thin uptime + telemetry strip (~28 px) ===
        // Sits below the telemetry panel so it always hugs the window edge.
        // Also square-cornered for the same reason as the top strip.
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

        // === BOTTOM TELEMETRY: speed chart + log (compact, resizable) ===
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

                egui::CentralPanel::default().show_inside(ui, |ui| {
                    speed_chart::show(ui, &self.speed_history, t);
                });
            });

        // === CONFIG WINDOW (floating, draggable) ===
        // The config editor lives in a floating egui Window so opening or
        // closing it does NOT reflow the central torrent table area. Prior
        // versions used an inline right-side panel, which forced the central
        // layout to recompute between passes and perturbed the auto-generated
        // ids of the per-row action cluster (badges + mark/archive buttons).
        // That manifested as
        //   `WARN egui::context: ... changed id between passes`
        // every time the user toggled the panel. With a floating Window the
        // central pane keeps the same rect tree across both pass-1 and pass-2,
        // which combined with `torrent_table.rs`'s positional `row.index()`
        // anchoring (see the egui id-stability rule in
        // `.trellis/spec/backend/quality-guidelines.md`) is what keeps the row
        // widget ids stable.
        let mut show_config_window = self.show_config_panel;
        if show_config_window {
            let window_response = egui::Window::new(t.configuration)
                .id(egui::Id::new("config_window"))
                .open(&mut show_config_window)
                .collapsible(false)
                .resizable(true)
                .default_size(egui::vec2(420.0, 560.0))
                .min_width(360.0)
                .anchor(egui::Align2::RIGHT_TOP, [-16.0, 64.0])
                // Pin the frame explicitly so the floating config card uses our
                // theme's surface + soft border + drop shadow combo rather than
                // egui's theme-derived default. Without this, an earlier round
                // shipped a state where the title-bar / body rendered with
                // washed-out, near-invisible text on certain platforms. The
                // body content still picks up `override_text_color` from the
                // global theme, so labels and the title bar always paint with
                // `text_primary` (dark ink) on top of the surface fill.
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
                });
            if let Some(inner) = window_response.and_then(|r| r.inner) {
                if inner.edited {
                    self.config_validation_errors.clear();
                    self.config_operation_error = None;
                    self.config_notice = None;
                }
                if inner.apply_requested {
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
            }
        }
        // Reflect the window's own close affordance (the X in its title bar
        // or Esc) back into our toggle state so the gear button stays in
        // sync.
        self.show_config_panel = show_config_window;

        // === DELETE CONFIRMATION DIALOG ===
        let mut close_dialog = false;
        let mut do_delete: Option<InfoHash> = None;
        if let Some(confirm) = &self.pending_delete {
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
                            do_delete = Some(hash.clone());
                            close_dialog = true;
                        }
                        if theme::secondary_button(
                            ui,
                            "cancel_delete_button",
                            t.cancel,
                            egui::vec2(80.0, 30.0),
                        )
                        .clicked()
                        {
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

        // === CENTRAL PANEL: the torrent table — the soul of the workspace ===
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

            // Right-anchored action cluster.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Language toggle sits at the far right (low-priority chrome).
                let language_toggle_clicked = theme::secondary_button(
                    ui,
                    "language_toggle_button",
                    self.language.toggle().label(),
                    egui::vec2(64.0, 30.0),
                )
                .clicked();
                if language_toggle_clicked {
                    self.language = self.language.toggle();
                }

                let engine_label = if self.engine_running { t.stop } else { t.start };
                let engine_tone = if self.engine_running {
                    theme::Tone::Danger
                } else {
                    theme::Tone::Success
                };
                let engine_toggle_clicked = theme::tone_button(
                    ui,
                    "engine_toggle_button",
                    engine_label,
                    engine_tone,
                    egui::vec2(86.0, 30.0),
                    true,
                )
                .clicked();
                if engine_toggle_clicked {
                    if self.engine_running {
                        self.send_command(EngineCommand::Stop);
                    } else {
                        self.send_command(EngineCommand::Start);
                    }
                }

                let config_label = if self.show_config_panel {
                    t.hide_config
                } else {
                    t.config
                };
                let config_toggle_clicked = theme::tone_button(
                    ui,
                    "config_panel_toggle_button",
                    config_label,
                    theme::Tone::Accent,
                    egui::vec2(104.0, 30.0),
                    self.show_config_panel,
                )
                .clicked();
                if config_toggle_clicked {
                    self.show_config_panel = !self.show_config_panel;
                    if self.show_config_panel {
                        self.send_command(EngineCommand::ListClients);
                    }
                }

                let announce_clicked = theme::secondary_button_enabled(
                    ui,
                    "announce_all_button",
                    t.announce_all_now,
                    egui::vec2(140.0, 30.0),
                    self.engine_running,
                )
                .clicked();
                if announce_clicked {
                    self.send_command(EngineCommand::AnnounceAllNow);
                }

                // Primary button — the one we most want the user to click.
                let add_clicked = theme::primary_button(
                    ui,
                    "add_torrent_button",
                    t.add_torrent,
                    egui::vec2(118.0, 30.0),
                )
                .clicked();
                if add_clicked
                    && let Some(paths) = rfd::FileDialog::new()
                        .add_filter("Torrent files", &["torrent"])
                        .pick_files()
                {
                    for path in paths {
                        self.send_command(EngineCommand::AddTorrent(path));
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base_state() -> ConfigEditState {
        ConfigEditState {
            min_upload_rate: "30".to_owned(),
            max_upload_rate: "170".to_owned(),
            min_download_rate: "0".to_owned(),
            max_download_rate: "0".to_owned(),
            simultaneous_seed: "5".to_owned(),
            upload_ratio_target: "-1.0".to_owned(),
            selected_client: "utorrent-3.5.0_43916.client".to_owned(),
            keep_torrent_with_zero_leechers: true,
            proxy_host: String::new(),
            proxy_port: String::new(),
        }
    }

    #[test]
    fn validated_config_reports_parse_and_proxy_errors() {
        let mut state = base_state();
        state.simultaneous_seed = "abc".to_owned();
        state.proxy_host = "127.0.0.1".to_owned();

        let errors = state
            .validated_config(&["utorrent-3.5.0_43916.client".to_owned()])
            .unwrap_err();

        assert!(errors.contains(&ConfigValidationIssue::InvalidNumber(
            ConfigField::SimultaneousSeed,
        )));
        assert!(errors.contains(&ConfigValidationIssue::ProxyPairRequired));
    }

    #[test]
    fn validated_config_rejects_unavailable_client() {
        let state = base_state();
        let errors = state
            .validated_config(&["qbittorrent-4.5.0.client".to_owned()])
            .unwrap_err();

        assert!(errors.contains(&ConfigValidationIssue::ClientUnavailable));
    }

    #[test]
    fn validated_config_collects_range_and_ratio_errors() {
        let mut state = base_state();
        state.min_upload_rate = "200".to_owned();
        state.max_upload_rate = "150".to_owned();
        state.min_download_rate = "10".to_owned();
        state.max_download_rate = "5".to_owned();
        state.upload_ratio_target = "-0.5".to_owned();

        let errors = state
            .validated_config(&["utorrent-3.5.0_43916.client".to_owned()])
            .unwrap_err();

        assert!(errors.contains(&ConfigValidationIssue::UploadRateRange));
        assert!(errors.contains(&ConfigValidationIssue::DownloadRateRange));
        assert!(errors.contains(&ConfigValidationIssue::UploadRatioTargetInvalid));
    }

    #[test]
    fn validated_config_builds_trimmed_proxy_config() {
        let mut state = base_state();
        state.proxy_host = " 127.0.0.1 ".to_owned();
        state.proxy_port = " 8080 ".to_owned();

        let config = state
            .validated_config(&["utorrent-3.5.0_43916.client".to_owned()])
            .unwrap();

        assert_eq!(config.proxy_host.as_deref(), Some("127.0.0.1"));
        assert_eq!(config.proxy_port, Some(8080));
        assert_eq!(config.client, "utorrent-3.5.0_43916.client");
    }
}
