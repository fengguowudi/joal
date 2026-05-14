//! Integration tests against the real `.torrent` samples shipped with the
//! Java project. These fixtures are the byte-level compatibility anchor: any
//! future refactor of the bencode scanner or `MockedTorrent` parsing must
//! keep producing the same 20-byte `info_hash` for each of them.
//!
//! Golden hashes were computed independently from a BEP-3 reference
//! implementation, so they double as a protocol-conformance check.

use joal_core::torrent::MockedTorrent;

const FIXTURE_ROOT: &str = "tests/fixtures";

struct Golden {
    file: &'static str,
    info_hash_hex: &'static str,
}

const GOLDENS: &[Golden] = &[
    Golden {
        file: "Audio_20160422_archive.torrent",
        info_hash_hex: "0647f4bbd1de0e60ac9f29c5190072aefc9d86ae",
    },
    Golden {
        file: "Ninja_Heat_160.avi.torrent",
        info_hash_hex: "7e911e0a3dd973282207bd9fb504385a28037c38",
    },
    Golden {
        file: "ubuntu-17.04-desktop-amd64.iso.torrent",
        info_hash_hex: "59066769b9ad42da2e508611c33d7c4480b3857b",
    },
];

#[tokio::test]
async fn info_hash_matches_golden_for_all_fixtures() {
    for g in GOLDENS {
        let path = format!("{}/{}", FIXTURE_ROOT, g.file);
        let torrent = MockedTorrent::from_file(&path)
            .await
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", g.file));

        assert_eq!(
            torrent.info_hash.to_hex(),
            g.info_hash_hex,
            "info_hash mismatch for {}",
            g.file,
        );

        assert!(!torrent.name.is_empty(), "{} produced empty name", g.file);
        assert!(
            torrent.piece_length > 0,
            "{} produced non-positive piece_length",
            g.file
        );
        assert!(
            torrent.piece_count > 0,
            "{} produced zero piece_count",
            g.file
        );
        assert!(
            !torrent.announce.is_empty(),
            "{} produced empty announce",
            g.file
        );
    }
}
