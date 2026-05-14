use std::net::{IpAddr, Ipv4Addr};

use joal_core::bandwidth::TorrentSeedStats;
use joal_core::client::{
    BitTorrentClient, BitTorrentClientConfig, ConnectionHandler, RequestEvent,
};
use joal_core::torrent::InfoHash;
mod common;
use common::sample_client_file;
use regex::Regex;

fn sample_info_hash() -> InfoHash {
    let mut bytes = [0u8; 20];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = i as u8;
    }
    InfoHash::from_bytes(bytes)
}

fn sample_connection() -> ConnectionHandler {
    ConnectionHandler::new(55555, IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)))
}

fn build_client() -> BitTorrentClient {
    let cfg = BitTorrentClientConfig::try_from(sample_client_file()).unwrap();
    BitTorrentClient::new(cfg).unwrap()
}

#[test]
fn qbittorrent_started_query_has_all_expected_fragments() {
    let client = build_client();
    let stats = TorrentSeedStats::new(12_345, 6_789, 0);
    let query = client
        .create_request_query(
            RequestEvent::Started,
            &sample_info_hash(),
            &stats,
            &sample_connection(),
        )
        .unwrap();

    assert!(
        query.contains("info_hash=%00%01%02%03%04%05%06%07%08%09%0a%0b%0c%0d%0e%0f%10%11%12%13")
    );
    assert!(query.contains("event=started"));
    assert!(query.contains("port=55555"));
    assert!(query.contains("uploaded=12345"));
    assert!(query.contains("downloaded=6789"));
    assert!(query.contains("left=0"));
    assert!(query.contains("numwant=200"));
    assert!(query.contains("peer_id=-qB4500-"));
    assert!(query.contains("key="));

    let leftover = Regex::new(r"\{[^}]*\}").unwrap();
    assert!(
        !leftover.is_match(&query),
        "unresolved placeholder leaked into announce query: {query}"
    );
}

#[test]
fn stopped_event_switches_to_numwant_on_stop() {
    let client = build_client();
    let stats = TorrentSeedStats::new(0, 0, 0);
    let query = client
        .create_request_query(
            RequestEvent::Stopped,
            &sample_info_hash(),
            &stats,
            &sample_connection(),
        )
        .unwrap();
    assert!(query.contains("event=stopped"));
    assert!(query.contains("numwant=0"));
}

#[test]
fn none_event_drops_event_kv_pair_entirely() {
    let client = build_client();
    let stats = TorrentSeedStats::new(0, 0, 0);
    let query = client
        .create_request_query(
            RequestEvent::None,
            &sample_info_hash(),
            &stats,
            &sample_connection(),
        )
        .unwrap();
    assert!(
        !query.contains("event="),
        "event= should not appear: {query}"
    );
    assert!(query.contains("numwant=200"));
}

#[test]
fn completed_event_is_present_verbatim() {
    let client = build_client();
    let query = client
        .create_request_query(
            RequestEvent::Completed,
            &sample_info_hash(),
            &TorrentSeedStats::new(1, 2, 3),
            &sample_connection(),
        )
        .unwrap();
    assert!(query.contains("event=completed"));
}
