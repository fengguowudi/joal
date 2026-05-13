//! Integration tests for [`joal_core::seed_manager::SeedManager`].
//!
//! Verifies the glue — not the individual collaborators (those have their
//! own tests under `src/`).
//!
//! 1. `start` boots against an on-disk `joal-conf/` fixture, subscribes land
//!    the initial snapshot, and the filename projection is correct.
//! 2. Add-torrent / announce round-trip pushes a fresh snapshot frame that
//!    includes the new torrent with a non-default `last_known_interval`.
//! 3. `stop` tears down the merger cleanly and publishes `GlobalSeedStopped`.

use std::sync::Arc;
use std::time::Duration;

use joal_core::events::EngineEvent;
use joal_core::seed_manager::SeedManager;
use joal_testing::sample_client_file;
use tokio::sync::broadcast::error::TryRecvError;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACTIVE_CLIENT_FILENAME: &str = "qbittorrent.client";

fn bencode_ok(interval: i64, complete: i64, incomplete: i64) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(b'd');
    out.extend_from_slice(b"8:complete");
    out.extend_from_slice(format!("i{complete}e").as_bytes());
    out.extend_from_slice(b"10:incomplete");
    out.extend_from_slice(format!("i{incomplete}e").as_bytes());
    out.extend_from_slice(b"8:interval");
    out.extend_from_slice(format!("i{interval}e").as_bytes());
    out.push(b'e');
    out
}

fn build_torrent_bytes(tag: u8, announce_url: &str) -> Vec<u8> {
    let mut pieces = vec![0u8; 20];
    pieces[0] = tag;
    let mut info = Vec::new();
    info.push(b'd');
    info.extend_from_slice(b"6:lengthi10e");
    info.extend_from_slice(b"4:name8:test.bin");
    info.extend_from_slice(b"12:piece lengthi10e");
    info.extend_from_slice(b"6:pieces20:");
    info.extend_from_slice(&pieces);
    info.push(b'e');
    let announce_prefix = format!("{}:", announce_url.len());
    let mut torrent = Vec::new();
    torrent.push(b'd');
    torrent.extend_from_slice(b"8:announce");
    torrent.extend_from_slice(announce_prefix.as_bytes());
    torrent.extend_from_slice(announce_url.as_bytes());
    torrent.extend_from_slice(b"4:info");
    torrent.extend_from_slice(&info);
    torrent.push(b'e');
    torrent
}

struct Fixture {
    _tmp: tempfile::TempDir,
    conf_root: std::path::PathBuf,
    torrents_dir: std::path::PathBuf,
    announce_url: String,
    server: Arc<MockServer>,
}

async fn make_fixture() -> Fixture {
    let server = Arc::new(MockServer::start().await);
    Mock::given(method("GET"))
        .and(path("/announce"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bencode_ok(1800, 2, 3)))
        .mount(&server)
        .await;
    let announce_url = format!("http://{}/announce", server.address());

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let clients_dir = root.join("clients");
    let torrents_dir = root.join("torrents");
    let archive_dir = torrents_dir.join("archived");
    std::fs::create_dir_all(&clients_dir).unwrap();
    std::fs::create_dir_all(&torrents_dir).unwrap();
    std::fs::create_dir_all(&archive_dir).unwrap();

    // Drop the sample `.client` on disk under the expected filename.
    tokio::fs::write(
        clients_dir.join(ACTIVE_CLIENT_FILENAME),
        sample_client_file(),
    )
    .await
    .unwrap();

    // Minimal config.json — keep rates low so the dispatcher never dominates.
    let config_json = format!(
        r#"{{
            "minUploadRate": 0,
            "maxUploadRate": 0,
            "simultaneousSeed": 1,
            "client": "{ACTIVE_CLIENT_FILENAME}",
            "keepTorrentWithZeroLeechers": true,
            "uploadRatioTarget": -1.0
        }}"#
    );
    tokio::fs::write(root.join("config.json"), config_json)
        .await
        .unwrap();

    Fixture {
        _tmp: tmp,
        conf_root: root,
        torrents_dir,
        announce_url,
        server,
    }
}

async fn wait_for_announce(server: &MockServer, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if let Some(reqs) = server.received_requests().await
            && !reqs.is_empty()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn start_publishes_initial_snapshot_with_active_client() {
    let fx = make_fixture().await;
    let mut sm = SeedManager::start(&fx.conf_root).await.expect("start");

    let snapshot = sm.snapshot();
    assert_eq!(snapshot.active_client_filename, ACTIVE_CLIENT_FILENAME);
    assert!(snapshot.torrents.is_empty());
    assert_eq!(snapshot.global_upload_speed_bps, 0);

    sm.stop().await;
}

#[tokio::test]
async fn announce_round_trip_updates_snapshot_frame() {
    let fx = make_fixture().await;
    let mut sm = SeedManager::start(&fx.conf_root).await.expect("start");

    // Drop a torrent into the watched folder *after* the manager is live so
    // the TorrentFileAdded → orchestrator → announce → merger path fires.
    let torrent_bytes = build_torrent_bytes(1, &fx.announce_url);
    tokio::fs::write(fx.torrents_dir.join("sample.torrent"), torrent_bytes)
        .await
        .unwrap();

    // Wait for the stub tracker to see the first announce.
    let hit = wait_for_announce(&fx.server, Duration::from_secs(10)).await;
    assert!(hit, "expected an announce to reach the stub tracker");

    // Pull frames until one reflects the registered torrent (the merger may
    // publish several intermediate frames: add → speed-recompute → announce).
    let mut rx = sm.snapshot_watch();
    let mut saw_torrent = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = rx.borrow_and_update().clone();
        if !frame.torrents.is_empty() {
            assert_eq!(frame.torrents.len(), 1);
            let t = &frame.torrents[0];
            assert_eq!(t.name, "test.bin");
            assert_eq!(t.total_size, 10);
            // Announce returned interval=1800; the facade carries it after
            // the first successful round-trip.
            if t.last_known_interval == Some(1800) {
                saw_torrent = true;
                break;
            }
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::select! {
            _ = rx.changed() => {}
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    assert!(
        saw_torrent,
        "expected a snapshot frame with the registered torrent and tracker interval"
    );

    sm.stop().await;
}

#[tokio::test]
async fn stop_emits_global_seed_stopped() {
    let fx = make_fixture().await;
    let mut sm = SeedManager::start(&fx.conf_root).await.expect("start");
    let mut events = sm.subscribe_events();
    sm.stop().await;

    let mut saw_stopped = false;
    for _ in 0..64 {
        match events.try_recv() {
            Ok(EngineEvent::GlobalSeedStopped) => {
                saw_stopped = true;
                break;
            }
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
        }
    }
    assert!(saw_stopped, "expected GlobalSeedStopped on the event bus");
}
