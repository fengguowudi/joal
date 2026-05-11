//! End-to-end orchestrator coverage (S9).
//!
//! Verifies:
//!   1. `ClientOrchestrator::start` boots against a stub tracker and the
//!      first announce arrives (`event=started`).
//!   2. `ClientOrchestrator::stop` drains the pool with `event=stopped`.

use std::sync::Arc;
use std::time::Duration;

use joal_core::announcer::AnnounceDataAccessor;
use joal_core::bandwidth::{BandwidthDispatcher, RandomSpeedProvider};
use joal_core::client::{BitTorrentClient, BitTorrentClientConfig, ConnectionHandler};
use joal_core::config::{AppConfiguration, JoalFolders};
use joal_core::events::NoopSink;
use joal_core::torrent::TorrentFileProvider;
use joal_core::ttorrent_client::{AnnouncerFactory, ClientOrchestrator};
use joal_testing::sample_client_file;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_torrent_bytes(tag: u8, announce_url: &str) -> Vec<u8> {
    // 10-byte file, 10-byte pieces → exactly one 20-byte SHA-1 in the pieces blob.
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

fn bencode_ok(interval: i64, complete: i64, incomplete: i64) -> Vec<u8> {
    // Keys must be ASCII-sorted; complete < incomplete < interval.
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

#[tokio::test]
async fn orchestrator_starts_and_stops_cleanly() {
    // ── Stub tracker that accepts any GET on /announce.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/announce"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bencode_ok(5, 2, 3)))
        .mount(&server)
        .await;
    let announce_url = format!(
        "{address}/announce",
        address = server.address().to_string().trim_end_matches('/')
    );
    let announce_url = format!("http://{announce_url}");

    // ── JoalFolders + a single .torrent file.
    let tmp = tempfile::tempdir().unwrap();
    let folders = JoalFolders::new(tmp.path());
    std::fs::create_dir_all(&folders.torrents_dir).unwrap();
    std::fs::create_dir_all(&folders.clients_dir).unwrap();
    let torrent_path = folders.torrents_dir.join("sample.torrent");
    tokio::fs::write(&torrent_path, build_torrent_bytes(1, &announce_url))
        .await
        .unwrap();

    // ── BitTorrentClient + ConnectionHandler + BandwidthDispatcher.
    let client_cfg = BitTorrentClientConfig::try_from(sample_client_file()).unwrap();
    let client = Arc::new(BitTorrentClient::new(client_cfg).unwrap());
    let connection = Arc::new(ConnectionHandler::with_port_only(51413));

    let app_config = AppConfiguration {
        min_upload_rate: 0,
        max_upload_rate: 0,
        simultaneous_seed: 1,
        client: "x.client".into(),
        keep_torrent_with_zero_leechers: true,
        upload_ratio_target: -1.0,
    };
    let bandwidth = Arc::new(BandwidthDispatcher::new(
        Duration::from_secs(1),
        RandomSpeedProvider::new(&app_config),
    ));

    let data_accessor =
        AnnounceDataAccessor::new(Arc::clone(&client), Arc::clone(&bandwidth), connection);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let factory = AnnouncerFactory::new(data_accessor, http, -1.0);

    // ── Torrent provider.
    let provider = TorrentFileProvider::new(&folders).unwrap();
    provider.start().await.unwrap();

    // ── Orchestrator.
    let events: Arc<dyn joal_core::events::EngineEventSink> = Arc::new(NoopSink);
    let orchestrator = ClientOrchestrator::new(
        app_config,
        Arc::clone(&provider),
        bandwidth,
        factory,
        &events,
    );
    orchestrator.start().await.unwrap();

    // Give the tick loop a moment to drain the queued STARTED request.
    for _ in 0..50 {
        if !server.received_requests().await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests.is_empty(),
        "expected at least one announce to have arrived"
    );
    let first_url = requests[0].url.to_string();
    assert!(
        first_url.contains("event=started"),
        "first announce should be a STARTED: {first_url}"
    );

    orchestrator.stop().await.unwrap();
    provider.stop().await;

    // After stop, the final set of requests must contain a STOPPED announce
    // (Java Client.stop() drains the queue + sends an explicit stop).
    let after_stop = server.received_requests().await.unwrap();
    assert!(
        after_stop
            .iter()
            .any(|r| r.url.query().unwrap_or("").contains("event=stopped")),
        "expected a STOPPED announce after orchestrator.stop()"
    );
}
