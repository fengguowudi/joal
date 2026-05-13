//! Generator family used by emulated BitTorrent clients.
//!
//! Mirrors Java `org.araymond.joal.core.client.emulated.generator` and its
//! `key`, `peerid`, `numwant` subpackages.

mod common;
pub mod key;
pub mod numwant;
pub mod peer_id;
pub mod url_encoder;

pub use key::{
    DigitRangeTransformedToHexWithoutLeadingZeroKeyAlgorithm, HashKeyAlgorithm,
    HashNoLeadingZeroKeyAlgorithm, KeyAlgorithmDef, KeyGenerator, RegexKeyAlgorithm,
};
pub use numwant::NumwantProvider;
pub use peer_id::{
    PEER_ID_LENGTH, PeerIdAlgorithmDef, PeerIdGenerator, RandomPoolWithChecksumPeerIdAlgorithm,
    RegexPeerIdAlgorithm,
};
pub use url_encoder::UrlEncoder;
