use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use joal_core::config::{self, AppConfiguration, JoalFolders};
use joal_core::events::EngineEvent;
use joal_core::seed_manager::SeedManager;
use joal_core::snapshot::EngineSnapshot;
use joal_core::torrent::{InfoHash, TorrentStateStore};
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tracing::{error, info, warn};

/// Commands sent from the UI thread to the tokio runtime thread.
#[derive(Debug)]
pub enum EngineCommand {
    Stop,
    Start,
    ApplyConfig(AppConfiguration),
    DeleteTorrent(InfoHash),
    SetTorrentInitialCompleted {
        info_hash: InfoHash,
        completed: bool,
    },
    AddTorrent(PathBuf),
    AnnounceAllNow,
    ListClients,
}

/// Responses sent from the tokio runtime back to the UI.
#[derive(Debug)]
pub enum EngineResponse {
    Stopped,
    Started {
        snapshot_rx: watch::Receiver<EngineSnapshot>,
        events_rx: broadcast::Receiver<EngineEvent>,
    },
    ConfigApplied,
    Error(String),
    ClientList(Vec<String>),
}

pub(crate) async fn send_response(
    resp_tx: &mpsc::Sender<EngineResponse>,
    response: EngineResponse,
) {
    if resp_tx.send(response).await.is_err() {
        warn!(target: "joal_app::cmd", "UI response receiver dropped");
    }
}

pub(crate) async fn command_handler(
    cmd_rx: mpsc::Receiver<EngineCommand>,
    resp_tx: mpsc::Sender<EngineResponse>,
    folders: Arc<JoalFolders>,
    shared_sm: Arc<Mutex<Option<SeedManager>>>,
    joal_conf: PathBuf,
) {
    CommandContext {
        cmd_rx,
        resp_tx,
        folders,
        shared_sm,
        joal_conf,
    }
    .run()
    .await;
}

struct CommandContext {
    cmd_rx: mpsc::Receiver<EngineCommand>,
    resp_tx: mpsc::Sender<EngineResponse>,
    folders: Arc<JoalFolders>,
    shared_sm: Arc<Mutex<Option<SeedManager>>>,
    joal_conf: PathBuf,
}

impl CommandContext {
    async fn run(mut self) {
        while let Some(cmd) = self.cmd_rx.recv().await {
            self.handle(cmd).await;
        }
    }

    async fn handle(&self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::ListClients => self.list_clients().await,
            EngineCommand::ApplyConfig(cfg) => self.apply_config(cfg).await,
            EngineCommand::DeleteTorrent(info_hash) => self.delete_torrent(info_hash).await,
            EngineCommand::SetTorrentInitialCompleted {
                info_hash,
                completed,
            } => {
                self.set_initial_completed(info_hash, completed).await;
            }
            EngineCommand::AddTorrent(source_path) => self.add_torrent(source_path).await,
            EngineCommand::AnnounceAllNow => self.announce_all_now().await,
            EngineCommand::Stop => self.stop_engine().await,
            EngineCommand::Start => self.start_engine().await,
        }
    }

    async fn list_clients(&self) {
        match config::list_client_files(&self.folders).await {
            Ok(clients) => send_response(&self.resp_tx, EngineResponse::ClientList(clients)).await,
            Err(e) => {
                send_response(
                    &self.resp_tx,
                    EngineResponse::Error(format!("Failed to list clients: {e}")),
                )
                .await;
            }
        }
    }

    async fn apply_config(&self, cfg: AppConfiguration) {
        apply_config_with_starter(
            cfg,
            &self.resp_tx,
            &self.folders,
            &self.shared_sm,
            &self.joal_conf,
            |path| async move { SeedManager::start(&path).await },
        )
        .await;
    }

    async fn delete_torrent(&self, info_hash: InfoHash) {
        let guard = self.shared_sm.lock().await;
        let Some(sm) = guard.as_ref() else {
            self.report_engine_not_running().await;
            return;
        };
        if let Err(e) = sm.delete_torrent(&info_hash).await {
            error!(target: "joal_app::cmd", error = %e, info_hash = %info_hash, "failed to clean torrent UI state on delete");
            send_response(
                &self.resp_tx,
                EngineResponse::Error(format!("Failed to delete torrent state: {e}")),
            )
            .await;
        }
    }

    async fn set_initial_completed(&self, info_hash: InfoHash, completed: bool) {
        let guard = self.shared_sm.lock().await;
        let result = if let Some(sm) = guard.as_ref() {
            sm.set_torrent_initial_completed(&info_hash, completed)
                .await
        } else {
            drop(guard);
            let store = TorrentStateStore::load(&self.folders).await;
            store.set_initial_completed(&info_hash, completed).await
        };
        if let Err(e) = result {
            error!(target: "joal_app::cmd", error = %e, info_hash = %info_hash, completed, "failed to persist torrent completed flag");
            send_response(
                &self.resp_tx,
                EngineResponse::Error(format!("Failed to save torrent state: {e}")),
            )
            .await;
        }
    }

    async fn add_torrent(&self, source_path: PathBuf) {
        let Some(filename) = source_path.file_name() else {
            return;
        };
        let dest = self.folders.torrents_dir.join(filename);
        if let Err(e) = tokio::fs::copy(&source_path, &dest).await {
            error!(target: "joal_app::cmd", error = %e, source = %source_path.display(), "failed to copy torrent file");
            send_response(
                &self.resp_tx,
                EngineResponse::Error(format!("Failed to add torrent: {e}")),
            )
            .await;
        } else {
            info!(target: "joal_app::cmd", dest = %dest.display(), "torrent file copied to torrents/");
        }
    }

    async fn announce_all_now(&self) {
        let guard = self.shared_sm.lock().await;
        if let Some(sm) = guard.as_ref() {
            sm.announce_all_now();
        } else {
            self.report_engine_not_running().await;
        }
    }

    async fn stop_engine(&self) {
        let mut guard = self.shared_sm.lock().await;
        if let Some(mut sm) = guard.take() {
            sm.stop().await;
            info!(target: "joal_app::cmd", "engine stopped");
        }
        send_response(&self.resp_tx, EngineResponse::Stopped).await;
    }

    async fn start_engine(&self) {
        let mut guard = self.shared_sm.lock().await;
        if guard.is_some() {
            send_response(
                &self.resp_tx,
                EngineResponse::Error("Engine is already running".to_owned()),
            )
            .await;
            return;
        }
        match SeedManager::start(&self.joal_conf).await {
            Ok(sm) => {
                let snapshot_rx = sm.snapshot_watch();
                let events_rx = sm.subscribe_events();
                *guard = Some(sm);
                info!(target: "joal_app::cmd", "engine started");
                send_response(
                    &self.resp_tx,
                    EngineResponse::Started {
                        snapshot_rx,
                        events_rx,
                    },
                )
                .await;
            }
            Err(e) => {
                error!(target: "joal_app::cmd", error = %e, "failed to start engine");
                send_response(
                    &self.resp_tx,
                    EngineResponse::Error(format!("Failed to start engine: {e}")),
                )
                .await;
            }
        }
    }

    async fn report_engine_not_running(&self) {
        send_response(
            &self.resp_tx,
            EngineResponse::Error("Engine is not running".to_owned()),
        )
        .await;
    }
}

async fn apply_config_with_starter<S, Fut>(
    cfg: AppConfiguration,
    resp_tx: &mpsc::Sender<EngineResponse>,
    folders: &JoalFolders,
    shared_sm: &Arc<Mutex<Option<SeedManager>>>,
    joal_conf: &std::path::Path,
    starter: S,
) where
    S: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<SeedManager>>,
{
    if let Err(e) = config::save(folders, &cfg).await {
        error!(target: "joal_app::cmd", error = %e, "failed to save config");
        send_response(
            resp_tx,
            EngineResponse::Error(format!("Failed to save config: {e}")),
        )
        .await;
        return;
    }
    info!(target: "joal_app::cmd", "config saved");

    let mut guard = shared_sm.lock().await;
    let was_running = if let Some(mut sm) = guard.take() {
        sm.stop().await;
        info!(target: "joal_app::cmd", "engine stopped for config apply");
        true
    } else {
        false
    };
    if was_running {
        send_response(resp_tx, EngineResponse::Stopped).await;
    }

    match starter(joal_conf.to_path_buf()).await {
        Ok(sm) => {
            let snapshot_rx = sm.snapshot_watch();
            let events_rx = sm.subscribe_events();
            *guard = Some(sm);
            drop(guard);
            info!(target: "joal_app::cmd", "engine restarted after config apply");
            send_response(
                resp_tx,
                EngineResponse::Started {
                    snapshot_rx,
                    events_rx,
                },
            )
            .await;
            send_response(resp_tx, EngineResponse::ConfigApplied).await;
        }
        Err(e) => {
            drop(guard);
            error!(
                target: "joal_app::cmd",
                error = %e,
                "failed to restart engine after saving config"
            );
            send_response(
                resp_tx,
                EngineResponse::Error(format!("Failed to restart engine with saved config: {e}")),
            )
            .await;
        }
    }
}
