//! Peer-id generation algorithms and config for the refresh-policy system.
//!
//! Ports Java `generator/peerid/generation/*` and `generator/peerid/*`.

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;

use super::common::compile_rand_regex;
use super::refresh_policy::{GenerateValue, RefreshPolicy};

/// Java constant `PeerIdGenerator.PEER_ID_LENGTH`.
pub const PEER_ID_LENGTH: usize = 20;

/// Algorithm used to generate a raw peer-id byte sequence.
pub trait PeerIdAlgorithm {
    /// Generate a peer-id using a deterministic or random source.
    fn generate(&self) -> Result<Vec<u8>, ClientError>;
}

fn generate_peer_id(algorithm: &PeerIdAlgorithmDef) -> Result<Vec<u8>, ClientError> {
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
        compile_rand_regex(&self.pattern)?;
        Ok(())
    }

    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> Result<Vec<u8>, ClientError> {
        super::common::sample_rand_regex(&self.pattern, rng)
    }
}

impl PeerIdAlgorithm for RegexPeerIdAlgorithm {
    fn generate(&self) -> Result<Vec<u8>, ClientError> {
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

    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> Vec<u8> {
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

        format!("{}{}", self.prefix, suffix).into_bytes()
    }
}

impl PeerIdAlgorithm for RandomPoolWithChecksumPeerIdAlgorithm {
    fn generate(&self) -> Result<Vec<u8>, ClientError> {
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

    pub fn generate(&self) -> Result<Vec<u8>, ClientError> {
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
    fn generate(&self) -> Result<Vec<u8>, ClientError> {
        generate_peer_id(&self.algorithm)
    }

    fn validate(&self) -> Result<(), ClientError> {
        self.algorithm.validate()
    }
}

/// Runtime generator matching Java `PeerIdGenerator` refresh wrappers.
pub type PeerIdGenerator = RefreshPolicy<PeerIdConfig>;
