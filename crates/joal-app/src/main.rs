#![windows_subsystem = "windows"]

mod command;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use command::command_handler;
use joal_core::seed_manager::SeedManager;
use tokio::runtime::Runtime;
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};

pub use command::{EngineCommand, EngineResponse};

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

fn configure_cjk_fonts(ctx: &egui::Context) {
    let candidates = [
        // Linux — Noto Sans CJK
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        // Linux — WenQuanYi
        "/usr/share/fonts/wqy-microhei/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        // macOS
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        // Windows
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyhbd.ttc",
    ];

    let font_path = candidates.iter().find(|p| std::path::Path::new(p).exists());
    let Some(font_path) = font_path else { return };

    if let Ok(font_data) = std::fs::read(font_path) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk".to_owned(),
            Arc::new(egui::FontData::from_owned(font_data)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_owned());
        ctx.set_fonts(fonts);
    }
}

fn init_tracing() {
    use std::fs;
    use std::time::SystemTime;
    use tracing_subscriber::{EnvFilter, fmt, Layer, layer::SubscriberExt, util::SubscriberInitExt};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,joal_core=debug,joal_app=debug"));

    // Create logs directory if it doesn't exist
    let logs_dir = std::path::Path::new("logs");
    if !logs_dir.exists() {
        let _ = fs::create_dir_all(logs_dir);
    }

    // Create log file with unique timestamp name
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let log_file = fs::File::create(logs_dir.join(format!("{timestamp}.log")))
        .expect("Failed to create log file");

    // Initialize the log channel for UI
    ui::init_log_channel();

    // Console layer (hidden by windows_subsystem = "windows" attribute)
    let console_layer = fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .with_filter(filter.clone());

    // File layer
    let file_layer = fmt::layer()
        .with_target(true)
        .with_ansi(false)
        .with_writer(log_file)
        .with_filter(filter.clone());

    // UI layer - sends logs to the global channel
    let sender = ui::get_log_sender().expect("Log channel not initialized");
    let ui_layer = fmt::layer()
        .with_target(true)
        .with_ansi(false)
        .with_writer(move || {
            // Create a writer that sends logs to the channel
            ChannelWriter {
                sender: sender.clone(),
            }
        })
        .with_filter(filter);

    // Initialize the subscriber
    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .with(ui_layer)
        .init();
}

/// Custom writer that sends log messages to a channel
struct ChannelWriter {
    sender: std::sync::mpsc::Sender<String>,
}

impl std::io::Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(msg) = String::from_utf8(buf.to_vec()) {
            let _ = self.sender.send(msg);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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
            // 1180x740 keeps the torrent table comfortable on a fresh install
            // (all 11 columns visible without horizontal scroll at their
            // post-narrowing widths) without feeling oversized on a 1366x768
            // laptop. The 960x600 floor still keeps the action cluster from
            // wrapping on 13" panels.
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([960.0, 600.0]),
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
