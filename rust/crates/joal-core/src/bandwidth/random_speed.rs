//! Random upload-speed sampler for the global bandwidth budget.
//!
//! Port of Java `org.araymond.joal.core.bandwith.RandomSpeedProvider`. Java
//! pulls `minUploadRate` / `maxUploadRate` from [`AppConfiguration`] (units:
//! **kilobytes per second**), multiplies by 1000 to reach bytes-per-second,
//! and samples a random value in `[min, max)` on every `refresh()`. This
//! port keeps the same behaviour but accepts an injectable
//! [`RandomSpeedSource`] so tests can make the sampler deterministic.
//!
//! [`AppConfiguration`]: crate::config::AppConfiguration

use rand::Rng;

use crate::config::AppConfiguration;

/// Pluggable sampler. Production uses [`ThreadRngSource`]; tests inject a
/// deterministic one (see this file's `tests` module).
pub trait RandomSpeedSource: Send + Sync + std::fmt::Debug {
    /// Return a value in `[min, max)` bytes/sec. Implementations must handle
    /// `min == max` by returning that value (Java behaviour).
    fn sample(&self, min_bytes_per_sec: u64, max_bytes_per_sec: u64) -> u64;
}

/// Default [`RandomSpeedSource`] — `rand::thread_rng().gen_range(min..max)`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ThreadRngSource;

impl RandomSpeedSource for ThreadRngSource {
    fn sample(&self, min_bytes_per_sec: u64, max_bytes_per_sec: u64) -> u64 {
        if min_bytes_per_sec == max_bytes_per_sec {
            return max_bytes_per_sec;
        }
        rand::thread_rng().gen_range(min_bytes_per_sec..max_bytes_per_sec)
    }
}

/// Owns the current sampled upload speed and refreshes it from the config.
///
/// Semantics match Java: `current_speed` is populated at construction time
/// (Java's `RandomSpeedProvider` constructor calls `refresh()`), and every
/// subsequent `refresh()` re-samples based on the current config values.
#[derive(Debug)]
pub struct RandomSpeedProvider {
    min_kib_per_sec: u64,
    max_kib_per_sec: u64,
    current_speed_bytes_per_sec: u64,
    source: Box<dyn RandomSpeedSource>,
}

impl RandomSpeedProvider {
    #[must_use]
    pub fn new(config: &AppConfiguration) -> Self {
        Self::with_source(config, Box::new(ThreadRngSource))
    }

    #[must_use]
    pub fn with_source(config: &AppConfiguration, source: Box<dyn RandomSpeedSource>) -> Self {
        let mut this = Self {
            min_kib_per_sec: config.min_upload_rate,
            max_kib_per_sec: config.max_upload_rate,
            current_speed_bytes_per_sec: 0,
            source,
        };
        this.refresh();
        this
    }

    /// Re-sample and update [`Self::current_speed`].
    pub fn refresh(&mut self) {
        let min = self.min_kib_per_sec.saturating_mul(1000);
        let max = self.max_kib_per_sec.saturating_mul(1000);
        self.current_speed_bytes_per_sec = self.source.sample(min, max);
    }

    /// Current sampled upload speed in bytes per second.
    #[must_use]
    pub const fn current_speed(&self) -> u64 {
        self.current_speed_bytes_per_sec
    }

    /// Re-read `min`/`max` from a (possibly reloaded) config without
    /// immediately refreshing; useful when hot-reloading `config.json`.
    pub fn update_limits(&mut self, config: &AppConfiguration) {
        self.min_kib_per_sec = config.min_upload_rate;
        self.max_kib_per_sec = config.max_upload_rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn config(min: u64, max: u64) -> AppConfiguration {
        AppConfiguration {
            min_upload_rate: min,
            max_upload_rate: max,
            simultaneous_seed: 1,
            client: "x.client".into(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
        }
    }

    #[derive(Debug, Default)]
    struct FixedSource {
        calls: Mutex<Vec<(u64, u64)>>,
        next: Mutex<Vec<u64>>,
    }

    impl FixedSource {
        fn new(values: Vec<u64>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                next: Mutex::new(values.into_iter().rev().collect()),
            }
        }
    }

    impl RandomSpeedSource for FixedSource {
        fn sample(&self, min: u64, max: u64) -> u64 {
            self.calls.lock().unwrap().push((min, max));
            self.next.lock().unwrap().pop().unwrap_or(max)
        }
    }

    #[test]
    fn constructor_seeds_current_speed() {
        let source = Box::new(FixedSource::new(vec![150_000]));
        let provider = RandomSpeedProvider::with_source(&config(100, 200), source);
        assert_eq!(provider.current_speed(), 150_000);
    }

    #[test]
    fn refresh_resamples_and_converts_kib_to_bytes() {
        let source = Box::new(FixedSource::new(vec![100_000, 175_500]));
        let mut provider = RandomSpeedProvider::with_source(&config(100, 200), source);
        assert_eq!(provider.current_speed(), 100_000);
        provider.refresh();
        assert_eq!(provider.current_speed(), 175_500);
    }

    #[test]
    fn produces_value_within_range_on_production_source() {
        let cfg = config(100, 200);
        let mut provider = RandomSpeedProvider::new(&cfg);
        for _ in 0..64 {
            provider.refresh();
            let kib = provider.current_speed() / 1000;
            assert!((100..200).contains(&kib), "out of range: {kib}");
        }
    }

    #[test]
    fn min_equals_max_returns_boundary() {
        let mut provider = RandomSpeedProvider::new(&config(50, 50));
        for _ in 0..8 {
            provider.refresh();
            assert_eq!(provider.current_speed(), 50_000);
        }
    }
}
