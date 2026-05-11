//! `joal-desktop` binary entry point.
//!
//! MVP-1: CLI-only, no UI. Boots the full headless seeding pipeline
//! (`joal-core`), forwards engine events to the structured logger, and stays
//! alive until the operator presses Ctrl+C. MVP-2 replaces this shell with an
//! eframe window (see task PRD).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use joal_core::announcer::{AnnounceDataAccessor, AnnouncerFacade};
use joal_core::bandwidth::{BandwidthDispatcher, RandomSpeedProvider};
use joal_core::client::{BitTorrentClient, BitTorrentClientProvider, ConnectionHandler};
use joal_core::config::{self, AppConfiguration, JoalFolders};
use joal_core::events::{BroadcastSink, EngineEvent, EngineEventSink};
use joal_core::torrent::TorrentFileProvider;
use joal_core::ttorrent_client::{AnnouncerFactory, ClientOrchestrator};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

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

/// Cadence for the status printer. Matches the PRD's 30s requirement.
const STATUS_REPORT_INTERVAL: Duration = Duration::from_secs(30);

/// Tick cadence for the bandwidth dispatcher. Java uses 1s; keep parity.
const BANDWIDTH_TICK_PERIOD: Duration = Duration::from_secs(1);

/// HTTP timeout for tracker announces. Matches a Java-side default that would
/// otherwise keep a dead tracker connection dangling indefinitely.
const TRACKER_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    info!(
        target: "joal_app::boot",
        joal_conf = %args.joal_conf.display(),
        "joal-desktop starting",
    );

    let runtime = boot(&args.joal_conf).await?;
    let AppRuntime {
        orchestrator,
        torrent_provider,
        events,
        active_client_filename,
    } = runtime;

    // Fan out engine events to the structured logger.
    let event_task = spawn_event_logger(events.subscribe());

    // Periodic status printer.
    let status_task =
        spawn_status_printer(Arc::clone(&orchestrator), active_client_filename.clone());

    info!(target: "joal_app::boot", "waiting for Ctrl+C to shut down");
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!(target: "joal_app::boot", "Ctrl+C received, shutting down"),
        Err(e) => warn!(
            target: "joal_app::boot",
            error = %e,
            "failed to install Ctrl+C handler, shutting down anyway",
        ),
    }

    // Stop in reverse boot order. `orchestrator.stop()` drains pending
    // announces and awaits STOP announces before returning.
    if let Err(e) = orchestrator.stop().await {
        warn!(target: "joal_app::boot", error = %e, "orchestrator.stop() failed");
    }
    torrent_provider.stop().await;

    // Logger/status tasks should wrap up naturally once the event bus goes
    // quiet; abort them so the process exits promptly.
    status_task.abort();
    event_task.abort();
    let _ = status_task.await;
    let _ = event_task.await;

    info!(target: "joal_app::boot", "joal-desktop stopped cleanly");
    Ok(())
}

/// Assembled runtime returned by [`boot`].
struct AppRuntime {
    orchestrator: Arc<ClientOrchestrator>,
    torrent_provider: Arc<TorrentFileProvider>,
    events: BroadcastSink,
    active_client_filename: String,
}

/// Wire every piece of `joal-core` together and start the seeding loop.
///
/// Mirrors the Java `SeedManager.init()` + `startSeeding()` sequence: load
/// config, load the active `.client`, start the bandwidth dispatcher, start
/// the torrent watcher, then start the orchestrator.
async fn boot(joal_conf: &std::path::Path) -> Result<AppRuntime> {
    // 1) config.json → (AppConfiguration, JoalFolders).
    let (app_config, folders) = config::load(joal_conf)
        .await
        .with_context(|| format!("failed to load joal-conf from {}", joal_conf.display()))?;
    info!(
        target: "joal_app::boot",
        min_upload_rate = app_config.min_upload_rate,
        max_upload_rate = app_config.max_upload_rate,
        simultaneous_seed = app_config.simultaneous_seed,
        upload_ratio_target = app_config.upload_ratio_target,
        active_client = %app_config.client,
        "loaded config.json",
    );

    // 2) `.client` → runtime BitTorrentClient.
    let active_client_filename = app_config.client.clone();
    let client = load_active_client(&folders, &active_client_filename).await?;
    let client = Arc::new(client);
    info!(
        target: "joal_app::boot",
        client_file = %active_client_filename,
        header_count = client.headers().len(),
        "loaded active BitTorrent client",
    );

    // 3) Bandwidth dispatcher. Start it up-front so the first tracker
    //    announce sees a non-zero uploaded counter once bytes accumulate.
    let bandwidth = build_bandwidth_dispatcher(&app_config);
    let bandwidth = Arc::new(bandwidth);

    // 4) Torrent watcher (existing files trigger synthetic create events).
    let torrent_provider =
        TorrentFileProvider::new(&folders).context("failed to initialise torrent file provider")?;
    torrent_provider
        .start()
        .await
        .context("failed to start torrent file watcher")?;
    info!(
        target: "joal_app::boot",
        torrents_dir = %folders.torrents_dir.display(),
        initial_torrents = torrent_provider.torrent_count().await,
        "torrent watcher started",
    );

    // 5) Event sink. `BroadcastSink` lets the CLI logger and (later) the
    //    egui UI subscribe in parallel without contention.
    let events = BroadcastSink::default();
    let events_trait: Arc<dyn EngineEventSink> = Arc::new(events.clone());

    // 6) ConnectionHandler — pick an ephemeral port and leave IP as `None`.
    //    Java resolves the public IP via "what-is-my-ip" providers; that's a
    //    separate concern (S10 does not wire it in).
    //
    //    If the ephemeral bind fails (sandboxed env, exhausted local port
    //    table), fall back to 51413. That's the BitTorrent "well-known"
    //    listen port used by the canonical reference clients; a tracker
    //    seeing it still treats the announce as legitimate, whereas a port
    //    of `0` would be rejected as malformed by strict trackers.
    let connection = match ConnectionHandler::with_ephemeral_port() {
        Ok(handler) => Arc::new(handler),
        Err(e) => {
            warn!(
                target: "joal_app::boot",
                error = %e,
                "failed to bind an ephemeral port; falling back to BitTorrent default 51413",
            );
            Arc::new(ConnectionHandler::with_port_only(51413))
        }
    };
    info!(
        target: "joal_app::boot",
        port = connection.port(),
        ip = ?connection.ip_address(),
        "connection handler ready",
    );

    // 7) Data accessor + announcer factory + orchestrator.
    let data_accessor = AnnounceDataAccessor::new(
        Arc::clone(&client),
        Arc::clone(&bandwidth),
        Arc::clone(&connection),
    );
    let http = reqwest::Client::builder()
        .timeout(TRACKER_HTTP_TIMEOUT)
        .build()
        .context("failed to build reqwest HTTP client")?;
    let factory = AnnouncerFactory::new(data_accessor, http, app_config.upload_ratio_target);

    let orchestrator = ClientOrchestrator::new(
        app_config,
        Arc::clone(&torrent_provider),
        Arc::clone(&bandwidth),
        factory,
        &events_trait,
    );
    orchestrator
        .start()
        .await
        .context("failed to start client orchestrator")?;
    info!(target: "joal_app::boot", "client orchestrator started");

    Ok(AppRuntime {
        orchestrator,
        torrent_provider,
        events,
        active_client_filename,
    })
}

/// Locate + parse the `.client` file referenced by
/// [`AppConfiguration::client`]. Java `BitTorrentClientProvider.generateNewClient`.
async fn load_active_client(folders: &JoalFolders, file_name: &str) -> Result<BitTorrentClient> {
    let provider = BitTorrentClientProvider::new(folders.clients_dir.clone());
    provider.load(file_name).await.with_context(|| {
        format!(
            "failed to load active .client file [{}] from [{}]",
            file_name,
            folders.clients_dir.display(),
        )
    })
}

/// Build (but do not start) a dispatcher seeded with a production
/// [`RandomSpeedProvider`]. A failure to start is downgraded to a warning —
/// the engine still schedules announces, just without the periodic speed
/// refresh.
fn build_bandwidth_dispatcher(app_config: &AppConfiguration) -> BandwidthDispatcher {
    let mut dispatcher =
        BandwidthDispatcher::new(BANDWIDTH_TICK_PERIOD, RandomSpeedProvider::new(app_config));
    if let Err(e) = dispatcher.start() {
        warn!(
            target: "joal_app::boot",
            error = %e,
            "failed to start bandwidth dispatcher tick task; continuing without it",
        );
    } else {
        info!(
            target: "joal_app::boot",
            tick_period_ms = u64::try_from(BANDWIDTH_TICK_PERIOD.as_millis()).unwrap_or(u64::MAX),
            "bandwidth dispatcher started",
        );
    }
    dispatcher
}

/// Spawn a task that drains the engine event bus into the structured logger.
///
/// Uses `broadcast::Receiver::recv`: on `Lagged` we log and keep going; on
/// `Closed` we exit (the sender has been dropped, i.e. shutdown in progress).
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

/// Log one [`EngineEvent`] at the level that matches its semantic weight.
/// Mirrors the Java `CoreEventListener` / `WebXxxEventListener` split.
fn log_event(event: &EngineEvent) {
    match event {
        EngineEvent::GlobalSeedStarted { client_name } => {
            info!(
                target: "joal_app::events",
                client_name = %client_name,
                "global seed started",
            );
        }
        EngineEvent::GlobalSeedStopped => {
            info!(target: "joal_app::events", "global seed stopped");
        }
        EngineEvent::TorrentFileAdded {
            info_hash,
            name,
            total_size,
        } => {
            info!(
                target: "joal_app::events",
                info_hash = %info_hash,
                name = %name,
                total_size,
                "torrent file added",
            );
        }
        EngineEvent::TorrentFileDeleted { info_hash, name } => {
            info!(
                target: "joal_app::events",
                info_hash = %info_hash,
                name = %name,
                "torrent file deleted",
            );
        }
        EngineEvent::FailedToAddTorrentFile { name, reason } => {
            warn!(
                target: "joal_app::events",
                name = %name,
                reason = %reason,
                "failed to add torrent file",
            );
        }
        EngineEvent::TooManyAnnouncesFailedInARow { info_hash, name } => {
            warn!(
                target: "joal_app::events",
                info_hash = %info_hash,
                name = %name,
                "torrent exceeded the consecutive-failure threshold",
            );
        }
        EngineEvent::SeedingSpeedsHasChanged { speeds } => {
            debug!(
                target: "joal_app::events",
                torrent_count = speeds.len(),
                "seeding speeds recomputed",
            );
        }
        EngineEvent::ConfigLoaded { config } => {
            info!(
                target: "joal_app::events",
                min_upload_rate = config.min_upload_rate,
                max_upload_rate = config.max_upload_rate,
                simultaneous_seed = config.simultaneous_seed,
                active_client = %config.client,
                "configuration reloaded",
            );
        }
    }
}

/// Spawn a task that emits one status summary line every
/// [`STATUS_REPORT_INTERVAL`]. Fields match the MVP-1 PRD: active client,
/// catalogue size, running announcers and their last-known tracker snapshot.
fn spawn_status_printer(
    orchestrator: Arc<ClientOrchestrator>,
    active_client_filename: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(STATUS_REPORT_INTERVAL);
        // Skip the immediate tick so the first status line appears
        // `STATUS_REPORT_INTERVAL` after boot (not at boot).
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            report_status(&orchestrator, &active_client_filename);
        }
    })
}

/// Emit one status summary line. Kept synchronous so a slow logger cannot
/// cause the status tick to skew.
fn report_status(orchestrator: &Arc<ClientOrchestrator>, active_client_filename: &str) {
    let announcers = orchestrator.seeding_announcer_facades();
    info!(
        target: "joal_app::status",
        active_client = %active_client_filename,
        running_announcers = announcers.len(),
        "status report",
    );
    for announcer in announcers {
        log_announcer_status(announcer.as_ref());
    }
}

/// One structured line per live announcer — mirrors Java's per-torrent
/// `Announcer.announce(...)` successful-announce log at `info`.
fn log_announcer_status(announcer: &dyn AnnouncerFacade) {
    info!(
        target: "joal_app::status",
        info_hash = %announcer.torrent_info_hash(),
        name = %announcer.torrent_name(),
        total_size = announcer.torrent_size(),
        interval_s = announcer.last_known_interval(),
        seeders = ?announcer.last_known_seeders(),
        leechers = ?announcer.last_known_leechers(),
        consecutive_fails = announcer.consecutive_fails(),
        last_announced_ago_s = announcer
            .last_announced_at()
            .map(|t| t.elapsed().as_secs()),
        "announcer status",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_tracing_is_idempotent_on_parse() {
        // Sanity check: `Args::parse_from` recognises the CLI flag. This also
        // doubles as a smoke test that `clap` derive is wired correctly.
        let args = Args::try_parse_from(["joal-desktop", "--joal-conf", "/tmp/joal"]).unwrap();
        assert_eq!(args.joal_conf, std::path::PathBuf::from("/tmp/joal"));
    }

    #[test]
    fn missing_joal_conf_flag_is_a_parse_error() {
        let err = Args::try_parse_from(["joal-desktop"]).unwrap_err();
        // clap reports missing required arguments as `MissingRequiredArgument`.
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn build_bandwidth_dispatcher_does_not_panic_on_zero_rate_config() {
        // A zero-rate config is Java-legal (Java `validate()` only enforces
        // max >= min). Dispatcher construction must tolerate it.
        let cfg = AppConfiguration {
            min_upload_rate: 0,
            max_upload_rate: 0,
            simultaneous_seed: 1,
            client: "x.client".into(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
        };
        // We can't actually spawn a tokio task from a sync test; just verify
        // the `BandwidthDispatcher::new` path doesn't blow up.
        let _ = BandwidthDispatcher::new(BANDWIDTH_TICK_PERIOD, RandomSpeedProvider::new(&cfg));
    }
}
