//! `joal-desktop` binary entry point.
//!
//! MVP-1: CLI-only, no UI. Delegates every piece of wiring to
//! [`joal_core::seed_manager::SeedManager`], forwards engine events to the
//! structured logger, reports a periodic status line from the snapshot
//! channel, and stays alive until the operator presses Ctrl+C. MVP-2
//! replaces this shell with an eframe window that subscribes to the same
//! snapshot channel.

use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use joal_core::events::EngineEvent;
use joal_core::seed_manager::SeedManager;
use joal_core::snapshot::{EngineSnapshot, TorrentStatus};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Cadence for the status printer. Matches the MVP-1 PRD's 30s requirement.
const STATUS_REPORT_INTERVAL: Duration = Duration::from_secs(30);

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
    info!(
        target: "joal_app::boot",
        joal_conf = %args.joal_conf.display(),
        "joal-desktop starting",
    );

    let mut seed_manager = SeedManager::start(&args.joal_conf).await?;

    let event_task = spawn_event_logger(seed_manager.subscribe_events());
    let status_task = spawn_status_printer(seed_manager.snapshot_watch());

    info!(target: "joal_app::boot", "waiting for Ctrl+C to shut down");
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!(target: "joal_app::boot", "Ctrl+C received, shutting down"),
        Err(e) => warn!(
            target: "joal_app::boot",
            error = %e,
            "failed to install Ctrl+C handler, shutting down anyway",
        ),
    }

    seed_manager.stop().await;

    status_task.abort();
    event_task.abort();
    let _ = status_task.await;
    let _ = event_task.await;

    info!(target: "joal_app::boot", "joal-desktop stopped cleanly");
    Ok(())
}

/// Spawn a task that drains the engine event bus into the structured logger.
fn spawn_event_logger(mut rx: broadcast::Receiver<EngineEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => log_event(&event),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        target: "joal_app::events",
                        skipped,
                        "event logger lagged behind; some events were dropped",
                    );
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!(target: "joal_app::events", "event bus closed, logger exiting");
                    break;
                }
            }
        }
    })
}

fn log_event(event: &EngineEvent) {
    match event {
        EngineEvent::GlobalSeedStarted { client_name } => {
            info!(target: "joal_app::events", %client_name, "global seed started");
        }
        EngineEvent::GlobalSeedStopped => {
            info!(target: "joal_app::events", "global seed stopped");
        }
        EngineEvent::TorrentFileAdded {
            info_hash,
            name,
            total_size,
        } => info!(
            target: "joal_app::events",
            %info_hash, %name, total_size, "torrent file added",
        ),
        EngineEvent::TorrentFileDeleted { info_hash, name } => info!(
            target: "joal_app::events", %info_hash, %name, "torrent file deleted",
        ),
        EngineEvent::FailedToAddTorrentFile { name, reason } => warn!(
            target: "joal_app::events", %name, %reason, "failed to add torrent file",
        ),
        EngineEvent::TooManyAnnouncesFailedInARow { info_hash, name } => warn!(
            target: "joal_app::events", %info_hash, %name,
            "torrent exceeded the consecutive-failure threshold",
        ),
        EngineEvent::ConfigLoaded { config } => info!(
            target: "joal_app::events",
            min_upload_rate = config.min_upload_rate,
            max_upload_rate = config.max_upload_rate,
            simultaneous_seed = config.simultaneous_seed,
            active_client = %config.client,
            "configuration reloaded",
        ),
    }
}

/// One status line every [`STATUS_REPORT_INTERVAL`], sourced from the
/// merger-maintained [`EngineSnapshot`]. No direct coupling to the
/// orchestrator / bandwidth dispatcher — the snapshot is the contract.
fn spawn_status_printer(
    snapshot_rx: tokio::sync::watch::Receiver<EngineSnapshot>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(STATUS_REPORT_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            report_status(&snapshot_rx.borrow());
        }
    })
}

fn report_status(snapshot: &EngineSnapshot) {
    info!(
        target: "joal_app::status",
        active_client = %snapshot.active_client_filename,
        running_announcers = snapshot.torrents.len(),
        global_bps = snapshot.global_upload_speed_bps,
        "status report",
    );
    for t in &snapshot.torrents {
        log_torrent_status(t);
    }
}

fn log_torrent_status(t: &TorrentStatus) {
    info!(
        target: "joal_app::status",
        info_hash = %t.info_hash,
        name = %t.name,
        total_size = t.total_size,
        uploaded = t.uploaded_bytes,
        current_speed_bps = t.current_speed_bps,
        interval_s = ?t.last_known_interval,
        seeders = ?t.last_known_seeders,
        leechers = ?t.last_known_leechers,
        consecutive_fails = t.consecutive_fails,
        last_announced_ago_s = t.last_announced_at.map(|i| i.elapsed().as_secs()),
        "torrent status",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joal_conf_flag_parses() {
        let args = Args::try_parse_from(["joal-desktop", "--joal-conf", "/tmp/joal"]).unwrap();
        assert_eq!(args.joal_conf, std::path::PathBuf::from("/tmp/joal"));
    }

    #[test]
    fn missing_joal_conf_flag_is_a_parse_error() {
        let err = Args::try_parse_from(["joal-desktop"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }
}
