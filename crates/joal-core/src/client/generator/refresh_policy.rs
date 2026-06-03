use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::event::RequestEvent;
use crate::torrent::InfoHash;

use super::common::{AccessAwareEntry, TimedState, default_shared_state, lock_state};

pub trait GenerateValue:
    Clone + Debug + PartialEq + Eq + Serialize + for<'de> Deserialize<'de>
{
    fn generate(&self) -> Result<Vec<u8>, ClientError>;
    fn validate(&self) -> Result<(), ClientError>;
}

#[allow(non_camel_case_types, private_interfaces)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "refreshOn")]
#[serde(bound = "C: GenerateValue")]
pub enum RefreshPolicy<C: GenerateValue> {
    NEVER {
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<TimedState>")]
        state: Arc<Mutex<TimedState>>,
    },
    ALWAYS {
        #[serde(flatten)]
        config: C,
    },
    TIMED {
        #[serde(rename = "refreshEvery")]
        refresh_every: i32,
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<TimedState>")]
        state: Arc<Mutex<TimedState>>,
    },
    TIMED_OR_AFTER_STARTED_ANNOUNCE {
        #[serde(rename = "refreshEvery")]
        refresh_every: i32,
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<TimedState>")]
        state: Arc<Mutex<TimedState>>,
    },
    TORRENT_VOLATILE {
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<HashMap<InfoHash, Vec<u8>>>")]
        state: Arc<Mutex<HashMap<InfoHash, Vec<u8>>>>,
    },
    TORRENT_PERSISTENT {
        #[serde(flatten)]
        config: C,
        #[serde(
            skip,
            default = "default_shared_state::<HashMap<InfoHash, AccessAwareEntry>>"
        )]
        state: Arc<Mutex<HashMap<InfoHash, AccessAwareEntry>>>,
    },
}

impl<C: GenerateValue> Clone for RefreshPolicy<C> {
    fn clone(&self) -> Self {
        match self {
            Self::NEVER { config, .. } => Self::NEVER {
                config: config.clone(),
                state: default_shared_state(),
            },
            Self::ALWAYS { config } => Self::ALWAYS {
                config: config.clone(),
            },
            Self::TIMED {
                refresh_every,
                config,
                ..
            } => Self::TIMED {
                refresh_every: *refresh_every,
                config: config.clone(),
                state: default_shared_state(),
            },
            Self::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                refresh_every,
                config,
                ..
            } => Self::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                refresh_every: *refresh_every,
                config: config.clone(),
                state: default_shared_state(),
            },
            Self::TORRENT_VOLATILE { config, .. } => Self::TORRENT_VOLATILE {
                config: config.clone(),
                state: default_shared_state(),
            },
            Self::TORRENT_PERSISTENT { config, .. } => Self::TORRENT_PERSISTENT {
                config: config.clone(),
                state: default_shared_state(),
            },
        }
    }
}

impl<C: GenerateValue> PartialEq for RefreshPolicy<C> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NEVER { config: l, .. }, Self::NEVER { config: r, .. })
            | (Self::ALWAYS { config: l }, Self::ALWAYS { config: r })
            | (
                Self::TORRENT_VOLATILE { config: l, .. },
                Self::TORRENT_VOLATILE { config: r, .. },
            )
            | (
                Self::TORRENT_PERSISTENT { config: l, .. },
                Self::TORRENT_PERSISTENT { config: r, .. },
            ) => l == r,
            (
                Self::TIMED {
                    refresh_every: l_re,
                    config: l,
                    ..
                },
                Self::TIMED {
                    refresh_every: r_re,
                    config: r,
                    ..
                },
            )
            | (
                Self::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                    refresh_every: l_re,
                    config: l,
                    ..
                },
                Self::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                    refresh_every: r_re,
                    config: r,
                    ..
                },
            ) => l_re == r_re && l == r,
            _ => false,
        }
    }
}

impl<C: GenerateValue> Eq for RefreshPolicy<C> {}

impl<C: GenerateValue> RefreshPolicy<C> {
    pub fn config(&self) -> &C {
        match self {
            Self::NEVER { config, .. }
            | Self::ALWAYS { config }
            | Self::TIMED { config, .. }
            | Self::TIMED_OR_AFTER_STARTED_ANNOUNCE { config, .. }
            | Self::TORRENT_VOLATILE { config, .. }
            | Self::TORRENT_PERSISTENT { config, .. } => config,
        }
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        self.config().validate()?;
        if let Self::TIMED { refresh_every, .. }
        | Self::TIMED_OR_AFTER_STARTED_ANNOUNCE { refresh_every, .. } = self
            && *refresh_every < 1
        {
            return Err(ClientError::Integrity(
                "refreshEvery must be greater than 0".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn get(&self, info_hash: &InfoHash, event: RequestEvent) -> Result<Vec<u8>, ClientError> {
        self.validate()?;
        match self {
            Self::NEVER { config, state, .. } => cached_value(config, state),
            Self::ALWAYS { config } => config.generate(),
            Self::TIMED {
                refresh_every,
                config,
                state,
            } => timed_value(config, state, *refresh_every),
            Self::TIMED_OR_AFTER_STARTED_ANNOUNCE {
                refresh_every,
                config,
                state,
            } => timed_or_started_value(config, state, *refresh_every, event),
            Self::TORRENT_VOLATILE { config, state } => {
                torrent_volatile_value(config, state, info_hash, event)
            }
            Self::TORRENT_PERSISTENT { config, state } => {
                torrent_persistent_value(config, state, info_hash)
            }
        }
    }
}

fn cached_value<C: GenerateValue>(
    config: &C,
    state: &Arc<Mutex<TimedState>>,
) -> Result<Vec<u8>, ClientError> {
    let mut state = lock_state(state);
    ensure_timed_value(config, &mut state)?;
    initialized_value(&state)
}

fn timed_value<C: GenerateValue>(
    config: &C,
    state: &Arc<Mutex<TimedState>>,
    refresh_every: i32,
) -> Result<Vec<u8>, ClientError> {
    let mut state = lock_state(state);
    if timed_value_expired(&state, refresh_every) {
        regenerate_timed_value(config, &mut state)?;
    }
    initialized_value(&state)
}

fn timed_or_started_value<C: GenerateValue>(
    config: &C,
    state: &Arc<Mutex<TimedState>>,
    refresh_every: i32,
    event: RequestEvent,
) -> Result<Vec<u8>, ClientError> {
    let mut state = lock_state(state);
    if timed_value_expired(&state, refresh_every) {
        regenerate_timed_value(config, &mut state)?;
    }
    let value = initialized_value(&state)?;
    if event == RequestEvent::Started {
        regenerate_timed_value(config, &mut state)?;
    }
    Ok(value)
}

fn torrent_volatile_value<C: GenerateValue>(
    config: &C,
    state: &Arc<Mutex<HashMap<InfoHash, Vec<u8>>>>,
    info_hash: &InfoHash,
    event: RequestEvent,
) -> Result<Vec<u8>, ClientError> {
    let mut state = lock_state(state);
    let value = if let Some(value) = state.get(info_hash) {
        value.clone()
    } else {
        let value = config.generate()?;
        state.insert(info_hash.clone(), value.clone());
        value
    };
    if event == RequestEvent::Stopped {
        state.remove(info_hash);
    }
    Ok(value)
}

fn torrent_persistent_value<C: GenerateValue>(
    config: &C,
    state: &Arc<Mutex<HashMap<InfoHash, AccessAwareEntry>>>,
    info_hash: &InfoHash,
) -> Result<Vec<u8>, ClientError> {
    let mut state = lock_state(state);
    if !state.contains_key(info_hash) {
        state.insert(info_hash.clone(), AccessAwareEntry::new(config.generate()?));
    }
    let value = state
        .get_mut(info_hash)
        .ok_or_else(|| ClientError::Integrity("missing persistent torrent value".to_owned()))?
        .get()
        .to_owned();
    let now = Instant::now();
    state.retain(|_, entry| !entry.should_evict(now));
    Ok(value)
}

fn ensure_timed_value<C: GenerateValue>(
    config: &C,
    state: &mut TimedState,
) -> Result<(), ClientError> {
    if state.value.is_none() {
        regenerate_timed_value(config, state)?;
    }
    Ok(())
}

fn regenerate_timed_value<C: GenerateValue>(
    config: &C,
    state: &mut TimedState,
) -> Result<(), ClientError> {
    state.value = Some(config.generate()?);
    state.last_generation = Some(Instant::now());
    Ok(())
}

fn timed_value_expired(state: &TimedState, refresh_every: i32) -> bool {
    state.last_generation.is_none_or(|last_generation| {
        last_generation.elapsed() >= Duration::from_secs(refresh_every as u64)
    })
}

fn initialized_value(state: &TimedState) -> Result<Vec<u8>, ClientError> {
    state
        .value
        .clone()
        .ok_or_else(|| ClientError::Integrity("missing generated refresh-policy value".to_owned()))
}
