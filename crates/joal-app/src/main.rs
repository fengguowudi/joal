#![windows_subsystem = "windows"]

mod engine;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use clap::Parser;
use joal_core::config::JoalFolders;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tracing::{info, warn};

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
    if let Some(path) = arg {
        return path;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe.parent().unwrap_or(exe.as_ref()).join("resources")
}

fn logs_dir_for(joal_conf: &Path) -> PathBuf {
    joal_conf.parent().unwrap_or(joal_conf).join("logs")
}

fn configure_cjk_fonts(ctx: &egui::Context) {
    let mut candidates = vec![
        PathBuf::from("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/wqy-microhei/wqy-microhei.ttc"),
        PathBuf::from("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"),
        PathBuf::from("/System/Library/Fonts/Hiragino Sans GB.ttc"),
        PathBuf::from("/System/Library/Fonts/PingFang.ttc"),
        PathBuf::from("/System/Library/Fonts/STHeiti Light.ttc"),
    ];
    if let Some(windows_dir) = std::env::var_os("WINDIR") {
        let fonts = PathBuf::from(windows_dir).join("Fonts");
        for name in ["msyh.ttc", "msyh.ttf", "simhei.ttf", "simsun.ttc"] {
            candidates.push(fonts.join(name));
        }
    }

    let Some(font_path) = candidates.iter().find(|path| path.is_file()) else {
        return;
    };
    let Ok(font_data) = std::fs::read(font_path) else {
        warn!(path = %font_path.display(), "failed to read CJK font");
        return;
    };

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

fn init_tracing(logs_dir: &Path) {
    use std::fs;
    use tracing_subscriber::{
        EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt,
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,joal_core=debug,joal_app=debug"));
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let file_layer = fs::create_dir_all(logs_dir)
        .and_then(|()| fs::File::create(logs_dir.join(format!("{timestamp}.log"))))
        .map_err(|error| {
            warn!(%error, directory = %logs_dir.display(), "file logging disabled");
            error
        })
        .ok()
        .map(|file| {
            fmt::layer()
                .with_target(true)
                .with_ansi(false)
                .with_writer(file)
                .with_filter(filter.clone())
        });
    let console_layer = fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .with_filter(filter);

    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .init();
    prune_logs(logs_dir, 10);
}

fn prune_logs(logs_dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return;
    };
    let mut logs = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
        .collect::<Vec<_>>();
    logs.sort();
    let remove_count = logs.len().saturating_sub(keep);
    for path in logs.into_iter().take(remove_count) {
        if let Err(error) = std::fs::remove_file(&path) {
            warn!(%error, path = %path.display(), "failed to remove old log");
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let joal_conf = resolve_joal_conf(args.joal_conf);
    init_tracing(&logs_dir_for(&joal_conf));
    info!(
        target: "joal_app::boot",
        joal_conf = %joal_conf.display(),
        "joal-desktop starting (egui mode)"
    );

    let runtime = Runtime::new()?;
    let repaint = engine::Repaint::default();
    let (link, shutdown_tx) = engine::spawn(runtime.handle(), joal_conf.clone(), repaint.clone());
    let folders = JoalFolders::new(&joal_conf);
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("JOAL Desktop")
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };

    let creator_repaint = repaint.clone();
    if let Err(error) = eframe::run_native(
        "JOAL Desktop",
        native_options,
        Box::new(move |cc| {
            creator_repaint.set(cc.egui_ctx.clone());
            configure_cjk_fonts(&cc.egui_ctx);
            ui::configure_visuals(&cc.egui_ctx);
            Ok(Box::new(ui::JoalApp::new(link, &folders, cc.storage)))
        }),
    ) {
        warn!(target: "joal_app::boot", %error, "eframe exited with error");
    }

    info!(target: "joal_app::boot", "window closed, shutting down engine");
    runtime.block_on(async {
        let (done_tx, done_rx) = oneshot::channel();
        let shutdown = async {
            if shutdown_tx
                .send(engine::EngineCommand::Shutdown(done_tx))
                .await
                .is_ok()
            {
                let _ = done_rx.await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(15), shutdown)
            .await
            .is_err()
        {
            warn!(target: "joal_app::boot", "engine shutdown timed out");
        }
    });
    info!(target: "joal_app::boot", "joal-desktop stopped cleanly");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_live_next_to_configuration_parent() {
        assert_eq!(
            logs_dir_for(Path::new("/tmp/joal/resources")),
            PathBuf::from("/tmp/joal/logs")
        );
    }
}
