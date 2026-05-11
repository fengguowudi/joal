//! Peer-id generation algorithms and refresh-policy runtime semantics.
//!
//! This file ports two Java layers together:
//! - `generator/peerid/generation/*` — the actual generation algorithms.
//! - `generator/peerid/*` — the refresh-policy wrapper (`refreshOn`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rand::Rng;
use rand_regex::Regex as RandRegex;
use regex_syntax::ParserBuilder;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::event::RequestEvent;
use crate::torrent::InfoHash;

/// Java constant `PeerIdGenerator.PEER_ID_LENGTH`.
pub const PEER_ID_LENGTH: usize = 20;
const TORRENT_PERSISTENT_TTL: Duration = Duration::from_hours(2);
const TORRENT_PERSISTENT_SWEEP_EVERY_GETS: usize = 30;

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

fn lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn default_shared_state<T: Default>() -> Arc<Mutex<T>> {
    Arc::new(Mutex::new(T::default()))
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

#[derive(Debug, Clone, Default)]
struct TimedPeerIdState {
    peer_id: Option<String>,
    last_generation: Option<Instant>,
}

#[derive(Debug, Clone)]
struct AccessAwarePeerId {
    peer_id: String,
    last_access: Instant,
    // `#[cfg(test)]`-only override: lets tests mark an entry as "already
    // expired" without needing `Instant::now().checked_sub(TTL)` — which
    // underflows on fresh Windows boots where the monotonic clock is
    // anchored to system uptime. Production code never touches this field.
    #[cfg(test)]
    force_stale: bool,
}

impl AccessAwarePeerId {
    fn new(peer_id: String) -> Self {
        Self {
            peer_id,
            last_access: Instant::now(),
            #[cfg(test)]
            force_stale: false,
        }
    }

    fn get_peer_id(&mut self) -> &str {
        self.last_access = Instant::now();
        #[cfg(test)]
        {
            // Any read resets the test-only stale flag so that a torrent
            // that gets re-touched after being force-expired behaves like a
            // freshly-accessed entry (mirrors production `last_access` reset).
            self.force_stale = false;
        }
        &self.peer_id
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

#[derive(Debug, Clone, Default)]
struct TorrentPersistentPeerIdState {
    peer_id_per_torrent: HashMap<InfoHash, AccessAwarePeerId>,
    get_counter: usize,
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

/// Runtime generator matching Java `PeerIdGenerator` refresh wrappers.
#[allow(non_camel_case_types, private_interfaces)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "refreshOn")]
pub enum PeerIdGenerator {
    NEVER {
        algorithm: PeerIdAlgorithmDef,
        #[serde(rename = "shouldUrlEncode")]
        should_url_encode: bool,
        #[serde(skip, default = "default_shared_state::<TimedPeerIdState>")]
        state: Arc<Mutex<TimedPeerIdState>>,
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
        #[serde(skip, default = "default_shared_state::<TimedPeerIdState>")]
        state: Arc<Mutex<TimedPeerIdState>>,
    },
    TORRENT_VOLATILE {
        algorithm: PeerIdAlgorithmDef,
        #[serde(rename = "shouldUrlEncode")]
        should_url_encode: bool,
        #[serde(skip, default = "default_shared_state::<HashMap<InfoHash, String>>")]
        state: Arc<Mutex<HashMap<InfoHash, String>>>,
    },
    TORRENT_PERSISTENT {
        algorithm: PeerIdAlgorithmDef,
        #[serde(rename = "shouldUrlEncode")]
        should_url_encode: bool,
        #[serde(skip, default = "default_shared_state::<TorrentPersistentPeerIdState>")]
        state: Arc<Mutex<TorrentPersistentPeerIdState>>,
    },
}

impl Clone for PeerIdGenerator {
    fn clone(&self) -> Self {
        match self {
            PeerIdGenerator::NEVER {
                algorithm,
                should_url_encode,
                ..
            } => Self::NEVER {
                algorithm: algorithm.clone(),
                should_url_encode: *should_url_encode,
                state: default_shared_state(),
            },
            PeerIdGenerator::ALWAYS {
                algorithm,
                should_url_encode,
            } => Self::ALWAYS {
                algorithm: algorithm.clone(),
                should_url_encode: *should_url_encode,
            },
            PeerIdGenerator::TIMED {
                refresh_every,
                algorithm,
                should_url_encode,
                ..
            } => Self::TIMED {
                refresh_every: *refresh_every,
                algorithm: algorithm.clone(),
                should_url_encode: *should_url_encode,
                state: default_shared_state(),
            },
            PeerIdGenerator::TORRENT_VOLATILE {
                algorithm,
                should_url_encode,
                ..
            } => Self::TORRENT_VOLATILE {
                algorithm: algorithm.clone(),
                should_url_encode: *should_url_encode,
                state: default_shared_state(),
            },
            PeerIdGenerator::TORRENT_PERSISTENT {
                algorithm,
                should_url_encode,
                ..
            } => Self::TORRENT_PERSISTENT {
                algorithm: algorithm.clone(),
                should_url_encode: *should_url_encode,
                state: default_shared_state(),
            },
        }
    }
}

#[allow(clippy::match_same_arms)]
impl PartialEq for PeerIdGenerator {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::NEVER {
                    algorithm: left_algorithm,
                    should_url_encode: left_should_url_encode,
                    ..
                },
                Self::NEVER {
                    algorithm: right_algorithm,
                    should_url_encode: right_should_url_encode,
                    ..
                },
            )
            | (
                Self::ALWAYS {
                    algorithm: left_algorithm,
                    should_url_encode: left_should_url_encode,
                },
                Self::ALWAYS {
                    algorithm: right_algorithm,
                    should_url_encode: right_should_url_encode,
                },
            )
            | (
                Self::TORRENT_VOLATILE {
                    algorithm: left_algorithm,
                    should_url_encode: left_should_url_encode,
                    ..
                },
                Self::TORRENT_VOLATILE {
                    algorithm: right_algorithm,
                    should_url_encode: right_should_url_encode,
                    ..
                },
            )
            | (
                Self::TORRENT_PERSISTENT {
                    algorithm: left_algorithm,
                    should_url_encode: left_should_url_encode,
                    ..
                },
                Self::TORRENT_PERSISTENT {
                    algorithm: right_algorithm,
                    should_url_encode: right_should_url_encode,
                    ..
                },
            ) => {
                left_algorithm == right_algorithm
                    && left_should_url_encode == right_should_url_encode
            }
            (
                Self::TIMED {
                    refresh_every: left_refresh_every,
                    algorithm: left_algorithm,
                    should_url_encode: left_should_url_encode,
                    ..
                },
                Self::TIMED {
                    refresh_every: right_refresh_every,
                    algorithm: right_algorithm,
                    should_url_encode: right_should_url_encode,
                    ..
                },
            ) => {
                left_refresh_every == right_refresh_every
                    && left_algorithm == right_algorithm
                    && left_should_url_encode == right_should_url_encode
            }
            _ => false,
        }
    }
}

impl Eq for PeerIdGenerator {}

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

    pub fn get(&self, info_hash: &InfoHash, event: RequestEvent) -> Result<String, ClientError> {
        self.validate()?;
        match self {
            PeerIdGenerator::NEVER {
                algorithm, state, ..
            } => {
                let mut state = lock_state(state);
                if state.peer_id.is_none() {
                    state.peer_id = Some(generate_peer_id(algorithm)?);
                    state.last_generation = Some(Instant::now());
                }
                Ok(state.peer_id.clone().expect("peer-id initialized"))
            }
            PeerIdGenerator::ALWAYS { algorithm, .. } => generate_peer_id(algorithm),
            PeerIdGenerator::TIMED {
                refresh_every,
                algorithm,
                state,
                ..
            } => {
                let mut state = lock_state(state);
                let should_regenerate = state.last_generation.is_none_or(|last_generation| {
                    last_generation.elapsed() >= Duration::from_secs(*refresh_every as u64)
                });
                if should_regenerate {
                    state.last_generation = Some(Instant::now());
                    state.peer_id = Some(generate_peer_id(algorithm)?);
                }
                Ok(state.peer_id.clone().expect("peer-id initialized"))
            }
            PeerIdGenerator::TORRENT_VOLATILE {
                algorithm, state, ..
            } => {
                let mut state = lock_state(state);
                if !state.contains_key(info_hash) {
                    state.insert(info_hash.clone(), generate_peer_id(algorithm)?);
                }
                let peer_id = state.get(info_hash).cloned().expect("peer-id initialized");
                if event == RequestEvent::Stopped {
                    state.remove(info_hash);
                }
                Ok(peer_id)
            }
            PeerIdGenerator::TORRENT_PERSISTENT {
                algorithm, state, ..
            } => {
                let mut state = lock_state(state);
                if !state.peer_id_per_torrent.contains_key(info_hash) {
                    state.peer_id_per_torrent.insert(
                        info_hash.clone(),
                        AccessAwarePeerId::new(generate_peer_id(algorithm)?),
                    );
                }

                let peer_id = state
                    .peer_id_per_torrent
                    .get_mut(info_hash)
                    .expect("peer-id initialized")
                    .get_peer_id()
                    .to_owned();

                state.get_counter += 1;
                if state.get_counter >= TORRENT_PERSISTENT_SWEEP_EVERY_GETS {
                    state.get_counter = 0;
                    let now = Instant::now();
                    state
                        .peer_id_per_torrent
                        .retain(|_, entry| !entry.should_evict(now));
                }

                Ok(peer_id)
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

    fn regex_generator() -> PeerIdAlgorithmDef {
        PeerIdAlgorithmDef::REGEX(RegexPeerIdAlgorithm {
            pattern: "[A-Z0-9]{20}".to_owned(),
        })
    }

    fn timed_generator(refresh_every: i32) -> PeerIdGenerator {
        PeerIdGenerator::TIMED {
            refresh_every,
            algorithm: regex_generator(),
            should_url_encode: false,
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
        assert!(!generator.should_url_encode());
        assert!(matches!(generator, PeerIdGenerator::NEVER { .. }));
    }

    #[test]
    fn never_reuses_same_peer_id_forever() {
        let generator = PeerIdGenerator::NEVER {
            algorithm: regex_generator(),
            should_url_encode: false,
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
            algorithm: regex_generator(),
            should_url_encode: false,
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
    fn torrent_persistent_reuses_per_torrent_and_sweeps_every_thirty_gets() {
        let generator = PeerIdGenerator::TORRENT_PERSISTENT {
            algorithm: regex_generator(),
            should_url_encode: false,
            state: default_shared_state(),
        };
        let stale_torrent = info_hash(1);
        let hot_torrent = info_hash(2);

        let stale_value = generator.get(&stale_torrent, RequestEvent::None).unwrap();
        let hot_value = generator.get(&hot_torrent, RequestEvent::None).unwrap();

        let PeerIdGenerator::TORRENT_PERSISTENT { state, .. } = &generator else {
            panic!("expected persistent generator");
        };
        {
            let mut state = lock_state(state);
            state
                .peer_id_per_torrent
                .get_mut(&stale_torrent)
                .unwrap()
                .mark_stale_for_test();
            state.get_counter = 0;
        }

        for _ in 0..29 {
            assert_eq!(
                generator.get(&hot_torrent, RequestEvent::None).unwrap(),
                hot_value
            );
        }
        assert_eq!(lock_state(state).peer_id_per_torrent.len(), 2);

        assert_eq!(
            generator.get(&hot_torrent, RequestEvent::None).unwrap(),
            hot_value
        );
        assert_eq!(lock_state(state).peer_id_per_torrent.len(), 1);

        let stale_after_evict = generator.get(&stale_torrent, RequestEvent::None).unwrap();
        assert_ne!(stale_value, stale_after_evict);
    }

    #[test]
    fn rejects_generated_peer_id_with_wrong_length() {
        let generator = PeerIdGenerator::ALWAYS {
            algorithm: PeerIdAlgorithmDef::REGEX(RegexPeerIdAlgorithm {
                pattern: "[A-Z]{19}".to_owned(),
            }),
            should_url_encode: false,
        };

        let error = generator
            .get(&info_hash(1), RequestEvent::None)
            .unwrap_err();
        assert!(matches!(error, ClientError::Integrity(_)));
    }
}
