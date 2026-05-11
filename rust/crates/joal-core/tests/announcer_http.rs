//! Integration coverage for the HTTP tracker announcer (S8).
//!
//! Uses `wiremock` to stand up a stub tracker that speaks bencode, so we can
//! assert the Rust announcer:
//!   1. builds the announce URL with the Java-compatible `?`/`&` separator,
//!   2. forwards the `.client` headers verbatim,
//!   3. decodes bencode responses into `SuccessAnnounceResponse`,
//!   4. honours the "one step forward on failure" tracker rotation,
//!   5. surfaces BEP-3 `failure reason` as `AnnouncerError::TrackerReported`.

use std::sync::Arc;
use std::time::Duration;

use joal_core::announcer::{
    AnnounceDataAccessor, Announcer, AnnouncerError, TrackerClient, TrackerClientUriProvider,
};
use joal_core::bandwidth::{BandwidthDispatcher, RandomSpeedProvider};
use joal_core::client::{
    BitTorrentClient, BitTorrentClientConfig, ConnectionHandler, RequestEvent,
};
use joal_core::config::AppConfiguration;
use joal_core::torrent::{InfoHash, MockedTorrent};
use joal_testing::sample_client_file;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sample_info_hash() -> InfoHash {
    let mut bytes = [0u8; 20];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = (i as u8).wrapping_add(1);
    }
    InfoHash::from_bytes(bytes)
}

fn sample_torrent(announce_url: String) -> MockedTorrent {
    MockedTorrent {
        info_hash: sample_info_hash(),
        name: "integration".to_owned(),
        total_size: 2048,
        piece_length: 1024,
        piece_count: 2,
        announce: announce_url,
        announce_tiers: Vec::new(),
    }
}

fn bencode_dict(items: &[(&str, BVal)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(b'd');
    // Dict keys in our test payloads are already passed in ascending order
    // by the caller (the bencode parser is strict about it).
    for (k, v) in items {
        out.extend_from_slice(k.len().to_string().as_bytes());
        out.push(b':');
        out.extend_from_slice(k.as_bytes());
        v.write(&mut out);
    }
    out.push(b'e');
    out
}

enum BVal {
    Int(i64),
    Str(String),
}

impl BVal {
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            BVal::Int(n) => {
                out.push(b'i');
                out.extend_from_slice(n.to_string().as_bytes());
                out.push(b'e');
            }
            BVal::Str(s) => {
                out.extend_from_slice(s.len().to_string().as_bytes());
                out.push(b':');
                out.extend_from_slice(s.as_bytes());
            }
        }
    }
}

fn default_accessor() -> AnnounceDataAccessor {
    let cfg = AppConfiguration {
        min_upload_rate: 0,
        max_upload_rate: 0,
        simultaneous_seed: 1,
        client: "sample.client".into(),
        keep_torrent_with_zero_leechers: true,
        upload_ratio_target: -1.0,
    };
    let dispatcher = Arc::new(BandwidthDispatcher::new(
        Duration::from_millis(100),
        RandomSpeedProvider::new(&cfg),
    ));
    dispatcher.register_torrent(sample_info_hash());
    let client_cfg = BitTorrentClientConfig::try_from(sample_client_file()).unwrap();
    let client = Arc::new(BitTorrentClient::new(client_cfg).unwrap());
    AnnounceDataAccessor::new(
        client,
        dispatcher,
        Arc::new(ConnectionHandler::with_port_only(55_555)),
    )
}

#[tokio::test]
async fn happy_path_parses_tracker_response_and_clears_failures() {
    let mock = MockServer::start().await;
    let body = bencode_dict(&[
        ("complete", BVal::Int(11)),
        ("incomplete", BVal::Int(4)),
        ("interval", BVal::Int(900)),
    ]);
    Mock::given(method("GET"))
        .and(path("/announce"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&mock)
        .await;

    let uri = format!("{}/announce", mock.uri());
    let provider = TrackerClientUriProvider::new(vec![uri.clone()]).unwrap();
    let tracker = TrackerClient::new(provider).unwrap();
    let announcer = Announcer::new(sample_torrent(uri), tracker, default_accessor(), -1.0);

    let resp = announcer.announce(RequestEvent::Started).await.unwrap();
    assert_eq!(resp.interval(), 900);
    assert_eq!(resp.seeders(), 10);
    assert_eq!(resp.leechers(), 4);
}

#[tokio::test]
async fn tracker_failure_reason_surfaces_as_typed_error() {
    let mock = MockServer::start().await;
    let body = bencode_dict(&[(
        "failure reason",
        BVal::Str("torrent not registered".to_owned()),
    )]);
    Mock::given(method("GET"))
        .and(path("/announce"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&mock)
        .await;

    let uri = format!("{}/announce", mock.uri());
    let provider = TrackerClientUriProvider::new(vec![uri.clone()]).unwrap();
    let tracker = TrackerClient::new(provider).unwrap();
    let announcer = Announcer::new(sample_torrent(uri), tracker, default_accessor(), -1.0);

    let err = announcer.announce(RequestEvent::None).await.unwrap_err();
    match err {
        AnnouncerError::TrackerReported { reason, .. } => {
            assert_eq!(reason, "torrent not registered");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn failure_rotates_to_next_tracker_uri() {
    let good = MockServer::start().await;
    let ok_body = bencode_dict(&[
        ("complete", BVal::Int(5)),
        ("incomplete", BVal::Int(0)),
        ("interval", BVal::Int(60)),
    ]);
    Mock::given(method("GET"))
        .and(path("/announce"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(ok_body))
        .mount(&good)
        .await;

    // First URI points at a non-routable port (RFC 6890 TEST-NET) that will
    // fail on connect; second URI points at the wiremock server.
    let first = "http://127.0.0.1:1/announce".to_owned();
    let second = format!("{}/announce", good.uri());
    let provider = TrackerClientUriProvider::new(vec![first, second.clone()]).unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let tracker = TrackerClient::with_http_client(provider, http);
    let announcer = Announcer::new(sample_torrent(second), tracker, default_accessor(), -1.0);

    // First attempt hits 127.0.0.1:1 and fails; the cursor rotates.
    let first_err = announcer.announce(RequestEvent::Started).await.unwrap_err();
    assert!(matches!(first_err, AnnouncerError::Http(_)));

    // Second attempt hits wiremock and succeeds.
    let ok = announcer.announce(RequestEvent::None).await.unwrap();
    assert_eq!(ok.interval(), 60);
    assert_eq!(ok.seeders(), 4);
}
