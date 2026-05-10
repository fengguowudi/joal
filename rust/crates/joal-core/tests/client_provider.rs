use joal_core::client::BitTorrentClientProvider;

const SAMPLE_CLIENT_JSON: &str = r#"{
    "peerIdGenerator": {
        "refreshOn": "NEVER",
        "algorithm": {"type": "REGEX", "pattern": "-qB4500-[A-Za-z]{12}"},
        "shouldUrlEncode": false
    },
    "keyGenerator": {
        "refreshOn": "ALWAYS",
        "algorithm": {"type": "HASH_NO_LEADING_ZERO", "length": 8},
        "keyCase": "upper"
    },
    "urlEncoder": {
        "encodingExclusionPattern": "[A-Za-z0-9_~\\(\\)\\!\\.\\*-]",
        "encodedHexCase": "lower"
    },
    "query": "info_hash={infohash}&peer_id={peerid}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&key={key}&event={event}&numwant={numwant}",
    "requestHeaders": [
        {"name": "User-Agent", "value": "qBittorrent/4.5.0"}
    ],
    "numwant": 200,
    "numwantOnStop": 0
}"#;

#[tokio::test]
async fn list_and_load_round_trip_through_semver_sort() {
    let tmp = tempfile::tempdir().unwrap();
    for name in [
        "qbittorrent-4.5.0.client",
        "qbittorrent-4.4.0.client",
        "utorrent-3.5.0_43916.client",
        "utorrent-3.5.0_44090.client",
        "notes.txt",
    ] {
        tokio::fs::write(tmp.path().join(name), SAMPLE_CLIENT_JSON)
            .await
            .unwrap();
    }

    let provider = BitTorrentClientProvider::new(tmp.path());
    let listing = provider.list_client_files().await.unwrap();
    assert_eq!(
        listing,
        vec![
            "qbittorrent-4.4.0.client".to_owned(),
            "qbittorrent-4.5.0.client".to_owned(),
            "utorrent-3.5.0_43916.client".to_owned(),
            "utorrent-3.5.0_44090.client".to_owned(),
        ]
    );

    let client = provider.load("qbittorrent-4.5.0.client").await.unwrap();
    assert!(client.query().contains("{infohash}"));
    assert_eq!(client.headers().len(), 1);
    assert_eq!(client.headers()[0].0, "User-Agent");
    assert_eq!(client.headers()[0].1, "qBittorrent/4.5.0");
}
