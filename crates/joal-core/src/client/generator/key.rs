//! Key generation algorithms and config for the refresh-policy system.
//!
//! Ports Java `generator/key/algorithm/*` and `generator/key/*`.

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::utils::Casing;

use super::common::{compile_rand_regex, string_from_ascii_regex_bytes};
use super::refresh_policy::{GenerateValue, RefreshPolicy};

#[cfg(test)]
use super::common::{default_shared_state, lock_state};

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

/// Algorithm used to generate a raw key string before `keyCase` is applied.
pub trait KeyAlgorithm {
    fn generate(&self) -> Result<String, ClientError>;
}

fn generate_key(algorithm: &KeyAlgorithmDef, key_case: Casing) -> Result<String, ClientError> {
    Ok(key_case.to_case(&algorithm.generate()?))
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
    fn generate(&self) -> Result<String, ClientError> {
        let mut rng = rand::thread_rng();
        Ok(self.generate_with_rng(&mut rng))
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
    fn generate(&self) -> Result<String, ClientError> {
        let mut rng = rand::thread_rng();
        Ok(self.generate_with_rng(&mut rng))
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
    fn generate(&self) -> Result<String, ClientError> {
        let mut rng = rand::thread_rng();
        Ok(self.generate_with_rng(&mut rng))
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
        let _ = compile_rand_regex(&self.pattern)?;
        Ok(())
    }

    fn generate_with_rng<R: Rng + ?Sized>(&self, rng: &mut R) -> Result<String, ClientError> {
        let generator = compile_rand_regex(&self.pattern)?;
        let bytes: Vec<u8> = rng.sample(&generator);
        string_from_ascii_regex_bytes(bytes)
    }
}

impl KeyAlgorithm for RegexKeyAlgorithm {
    fn generate(&self) -> Result<String, ClientError> {
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

    pub fn generate(&self) -> Result<String, ClientError> {
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
    fn generate(&self) -> Result<String, ClientError> {
        generate_key(&self.algorithm, self.key_case)
    }

    fn validate(&self) -> Result<(), ClientError> {
        self.algorithm.validate()
    }
}

/// Runtime generator matching Java `KeyGenerator` refresh wrappers.
pub type KeyGenerator = RefreshPolicy<KeyConfig>;

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

    fn regex_algorithm() -> KeyAlgorithmDef {
        KeyAlgorithmDef::REGEX(RegexKeyAlgorithm {
            pattern: "[A-Z0-9]{8}".to_owned(),
        })
    }

    #[test]
    fn hash_algorithm_snapshot_with_seed_and_upper_hex() {
        let algo = HashKeyAlgorithm { length: 8 };
        let mut rng = StdRng::seed_from_u64(0xface_cafe);
        let got = algo.generate_with_rng(&mut rng);
        assert_eq!(got.len(), 8);
        assert!(
            got.chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_lowercase())
        );
        assert_eq!(got, "3E893B23");
    }

    #[test]
    fn hash_no_leading_zero_removes_zero_prefix() {
        assert_eq!(
            HashNoLeadingZeroKeyAlgorithm::remove_leading_zeroes("0001AF"),
            "1AF"
        );
        assert_eq!(
            HashNoLeadingZeroKeyAlgorithm::remove_leading_zeroes("ABCD"),
            "ABCD"
        );
        assert_eq!(
            HashNoLeadingZeroKeyAlgorithm::remove_leading_zeroes("0000"),
            ""
        );
    }

    #[test]
    fn hash_no_leading_zero_snapshot_with_seed() {
        let algo = HashNoLeadingZeroKeyAlgorithm { length: 8 };
        let mut rng = StdRng::seed_from_u64(0x1234_9999);
        let got = algo.generate_with_rng(&mut rng);
        assert!(
            got.chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_lowercase())
        );
        assert!(!got.starts_with('0'));
        assert_eq!(got, "E3874A3F");
    }

    #[test]
    fn digit_range_transformed_to_hex_snapshot_with_seed() {
        let algo =
            DigitRangeTransformedToHexWithoutLeadingZeroKeyAlgorithm::new(1_000_000, 9_999_999)
                .unwrap();
        let mut rng = StdRng::seed_from_u64(0xdead_beef);
        let got = algo.generate_with_rng(&mut rng);
        assert!(
            got.chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        );
        assert_eq!(got, "1dc3fa");
    }

    #[test]
    fn regex_algorithm_snapshot_with_seed() {
        let algo = RegexKeyAlgorithm::new(r"[A-Z0-9]{8}").unwrap();
        let mut rng = StdRng::seed_from_u64(0x1357_2468);
        let got = algo.generate_with_rng(&mut rng).unwrap();
        let re = Regex::new(r"\A[A-Z0-9]{8}\z").unwrap();
        assert!(re.is_match(&got));
        assert_eq!(got, "5V6INXKR");
    }

    #[test]
    fn key_case_applies_after_algorithm() {
        let generator = KeyGenerator::ALWAYS {
            config: KeyConfig {
                algorithm: KeyAlgorithmDef::HASH(HashKeyAlgorithm { length: 4 }),
                key_case: Casing::Lower,
            },
        };
        let got = generator.get(&info_hash(1), RequestEvent::None).unwrap();
        assert_eq!(got.len(), 4);
        assert!(got.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(got, got.to_ascii_lowercase());
    }

    #[test]
    fn serde_dispatch_matches_java_type_tag() {
        let json = r#"{"type":"HASH_NO_LEADING_ZERO","length":8}"#;
        let algo: KeyAlgorithmDef = serde_json::from_str(json).unwrap();
        match algo {
            KeyAlgorithmDef::HASH_NO_LEADING_ZERO(inner) => assert_eq!(inner.length, 8),
            KeyAlgorithmDef::HASH(_)
            | KeyAlgorithmDef::REGEX(_)
            | KeyAlgorithmDef::DIGIT_RANGE_TRANSFORMED_TO_HEX_WITHOUT_LEADING_ZEROES(_) => {
                panic!("unexpected variant: {algo:?}")
            }
        }
    }

    #[test]
    fn generator_shell_parses_torrent_persistent_variant() {
        let json = r#"{
            "refreshOn":"TORRENT_PERSISTENT",
            "algorithm":{"type":"HASH_NO_LEADING_ZERO","length":8},
            "keyCase":"upper"
        }"#;
        let generator: KeyGenerator = serde_json::from_str(json).unwrap();
        assert_eq!(generator.config().key_case(), Casing::Upper);
        assert!(matches!(generator, KeyGenerator::TORRENT_PERSISTENT { .. }));
    }

    #[test]
    fn never_reuses_same_key_forever() {
        let generator = KeyGenerator::NEVER {
            config: KeyConfig {
                algorithm: regex_algorithm(),
                key_case: Casing::Upper,
            },
            state: default_shared_state(),
        };
        let first = generator.get(&info_hash(1), RequestEvent::None).unwrap();
        let second = generator.get(&info_hash(2), RequestEvent::Stopped).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn timed_reuses_until_refresh_every_elapsed() {
        let generator = KeyGenerator::TIMED {
            refresh_every: 60,
            config: KeyConfig {
                algorithm: regex_algorithm(),
                key_case: Casing::Upper,
            },
            state: default_shared_state(),
        };
        let first = generator.get(&info_hash(1), RequestEvent::None).unwrap();
        let second = generator
            .get(&info_hash(1), RequestEvent::Completed)
            .unwrap();
        assert_eq!(first, second);

        let KeyGenerator::TIMED { state, .. } = &generator else {
            panic!("expected timed generator");
        };
        lock_state(state).last_generation =
            Some(Instant::now().checked_sub(Duration::from_secs(61)).unwrap());

        let third = generator.get(&info_hash(1), RequestEvent::None).unwrap();
        assert_ne!(first, third);
    }

    #[test]
    fn timed_or_after_started_announce_rotates_after_returning_started_key() {
        let generator = KeyGenerator::TIMED_OR_AFTER_STARTED_ANNOUNCE {
            refresh_every: 60,
            config: KeyConfig {
                algorithm: regex_algorithm(),
                key_case: Casing::Upper,
            },
            state: default_shared_state(),
        };
        let started = generator.get(&info_hash(1), RequestEvent::Started).unwrap();
        let after_started = generator.get(&info_hash(1), RequestEvent::None).unwrap();
        assert_ne!(started, after_started);
        let same_after_started = generator
            .get(&info_hash(1), RequestEvent::Completed)
            .unwrap();
        assert_eq!(after_started, same_after_started);
    }

    #[test]
    fn torrent_volatile_reuses_per_torrent_and_evicts_on_stopped() {
        let generator = KeyGenerator::TORRENT_VOLATILE {
            config: KeyConfig {
                algorithm: regex_algorithm(),
                key_case: Casing::Upper,
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
        let generator = KeyGenerator::TORRENT_PERSISTENT {
            config: KeyConfig {
                algorithm: regex_algorithm(),
                key_case: Casing::Upper,
            },
            state: default_shared_state(),
        };
        let stale_torrent = info_hash(1);
        let hot_torrent = info_hash(2);

        let stale_value = generator.get(&stale_torrent, RequestEvent::None).unwrap();
        let hot_value = generator.get(&hot_torrent, RequestEvent::None).unwrap();

        let KeyGenerator::TORRENT_PERSISTENT { state, .. } = &generator else {
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
}
