mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use joal_core::config::{self, AppConfiguration, JoalFolders};
use joal_core::events::EngineEvent;
use joal_core::seed_manager::SeedManager;
use joal_core::snapshot::EngineSnapshot;
use joal_core::torrent::InfoHash;
use tokio::runtime::Runtime;
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tracing::{error, info, warn};

/// JOAL desktop — BitTorrent seeding client simulator.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the `joal-conf` directory (must contain `config.json`,
    /// `clients/` and `torrents/`). Defaults to `resources/` next to the
    /// executable.
    #[arg(long = "joal-conf", value_name = "DIR")]
    joal_conf: Option<PathBuf>,
}

fn resolve_joal_conf(arg: Option<PathBuf>) -> PathBuf {
    if let Some(p) = arg {
        return p;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe.parent().unwrap_or(exe.as_ref()).join("resources")
}

/// Commands sent from the UI thread to the tokio runtime thread.
#[derive(Debug)]
pub enum EngineCommand {
    Stop,
    Start,
    SaveConfig(AppConfiguration),
    DeleteTorrent(InfoHash),
    AddTorrent(PathBuf),
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
    Error(String),
    ClientList(Vec<String>),
}

fn configure_cjk_fonts(ctx: &egui::Context) {
    let font_path = std::path::Path::new("C:\\Windows\\Fonts\\msyh.ttc");
    if let Ok(font_data) = std::fs::read(font_path) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "msyh".to_owned(),
            Arc::new(egui::FontData::from_owned(font_data)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "msyh".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("msyh".to_owned());
        ctx.set_fonts(fonts);
    }
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,joal_core=debug,joal_app=debug"));
    fmt().with_env_filter(filter).with_target(true).init();
}

fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let joal_conf = resolve_joal_conf(args.joal_conf);
    info!(
        target: "joal_app::boot",
        joal_conf = %joal_conf.display(),
        "joal-desktop starting (egui mode)",
    );

    let rt = Runtime::new()?;
    let seed_manager = rt.block_on(SeedManager::start(&joal_conf))?;

    let snapshot_rx = seed_manager.snapshot_watch();
    let events_rx = seed_manager.subscribe_events();
    let folders = seed_manager.folders().clone();
    let started_at = Instant::now();

    // Command channel: UI -> tokio runtime
    let (cmd_tx, cmd_rx) = mpsc::channel::<EngineCommand>(32);
    // Response channel: tokio runtime -> UI
    let (resp_tx, resp_rx) = mpsc::channel::<EngineResponse>(32);

    let folders_arc = Arc::new(folders);

    // Share the SeedManager with the command handler
    let shared_sm = Arc::new(Mutex::new(Some(seed_manager)));

    // Spawn the command handler on the tokio runtime
    let cmd_folders = folders_arc.clone();
    let cmd_sm = shared_sm.clone();
    let joal_conf_for_cmd = joal_conf;
    rt.spawn(command_handler(
        cmd_rx,
        resp_tx,
        cmd_folders,
        cmd_sm,
        joal_conf_for_cmd,
    ));

    let app = ui::JoalApp::new(
        snapshot_rx,
        events_rx,
        started_at,
        cmd_tx,
        resp_rx,
        folders_arc,
    );

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("JOAL Desktop")
            .with_inner_size([1024.0, 720.0])
            .with_min_inner_size([640.0, 480.0]),
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        "JOAL Desktop",
        native_options,
        Box::new(move |cc| {
            configure_cjk_fonts(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    ) {
        warn!(target: "joal_app::boot", error = %e, "eframe exited with error");
    }

    info!(target: "joal_app::boot", "window closed, shutting down engine");
    rt.block_on(async {
        let mut guard = shared_sm.lock().await;
        if let Some(sm) = guard.as_mut() {
            sm.stop().await;
        }
    });
    info!(target: "joal_app::boot", "joal-desktop stopped cleanly");
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn command_handler(
    mut cmd_rx: mpsc::Receiver<EngineCommand>,
    resp_tx: mpsc::Sender<EngineResponse>,
    folders: Arc<JoalFolders>,
    shared_sm: Arc<Mutex<Option<SeedManager>>>,
    joal_conf: PathBuf,
) {
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            EngineCommand::ListClients => match config::list_client_files(&folders).await {
                Ok(clients) => {
                    let _ = resp_tx.send(EngineResponse::ClientList(clients)).await;
                }
                Err(e) => {
                    let _ = resp_tx
                        .send(EngineResponse::Error(format!(
                            "Failed to list clients: {e}"
                        )))
                        .await;
                }
            },
            EngineCommand::SaveConfig(cfg) => match config::save(&folders, &cfg).await {
                Ok(()) => {
                    info!(target: "joal_app::cmd", "config saved");
                }
                Err(e) => {
                    error!(target: "joal_app::cmd", error = %e, "failed to save config");
                    let _ = resp_tx
                        .send(EngineResponse::Error(format!("Failed to save config: {e}")))
                        .await;
                }
            },
            EngineCommand::DeleteTorrent(info_hash) => {
                let guard = shared_sm.lock().await;
                if let Some(sm) = guard.as_ref() {
                    sm.delete_torrent(&info_hash).await;
                } else {
                    let _ = resp_tx
                        .send(EngineResponse::Error(
                            "Engine is not running".to_owned(),
                        ))
                        .await;
                }
            }
            EngineCommand::AddTorrent(source_path) => {
                if let Some(filename) = source_path.file_name() {
                    let dest = folders.torrents_dir.join(filename);
                    if let Err(e) = tokio::fs::copy(&source_path, &dest).await {
                        error!(
                            target: "joal_app::cmd",
                            error = %e,
                            source = %source_path.display(),
                            "failed to copy torrent file"
                        );
                        let _ = resp_tx
                            .send(EngineResponse::Error(format!("Failed to add torrent: {e}")))
                            .await;
                    } else {
                        info!(
                            target: "joal_app::cmd",
                            dest = %dest.display(),
                            "torrent file copied to torrents/"
                        );
                    }
                }
            }
            EngineCommand::Stop => {
                let mut guard = shared_sm.lock().await;
                if let Some(mut sm) = guard.take() {
                    sm.stop().await;
                    info!(target: "joal_app::cmd", "engine stopped");
                }
                let _ = resp_tx.send(EngineResponse::Stopped).await;
            }
            EngineCommand::Start => {
                let mut guard = shared_sm.lock().await;
                if guard.is_some() {
                    // Already running
                    let _ = resp_tx
                        .send(EngineResponse::Error(
                            "Engine is already running".to_owned(),
                        ))
                        .await;
                    continue;
                }
                match SeedManager::start(&joal_conf).await {
                    Ok(sm) => {
                        let snapshot_rx = sm.snapshot_watch();
                        let events_rx = sm.subscribe_events();
                        *guard = Some(sm);
                        info!(target: "joal_app::cmd", "engine started");
                        let _ = resp_tx
                            .send(EngineResponse::Started {
                                snapshot_rx,
                                events_rx,
                            })
                            .await;
                    }
                    Err(e) => {
                        error!(target: "joal_app::cmd", error = %e, "failed to start engine");
                        let _ = resp_tx
                            .send(EngineResponse::Error(format!(
                                "Failed to start engine: {e}"
                            )))
                            .await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joal_conf_flag_parses() {
        let args = Args::try_parse_from(["joal-desktop", "--joal-conf", "/tmp/joal"]).unwrap();
        assert_eq!(args.joal_conf, Some(PathBuf::from("/tmp/joal")));
    }

    #[test]
    fn missing_joal_conf_flag_uses_default() {
        let args = Args::try_parse_from(["joal-desktop"]).unwrap();
        assert_eq!(args.joal_conf, None);
        let resolved = resolve_joal_conf(args.joal_conf);
        assert!(resolved.ends_with("resources"));
    }
}
