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
use joal_core::torrent::{InfoHash, TorrentStateStore};
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
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([640.0, 480.0]),
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        "JOAL Desktop",
        native_options,
        Box::new(move |cc| {
            configure_cjk_fonts(&cc.egui_ctx);
            ui::configure_visuals(&cc.egui_ctx);
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
            EngineCommand::ApplyConfig(cfg) => {
                apply_config_with_starter(
                    cfg,
                    &resp_tx,
                    &folders,
                    &shared_sm,
                    &joal_conf,
                    |path| async move { SeedManager::start(&path).await },
                )
                .await;
            }
            EngineCommand::DeleteTorrent(info_hash) => {
                let guard = shared_sm.lock().await;
                if let Some(sm) = guard.as_ref() {
                    if let Err(e) = sm.delete_torrent(&info_hash).await {
                        error!(
                            target: "joal_app::cmd",
                            error = %e,
                            info_hash = %info_hash,
                            "failed to clean torrent UI state on delete"
                        );
                        let _ = resp_tx
                            .send(EngineResponse::Error(format!(
                                "Failed to delete torrent state: {e}"
                            )))
                            .await;
                    }
                } else {
                    let _ = resp_tx
                        .send(EngineResponse::Error("Engine is not running".to_owned()))
                        .await;
                }
            }
            EngineCommand::SetTorrentInitialCompleted {
                info_hash,
                completed,
            } => {
                let guard = shared_sm.lock().await;
                let result = if let Some(sm) = guard.as_ref() {
                    sm.set_torrent_initial_completed(&info_hash, completed)
                        .await
                } else {
                    drop(guard);
                    let store = TorrentStateStore::load(&folders).await;
                    store.set_initial_completed(&info_hash, completed).await
                };
                if let Err(e) = result {
                    error!(
                        target: "joal_app::cmd",
                        error = %e,
                        info_hash = %info_hash,
                        completed,
                        "failed to persist torrent completed flag"
                    );
                    let _ = resp_tx
                        .send(EngineResponse::Error(format!(
                            "Failed to save torrent state: {e}"
                        )))
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
            EngineCommand::AnnounceAllNow => {
                let guard = shared_sm.lock().await;
                if let Some(sm) = guard.as_ref() {
                    sm.announce_all_now();
                } else {
                    let _ = resp_tx
                        .send(EngineResponse::Error("Engine is not running".to_owned()))
                        .await;
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
        let _ = resp_tx
            .send(EngineResponse::Error(format!("Failed to save config: {e}")))
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
        let _ = resp_tx.send(EngineResponse::Stopped).await;
    }

    match starter(joal_conf.to_path_buf()).await {
        Ok(sm) => {
            let snapshot_rx = sm.snapshot_watch();
            let events_rx = sm.subscribe_events();
            *guard = Some(sm);
            drop(guard);
            info!(target: "joal_app::cmd", "engine restarted after config apply");
            let _ = resp_tx
                .send(EngineResponse::Started {
                    snapshot_rx,
                    events_rx,
                })
                .await;
            let _ = resp_tx.send(EngineResponse::ConfigApplied).await;
        }
        Err(e) => {
            drop(guard);
            error!(
                target: "joal_app::cmd",
                error = %e,
                "failed to restart engine after saving config"
            );
            let _ = resp_tx
                .send(EngineResponse::Error(format!(
                    "Failed to restart engine with saved config: {e}"
                )))
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use joal_core::seed_manager::{EngineOptions, IpResolver};
    use tempfile::TempDir;

    struct StaticIpResolver;

    impl IpResolver for StaticIpResolver {
        fn resolve<'a>(
            &'a self,
            _proxy_url: Option<&'a str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<IpAddr>> + Send + 'a>>
        {
            Box::pin(async { None })
        }
    }

    async fn create_test_joal_conf() -> (TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let clients = root.join("clients");
        let torrents = root.join("torrents");
        tokio::fs::create_dir_all(&clients).await.unwrap();
        tokio::fs::create_dir_all(torrents.join("archived"))
            .await
            .unwrap();

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .map(PathBuf::from)
            .unwrap();
        let resources = workspace.join("resources");
        tokio::fs::copy(resources.join("config.json"), root.join("config.json"))
            .await
            .unwrap();
        tokio::fs::copy(
            resources
                .join("clients")
                .join("utorrent-3.5.0_43916.client"),
            clients.join("utorrent-3.5.0_43916.client"),
        )
        .await
        .unwrap();

        (temp, root)
    }

    async fn start_test_seed_manager(joal_conf: &std::path::Path) -> Result<SeedManager> {
        SeedManager::start_with(
            joal_conf,
            EngineOptions {
                ip_resolver: Box::new(StaticIpResolver),
            },
        )
        .await
    }

    async fn take_responses(
        resp_rx: &mut mpsc::Receiver<EngineResponse>,
        count: usize,
    ) -> Vec<EngineResponse> {
        let mut responses = Vec::with_capacity(count);
        for _ in 0..count {
            responses.push(
                tokio::time::timeout(std::time::Duration::from_secs(5), resp_rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        responses
    }

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

    #[tokio::test]
    async fn apply_config_returns_error_and_does_not_restart_when_save_fails() {
        let temp = tempfile::tempdir().unwrap();
        let folders = JoalFolders::new(temp.path());
        let shared_sm = Arc::new(Mutex::new(None));
        let (resp_tx, mut resp_rx) = mpsc::channel(8);
        let start_count = Arc::new(AtomicUsize::new(0));
        let start_count_for_closure = Arc::clone(&start_count);

        let invalid_cfg = AppConfiguration {
            min_upload_rate: 30,
            max_upload_rate: 170,
            min_download_rate: 0,
            max_download_rate: 0,
            simultaneous_seed: 0,
            client: "utorrent-3.5.0_43916.client".to_owned(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
            proxy_host: None,
            proxy_port: None,
        };

        apply_config_with_starter(
            invalid_cfg,
            &resp_tx,
            &folders,
            &shared_sm,
            temp.path(),
            move |_| {
                let start_count = Arc::clone(&start_count_for_closure);
                async move {
                    start_count.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!("starter should not run when save fails");
                }
            },
        )
        .await;

        let responses = take_responses(&mut resp_rx, 1).await;
        assert_eq!(start_count.load(Ordering::SeqCst), 0);
        assert!(matches!(
            responses.as_slice(),
            [EngineResponse::Error(message)]
            if message.contains("Failed to save config")
        ));
        assert!(resp_rx.try_recv().is_err());
        assert!(!folders.config_file().exists());
    }

    #[tokio::test]
    async fn apply_config_saves_and_restarts_engine_once() {
        let (_temp, joal_conf) = create_test_joal_conf().await;
        let folders = JoalFolders::new(&joal_conf);
        let initial_engine = start_test_seed_manager(&joal_conf).await.unwrap();
        let shared_sm = Arc::new(Mutex::new(Some(initial_engine)));
        let (resp_tx, mut resp_rx) = mpsc::channel(8);
        let start_count = Arc::new(AtomicUsize::new(0));
        let start_count_for_closure = Arc::clone(&start_count);

        let cfg = AppConfiguration {
            min_upload_rate: 40,
            max_upload_rate: 200,
            min_download_rate: 5,
            max_download_rate: 15,
            simultaneous_seed: 7,
            client: "utorrent-3.5.0_43916.client".to_owned(),
            keep_torrent_with_zero_leechers: false,
            upload_ratio_target: 2.0,
            proxy_host: None,
            proxy_port: None,
        };

        apply_config_with_starter(
            cfg.clone(),
            &resp_tx,
            &folders,
            &shared_sm,
            &joal_conf,
            move |path| {
                let start_count = Arc::clone(&start_count_for_closure);
                async move {
                    start_count.fetch_add(1, Ordering::SeqCst);
                    start_test_seed_manager(&path).await
                }
            },
        )
        .await;

        let responses = take_responses(&mut resp_rx, 3).await;
        assert_eq!(start_count.load(Ordering::SeqCst), 1);
        assert!(matches!(responses[0], EngineResponse::Stopped));
        assert!(matches!(responses[1], EngineResponse::Started { .. }));
        assert!(matches!(responses[2], EngineResponse::ConfigApplied));
        assert!(resp_rx.try_recv().is_err());

        let (saved_cfg, _) = config::load(&joal_conf).await.unwrap();
        assert_eq!(saved_cfg, cfg);

        let mut guard = shared_sm.lock().await;
        if let Some(mut sm) = guard.take() {
            sm.stop().await;
        }
    }
}
