use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rand_regex::Regex as RandRegex;
use regex_syntax::ParserBuilder;

use crate::client::error::ClientError;

pub(super) const TORRENT_PERSISTENT_TTL: Duration = Duration::from_hours(2);

pub(super) fn compile_rand_regex(pattern: &str) -> Result<RandRegex, ClientError> {
    let hir = ParserBuilder::new()
        .build()
        .parse(pattern)
        .map_err(|e| ClientError::InvalidRegex(format!("{pattern}: {e}")))?;
    RandRegex::with_hir(hir, 100).map_err(|e| ClientError::InvalidRegex(format!("{pattern}: {e}")))
}

pub(super) fn string_from_ascii_regex_bytes(bytes: Vec<u8>) -> Result<String, ClientError> {
    String::from_utf8(bytes).map_err(|e| ClientError::NonUtf8Output(e.to_string()))
}

pub(super) fn lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn default_shared_state<T: Default>() -> Arc<Mutex<T>> {
    Arc::new(Mutex::new(T::default()))
}

#[derive(Debug, Clone, Default)]
pub(super) struct TimedState {
    pub value: Option<String>,
    pub last_generation: Option<Instant>,
}

#[derive(Debug, Clone)]
pub(super) struct AccessAwareEntry {
    value: String,
    last_access: Instant,
    #[cfg(test)]
    force_stale: bool,
}

impl AccessAwareEntry {
    pub fn new(value: String) -> Self {
        Self {
            value,
            last_access: Instant::now(),
            #[cfg(test)]
            force_stale: false,
        }
    }

    pub fn get(&mut self) -> &str {
        self.last_access = Instant::now();
        #[cfg(test)]
        {
            self.force_stale = false;
        }
        &self.value
    }

    pub fn should_evict(&self, now: Instant) -> bool {
        #[cfg(test)]
        if self.force_stale {
            return true;
        }
        now.duration_since(self.last_access) >= TORRENT_PERSISTENT_TTL
    }

    #[cfg(test)]
    pub fn mark_stale_for_test(&mut self) {
        self.force_stale = true;
    }
}
