//! Peer-id generation algorithms and config for the refresh-policy system.
//!
//! Ports Java `generator/peerid/generation/*` and `generator/peerid/*`.

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;

use super::common::{compile_rand_regex, string_from_ascii_regex_bytes};
use super::refresh_policy::{GenerateValue, RefreshPolicy};

#[cfg(test)]
use super::common::{default_shared_state, lock_state};

/// Java constant `PeerIdGenerator.PEER_ID_LENGTH`.
pub const PEER_ID_LENGTH: usize = 20;

/// Algorithm used to generate a raw peer-id string.
pub trait PeerIdAlgorithm {
    /// Generate a peer-id using a deterministic or random source.
    fn generate(&self) -> Result<String, ClientError>;
}

fn generate_peer_id(algorithm: &PeerIdAlgorithmDef) -> Result<String, ClientError> {
    let peer_id = algorithm.generate()?;
    if peer_id.len() != PEER_ID_LENGTH {
        return Err(ClientError::Integrity(format!(
            "PeerId length was supposed to be {PEER_ID_LENGTH}, but a length of {} was generated. Throw exception to prevent sending invalid PeerId to tracker",
            peer_id.len()
        )));
    }
    Ok(peer_id)
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

/// Config for peer-id generation: algorithm + URL-encoding flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerIdConfig {
    pub algorithm: PeerIdAlgorithmDef,
    #[serde(rename = "shouldUrlEncode")]
    pub should_url_encode: bool,
}

impl PeerIdConfig {
    #[must_use]
    pub fn should_url_encode(&self) -> bool {
        self.should_url_encode
    }
}

impl GenerateValue for PeerIdConfig {
    fn generate(&self) -> Result<String, ClientError> {
        generate_peer_id(&self.algorithm)
    }

    fn validate(&self) -> Result<(), ClientError> {
        self.algorithm.validate()
    }
}

/// Runtime generator matching Java `PeerIdGenerator` refresh wrappers.
pub type PeerIdGenerator = RefreshPolicy<PeerIdConfig>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::event::RequestEvent;
    use crate::torrent::InfoHash;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use regex::Regex;
    use std::time::{Duration, Instant};

    fn info_hash(fill: u8) -> InfoHash {
        InfoHash::from_bytes([fill; 20])
    }

    fn regex_algorithm() -> PeerIdAlgorithmDef {
        PeerIdAlgorithmDef::REGEX(RegexPeerIdAlgorithm {
            pattern: "[A-Z0-9]{20}".to_owned(),
        })
    }

    fn timed_generator(refresh_every: i32) -> PeerIdGenerator {
        PeerIdGenerator::TIMED {
            refresh_every,
            config: PeerIdConfig {
                algorithm: regex_algorithm(),
                should_url_encode: false,
            },
            state: default_shared_state(),
        }
    }

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
        assert!(!generator.config().should_url_encode());
        assert!(matches!(generator, PeerIdGenerator::NEVER { .. }));
    }

    #[test]
    fn never_reuses_same_peer_id_forever() {
        let generator = PeerIdGenerator::NEVER {
            config: PeerIdConfig {
                algorithm: regex_algorithm(),
                should_url_encode: false,
            },
            state: default_shared_state(),
        };
        let first = generator.get(&info_hash(1), RequestEvent::None).unwrap();
        let second = generator.get(&info_hash(2), RequestEvent::Stopped).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn timed_reuses_until_refresh_every_elapsed() {
        let generator = timed_generator(60);
        let first = generator.get(&info_hash(1), RequestEvent::None).unwrap();
        let second = generator
            .get(&info_hash(1), RequestEvent::Completed)
            .unwrap();
        assert_eq!(first, second);

        let PeerIdGenerator::TIMED { state, .. } = &generator else {
            panic!("expected timed generator");
        };
        lock_state(state).last_generation =
            Some(Instant::now().checked_sub(Duration::from_secs(61)).unwrap());

        let third = generator.get(&info_hash(1), RequestEvent::None).unwrap();
        assert_ne!(first, third);
    }

    #[test]
    fn torrent_volatile_reuses_per_torrent_and_evicts_on_stopped() {
        let generator = PeerIdGenerator::TORRENT_VOLATILE {
            config: PeerIdConfig {
                algorithm: regex_algorithm(),
                should_url_encode: false,
            },
            state: default_shared_state(),
        };
        let torrent_a = info_hash(1);
        let torrent_b = info_hash(2);

        let first_a = generator.get(&torrent_a, RequestEvent::None).unwrap();
        let second_a = generator.get(&torrent_a, RequestEvent::Completed).unwrap();
        let first_b = generator.get(&torrent_b, RequestEvent::None).unwrap();
        let stopped_a = generator.get(&torrent_a, RequestEvent::Stopped).unwrap();
        let after_stop_a = generator.get(&torrent_a, RequestEvent::None).unwrap();

        assert_eq!(first_a, second_a);
        assert_eq!(first_a, stopped_a);
        assert_ne!(first_a, first_b);
        assert_ne!(first_a, after_stop_a);
    }

    #[test]
    fn torrent_persistent_reuses_per_torrent_and_sweeps_on_each_get() {
        let generator = PeerIdGenerator::TORRENT_PERSISTENT {
            config: PeerIdConfig {
                algorithm: regex_algorithm(),
                should_url_encode: false,
            },
            state: default_shared_state(),
        };
        let stale_torrent = info_hash(1);
        let hot_torrent = info_hash(2);

        let stale_value = generator.get(&stale_torrent, RequestEvent::None).unwrap();
        let hot_value = generator.get(&hot_torrent, RequestEvent::None).unwrap();

        let PeerIdGenerator::TORRENT_PERSISTENT { state, .. } = &generator else {
            panic!("expected persistent generator");
        };
        lock_state(state)
            .get_mut(&stale_torrent)
            .unwrap()
            .mark_stale_for_test();

        assert_eq!(
            generator.get(&hot_torrent, RequestEvent::None).unwrap(),
            hot_value
        );
        assert_eq!(lock_state(state).len(), 1);

        let stale_after_evict = generator.get(&stale_torrent, RequestEvent::None).unwrap();
        assert_ne!(stale_value, stale_after_evict);
    }

    #[test]
    fn rejects_generated_peer_id_with_wrong_length() {
        let generator = PeerIdGenerator::ALWAYS {
            config: PeerIdConfig {
                algorithm: PeerIdAlgorithmDef::REGEX(RegexPeerIdAlgorithm {
                    pattern: "[A-Z]{19}".to_owned(),
                }),
                should_url_encode: false,
            },
        };

        let error = generator
            .get(&info_hash(1), RequestEvent::None)
            .unwrap_err();
        assert!(matches!(error, ClientError::Integrity(_)));
    }
}
