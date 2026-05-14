use std::fs;
use std::path::PathBuf;

use joal_core::client::{
    BitTorrentClientConfig, Casing, HashNoLeadingZeroKeyAlgorithm, KeyAlgorithmDef, KeyConfig,
    KeyGenerator, PeerIdAlgorithmDef, PeerIdConfig, PeerIdGenerator, RegexPeerIdAlgorithm,
};
mod common;
use common::sample_client_file;

#[test]
fn qbittorrent_4_5_0_golden_parses_from_embedded_repository_file() {
    let cfg = BitTorrentClientConfig::try_from(sample_client_file()).unwrap();

    assert_eq!(
        cfg.query,
        "info_hash={infohash}&peer_id={peerid}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&corrupt=0&key={key}&event={event}&numwant={numwant}&compact=1&no_peer_id=1&supportcrypto=1&redundant=0"
    );
    assert_eq!(cfg.numwant, 200);
    assert_eq!(cfg.numwant_on_stop, 0);

    assert!(matches!(
        cfg.peer_id_generator,
        PeerIdGenerator::NEVER {
            config: PeerIdConfig {
                algorithm: PeerIdAlgorithmDef::REGEX(RegexPeerIdAlgorithm { pattern }),
                should_url_encode: false,
            },
            ..
        } if pattern == "-qB4500-[A-Za-z0-9_~\\(\\)\\!\\.\\*-]{12}"
    ));

    assert!(matches!(
        cfg.key_generator,
        Some(KeyGenerator::TORRENT_PERSISTENT {
            config: KeyConfig {
                algorithm: KeyAlgorithmDef::HASH_NO_LEADING_ZERO(HashNoLeadingZeroKeyAlgorithm {
                    length: 8
                }),
                key_case: Casing::Upper,
            },
            ..
        })
    ));

    assert_eq!(
        cfg.url_encoder.encoding_exclusion_pattern(),
        r"[A-Za-z0-9_~\(\)\!\.\*-]"
    );
    assert_eq!(cfg.url_encoder.encoded_hex_case(), Casing::Lower);

    assert_eq!(cfg.request_headers.len(), 3);
    assert_eq!(cfg.request_headers[0].name, "User-Agent");
    assert_eq!(cfg.request_headers[0].value, "qBittorrent/4.5.0");
    assert_eq!(cfg.request_headers[1].name, "Accept-Encoding");
    assert_eq!(cfg.request_headers[1].value, "gzip");
    assert_eq!(cfg.request_headers[2].name, "Connection");
    assert_eq!(cfg.request_headers[2].value, "close");
}

#[test]
fn all_repository_client_files_parse_with_java_compatible_tags() {
    let clients_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/clients");

    let entries = fs::read_dir(&clients_dir).unwrap();
    let mut parsed = 0usize;
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("client") {
            continue;
        }
        let contents = fs::read_to_string(&path).unwrap();
        let config = BitTorrentClientConfig::try_from(contents.as_str())
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        assert!(
            !config.query.is_empty(),
            "{} has an empty query",
            path.display()
        );
        parsed += 1;
    }

    assert!(parsed > 0, "no repository .client files were parsed");
}
