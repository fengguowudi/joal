//! Key generation algorithms and config for the refresh-policy system.
//!
//! Ports Java `generator/key/algorithm/*` and `generator/key/*`.

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::utils::Casing;

use super::common::compile_rand_regex;
use super::refresh_policy::{GenerateValue, RefreshPolicy};

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

/// Algorithm used to generate a raw key byte sequence before `keyCase` is applied.
///
/// All bundled key algorithms produce ASCII output, but we return `Vec<u8>` for
/// consistency with the peer-id side of the trait family.
pub trait KeyAlgorithm {
    fn generate(&self) -> Result<Vec<u8>, ClientError>;
}

fn generate_key(algorithm: &KeyAlgorithmDef, key_case: Casing) -> Result<Vec<u8>, ClientError> {
    let raw = algorithm.generate()?;
    // All shipped key algorithms emit ASCII, so the round-trip is lossless.
    let raw_str = std::str::from_utf8(&raw)
        .map_err(|e| ClientError::NonUtf8Output(format!("key algorithm: {e}")))?;
    Ok(key_case.to_case(raw_str).into_bytes())
}

/// `type = "HASH"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashKeyAlgorithm {
    pub length: usize,
}

impl HashKeyAlgorithm {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> String {
        let mut out = String::with_capacity(self.length);
        for _ in 0..self.length {
            let idx = rng.gen_range(0..HEX_UPPER.len());
            out.push(char::from(HEX_UPPER[idx]));
        }
        out
    }
}

impl KeyAlgorithm for HashKeyAlgorithm {
    fn generate(&self) -> Result<Vec<u8>, ClientError> {
        let mut rng = rand::thread_rng();
        Ok(self.generate_with_rng(&mut rng).into_bytes())
    }
}

/// `type = "HASH_NO_LEADING_ZERO"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashNoLeadingZeroKeyAlgorithm {
    pub length: usize,
}

impl HashNoLeadingZeroKeyAlgorithm {
    fn remove_leading_zeroes(s: &str) -> String {
        s.trim_start_matches('0').to_owned()
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> String {
        let base = HashKeyAlgorithm {
            length: self.length,
        }
        .generate_with_rng(rng);
        Self::remove_leading_zeroes(&base)
    }
}

impl KeyAlgorithm for HashNoLeadingZeroKeyAlgorithm {
    fn generate(&self) -> Result<Vec<u8>, ClientError> {
        let mut rng = rand::thread_rng();
        Ok(self.generate_with_rng(&mut rng).into_bytes())
    }
}

/// `type = "DIGIT_RANGE_TRANSFORMED_TO_HEX_WITHOUT_LEADING_ZEROES"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigitRangeTransformedToHexWithoutLeadingZeroKeyAlgorithm {
    #[serde(rename = "inclusiveLowerBound")]
    pub inclusive_lower_bound: u64,
    #[serde(rename = "inclusiveUpperBound")]
    pub inclusive_upper_bound: u64,
}

impl DigitRangeTransformedToHexWithoutLeadingZeroKeyAlgorithm {
    pub fn new(
        inclusive_lower_bound: u64,
        inclusive_upper_bound: u64,
    ) -> Result<Self, ClientError> {
        let algorithm = Self {
            inclusive_lower_bound,
            inclusive_upper_bound,
        };
        algorithm.validate()?;
        Ok(algorithm)
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        if self.inclusive_upper_bound < self.inclusive_lower_bound {
            return Err(ClientError::Integrity(
                "inclusiveUpperBound must be greater than inclusiveLowerBound".to_owned(),
            ));
        }
        Ok(())
    }

    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> String {
        let n = rng.gen_range(self.inclusive_lower_bound..=self.inclusive_upper_bound);
        format!("{n:x}")
    }
}

impl KeyAlgorithm for DigitRangeTransformedToHexWithoutLeadingZeroKeyAlgorithm {
    fn generate(&self) -> Result<Vec<u8>, ClientError> {
        let mut rng = rand::thread_rng();
        Ok(self.generate_with_rng(&mut rng).into_bytes())
    }
}

/// `type = "REGEX"`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegexKeyAlgorithm {
    pub pattern: String,
}

impl RegexKeyAlgorithm {
    pub fn new(pattern: impl Into<String>) -> Result<Self, ClientError> {
        let algorithm = Self {
            pattern: pattern.into(),
        };
        algorithm.validate()?;
        Ok(algorithm)
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        if self.pattern.trim().is_empty() {
            return Err(ClientError::Integrity(
                "peerId algorithm pattern must not be null.".to_owned(),
            ));
        }
        compile_rand_regex(&self.pattern)?;
        Ok(())
    }

    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> Result<Vec<u8>, ClientError> {
        let generator = compile_rand_regex(&self.pattern)?;
        let bytes: Vec<u8> = rng.sample(&generator);
        Ok(bytes)
    }
}

impl KeyAlgorithm for RegexKeyAlgorithm {
    fn generate(&self) -> Result<Vec<u8>, ClientError> {
        let mut rng = rand::thread_rng();
        self.generate_with_rng(&mut rng)
    }
}

/// Serde dispatch matching Java `@JsonTypeInfo(property = "type")`.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum KeyAlgorithmDef {
    HASH(HashKeyAlgorithm),
    HASH_NO_LEADING_ZERO(HashNoLeadingZeroKeyAlgorithm),
    REGEX(RegexKeyAlgorithm),
    DIGIT_RANGE_TRANSFORMED_TO_HEX_WITHOUT_LEADING_ZEROES(
        DigitRangeTransformedToHexWithoutLeadingZeroKeyAlgorithm,
    ),
}

impl KeyAlgorithmDef {
    pub fn validate(&self) -> Result<(), ClientError> {
        match self {
            KeyAlgorithmDef::HASH(_) | KeyAlgorithmDef::HASH_NO_LEADING_ZERO(_) => Ok(()),
            KeyAlgorithmDef::REGEX(inner) => inner.validate(),
            KeyAlgorithmDef::DIGIT_RANGE_TRANSFORMED_TO_HEX_WITHOUT_LEADING_ZEROES(inner) => {
                inner.validate()
            }
        }
    }

    pub fn generate(&self) -> Result<Vec<u8>, ClientError> {
        self.validate()?;
        match self {
            KeyAlgorithmDef::HASH(inner) => inner.generate(),
            KeyAlgorithmDef::HASH_NO_LEADING_ZERO(inner) => inner.generate(),
            KeyAlgorithmDef::REGEX(inner) => inner.generate(),
            KeyAlgorithmDef::DIGIT_RANGE_TRANSFORMED_TO_HEX_WITHOUT_LEADING_ZEROES(inner) => {
                inner.generate()
            }
        }
    }
}

/// Config for key generation: algorithm + casing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyConfig {
    pub algorithm: KeyAlgorithmDef,
    #[serde(rename = "keyCase")]
    pub key_case: Casing,
}

impl KeyConfig {
    #[must_use]
    pub fn key_case(&self) -> Casing {
        self.key_case
    }
}

impl GenerateValue for KeyConfig {
    fn generate(&self) -> Result<Vec<u8>, ClientError> {
        generate_key(&self.algorithm, self.key_case)
    }

    fn validate(&self) -> Result<(), ClientError> {
        self.algorithm.validate()
    }
}

/// Runtime generator matching Java `KeyGenerator` refresh wrappers.
pub type KeyGenerator = RefreshPolicy<KeyConfig>;
