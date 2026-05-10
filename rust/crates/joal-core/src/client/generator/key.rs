//! Key generation algorithms and S4 refresh-policy shell.
//!
//! Ports Java `generator/key/algorithm/*` and the parseable outer
//! `generator/key/*` wrapper (`refreshOn`, `keyCase`). Full refresh semantics
//! are deferred to S5; for S4 we only need the algorithm layer plus enough of
//! the outer shell to deserialize real `.client` files.

use rand::Rng;
use rand_regex::Regex as RandRegex;
use regex_syntax::ParserBuilder;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::event::RequestEvent;
use crate::client::utils::Casing;

/// Algorithm used to generate a raw key string before `keyCase` is applied.
pub trait KeyAlgorithm {
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

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

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

/// Parseable shell for Java `KeyGenerator` refresh wrappers.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "refreshOn")]
pub enum KeyGenerator {
    NEVER {
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
    },
    ALWAYS {
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
    },
    TIMED {
        #[serde(rename = "refreshEvery")]
        refresh_every: i32,
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
    },
    TIMED_OR_AFTER_STARTED_ANNOUNCE {
        #[serde(rename = "refreshEvery")]
        refresh_every: i32,
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
    },
    TORRENT_VOLATILE {
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
    },
    TORRENT_PERSISTENT {
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
    },
}

impl KeyGenerator {
    pub fn validate(&self) -> Result<(), ClientError> {
        self.algorithm().validate()?;
        if matches!(
            self,
            KeyGenerator::NEVER { .. } | KeyGenerator::ALWAYS { .. }
        ) {
            return Ok(());
        }
        let refresh_every = match self {
            KeyGenerator::TIMED { refresh_every, .. }
            | KeyGenerator::TIMED_OR_AFTER_STARTED_ANNOUNCE { refresh_every, .. } => {
                Some(*refresh_every)
            }
            KeyGenerator::TORRENT_VOLATILE { .. } | KeyGenerator::TORRENT_PERSISTENT { .. } => None,
            KeyGenerator::NEVER { .. } | KeyGenerator::ALWAYS { .. } => unreachable!(),
        };
        if let Some(refresh_every) = refresh_every
            && refresh_every < 1
        {
            return Err(ClientError::Integrity(
                "refreshEvery must be greater than 0".to_owned(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn algorithm(&self) -> &KeyAlgorithmDef {
        match self {
            KeyGenerator::NEVER { algorithm, .. }
            | KeyGenerator::ALWAYS { algorithm, .. }
            | KeyGenerator::TIMED { algorithm, .. }
            | KeyGenerator::TIMED_OR_AFTER_STARTED_ANNOUNCE { algorithm, .. }
            | KeyGenerator::TORRENT_VOLATILE { algorithm, .. }
            | KeyGenerator::TORRENT_PERSISTENT { algorithm, .. } => algorithm,
        }
    }

    #[must_use]
    pub const fn key_case(&self) -> Casing {
        match self {
            KeyGenerator::NEVER { key_case, .. }
            | KeyGenerator::ALWAYS { key_case, .. }
            | KeyGenerator::TIMED { key_case, .. }
            | KeyGenerator::TIMED_OR_AFTER_STARTED_ANNOUNCE { key_case, .. }
            | KeyGenerator::TORRENT_VOLATILE { key_case, .. }
            | KeyGenerator::TORRENT_PERSISTENT { key_case, .. } => *key_case,
        }
    }

    pub fn get(&self, _event: RequestEvent) -> Result<String, ClientError> {
        match self {
            KeyGenerator::ALWAYS {
                algorithm,
                key_case,
            } => Ok(key_case.to_case(&algorithm.generate()?)),
            KeyGenerator::NEVER { .. }
            | KeyGenerator::TIMED { .. }
            | KeyGenerator::TIMED_OR_AFTER_STARTED_ANNOUNCE { .. }
            | KeyGenerator::TORRENT_VOLATILE { .. }
            | KeyGenerator::TORRENT_PERSISTENT { .. } => Err(ClientError::Integrity(
                "key refresh semantics beyond ALWAYS are deferred to S5".to_owned(),
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
            algorithm: KeyAlgorithmDef::HASH(HashKeyAlgorithm { length: 4 }),
            key_case: Casing::Lower,
        };
        let got = generator.get(RequestEvent::None).unwrap();
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
        assert_eq!(generator.key_case(), Casing::Upper);
        assert!(matches!(generator, KeyGenerator::TORRENT_PERSISTENT { .. }));
    }
}
