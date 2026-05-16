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

/// Same shape as [`RandomSpeedProvider`] but driven by `min/maxDownloadRate`.
///
/// Kept as a sibling type rather than a generic so call sites read clearly
/// (`download.current_speed()` vs. `upload.current_speed()`) and so the
/// common case — download faker disabled (0/0) — short-circuits to a fixed
/// `0` with no allocation.
#[derive(Debug)]
pub struct DownloadSpeedProvider {
    min_kib_per_sec: u64,
    max_kib_per_sec: u64,
    current_speed_bytes_per_sec: u64,
    source: Box<dyn RandomSpeedSource>,
}

impl DownloadSpeedProvider {
    #[must_use]
    pub fn new(config: &AppConfiguration) -> Self {
        Self::with_source(config, Box::new(ThreadRngSource))
    }

    #[must_use]
    pub fn with_source(config: &AppConfiguration, source: Box<dyn RandomSpeedSource>) -> Self {
        let mut this = Self {
            min_kib_per_sec: config.min_download_rate,
            max_kib_per_sec: config.max_download_rate,
            current_speed_bytes_per_sec: 0,
            source,
        };
        this.refresh();
        this
    }

    pub fn refresh(&mut self) {
        if self.min_kib_per_sec == 0 && self.max_kib_per_sec == 0 {
            self.current_speed_bytes_per_sec = 0;
            return;
        }
        let min = self.min_kib_per_sec.saturating_mul(1000);
        let max = self.max_kib_per_sec.saturating_mul(1000);
        self.current_speed_bytes_per_sec = self.source.sample(min, max);
    }

    #[must_use]
    pub const fn current_speed(&self) -> u64 {
        self.current_speed_bytes_per_sec
    }

    /// Whether the download faker is enabled (both bounds non-zero is enough,
    /// 0/0 disables it entirely so the dispatcher hot-path can skip work).
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !(self.min_kib_per_sec == 0 && self.max_kib_per_sec == 0)
    }

    pub fn update_limits(&mut self, config: &AppConfiguration) {
        self.min_kib_per_sec = config.min_download_rate;
        self.max_kib_per_sec = config.max_download_rate;
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
            min_download_rate: 0,
            max_download_rate: 0,
            simultaneous_seed: 1,
            client: "x.client".into(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
            proxy_host: None,
            proxy_port: None,
        }
    }

    fn config_with_download(min_dl: u64, max_dl: u64) -> AppConfiguration {
        AppConfiguration {
            min_upload_rate: 0,
            max_upload_rate: 0,
            min_download_rate: min_dl,
            max_download_rate: max_dl,
            simultaneous_seed: 1,
            client: "x.client".into(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
            proxy_host: None,
            proxy_port: None,
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

    #[test]
    fn download_provider_disabled_when_both_zero() {
        // The hot-path optimisation: 0/0 stays 0 without ever calling the
        // sampler, so a misconfigured rng would not leak in.
        let source = Box::new(FixedSource::new(vec![999_999]));
        let provider = DownloadSpeedProvider::with_source(&config_with_download(0, 0), source);
        assert!(!provider.is_enabled());
        assert_eq!(provider.current_speed(), 0);
    }

    #[test]
    fn download_provider_samples_when_enabled() {
        let source = Box::new(FixedSource::new(vec![60_000, 90_000]));
        let mut provider =
            DownloadSpeedProvider::with_source(&config_with_download(50, 100), source);
        assert!(provider.is_enabled());
        assert_eq!(provider.current_speed(), 60_000);
        provider.refresh();
        assert_eq!(provider.current_speed(), 90_000);
    }

    #[test]
    fn download_provider_update_limits_can_disable_at_runtime() {
        let source = Box::new(FixedSource::new(vec![60_000, 0]));
        let mut provider =
            DownloadSpeedProvider::with_source(&config_with_download(50, 100), source);
        assert_eq!(provider.current_speed(), 60_000);
        provider.update_limits(&config_with_download(0, 0));
        provider.refresh();
        assert!(!provider.is_enabled());
        assert_eq!(provider.current_speed(), 0);
    }
}
