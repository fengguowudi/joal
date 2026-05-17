//! Top-level orchestrator. Mirror of Java `SeedManager`.
//!
//! `SeedManager` is the composition root: one call to [`SeedManager::start`]
//! loads `joal-conf/`, builds the bandwidth dispatcher, torrent watcher,
//! announcer factory and client orchestrator, then spawns the merger task
//! that keeps an [`EngineSnapshot`] publication in lock-step with the live
//! engine.
//!
//! # Shape of the public API
//!
//! ```text
//!     ┌──────────────┐                                      ┌──────────────┐
//!     │  joal-app    │──start()─▶┌─────────────┐──subscribe─│  egui UI /   │
//!     │  (CLI / UI)  │◀ snapshot │ SeedManager │─events ───▶│  CLI logger  │
//!     └──────────────┘           └─────────────┘            └──────────────┘
//! ```
//!
//! * [`SeedManager::subscribe_events`] — fresh [`broadcast::Receiver`] for
//!   transitions (torrent added / removed, too-many-failures, config
//!   reloaded, global start/stop).
//! * [`SeedManager::snapshot`] — current [`EngineSnapshot`] frame.
//! * [`SeedManager::snapshot_watch`] — `watch::Receiver` so consumers can
//!   `.changed().await` and pull the newest frame.
//!
//! # Merger task
//!
//! The merger owns the snapshot state. It `select!`s on three inputs:
//!
//! 1. The broadcast bus — for [`EngineEvent`] transitions that affect the
//!    torrent list (add / remove / too-many-failures).
//! 2. An mpsc `MergerPoke` mailbox — fed by [`BandwidthDispatcher`] when it
//!    recomputes speeds, and by the announcer handler chain
//!    ([`MergerPokeNotifier`][crate::orchestrator::MergerPokeNotifier])
//!    after every announce round-trip.
//! 3. A shutdown oneshot — closed by [`SeedManager::stop`].
//!
//! On every wake-up it rebuilds the snapshot from scratch by joining the
//! orchestrator's announcer list with
//! `BandwidthDispatcher::get_seed_stat_for_torrent` + `speed_map`. Rebuilding
//! is O(torrents) and cheaper than the alternative (diffing event payloads)
//! for the 10–100 torrent case joal targets.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::announcer::AnnounceDataAccessor;
use crate::bandwidth::{BandwidthDispatcher, DownloadSpeedProvider, RandomSpeedProvider};
use crate::client::{
    BitTorrentClient, BitTorrentClientProvider, ConnectionHandler, fetch_public_ip,
};
use crate::config::{self, JoalFolders};
use crate::events::{BroadcastSink, EngineEvent, EngineEventSink};
use crate::orchestrator::{AnnouncerFactory, ClientOrchestrator};
use crate::snapshot::{EngineSnapshot, MergerPoke, TorrentStatus};
use crate::torrent::TorrentFileProvider;

// Re-export for convenience — the UI needs these to call config helpers.
pub use crate::config::AppConfiguration;

/// How often the bandwidth dispatcher credits per-torrent `uploaded`
/// counters. Java uses 1s — keep parity.
const BANDWIDTH_TICK_PERIOD: Duration = Duration::from_secs(1);

/// HTTP timeout for tracker announces. Matches the Java-side default that
/// would otherwise keep a dead tracker connection dangling indefinitely.
const TRACKER_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Capacity of the merger-poke mailbox. Pokes are coalesced by the merger
/// (it rebuilds the whole snapshot on every wake-up), so a full queue is
/// safe to drop — 64 is well above the worst-case burst on a fresh start.
const MERGER_POKE_CAPACITY: usize = 64;

/// Resolves the public IP address reported to trackers.
///
/// The default implementation ([`DefaultIpResolver`]) polls third-party
/// HTTP providers. Tests can inject a fixed IP or `None`.
pub trait IpResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        proxy_url: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<IpAddr>> + Send + 'a>>;
}

/// Default IP resolver that polls external HTTP providers.
pub struct DefaultIpResolver;

impl IpResolver for DefaultIpResolver {
    fn resolve<'a>(
        &'a self,
        proxy_url: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<IpAddr>> + Send + 'a>> {
        Box::pin(fetch_public_ip(proxy_url))
    }
}

/// Optional overrides for engine construction. Pass to
/// [`SeedManager::start_with`] to inject test doubles.
pub struct EngineOptions {
    pub ip_resolver: Box<dyn IpResolver>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            ip_resolver: Box::new(DefaultIpResolver),
        }
    }
}

/// Composition root.
///
/// Owned by `joal-app` (CLI or egui front-end). Construct via
/// [`SeedManager::start`], drop (or call [`SeedManager::stop`]) to tear down.
pub struct SeedManager {
    events: BroadcastSink,
    snapshot_rx: watch::Receiver<EngineSnapshot>,
    orchestrator: Arc<ClientOrchestrator>,
    torrent_provider: Arc<TorrentFileProvider>,
    bandwidth: Arc<BandwidthDispatcher>,
    merger_poke_tx: mpsc::Sender<MergerPoke>,
    /// Persistent UI flags ([initial completed] and friends). Held here so
    /// the EngineCommand handler (PR3) can mutate them without re-reading
    /// from disk; the field is read from the merger deps via `Arc::clone`.
    state_store: Arc<crate::torrent::TorrentStateStore>,
    merger: Option<JoinHandle<()>>,
    merger_shutdown: Option<oneshot::Sender<()>>,
    active_client_filename: String,
    folders: JoalFolders,
}

impl SeedManager {
    /// Boot the engine with default options. Convenience wrapper around
    /// [`SeedManager::start_with`].
    #[allow(clippy::too_many_lines)]
    pub async fn start(joal_conf: &std::path::Path) -> Result<Self> {
        Self::start_with(joal_conf, EngineOptions::default()).await
    }

    /// Boot every piece of `joal-core` from a `joal-conf/` directory using
    /// the provided options (IP resolver, etc.).
    #[allow(clippy::too_many_lines)]
    pub async fn start_with(joal_conf: &std::path::Path, options: EngineOptions) -> Result<Self> {
        let (app_config, folders) = config::load(joal_conf)
            .await
            .with_context(|| format!("failed to load joal-conf from {}", joal_conf.display()))?;
        info!(
            target: "joal_core::seed_manager",
            min_upload_rate = app_config.min_upload_rate,
            max_upload_rate = app_config.max_upload_rate,
            simultaneous_seed = app_config.simultaneous_seed,
            upload_ratio_target = app_config.upload_ratio_target,
            active_client = %app_config.client,
            "loaded config.json",
        );

        let active_client_filename = app_config.client.clone();
        let client = Arc::new(load_active_client(&folders, &active_client_filename).await?);

        let (poke_tx, poke_rx) = mpsc::channel::<MergerPoke>(MERGER_POKE_CAPACITY);

        let bandwidth = BandwidthDispatcher::new(
            BANDWIDTH_TICK_PERIOD,
            RandomSpeedProvider::new(&app_config),
            DownloadSpeedProvider::new(&app_config),
        );
        let bandwidth = Arc::new(bandwidth);
        bandwidth
            .start()
            .context("failed to start bandwidth dispatcher")?;
        bandwidth.set_merger_poke(Some(poke_tx.clone()));

        let torrent_provider = TorrentFileProvider::new(&folders)
            .context("failed to initialise torrent file provider")?;
        torrent_provider
            .start()
            .await
            .context("failed to start torrent file watcher")?;

        let events = BroadcastSink::default();
        let events_trait: Arc<dyn EngineEventSink> = Arc::new(events.clone());

        // Publish ConfigLoaded *after* subscribing-capable sink exists but
        // *before* the merger task starts: the merger keeps the active-client
        // filename cached and the egui ViewModel's very first `.changed()`
        // wake-up should reflect the loaded settings.
        events_trait.publish(EngineEvent::ConfigLoaded {
            config: app_config.clone(),
        });

        let mut connection = ConnectionHandler::with_ephemeral_port()
            .unwrap_or_else(|_| ConnectionHandler::with_port_only(51413));

        let proxy_url = app_config.proxy_url();
        let ip = options.ip_resolver.resolve(proxy_url.as_deref()).await;
        if let Some(addr) = ip {
            info!(
                target: "joal_core::seed_manager",
                ip = %addr,
                port = connection.port(),
                "IP reported to tracker",
            );
            connection.set_ip_address(Some(addr));
        } else {
            warn!(
                target: "joal_core::seed_manager",
                "failed to fetch public IP, tracker will not receive IP",
            );
        }
        let connection = Arc::new(connection);

        let data_accessor = AnnounceDataAccessor::new(
            Arc::clone(&client),
            Arc::clone(&bandwidth),
            Arc::clone(&connection),
        );
        let mut http_builder = reqwest::Client::builder().timeout(TRACKER_HTTP_TIMEOUT);
        if let Some(proxy_url) = app_config.proxy_url() {
            info!(
                target: "joal_core::seed_manager",
                proxy = %proxy_url,
                "HTTP client configured with proxy",
            );
            let proxy = reqwest::Proxy::all(&proxy_url).context("failed to parse proxy URL")?;
            http_builder = http_builder.proxy(proxy);
        }
        let http = http_builder
            .build()
            .context("failed to build reqwest HTTP client")?;
        let factory = AnnouncerFactory::new(data_accessor, http, app_config.upload_ratio_target);

        let client_name = derive_client_name(&client);
        let state_store = crate::torrent::TorrentStateStore::load(&folders).await;
        let orchestrator = ClientOrchestrator::new(
            app_config,
            Arc::clone(&torrent_provider),
            Arc::clone(&bandwidth),
            factory,
            &events_trait,
            Some(poke_tx.clone()),
            Arc::clone(&state_store),
        );
        orchestrator
            .start()
            .await
            .context("failed to start client orchestrator")?;

        events_trait.publish(EngineEvent::GlobalSeedStarted { client_name });

        // Snapshot publication channel. The initial frame carries the active
        // client filename so subscribers that attach mid-session always have
        // something useful to render.
        let initial = EngineSnapshot {
            active_client_filename: active_client_filename.clone(),
            ..EngineSnapshot::default()
        };
        let (snapshot_tx, snapshot_rx) = watch::channel(initial);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let merger_handle = spawn_merger(
            MergerDeps {
                orchestrator: Arc::clone(&orchestrator),
                bandwidth: Arc::clone(&bandwidth),
                state_store: Arc::clone(&state_store),
                active_client_filename: active_client_filename.clone(),
            },
            events.subscribe(),
            poke_rx,
            shutdown_rx,
            snapshot_tx,
        );

        Ok(Self {
            events,
            snapshot_rx,
            orchestrator,
            torrent_provider,
            bandwidth,
            merger_poke_tx: poke_tx,
            state_store,
            merger: Some(merger_handle),
            merger_shutdown: Some(shutdown_tx),
            active_client_filename,
            folders,
        })
    }

    /// Subscribe to engine transitions. Every call returns a fresh receiver;
    /// the underlying broadcast channel drops older events once the ring
    /// buffer fills.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    /// Clone of the latest snapshot frame. For step-by-step polling. Most
    /// consumers should use [`SeedManager::snapshot_watch`] instead.
    #[must_use]
    pub fn snapshot(&self) -> EngineSnapshot {
        self.snapshot_rx.borrow().clone()
    }

    /// Watch receiver for snapshot frames. Use `.changed().await` to wait
    /// for the next frame, then `.borrow()` to read it.
    #[must_use]
    pub fn snapshot_watch(&self) -> watch::Receiver<EngineSnapshot> {
        self.snapshot_rx.clone()
    }

    /// Filename of the active `.client` loaded at boot.
    #[must_use]
    pub fn active_client_filename(&self) -> &str {
        &self.active_client_filename
    }

    /// The folder layout used by this engine instance.
    #[must_use]
    pub fn folders(&self) -> &JoalFolders {
        &self.folders
    }

    /// Read-only access to the persistent UI flag store.
    #[must_use]
    pub fn state_store(&self) -> &Arc<crate::torrent::TorrentStateStore> {
        &self.state_store
    }

    /// Pull the next regular announce for every live torrent forward to now.
    pub fn announce_all_now(&self) {
        if self
            .merger_poke_tx
            .try_send(MergerPoke::ImmediateAnnounceAll)
            .is_err()
        {
            self.orchestrator.announce_all_now();
        }
    }

    /// Persist and apply the UI's "initial completed" checkbox.
    pub async fn set_torrent_initial_completed(
        &self,
        info_hash: &crate::torrent::InfoHash,
        completed: bool,
    ) -> std::result::Result<(), crate::torrent::StateStoreError> {
        self.state_store
            .set_initial_completed(info_hash, completed)
            .await?;
        let flipped_to_done = self.bandwidth.force_initial_completed(info_hash, completed);
        if completed
            && flipped_to_done
            && self
                .merger_poke_tx
                .try_send(MergerPoke::TorrentCompleted(info_hash.clone()))
                .is_err()
        {
            self.orchestrator.mark_completed_pending(info_hash);
        }
        Ok(())
    }

    /// Move a torrent to the archive folder by info-hash.
    pub async fn delete_torrent(
        &self,
        info_hash: &crate::torrent::InfoHash,
    ) -> std::result::Result<(), crate::torrent::StateStoreError> {
        self.torrent_provider
            .move_to_archive_folder(info_hash)
            .await;
        self.state_store.forget(info_hash).await
    }

    /// Tear down every spawned task in reverse boot order.
    ///
    /// Safe to call at most once. Idempotent-ish: a second call is a no-op
    /// (every handle slot is `Option::take`'d).
    pub async fn stop(&mut self) {
        if let Err(e) = self.orchestrator.stop().await {
            warn!(
                target: "joal_core::seed_manager",
                error = %e,
                "orchestrator.stop() reported",
            );
        }
        self.torrent_provider.stop().await;
        // Dispatcher::stop consumes &mut self, but ours lives behind Arc; the
        // spawned tick task aborts on drop, which is exactly what we want
        // once the last Arc clone is released. Clearing the merger poke hook
        // first drops its mpsc::Sender so any in-flight try_send stays safe.
        self.bandwidth.set_merger_poke(None);

        if let Some(tx) = self.merger_shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.merger.take() {
            let _ = handle.await;
        }
        self.events.publish(EngineEvent::GlobalSeedStopped);
        info!(target: "joal_core::seed_manager", "seed manager stopped");
    }
}

impl Drop for SeedManager {
    fn drop(&mut self) {
        // If the caller forgot `.stop().await`, abort the merger so the
        // tokio runtime can exit. Everything else (orchestrator tick,
        // bandwidth tick, watcher) aborts on its own `Arc` drop.
        if let Some(handle) = self.merger.take() {
            handle.abort();
        }
    }
}

impl std::fmt::Debug for SeedManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeedManager")
            .field("active_client_filename", &self.active_client_filename)
            .field("merger_running", &self.merger.is_some())
            .finish_non_exhaustive()
    }
}

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

/// Derive a human-readable client name for `GlobalSeedStarted`. The
/// `BitTorrentClient` stores headers verbatim; use the `User-Agent` header
/// if present, else fall back to the configured filename (resolved by the
/// caller).
fn derive_client_name(client: &BitTorrentClient) -> String {
    client
        .headers()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("User-Agent"))
        .map_or_else(|| "unknown".to_owned(), |(_, v)| v.clone())
}

struct MergerDeps {
    orchestrator: Arc<ClientOrchestrator>,
    bandwidth: Arc<BandwidthDispatcher>,
    state_store: Arc<crate::torrent::TorrentStateStore>,
    active_client_filename: String,
}

fn spawn_merger(
    deps: MergerDeps,
    mut events_rx: broadcast::Receiver<EngineEvent>,
    mut poke_rx: mpsc::Receiver<MergerPoke>,
    mut shutdown: oneshot::Receiver<()>,
    snapshot_tx: watch::Sender<EngineSnapshot>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Publish once up-front so consumers that attached between the
        // channel creation and the first wake-up see a non-default frame.
        publish_snapshot(&deps, &snapshot_tx);
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    debug!(target: "joal_core::seed_manager::merger", "shutdown received");
                    break;
                }
                poke = poke_rx.recv() => match poke {
                    Some(MergerPoke::SpeedRecomputed | MergerPoke::AnnouncerUpdated) => {
                        publish_snapshot(&deps, &snapshot_tx);
                    }
                    Some(MergerPoke::TorrentCompleted(info_hash)) => {
                        deps.orchestrator.mark_completed_pending(&info_hash);
                        publish_snapshot(&deps, &snapshot_tx);
                    }
                    Some(MergerPoke::ImmediateAnnounceAll) => {
                        deps.orchestrator.announce_all_now();
                    }
                    None => {
                        debug!(target: "joal_core::seed_manager::merger", "poke channel closed");
                        break;
                    }
                },
                evt = events_rx.recv() => match evt {
                    Ok(event) if affects_snapshot(&event) => {
                        publish_snapshot(&deps, &snapshot_tx);
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            target: "joal_core::seed_manager::merger",
                            skipped,
                            "merger lagged behind event bus; forcing a rebuild",
                        );
                        publish_snapshot(&deps, &snapshot_tx);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!(target: "joal_core::seed_manager::merger", "event bus closed");
                        break;
                    }
                },
            }
        }
    })
}

fn affects_snapshot(event: &EngineEvent) -> bool {
    matches!(
        event,
        EngineEvent::TorrentFileAdded { .. }
            | EngineEvent::TorrentFileDeleted { .. }
            | EngineEvent::TooManyAnnouncesFailedInARow { .. }
            | EngineEvent::GlobalSeedStarted { .. }
            | EngineEvent::GlobalSeedStopped
    )
}

fn publish_snapshot(deps: &MergerDeps, snapshot_tx: &watch::Sender<EngineSnapshot>) {
    let frame = build_snapshot(deps);
    // `send_replace` always replaces, even if no subscribers — exactly what
    // we want for a watch channel projecting state.
    snapshot_tx.send_replace(frame);
}

fn build_snapshot(deps: &MergerDeps) -> EngineSnapshot {
    let speeds = deps.bandwidth.speed_map();
    let download_speeds = deps.bandwidth.download_speed_map();
    let announcers = deps.orchestrator.announcers_snapshot();

    let mut torrents = Vec::with_capacity(announcers.len());
    let mut global_bps: u64 = 0;
    let mut global_dl_bps: u64 = 0;
    for announcer in &announcers {
        let snap = announcer.facade_snapshot();
        let current_speed_bps = speeds
            .get(&snap.torrent_info_hash)
            .map_or(0, crate::bandwidth::Speed::bytes_per_second);
        let current_download_speed_bps = download_speeds
            .get(&snap.torrent_info_hash)
            .map_or(0, crate::bandwidth::Speed::bytes_per_second);
        global_bps = global_bps.saturating_add(current_speed_bps);
        global_dl_bps = global_dl_bps.saturating_add(current_download_speed_bps);
        let stats = deps
            .bandwidth
            .get_seed_stat_for_torrent(&snap.torrent_info_hash);
        let initial_completed = deps
            .state_store
            .is_initial_completed(&snap.torrent_info_hash);
        torrents.push(TorrentStatus {
            info_hash: snap.torrent_info_hash,
            name: snap.torrent_name,
            total_size: snap.torrent_size,
            uploaded_bytes: stats.uploaded(),
            downloaded_bytes: stats.downloaded(),
            left_bytes: stats.left(),
            current_speed_bps,
            current_download_speed_bps,
            initial_completed,
            last_known_interval: to_u32(snap.last_known_interval),
            last_known_seeders: snap.last_known_seeders.and_then(to_u32),
            last_known_leechers: snap.last_known_leechers.and_then(to_u32),
            consecutive_fails: snap.consecutive_fails,
            last_announced_at: snap.last_announced_at,
        });
    }

    EngineSnapshot {
        active_client_filename: deps.active_client_filename.clone(),
        global_upload_speed_bps: global_bps,
        global_download_speed_bps: global_dl_bps,
        torrents,
    }
}

fn to_u32(value: i32) -> Option<u32> {
    if value < 0 { None } else { Some(value as u32) }
}
