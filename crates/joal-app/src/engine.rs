use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use joal_core::config::{self, AppConfiguration, JoalFolders};
use joal_core::events::EngineEvent;
use joal_core::seed_manager::SeedManager;
use joal_core::snapshot::EngineSnapshot;
use joal_core::torrent::{InfoHash, TorrentStateStore};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{error, info, warn};

#[derive(Debug)]
pub enum EngineCommand {
    Start,
    Stop,
    ApplyConfig(AppConfiguration),
    ArchiveTorrent(InfoHash),
    SetTorrentInitialCompleted {
        info_hash: InfoHash,
        completed: bool,
    },
    AddTorrent(PathBuf),
    AnnounceAllNow,
    ListClients,
    Shutdown(oneshot::Sender<()>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnginePhase {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineErrorKind {
    Start,
    SaveConfig,
    RestartAfterConfig,
    ListClients,
    AddTorrent,
    ArchiveTorrent,
    SaveTorrentState,
    NotRunning,
}

#[derive(Debug)]
pub enum UiMessage {
    Phase(EnginePhase),
    Event(EngineEvent),
    EventsLagged(u64),
    Clients(Vec<String>),
    ConfigApplied {
        restarted: bool,
    },
    Error {
        kind: EngineErrorKind,
        detail: String,
    },
}

pub struct EngineLink {
    pub cmd_tx: mpsc::Sender<EngineCommand>,
    pub msg_rx: mpsc::Receiver<UiMessage>,
    pub snapshot_rx: watch::Receiver<EngineSnapshot>,
}

#[derive(Clone, Default)]
pub struct Repaint(Arc<OnceLock<egui::Context>>);

impl Repaint {
    pub fn set(&self, ctx: egui::Context) {
        let _ = self.0.set(ctx);
    }

    fn wake(&self) {
        if let Some(ctx) = self.0.get() {
            ctx.request_repaint();
        }
    }
}

pub fn spawn(
    runtime: &tokio::runtime::Handle,
    joal_conf: PathBuf,
    repaint: Repaint,
) -> (EngineLink, mpsc::Sender<EngineCommand>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(32);
    let (msg_tx, msg_rx) = mpsc::channel(256);
    let (snapshot_tx, snapshot_rx) = watch::channel(EngineSnapshot::default());
    let folders = JoalFolders::new(&joal_conf);
    let supervisor = Supervisor {
        cmd_rx,
        msg_tx,
        snapshot_tx,
        folders,
        joal_conf,
        repaint,
        engine: None,
        engine_snapshot_rx: None,
        engine_events_rx: None,
        current_config: None,
    };
    runtime.spawn(supervisor.run());
    (
        EngineLink {
            cmd_tx: cmd_tx.clone(),
            msg_rx,
            snapshot_rx,
        },
        cmd_tx,
    )
}

struct Supervisor {
    cmd_rx: mpsc::Receiver<EngineCommand>,
    msg_tx: mpsc::Sender<UiMessage>,
    snapshot_tx: watch::Sender<EngineSnapshot>,
    folders: JoalFolders,
    joal_conf: PathBuf,
    repaint: Repaint,
    engine: Option<SeedManager>,
    engine_snapshot_rx: Option<watch::Receiver<EngineSnapshot>>,
    engine_events_rx: Option<broadcast::Receiver<EngineEvent>>,
    current_config: Option<AppConfiguration>,
}

impl Supervisor {
    async fn run(mut self) {
        self.list_clients().await;
        self.start_engine(EngineErrorKind::Start).await;
        self.repaint.wake();

        loop {
            tokio::select! {
                command = self.cmd_rx.recv() => {
                    match command {
                        Some(EngineCommand::Shutdown(done)) => {
                            self.stop_engine().await;
                            let _ = done.send(());
                            break;
                        }
                        Some(command) => self.handle(command).await,
                        None => {
                            self.stop_engine().await;
                            break;
                        }
                    }
                }
                changed = snapshot_changed(&mut self.engine_snapshot_rx), if self.engine.is_some() => {
                    if changed.is_ok()
                        && let Some(rx) = self.engine_snapshot_rx.as_mut()
                    {
                        let frame = rx.borrow_and_update().clone();
                        self.snapshot_tx.send_replace(frame);
                    }
                }
                event = next_event(&mut self.engine_events_rx), if self.engine.is_some() => {
                    match event {
                        Ok(event) => self.send(UiMessage::Event(event)).await,
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            self.send(UiMessage::EventsLagged(count)).await;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            self.engine_events_rx = None;
                        }
                    }
                }
            }
            self.repaint.wake();
        }
    }

    async fn send(&self, message: UiMessage) {
        if self.msg_tx.send(message).await.is_err() {
            warn!(target: "joal_app::engine", "UI message receiver dropped");
        }
    }

    async fn handle(&mut self, command: EngineCommand) {
        match command {
            EngineCommand::Start => self.start_engine(EngineErrorKind::Start).await,
            EngineCommand::Stop => self.stop_engine().await,
            EngineCommand::ApplyConfig(config) => self.apply_config(config).await,
            EngineCommand::ArchiveTorrent(info_hash) => self.archive_torrent(info_hash).await,
            EngineCommand::SetTorrentInitialCompleted {
                info_hash,
                completed,
            } => self.set_initial_completed(info_hash, completed).await,
            EngineCommand::AddTorrent(path) => self.add_torrent(path).await,
            EngineCommand::AnnounceAllNow => self.announce_all_now().await,
            EngineCommand::ListClients => self.list_clients().await,
            EngineCommand::Shutdown(done) => {
                self.stop_engine().await;
                let _ = done.send(());
            }
        }
    }

    async fn start_engine(&mut self, error_kind: EngineErrorKind) {
        if self.engine.is_some() {
            return;
        }
        self.send(UiMessage::Phase(EnginePhase::Starting)).await;
        match SeedManager::start(&self.joal_conf).await {
            Ok(engine) => {
                self.engine_snapshot_rx = Some(engine.snapshot_watch());
                self.engine_events_rx = Some(engine.subscribe_events());
                self.snapshot_tx.send_replace(engine.snapshot());
                self.engine = Some(engine);
                match config::load(&self.joal_conf).await {
                    Ok((config, _)) => {
                        self.current_config = Some(config.clone());
                        self.send(UiMessage::Event(EngineEvent::ConfigLoaded { config }))
                            .await;
                    }
                    Err(error) => {
                        warn!(target: "joal_app::engine", %error, "failed to reload config after start");
                    }
                }
                self.send(UiMessage::Phase(EnginePhase::Running)).await;
                info!(target: "joal_app::engine", "engine started");
            }
            Err(error) => {
                let detail = format!("{error:#}");
                error!(target: "joal_app::engine", error = %detail, "failed to start engine");
                self.send(UiMessage::Error {
                    kind: error_kind,
                    detail,
                })
                .await;
                self.send(UiMessage::Phase(EnginePhase::Failed)).await;
            }
        }
    }

    async fn stop_engine(&mut self) {
        let Some(mut engine) = self.engine.take() else {
            self.send(UiMessage::Phase(EnginePhase::Stopped)).await;
            return;
        };
        self.send(UiMessage::Phase(EnginePhase::Stopping)).await;
        engine.stop().await;
        self.engine_snapshot_rx = None;
        self.engine_events_rx = None;
        self.snapshot_tx
            .send_replace(stopped_snapshot(engine.snapshot()));
        self.send(UiMessage::Event(EngineEvent::GlobalSeedStopped))
            .await;
        self.send(UiMessage::Phase(EnginePhase::Stopped)).await;
        info!(target: "joal_app::engine", "engine stopped");
    }

    async fn apply_config(&mut self, config: AppConfiguration) {
        if let Err(error) = config::save(&self.folders, &config).await {
            self.send(UiMessage::Error {
                kind: EngineErrorKind::SaveConfig,
                detail: format!("{error:#}"),
            })
            .await;
            return;
        }

        let previous = self.current_config.replace(config.clone());
        let hot_apply = self.engine.is_some()
            && previous
                .as_ref()
                .is_some_and(|old| only_rate_limits_changed(old, &config));
        if hot_apply {
            if let Some(engine) = &self.engine {
                engine.apply_rate_limits(&config);
            }
            self.send(UiMessage::ConfigApplied { restarted: false })
                .await;
            return;
        }

        self.stop_engine().await;
        self.start_engine(EngineErrorKind::RestartAfterConfig).await;
        if self.engine.is_some() {
            self.send(UiMessage::ConfigApplied { restarted: true })
                .await;
        }
    }

    async fn list_clients(&self) {
        match config::list_client_files(&self.folders).await {
            Ok(clients) => self.send(UiMessage::Clients(clients)).await,
            Err(error) => {
                self.send(UiMessage::Error {
                    kind: EngineErrorKind::ListClients,
                    detail: format!("{error:#}"),
                })
                .await;
            }
        }
    }

    async fn archive_torrent(&self, info_hash: InfoHash) {
        let Some(engine) = &self.engine else {
            self.not_running().await;
            return;
        };
        if let Err(error) = engine.delete_torrent(&info_hash).await {
            self.send(UiMessage::Error {
                kind: EngineErrorKind::ArchiveTorrent,
                detail: format!("{error:#}"),
            })
            .await;
        }
    }

    async fn set_initial_completed(&self, info_hash: InfoHash, completed: bool) {
        let result = if let Some(engine) = &self.engine {
            engine
                .set_torrent_initial_completed(&info_hash, completed)
                .await
        } else {
            TorrentStateStore::load(&self.folders)
                .await
                .set_initial_completed(&info_hash, completed)
                .await
        };
        if let Err(error) = result {
            self.send(UiMessage::Error {
                kind: EngineErrorKind::SaveTorrentState,
                detail: format!("{error:#}"),
            })
            .await;
        }
    }

    async fn add_torrent(&self, source_path: PathBuf) {
        let Some(filename) = source_path.file_name() else {
            self.send(UiMessage::Error {
                kind: EngineErrorKind::AddTorrent,
                detail: "selected path has no file name".to_owned(),
            })
            .await;
            return;
        };
        let destination = self.folders.torrents_dir.join(filename);
        match tokio::fs::copy(&source_path, &destination).await {
            Ok(_) => {
                info!(target: "joal_app::engine", destination = %destination.display(), "torrent copied");
            }
            Err(error) => {
                self.send(UiMessage::Error {
                    kind: EngineErrorKind::AddTorrent,
                    detail: format!("{error:#}"),
                })
                .await;
            }
        }
    }

    async fn announce_all_now(&self) {
        if let Some(engine) = &self.engine {
            engine.announce_all_now();
        } else {
            self.not_running().await;
        }
    }

    async fn not_running(&self) {
        self.send(UiMessage::Error {
            kind: EngineErrorKind::NotRunning,
            detail: String::new(),
        })
        .await;
    }
}

async fn snapshot_changed(
    receiver: &mut Option<watch::Receiver<EngineSnapshot>>,
) -> Result<(), watch::error::RecvError> {
    match receiver {
        Some(receiver) => receiver.changed().await,
        None => std::future::pending().await,
    }
}

async fn next_event(
    receiver: &mut Option<broadcast::Receiver<EngineEvent>>,
) -> Result<EngineEvent, broadcast::error::RecvError> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

fn stopped_snapshot(mut snapshot: EngineSnapshot) -> EngineSnapshot {
    snapshot.global_upload_speed_bps = 0;
    snapshot.global_download_speed_bps = 0;
    snapshot.torrents = snapshot
        .torrents
        .iter()
        .cloned()
        .map(|mut torrent| {
            torrent.current_speed_bps = 0;
            torrent.current_download_speed_bps = 0;
            torrent
        })
        .collect::<Vec<_>>()
        .into();
    snapshot
}

fn only_rate_limits_changed(previous: &AppConfiguration, next: &AppConfiguration) -> bool {
    let mut normalized = previous.clone();
    normalized.min_upload_rate = next.min_upload_rate;
    normalized.max_upload_rate = next.max_upload_rate;
    normalized.min_download_rate = next.min_download_rate;
    normalized.max_download_rate = next.max_download_rate;
    normalized == *next
}

#[cfg(test)]
mod tests {
    use joal_core::torrent::InfoHash;

    use super::*;

    fn config() -> AppConfiguration {
        AppConfiguration {
            min_upload_rate: 10,
            max_upload_rate: 100,
            min_download_rate: 0,
            max_download_rate: 0,
            simultaneous_seed: 5,
            client: "qbittorrent.client".to_owned(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
            proxy_host: None,
            proxy_port: None,
        }
    }

    #[test]
    fn detects_rate_only_changes() {
        let old = config();
        let mut next = old.clone();
        next.max_upload_rate = 200;
        assert!(only_rate_limits_changed(&old, &next));
        next.client = "transmission.client".to_owned();
        assert!(!only_rate_limits_changed(&old, &next));
    }

    #[test]
    fn stopping_clears_every_speed() {
        let torrent = joal_core::snapshot::TorrentStatus {
            info_hash: InfoHash::from_bytes([1; 20]),
            name: "one".to_owned(),
            total_size: 100,
            uploaded_bytes: 20,
            downloaded_bytes: 10,
            left_bytes: 90,
            current_speed_bps: 50,
            current_download_speed_bps: 60,
            initial_completed: false,
            last_known_interval: None,
            last_known_seeders: None,
            last_known_leechers: None,
            consecutive_fails: 0,
            last_announced_at: None,
        };
        let snapshot = EngineSnapshot {
            global_upload_speed_bps: 50,
            global_download_speed_bps: 60,
            torrents: vec![torrent].into(),
            ..EngineSnapshot::default()
        };
        let stopped = stopped_snapshot(snapshot);
        assert_eq!(stopped.global_upload_speed_bps, 0);
        assert_eq!(stopped.global_download_speed_bps, 0);
        assert_eq!(stopped.torrents[0].current_speed_bps, 0);
        assert_eq!(stopped.torrents[0].current_download_speed_bps, 0);
        assert_eq!(stopped.torrents[0].uploaded_bytes, 20);
    }
}
