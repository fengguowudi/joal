//! Generator family used by emulated BitTorrent clients.
//!
//! Mirrors Java `org.araymond.joal.core.client.emulated.generator` and its
//! `key`, `peerid`, `numwant` subpackages.

mod common;
pub mod key;
pub mod numwant;
pub mod peer_id;
pub mod refresh_policy;
pub mod url_encoder;

pub use key::{
    DigitRangeTransformedToHexWithoutLeadingZeroKeyAlgorithm, HashKeyAlgorithm,
    HashNoLeadingZeroKeyAlgorithm, KeyAlgorithmDef, KeyConfig, KeyGenerator, RegexKeyAlgorithm,
};
pub use numwant::NumwantProvider;
pub use peer_id::{
    PEER_ID_LENGTH, PeerIdAlgorithmDef, PeerIdConfig, PeerIdGenerator,
    RandomPoolWithChecksumPeerIdAlgorithm, RegexPeerIdAlgorithm,
};
pub use refresh_policy::{GenerateValue, RefreshPolicy};
pub use url_encoder::UrlEncoder;
