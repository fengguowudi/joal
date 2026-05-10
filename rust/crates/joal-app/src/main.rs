//! `joal-desktop` binary entry point.
//!
//! MVP-1: CLI-only, no UI. Prints loaded config / client / torrent state and
//! keeps the seeding loop alive. MVP-2 replaces this shell with an eframe
//! window (see task PRD).

use anyhow::Result;
use clap::Parser;

/// JOAL desktop — BitTorrent seeding client simulator.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the `joal-conf` directory (must contain `config.json`,
    /// `clients/` and `torrents/`). Equivalent to the Java flag
    /// `--joal-conf=PATH`.
    #[arg(long = "joal-conf", value_name = "DIR")]
    joal_conf: std::path::PathBuf,
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,joal_core=debug,joal_app=debug"));
    fmt().with_env_filter(filter).with_target(true).init();
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    tracing::info!(joal_conf = %args.joal_conf.display(), "joal-desktop starting");

    // Subsequent steps wire in config::load -> seed_manager::run(...).
    // For S1 we stop here so the workspace compiles end-to-end.
    tracing::warn!("MVP-1 scaffolding only — seeding loop not wired yet");
    Ok(())
}
