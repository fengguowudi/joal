mod config_panel;
mod config_state;
mod format;
mod health;
pub mod i18n;
mod log_panel;
mod speed_chart;
mod status_bar;
mod telemetry;
mod theme;
mod torrent_table;

use std::time::Duration;

use joal_core::config::{AppConfiguration, JoalFolders};
use joal_core::events::EngineEvent;
use joal_core::torrent::InfoHash;
use tracing::warn;

use crate::engine::{EngineCommand, EngineErrorKind, EngineLink, EnginePhase, UiMessage};
use config_state::ConfigPanelState;
use i18n::{Language, Tr, tr};
use telemetry::Telemetry;

pub fn configure_visuals(ctx: &egui::Context) {
    theme::apply(ctx);
}

struct ArchiveConfirmation {
    info_hash: InfoHash,
    name: String,
    just_opened: bool,
}

struct Banner {
    kind: EngineErrorKind,
    detail: String,
}

pub struct JoalApp {
    engine: EngineLink,
    telemetry: Telemetry,
    table: torrent_table::TableState,
    config: ConfigPanelState,
    pending_archive: Option<ArchiveConfirmation>,
    banner: Option<Banner>,
    language: Language,
}

impl JoalApp {
    pub fn new(
        engine: EngineLink,
        folders: &JoalFolders,
        storage: Option<&dyn eframe::Storage>,
    ) -> Self {
        let snapshot = engine.snapshot_rx.borrow().clone();
        let initial_config = load_config_sync(folders);
        let mut table = torrent_table::TableState::default();
        table.on_snapshot(&snapshot);
        let language = storage
            .and_then(|storage| storage.get_string("language"))
            .and_then(|code| Language::from_code(&code))
            .unwrap_or_else(system_language);
        let config = ConfigPanelState::new(&snapshot, initial_config.as_ref());
        Self {
            telemetry: Telemetry::new(snapshot, initial_config.as_ref()),
            config,
            engine,
            table,
            pending_archive: None,
            banner: None,
            language,
        }
    }

    fn on_message(&mut self, message: UiMessage) {
        match message {
            UiMessage::Phase(phase) => {
                self.telemetry.on_phase(phase);
                if phase == EnginePhase::Running
                    && self
                        .banner
                        .as_ref()
                        .is_some_and(|banner| banner.kind == EngineErrorKind::Start)
                {
                    self.banner = None;
                }
            }
            UiMessage::Event(event) => {
                if let EngineEvent::ConfigLoaded { config } = &event {
                    self.config.on_config(&self.telemetry.snapshot, config);
                }
                self.telemetry.on_event(event);
            }
            UiMessage::EventsLagged(count) => self.telemetry.on_lagged(count),
            UiMessage::Clients(clients) => self.config.clients = clients,
            UiMessage::ConfigApplied { restarted } => self.config.on_applied(restarted),
            UiMessage::Error { kind, detail } => {
                self.telemetry.on_error(kind, detail.clone());
                if matches!(
                    kind,
                    EngineErrorKind::SaveConfig | EngineErrorKind::RestartAfterConfig
                ) {
                    self.config.open = true;
                    self.config.on_error(detail);
                } else {
                    self.banner = Some(Banner { kind, detail });
                }
            }
        }
    }

    fn send_command(&self, command: EngineCommand) {
        if let Err(error) = self.engine.cmd_tx.try_send(command) {
            warn!(%error, "failed to enqueue UI command");
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let files = ctx.input(|input| input.raw.dropped_files.clone());
        let mut ignored = 0_usize;
        for file in files {
            let is_torrent = file.path.as_ref().is_some_and(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("torrent"))
            });
            if is_torrent {
                if let Some(path) = file.path {
                    self.send_command(EngineCommand::AddTorrent(path));
                }
            } else {
                ignored += 1;
            }
        }
        if ignored > 0 {
            self.telemetry.on_dropped_files(ignored);
        }
    }

    fn handle_table_action(&mut self, action: torrent_table::TableAction) {
        match action {
            torrent_table::TableAction::Sort(column) => {
                self.table.toggle_sort(column, &self.telemetry.snapshot);
            }
            torrent_table::TableAction::ToggleCompleted {
                info_hash,
                completed,
            } => self.send_command(EngineCommand::SetTorrentInitialCompleted {
                info_hash,
                completed,
            }),
            torrent_table::TableAction::RequestArchive { info_hash, name } => {
                self.pending_archive = Some(ArchiveConfirmation {
                    info_hash,
                    name,
                    just_opened: true,
                });
            }
            torrent_table::TableAction::AddTorrent => self.pick_and_add_torrents(),
            torrent_table::TableAction::ClearFilters => {
                self.table.clear_filters(&self.telemetry.snapshot);
            }
        }
    }

    fn pick_and_add_torrents(&self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("Torrent files", &["torrent"])
            .pick_files()
        {
            for path in paths {
                self.send_command(EngineCommand::AddTorrent(path));
            }
        }
    }

    fn draw_top_panel(&mut self, ui: &mut egui::Ui, t: &Tr) {
        egui::Panel::top("top_panel")
            .frame(theme::strip_frame(theme::surface()))
            .show_inside(ui, |ui| {
                self.draw_top_status_row(ui, t);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                torrent_table::toolbar(ui, &self.telemetry.snapshot, &mut self.table, t);
                self.draw_banner(ui, t);
            });
    }

    fn draw_top_status_row(&mut self, ui: &mut egui::Ui, t: &Tr) {
        ui.horizontal(|ui| {
            status_bar::top_bar_status(
                ui,
                &self.telemetry.snapshot,
                self.telemetry.phase,
                self.telemetry.download_enabled,
                t,
            );
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
            egui::vec2(56.0, 30.0),
        )
        .clicked()
        {
            self.language = self.language.toggle();
        }
    }

    fn draw_engine_toggle(&self, ui: &mut egui::Ui, t: &Tr) {
        let phase = self.telemetry.phase;
        let (label, tone, enabled) = match phase {
            EnginePhase::Starting => (t.starting, theme::Tone::Info, false),
            EnginePhase::Stopping => (t.stopping, theme::Tone::Info, false),
            EnginePhase::Running => (t.stop, theme::Tone::Danger, true),
            EnginePhase::Stopped => (t.start, theme::Tone::Success, true),
            EnginePhase::Failed => (t.retry, theme::Tone::Success, true),
        };
        if theme::tone_button_enabled(
            ui,
            "engine_toggle_button",
            label,
            tone,
            egui::vec2(96.0, 30.0),
            true,
            enabled,
        )
        .clicked()
        {
            self.send_command(if phase == EnginePhase::Running {
                EngineCommand::Stop
            } else {
                EngineCommand::Start
            });
        }
    }

    fn draw_config_toggle(&mut self, ui: &mut egui::Ui, t: &Tr) {
        let label = if self.config.open {
            t.hide_config
        } else {
            t.config
        };
        if theme::tone_button(
            ui,
            "config_panel_toggle_button",
            label,
            theme::Tone::Accent,
            egui::vec2(96.0, 30.0),
            self.config.open,
        )
        .clicked()
        {
            self.config.open = !self.config.open;
            if self.config.open {
                self.send_command(EngineCommand::ListClients);
            }
        }
    }

    fn draw_announce_button(&self, ui: &mut egui::Ui, t: &Tr) {
        if theme::secondary_button_enabled(
            ui,
            "announce_all_button",
            t.announce_all_now,
            egui::vec2(132.0, 30.0),
            self.telemetry.phase == EnginePhase::Running,
        )
        .clicked()
        {
            self.send_command(EngineCommand::AnnounceAllNow);
        }
    }

    fn draw_add_torrent_button(&mut self, ui: &mut egui::Ui, t: &Tr) {
        if theme::primary_button(
            ui,
            "add_torrent_button",
            t.add_torrent,
            egui::vec2(124.0, 30.0),
        )
        .clicked()
        {
            self.pick_and_add_torrents();
        }
    }

    fn draw_banner(&mut self, ui: &mut egui::Ui, t: &Tr) {
        let Some(banner) = &self.banner else {
            return;
        };
        let mut dismiss = false;
        let mut retry = false;
        theme::tone_frame(theme::Tone::Danger).show(ui, |ui| {
            ui.horizontal(|ui| {
                let label = telemetry::error_kind_text(banner.kind, t);
                let text = if banner.detail.is_empty() {
                    label.to_owned()
                } else {
                    format!("{label}: {}", banner.detail)
                };
                ui.add(egui::Label::new(egui::RichText::new(&text)).truncate())
                    .on_hover_ui(|ui| {
                        ui.label(&text);
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    dismiss = theme::quiet_secondary_button(
                        ui,
                        "dismiss_error_banner",
                        t.dismiss,
                        egui::vec2(56.0, 26.0),
                    )
                    .clicked();
                    if banner.kind == EngineErrorKind::Start {
                        retry = theme::secondary_button(
                            ui,
                            "retry_start",
                            t.retry,
                            egui::vec2(72.0, 26.0),
                        )
                        .clicked();
                    }
                });
            });
        });
        if dismiss {
            self.banner = None;
        }
        if retry {
            self.banner = None;
            self.send_command(EngineCommand::Start);
        }
    }

    fn draw_telemetry_panel(&mut self, ui: &mut egui::Ui, t: &Tr) {
        egui::Panel::bottom("telemetry_panel")
            .resizable(true)
            .default_size(150.0)
            .min_size(110.0)
            .max_size(320.0)
            .frame(theme::strip_frame(theme::surface()))
            .show_inside(ui, |ui| {
                egui::Panel::right("log_panel")
                    .resizable(true)
                    .default_size(420.0)
                    .min_size(260.0)
                    .show_inside(ui, |ui| {
                        log_panel::show(
                            ui,
                            &self.telemetry.logs,
                            &mut self.telemetry.log_auto_scroll,
                            &mut self.telemetry.problems_only,
                            t,
                        );
                    });
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    speed_chart::show(
                        ui,
                        &self.telemetry.speed_history,
                        self.telemetry.download_enabled,
                        t,
                    );
                });
            });
    }

    fn draw_footer(&mut self, ui: &mut egui::Ui, t: &Tr) {
        egui::Panel::bottom("footer_status")
            .frame(theme::strip_frame(theme::surface()))
            .show_inside(ui, |ui| {
                status_bar::bottom_bar(
                    ui,
                    &self.telemetry.snapshot,
                    self.telemetry.started_at,
                    self.telemetry.total_uploaded,
                    t,
                );
            });
    }

    fn draw_config_panel(&mut self, ui: &mut egui::Ui, t: &Tr) {
        if !self.config.open {
            return;
        }
        let action = egui::Panel::right("config_panel")
            .resizable(true)
            .default_size(380.0)
            .min_size(340.0)
            .max_size(480.0)
            .frame(theme::strip_frame(theme::surface()))
            .show_inside(ui, |ui| {
                ui.heading(t.configuration);
                egui::ScrollArea::vertical()
                    .show(ui, |ui| config_panel::show(ui, &mut self.config, t))
            })
            .inner
            .inner;
        if action.edited {
            self.config.on_edited();
        }
        if action.apply_requested
            && let Some(config) = self.config.begin_apply()
        {
            self.send_command(EngineCommand::ApplyConfig(config));
        }
    }

    fn draw_archive_confirmation(&mut self, ui: &mut egui::Ui, t: &Tr) {
        let Some(confirmation) = &self.pending_archive else {
            return;
        };
        let mut close = false;
        let mut archive = None;
        let modal = egui::Modal::new(egui::Id::new("archive_confirmation"))
            .frame(theme::window_frame())
            .show(ui.ctx(), |ui| {
                ui.set_max_width(420.0);
                ui.heading(t.archive_title);
                ui.add(egui::Label::new(&confirmation.name).truncate())
                    .on_hover_ui(|ui| {
                        ui.label(&confirmation.name);
                    });
                ui.label(egui::RichText::new(t.archive_hint).color(theme::text_secondary()));
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::tone_button(
                        ui,
                        "confirm_archive_button",
                        t.archive_confirm,
                        theme::Tone::Danger,
                        egui::vec2(96.0, 30.0),
                        true,
                    )
                    .clicked()
                    {
                        archive = Some(confirmation.info_hash.clone());
                        close = true;
                    }
                    let cancel = theme::secondary_button(
                        ui,
                        "cancel_archive_button",
                        t.cancel,
                        egui::vec2(80.0, 30.0),
                    );
                    if confirmation.just_opened {
                        cancel.request_focus();
                    }
                    if cancel.clicked() {
                        close = true;
                    }
                });
            });
        if modal.should_close() {
            close = true;
        }
        if let Some(info_hash) = archive {
            self.send_command(EngineCommand::ArchiveTorrent(info_hash));
        }
        if close {
            self.pending_archive = None;
        } else if let Some(confirmation) = &mut self.pending_archive {
            confirmation.just_opened = false;
        }
    }

    fn draw_central_table(&mut self, ui: &mut egui::Ui, t: &Tr) {
        let action = egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::app_background())
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show_inside(ui, |ui| {
                let available = ui.available_rect_before_wrap();
                let action = theme::panel_frame()
                    .show(ui, |ui| {
                        torrent_table::show(
                            ui,
                            &self.telemetry.snapshot,
                            &self.table,
                            torrent_table::TableView {
                                download_enabled: self.telemetry.download_enabled,
                                phase: self.telemetry.phase,
                            },
                            t,
                        )
                    })
                    .inner;
                if ui.ctx().input(|input| !input.raw.hovered_files.is_empty()) {
                    ui.painter().rect_filled(
                        available,
                        egui::CornerRadius::same(theme::CR_PANEL),
                        theme::primary_color().gamma_multiply(0.08),
                    );
                    ui.painter().text(
                        available.center(),
                        egui::Align2::CENTER_CENTER,
                        t.drop_overlay,
                        egui::TextStyle::Heading.resolve(ui.style()),
                        theme::primary_color(),
                    );
                }
                action
            })
            .inner;
        if let Some(action) = action {
            self.handle_table_action(action);
        }
    }
}

impl eframe::App for JoalApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(message) = self.engine.msg_rx.try_recv() {
            self.on_message(message);
        }
        if self.engine.snapshot_rx.has_changed().unwrap_or(false) {
            let snapshot = self.engine.snapshot_rx.borrow_and_update().clone();
            self.telemetry.on_snapshot(snapshot);
            self.table.on_snapshot(&self.telemetry.snapshot);
        }
        self.handle_dropped_files(ctx);
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = tr(self.language);
        self.draw_top_panel(ui, t);
        self.draw_telemetry_panel(ui, t);
        self.draw_footer(ui, t);
        self.draw_config_panel(ui, t);
        self.draw_central_table(ui, t);
        self.draw_archive_confirmation(ui, t);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("language", self.language.code().to_owned());
    }
}

fn load_config_sync(folders: &JoalFolders) -> Option<AppConfiguration> {
    let bytes = std::fs::read(folders.config_file()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn system_language() -> Language {
    if sys_locale::get_locale().is_some_and(|locale| locale.starts_with("zh")) {
        Language::Chinese
    } else {
        Language::English
    }
}
