//! Key generation algorithms and refresh-policy runtime semantics.
//!
//! Ports Java `generator/key/algorithm/*` and `generator/key/*` including the
//! stateful refresh wrappers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rand::Rng;
use rand_regex::Regex as RandRegex;
use regex_syntax::ParserBuilder;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::event::RequestEvent;
use crate::client::utils::Casing;
use crate::torrent::InfoHash;

const TORRENT_PERSISTENT_TTL: Duration = Duration::from_hours(2);
const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

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

fn lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn default_shared_state<T: Default>() -> Arc<Mutex<T>> {
    Arc::new(Mutex::new(T::default()))
}

fn generate_key(algorithm: &KeyAlgorithmDef, key_case: Casing) -> Result<String, ClientError> {
    Ok(key_case.to_case(&algorithm.generate()?))
}

#[derive(Debug, Clone, Default)]
struct TimedKeyState {
    key: Option<String>,
    last_generation: Option<Instant>,
}

#[derive(Debug, Clone)]
struct AccessAwareKey {
    key: String,
    last_access: Instant,
    // `#[cfg(test)]`-only override: lets tests mark an entry as "already
    // expired" without needing `Instant::now().checked_sub(TTL)` — which
    // underflows on fresh Windows boots where the monotonic clock is
    // anchored to system uptime. Production code never touches this field.
    #[cfg(test)]
    force_stale: bool,
}

impl AccessAwareKey {
    fn new(key: String) -> Self {
        Self {
            key,
            last_access: Instant::now(),
            #[cfg(test)]
            force_stale: false,
        }
    }

    fn get_key(&mut self) -> &str {
        self.last_access = Instant::now();
        #[cfg(test)]
        {
            // Any read resets the test-only stale flag so that a torrent
            // that gets re-touched after being force-expired behaves like a
            // freshly-accessed entry (mirrors production `last_access` reset).
            self.force_stale = false;
        }
        &self.key
    }

    fn should_evict(&self, now: Instant) -> bool {
        #[cfg(test)]
        if self.force_stale {
            return true;
        }
        now.duration_since(self.last_access) >= TORRENT_PERSISTENT_TTL
    }

    #[cfg(test)]
    fn mark_stale_for_test(&mut self) {
        self.force_stale = true;
    }
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

/// Runtime generator matching Java `KeyGenerator` refresh wrappers.
#[allow(non_camel_case_types, private_interfaces)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "refreshOn")]
pub enum KeyGenerator {
    NEVER {
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
        #[serde(skip, default = "default_shared_state::<TimedKeyState>")]
        state: Arc<Mutex<TimedKeyState>>,
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
        #[serde(skip, default = "default_shared_state::<TimedKeyState>")]
        state: Arc<Mutex<TimedKeyState>>,
    },
    TIMED_OR_AFTER_STARTED_ANNOUNCE {
        #[serde(rename = "refreshEvery")]
        refresh_every: i32,
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
        #[serde(skip, default = "default_shared_state::<TimedKeyState>")]
        state: Arc<Mutex<TimedKeyState>>,
    },
    TORRENT_VOLATILE {
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
        #[serde(skip, default = "default_shared_state::<HashMap<InfoHash, String>>")]
        state: Arc<Mutex<HashMap<InfoHash, String>>>,
    },
    TORRENT_PERSISTENT {
        algorithm: KeyAlgorithmDef,
        #[serde(rename = "keyCase")]
        key_case: Casing,
        #[serde(
            skip,
            default = "default_shared_state::<HashMap<InfoHash, AccessAwareKey>>"
        )]
        state: Arc<Mutex<HashMap<InfoHash, AccessAwareKey>>>,
    },
}

impl Clone for KeyGenerator {
    fn clone(&self) -> Self {
        match self {
            KeyGenerator::NEVER {
                algorithm,
                key_case,
                ..
            } => Self::NEVER {
                algorithm: algorithm.clone(),
                key_case: *key_case,
                state: default_shared_state(),
            },
            KeyGenerator::ALWAYS {
                algorithm,
                key_case,
            } => Self::ALWAYS {
                algorithm: algorithm.clone(),
                key_case: *key_case,
            },
            KeyGenerator::TIMED {
                refresh_every,
                algorithm,
                key_case,
                ..
            } => Self::TIMED {
                refresh_every: *refresh_every,
                algorithm: algorithm.clone(),
                key_case: *key_case,
                state: default_shared_state(),
            },
            KeyGenerator::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                refresh_every,
                algorithm,
                key_case,
                ..
            } => Self::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                refresh_every: *refresh_every,
                algorithm: algorithm.clone(),
                key_case: *key_case,
                state: default_shared_state(),
            },
            KeyGenerator::TORRENT_VOLATILE {
                algorithm,
                key_case,
                ..
            } => Self::TORRENT_VOLATILE {
                algorithm: algorithm.clone(),
                key_case: *key_case,
                state: default_shared_state(),
            },
            KeyGenerator::TORRENT_PERSISTENT {
                algorithm,
                key_case,
                ..
            } => Self::TORRENT_PERSISTENT {
                algorithm: algorithm.clone(),
                key_case: *key_case,
                state: default_shared_state(),
            },
        }
    }
}

#[allow(clippy::match_same_arms)]
impl PartialEq for KeyGenerator {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::NEVER {
                    algorithm: left_algorithm,
                    key_case: left_key_case,
                    ..
                },
                Self::NEVER {
                    algorithm: right_algorithm,
                    key_case: right_key_case,
                    ..
                },
            )
            | (
                Self::ALWAYS {
                    algorithm: left_algorithm,
                    key_case: left_key_case,
                },
                Self::ALWAYS {
                    algorithm: right_algorithm,
                    key_case: right_key_case,
                },
            )
            | (
                Self::TORRENT_VOLATILE {
                    algorithm: left_algorithm,
                    key_case: left_key_case,
                    ..
                },
                Self::TORRENT_VOLATILE {
                    algorithm: right_algorithm,
                    key_case: right_key_case,
                    ..
                },
            )
            | (
                Self::TORRENT_PERSISTENT {
                    algorithm: left_algorithm,
                    key_case: left_key_case,
                    ..
                },
                Self::TORRENT_PERSISTENT {
                    algorithm: right_algorithm,
                    key_case: right_key_case,
                    ..
                },
            ) => left_algorithm == right_algorithm && left_key_case == right_key_case,
            (
                Self::TIMED {
                    refresh_every: left_refresh_every,
                    algorithm: left_algorithm,
                    key_case: left_key_case,
                    ..
                },
                Self::TIMED {
                    refresh_every: right_refresh_every,
                    algorithm: right_algorithm,
                    key_case: right_key_case,
                    ..
                },
            )
            | (
                Self::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                    refresh_every: left_refresh_every,
                    algorithm: left_algorithm,
                    key_case: left_key_case,
                    ..
                },
                Self::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                    refresh_every: right_refresh_every,
                    algorithm: right_algorithm,
                    key_case: right_key_case,
                    ..
                },
            ) => {
                left_refresh_every == right_refresh_every
                    && left_algorithm == right_algorithm
                    && left_key_case == right_key_case
            }
            _ => false,
        }
    }
}

impl Eq for KeyGenerator {}

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

    pub fn get(&self, info_hash: &InfoHash, event: RequestEvent) -> Result<String, ClientError> {
        self.validate()?;
        match self {
            KeyGenerator::NEVER {
                algorithm,
                key_case,
                state,
            } => {
                let mut state = lock_state(state);
                if state.key.is_none() {
                    state.key = Some(generate_key(algorithm, *key_case)?);
                    state.last_generation = Some(Instant::now());
                }
                Ok(state.key.clone().expect("key initialized"))
            }
            KeyGenerator::ALWAYS {
                algorithm,
                key_case,
            } => generate_key(algorithm, *key_case),
            KeyGenerator::TIMED {
                refresh_every,
                algorithm,
                key_case,
                state,
            } => {
                let mut state = lock_state(state);
                let should_regenerate = state.last_generation.is_none_or(|last_generation| {
                    last_generation.elapsed() >= Duration::from_secs(*refresh_every as u64)
                });
                if should_regenerate {
                    state.last_generation = Some(Instant::now());
                    state.key = Some(generate_key(algorithm, *key_case)?);
                }
                Ok(state.key.clone().expect("key initialized"))
            }
            KeyGenerator::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                refresh_every,
                algorithm,
                key_case,
                state,
            } => {
                let mut state = lock_state(state);
                let should_regenerate = state.last_generation.is_none_or(|last_generation| {
                    last_generation.elapsed() >= Duration::from_secs(*refresh_every as u64)
                });
                if should_regenerate {
                    state.last_generation = Some(Instant::now());
                    state.key = Some(generate_key(algorithm, *key_case)?);
                }

                let key = state.key.clone().expect("key initialized");
                if event == RequestEvent::Started {
                    state.key = Some(generate_key(algorithm, *key_case)?);
                }
                Ok(key)
            }
            KeyGenerator::TORRENT_VOLATILE {
                algorithm,
                key_case,
                state,
            } => {
                let mut state = lock_state(state);
                if !state.contains_key(info_hash) {
                    state.insert(info_hash.clone(), generate_key(algorithm, *key_case)?);
                }
                let key = state.get(info_hash).cloned().expect("key initialized");
                if event == RequestEvent::Stopped {
                    state.remove(info_hash);
                }
                Ok(key)
            }
            KeyGenerator::TORRENT_PERSISTENT {
                algorithm,
                key_case,
                state,
            } => {
                let mut state = lock_state(state);
                if !state.contains_key(info_hash) {
                    state.insert(
                        info_hash.clone(),
                        AccessAwareKey::new(generate_key(algorithm, *key_case)?),
                    );
                }
                let key = state
                    .get_mut(info_hash)
                    .expect("key initialized")
                    .get_key()
                    .to_owned();
                let now = Instant::now();
                state.retain(|_, entry| !entry.should_evict(now));
                Ok(key)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use regex::Regex;

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
            algorithm: KeyAlgorithmDef::HASH(HashKeyAlgorithm { length: 4 }),
            key_case: Casing::Lower,
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
        assert_eq!(generator.key_case(), Casing::Upper);
        assert!(matches!(generator, KeyGenerator::TORRENT_PERSISTENT { .. }));
    }

    #[test]
    fn never_reuses_same_key_forever() {
        let generator = KeyGenerator::NEVER {
            algorithm: regex_algorithm(),
            key_case: Casing::Upper,
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
            algorithm: regex_algorithm(),
            key_case: Casing::Upper,
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
            algorithm: regex_algorithm(),
            key_case: Casing::Upper,
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
            algorithm: regex_algorithm(),
            key_case: Casing::Upper,
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
            algorithm: regex_algorithm(),
            key_case: Casing::Upper,
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
