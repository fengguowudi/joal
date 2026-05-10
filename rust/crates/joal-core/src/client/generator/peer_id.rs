//! Peer-id generation algorithms and S4 refresh-policy shell.
//!
//! This file ports two Java layers together:
//! - `generator/peerid/generation/*` — the actual generation algorithms.
//! - `generator/peerid/*` — the refresh-policy wrapper (`refreshOn`).
//!
//! Per the task scope, S4 only needs the algorithm layer plus a parseable shell
//! for refresh policies so `.client` files deserialize successfully. The real
//! refresh behaviour beyond `ALWAYS` / `NEVER` will be completed in S5.

use rand::Rng;
use rand_regex::Regex as RandRegex;
use regex_syntax::ParserBuilder;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::event::RequestEvent;

/// Java constant `PeerIdGenerator.PEER_ID_LENGTH`.
pub const PEER_ID_LENGTH: usize = 20;

/// Algorithm used to generate a raw peer-id string.
pub trait PeerIdAlgorithm {
    /// Generate a peer-id using a deterministic or random source.
    fn generate(&self) -> Result<String, ClientError>;
}

fn compile_rand_regex(pattern: &str) -> Result<RandRegex, ClientError> {
    let hir = ParserBuilder::new()
        .build()
        .parse(pattern)
        .map_err(|e| ClientError::InvalidRegex(format!("{pattern}: {e}")))?;
    RandRegex::with_hir(hir, 100).map_err(|e| ClientError::InvalidRegex(format!("{pattern}: {e}")))
}

fn string_from_ascii_regex_bytes(bytes: Vec<u8>) -> Result<String, ClientError> {
    String::from_utf8(bytes).map_err(|e| ClientError::NonUtf8Output(e.to_string()))
}

/// `type = "REGEX"`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegexPeerIdAlgorithm {
    pub pattern: String,
}

impl RegexPeerIdAlgorithm {
    pub fn new(pattern: impl Into<String>) -> Result<Self, ClientError> {
        let algorithm = Self {
            pattern: pattern.into(),
        };
        algorithm.validate()?;
        Ok(algorithm)
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        let _ = compile_rand_regex(&self.pattern)?;
        Ok(())
    }

    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> Result<String, ClientError> {
        let generator = compile_rand_regex(&self.pattern)?;
        let bytes: Vec<u8> = rng.sample(&generator);
        string_from_ascii_regex_bytes(bytes)
    }
}

impl PeerIdAlgorithm for RegexPeerIdAlgorithm {
    fn generate(&self) -> Result<String, ClientError> {
        let mut rng = rand::thread_rng();
        self.generate_with_rng(&mut rng)
    }
}

/// `type = "RANDOM_POOL_WITH_CHECKSUM"`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RandomPoolWithChecksumPeerIdAlgorithm {
    pub prefix: String,
    #[serde(rename = "charactersPool")]
    pub characters_pool: String,
    pub base: usize,
}

impl RandomPoolWithChecksumPeerIdAlgorithm {
    pub fn new(
        prefix: impl Into<String>,
        characters_pool: impl Into<String>,
        base: usize,
    ) -> Result<Self, ClientError> {
        let algorithm = Self {
            prefix: prefix.into(),
            characters_pool: characters_pool.into(),
            base,
        };
        algorithm.validate()?;
        Ok(algorithm)
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        if self.prefix.trim().is_empty() {
            return Err(ClientError::Integrity(
                "peerId algorithm prefix must not be null or empty.".to_owned(),
            ));
        }
        if self.characters_pool.trim().is_empty() {
            return Err(ClientError::Integrity(
                "peerId algorithm charactersPool must not be null or empty.".to_owned(),
            ));
        }
        if self.base == 0 {
            return Err(ClientError::Integrity(
                "peerId algorithm base must not be null.".to_owned(),
            ));
        }
        if self.prefix.len() >= PEER_ID_LENGTH {
            return Err(ClientError::Integrity(format!(
                "peerId algorithm prefix must be shorter than {PEER_ID_LENGTH}."
            )));
        }
        Ok(())
    }

    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> String {
        let suffix_length = PEER_ID_LENGTH - self.prefix.len();
        let random_len = suffix_length.saturating_sub(1);
        let mut random_bytes = vec![0_u8; random_len];
        rng.fill(random_bytes.as_mut_slice());

        let pool: Vec<char> = self.characters_pool.chars().collect();
        let mut suffix = String::with_capacity(suffix_length);
        let mut total = 0usize;

        for byte in random_bytes {
            let mut val = usize::from(byte);
            val %= self.base;
            total += val;
            suffix.push(pool[val]);
        }

        let checksum_idx = if total.is_multiple_of(self.base) {
            0
        } else {
            self.base - (total % self.base)
        };
        suffix.push(pool[checksum_idx]);

        format!("{}{}", self.prefix, suffix)
    }
}

impl PeerIdAlgorithm for RandomPoolWithChecksumPeerIdAlgorithm {
    fn generate(&self) -> Result<String, ClientError> {
        let mut rng = rand::thread_rng();
        Ok(self.generate_with_rng(&mut rng))
    }
}

/// Serde dispatch matching Java `@JsonTypeInfo(property = "type")`.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PeerIdAlgorithmDef {
    REGEX(RegexPeerIdAlgorithm),
    RANDOM_POOL_WITH_CHECKSUM(RandomPoolWithChecksumPeerIdAlgorithm),
}

impl PeerIdAlgorithmDef {
    pub fn validate(&self) -> Result<(), ClientError> {
        match self {
            PeerIdAlgorithmDef::REGEX(inner) => inner.validate(),
            PeerIdAlgorithmDef::RANDOM_POOL_WITH_CHECKSUM(inner) => inner.validate(),
        }
    }

    pub fn generate(&self) -> Result<String, ClientError> {
        self.validate()?;
        match self {
            PeerIdAlgorithmDef::REGEX(inner) => inner.generate(),
            PeerIdAlgorithmDef::RANDOM_POOL_WITH_CHECKSUM(inner) => inner.generate(),
        }
    }
}

/// Parseable shell for Java `PeerIdGenerator` refresh wrappers.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "refreshOn")]
pub enum PeerIdGenerator {
    NEVER {
        algorithm: PeerIdAlgorithmDef,
        #[serde(rename = "shouldUrlEncode")]
        should_url_encode: bool,
    },
    ALWAYS {
        algorithm: PeerIdAlgorithmDef,
        #[serde(rename = "shouldUrlEncode")]
        should_url_encode: bool,
    },
    TIMED {
        #[serde(rename = "refreshEvery")]
        refresh_every: i32,
        algorithm: PeerIdAlgorithmDef,
        #[serde(rename = "shouldUrlEncode")]
        should_url_encode: bool,
    },
    TORRENT_VOLATILE {
        algorithm: PeerIdAlgorithmDef,
        #[serde(rename = "shouldUrlEncode")]
        should_url_encode: bool,
    },
    TORRENT_PERSISTENT {
        algorithm: PeerIdAlgorithmDef,
        #[serde(rename = "shouldUrlEncode")]
        should_url_encode: bool,
    },
}

impl PeerIdGenerator {
    pub fn validate(&self) -> Result<(), ClientError> {
        self.algorithm().validate()?;
        if let PeerIdGenerator::TIMED { refresh_every, .. } = self
            && *refresh_every < 1
        {
            return Err(ClientError::Integrity(
                "refreshEvery must be greater than 0".to_owned(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn algorithm(&self) -> &PeerIdAlgorithmDef {
        match self {
            PeerIdGenerator::NEVER { algorithm, .. }
            | PeerIdGenerator::ALWAYS { algorithm, .. }
            | PeerIdGenerator::TIMED { algorithm, .. }
            | PeerIdGenerator::TORRENT_VOLATILE { algorithm, .. }
            | PeerIdGenerator::TORRENT_PERSISTENT { algorithm, .. } => algorithm,
        }
    }

    #[must_use]
    pub const fn should_url_encode(&self) -> bool {
        match self {
            PeerIdGenerator::NEVER {
                should_url_encode, ..
            }
            | PeerIdGenerator::ALWAYS {
                should_url_encode, ..
            }
            | PeerIdGenerator::TIMED {
                should_url_encode, ..
            }
            | PeerIdGenerator::TORRENT_VOLATILE {
                should_url_encode, ..
            }
            | PeerIdGenerator::TORRENT_PERSISTENT {
                should_url_encode, ..
            } => *should_url_encode,
        }
    }

    pub fn get(&self, _event: RequestEvent) -> Result<String, ClientError> {
        match self {
            PeerIdGenerator::ALWAYS { algorithm, .. } => algorithm.generate(),
            PeerIdGenerator::NEVER { .. }
            | PeerIdGenerator::TIMED { .. }
            | PeerIdGenerator::TORRENT_VOLATILE { .. }
            | PeerIdGenerator::TORRENT_PERSISTENT { .. } => Err(ClientError::Integrity(
                "peer-id refresh semantics beyond ALWAYS are deferred to S5".to_owned(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use regex::Regex;

    #[test]
    fn regex_algorithm_snapshot_with_seed() {
        let algo = RegexPeerIdAlgorithm::new(r"-qB4500-[A-Za-z0-9_~\(\)\!\.\*-]{12}").unwrap();
        let mut rng = StdRng::seed_from_u64(0x5eed_1234);
        let got = algo.generate_with_rng(&mut rng).unwrap();
        let re = Regex::new(r"\A-qB4500-[A-Za-z0-9_~\(\)!\.\*-]{12}\z").unwrap();
        assert!(re.is_match(&got));
        assert_eq!(got.len(), PEER_ID_LENGTH);
        assert_eq!(got, "-qB4500-KXzcDRnO4BIm");
    }

    #[test]
    fn random_pool_with_checksum_snapshot_and_checksum_invariant() {
        let algo = RandomPoolWithChecksumPeerIdAlgorithm::new(
            "-TR4050-",
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
            62,
        )
        .unwrap();
        let mut rng = StdRng::seed_from_u64(0x1234_5678);
        let got = algo.generate_with_rng(&mut rng);
        assert_eq!(got.len(), PEER_ID_LENGTH);
        assert_eq!(got, "-TR4050-b2G3T3P7OsGW");

        let suffix = &got[algo.prefix.len()..];
        let pool: Vec<char> = algo.characters_pool.chars().collect();
        let mut total = 0usize;
        for ch in suffix.chars() {
            total += pool.iter().position(|candidate| *candidate == ch).unwrap();
        }
        assert_eq!(total % algo.base, 0);
    }

    #[test]
    fn random_pool_rejects_bad_inputs() {
        assert!(RandomPoolWithChecksumPeerIdAlgorithm::new("", "ABC", 3).is_err());
        assert!(RandomPoolWithChecksumPeerIdAlgorithm::new("X", "", 3).is_err());
        assert!(RandomPoolWithChecksumPeerIdAlgorithm::new("X", "ABC", 0).is_err());
        assert!(
            RandomPoolWithChecksumPeerIdAlgorithm::new("12345678901234567890", "ABC", 3).is_err()
        );
    }

    #[test]
    fn serde_dispatch_matches_java_type_tag() {
        let json = r#"{"type":"REGEX","pattern":"-qB4500-[A-Z]{12}"}"#;
        let algo: PeerIdAlgorithmDef = serde_json::from_str(json).unwrap();
        match algo {
            PeerIdAlgorithmDef::REGEX(inner) => assert_eq!(inner.pattern, "-qB4500-[A-Z]{12}"),
            PeerIdAlgorithmDef::RANDOM_POOL_WITH_CHECKSUM(_) => {
                panic!("unexpected variant: {algo:?}")
            }
        }
    }

    #[test]
    fn refresh_shell_parses_never_variant_from_client_json() {
        let json = r#"{
            "refreshOn":"NEVER",
            "algorithm":{"type":"REGEX","pattern":"-qB4500-[A-Z]{12}"},
            "shouldUrlEncode":false
        }"#;
        let generator: PeerIdGenerator = serde_json::from_str(json).unwrap();
        assert!(!generator.should_url_encode());
        assert!(matches!(generator, PeerIdGenerator::NEVER { .. }));
    }
}
