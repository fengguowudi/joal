//! `.client` file configuration types.
//!
//! Port of Java `org.araymond.joal.core.client.emulated.BitTorrentClientConfig`.
//! S4 introduced the static configuration shell; S5 now includes the runtime
//! refresh semantics used by peer-id/key generators as well.

use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::generator::{KeyGenerator, PeerIdGenerator, UrlEncoder};

/// Static configuration loaded from `resources/clients/*.client` or from a
/// user's `joal-conf/clients/*.client` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitTorrentClientConfig {
    #[serde(rename = "peerIdGenerator")]
    pub peer_id_generator: PeerIdGenerator,
    pub query: String,
    #[serde(rename = "keyGenerator")]
    pub key_generator: Option<KeyGenerator>,
    #[serde(rename = "urlEncoder")]
    pub url_encoder: UrlEncoder,
    #[serde(rename = "requestHeaders")]
    pub request_headers: Vec<HttpHeader>,
    pub numwant: i32,
    #[serde(rename = "numwantOnStop")]
    pub numwant_on_stop: i32,
}

impl BitTorrentClientConfig {
    pub fn validate(&self) -> Result<(), ClientError> {
        self.peer_id_generator.validate()?;
        if let Some(key_generator) = &self.key_generator {
            key_generator.validate()?;
        }
        self.url_encoder.validate()?;

        if self.query.contains("{key}") && self.key_generator.is_none() {
            return Err(ClientError::Integrity(
                "Query string contains {key}, but no keyGenerator was found in .client file"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

/// A static HTTP request header emitted by the emulated client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

impl TryFrom<&str> for BitTorrentClientConfig {
    type Error = ClientError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let cfg: Self = serde_json::from_str(value)
            .map_err(|e| ClientError::Integrity(format!("failed to parse .client JSON: {e}")))?;
        cfg.validate()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::generator::{HashNoLeadingZeroKeyAlgorithm, KeyAlgorithmDef, KeyConfig};
    use crate::client::utils::Casing;

    #[test]
    fn rejects_missing_key_generator_when_query_references_key() {
        let json = r#"{
            "peerIdGenerator": {
                "refreshOn": "NEVER",
                "algorithm": {"type": "REGEX", "pattern": "-qB4500-[A-Z]{12}"},
                "shouldUrlEncode": false
            },
            "urlEncoder": {
                "encodingExclusionPattern": "[A-Za-z0-9]",
                "encodedHexCase": "lower"
            },
            "query": "foo={key}",
            "requestHeaders": [],
            "numwant": 200,
            "numwantOnStop": 0
        }"#;

        let cfg: BitTorrentClientConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn accepts_query_without_key_generator_when_key_not_used() {
        let json = r#"{
            "peerIdGenerator": {
                "refreshOn": "NEVER",
                "algorithm": {"type": "REGEX", "pattern": "-qB4500-[A-Z]{12}"},
                "shouldUrlEncode": false
            },
            "urlEncoder": {
                "encodingExclusionPattern": "[A-Za-z0-9]",
                "encodedHexCase": "lower"
            },
            "query": "foo=bar",
            "requestHeaders": [],
            "numwant": 200,
            "numwantOnStop": 0
        }"#;

        let cfg: BitTorrentClientConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn parses_minimal_client_json() {
        let json = r#"{
            "peerIdGenerator": {
                "refreshOn": "NEVER",
                "algorithm": {"type": "REGEX", "pattern": "-qB4500-[A-Z]{12}"},
                "shouldUrlEncode": false
            },
            "keyGenerator": {
                "refreshOn": "ALWAYS",
                "algorithm": {"type": "HASH_NO_LEADING_ZERO", "length": 8},
                "keyCase": "upper"
            },
            "urlEncoder": {
                "encodingExclusionPattern": "[A-Za-z0-9]",
                "encodedHexCase": "lower"
            },
            "query": "info_hash={infohash}&key={key}",
            "requestHeaders": [
                {"name": "User-Agent", "value": "qBittorrent/4.5.0"}
            ],
            "numwant": 200,
            "numwantOnStop": 0
        }"#;

        let cfg = BitTorrentClientConfig::try_from(json).unwrap();
        assert_eq!(cfg.numwant, 200);
        assert_eq!(cfg.numwant_on_stop, 0);
        assert_eq!(cfg.request_headers.len(), 1);
        assert!(matches!(
            cfg.key_generator,
            Some(KeyGenerator::ALWAYS {
                config: KeyConfig {
                    algorithm: KeyAlgorithmDef::HASH_NO_LEADING_ZERO(
                        HashNoLeadingZeroKeyAlgorithm { length: 8 }
                    ),
                    key_case: Casing::Upper,
                },
            })
        ));
    }
}
