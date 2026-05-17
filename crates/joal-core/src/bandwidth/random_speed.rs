//! Random upload-speed sampler for the global bandwidth budget.
//!
//! Originally a port of Java `org.araymond.joal.core.bandwith.RandomSpeedProvider`,
//! which redrew a uniform `[min, max)` sample on every 20-minute refresh and
//! held that value constant in between. The Rust port keeps the
//! `[min, max]` band semantics and the kB-per-second config units (multiplied
//! by 1000 to reach bytes-per-second) but **evolves the speed continuously**
//! via [`RandomSpeedProvider::step`] — a reflected symmetric random walk that
//! the dispatcher advances every tick. Long-term mean is still `(min+max)/2`
//! so the tracker-visible cumulative `uploaded` curve is unaffected, but the
//! instantaneous speed is no longer pinned for tens of minutes.
//!
//! [`AppConfiguration`]: crate::config::AppConfiguration

use rand::Rng;

use crate::config::AppConfiguration;

/// Per-tick step ceiling, expressed as a fraction of `(max - min)` bytes/sec.
///
/// At the dispatcher's default 1s tick this is also the maximum *per-second*
/// change. Tuned for a "gentle" feel — speeds change slowly enough to look
/// like real-world bandwidth drift but fast enough to never appear frozen
/// between tracker announces (the original 20-minute hold this replaces).
const MAX_STEP_PER_TICK_RATIO: f64 = 0.02;

/// Pluggable sampler. Production uses [`ThreadRngSource`]; tests inject a
/// deterministic one (see this file's `tests` module).
pub trait RandomSpeedSource: Send + Sync + std::fmt::Debug {
    /// Return a value in `[min, max)` bytes/sec. Implementations must handle
    /// `min == max` by returning that value (Java behaviour).
    fn sample(&self, min_bytes_per_sec: u64, max_bytes_per_sec: u64) -> u64;

    /// Return a value in `[-max_abs, max_abs]` bytes/sec — the per-tick walk
    /// increment used by [`RandomSpeedProvider::step`].
    ///
    /// Defaults to `0` so existing test sources that only care about
    /// [`sample`] keep their behaviour (i.e. constant speed).
    /// Production [`ThreadRngSource`] and walk-aware tests override this.
    fn sample_delta(&self, max_abs_bytes_per_sec: u64) -> i64 {
        let _ = max_abs_bytes_per_sec;
        0
    }
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

    fn sample_delta(&self, max_abs_bytes_per_sec: u64) -> i64 {
        if max_abs_bytes_per_sec == 0 {
            return 0;
        }
        let bound = i64::try_from(max_abs_bytes_per_sec).unwrap_or(i64::MAX);
        rand::thread_rng().gen_range(-bound..=bound)
    }
}

/// Mirror `x` into `[min, max]` using a triangular wave so a random-walk step
/// never escapes the configured band. Equivalent to repeatedly bouncing off
/// the walls but folds in O(1) for any delta magnitude.
///
/// Long-term result: a uniform stationary distribution on `[min, max]`, so
/// the mean of a walk driven by a symmetric `sample_delta` is `(min+max)/2`
/// — which is exactly what the original uniform `sample(min, max)` produced.
fn reflect(x: i128, min: u64, max: u64) -> u64 {
    let lo = i128::from(min);
    let hi = i128::from(max);
    if hi <= lo {
        return min;
    }
    let span = hi - lo;
    let two_span = span * 2;
    let mut y = ((x - lo) % two_span + two_span) % two_span;
    if y > span {
        y = two_span - y;
    }
    u64::try_from(lo + y).unwrap_or(min)
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

    /// Re-sample and update [`Self::current_speed`]. Used to "jump" to a fresh
    /// random starting point — not called every tick (see [`Self::step`]).
    pub fn refresh(&mut self) {
        let min = self.min_kib_per_sec.saturating_mul(1000);
        let max = self.max_kib_per_sec.saturating_mul(1000);
        self.current_speed_bytes_per_sec = self.source.sample(min, max);
    }

    /// Advance the current speed by one tick of a reflected random walk.
    ///
    /// The delta is drawn from [`RandomSpeedSource::sample_delta`] with
    /// bound `MAX_STEP_PER_TICK_RATIO * (max - min)`; the new position is
    /// reflected back into `[min, max]` so the walker never escapes the band.
    /// On `min == max` the speed is pinned to the boundary (matches the
    /// original `sample()` semantics).
    pub fn step(&mut self) {
        let min = self.min_kib_per_sec.saturating_mul(1000);
        let max = self.max_kib_per_sec.saturating_mul(1000);
        if min >= max {
            self.current_speed_bytes_per_sec = max;
            return;
        }
        let span = max - min;
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let max_step = (((span as f64) * MAX_STEP_PER_TICK_RATIO) as u64).max(1);
        let delta = self.source.sample_delta(max_step);
        let next = i128::from(self.current_speed_bytes_per_sec) + i128::from(delta);
        self.current_speed_bytes_per_sec = reflect(next, min, max);
    }

    /// Current sampled upload speed in bytes per second.
    #[must_use]
    pub const fn current_speed(&self) -> u64 {
        self.current_speed_bytes_per_sec
    }

    /// Re-read `min`/`max` from a (possibly reloaded) config without
    /// immediately refreshing; useful when hot-reloading `config.json`.
    /// Clamps the current speed into the new band so the walk doesn't
    /// resume from a stale, out-of-range position.
    pub fn update_limits(&mut self, config: &AppConfiguration) {
        self.min_kib_per_sec = config.min_upload_rate;
        self.max_kib_per_sec = config.max_upload_rate;
        let min = self.min_kib_per_sec.saturating_mul(1000);
        let max = self.max_kib_per_sec.saturating_mul(1000);
        self.current_speed_bytes_per_sec = self.current_speed_bytes_per_sec.clamp(min, max);
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

    /// Reflected random-walk step. No-op when the faker is disabled (0/0) so
    /// the dispatcher hot-path retains its "no work for disabled download"
    /// optimisation.
    pub fn step(&mut self) {
        if !self.is_enabled() {
            self.current_speed_bytes_per_sec = 0;
            return;
        }
        let min = self.min_kib_per_sec.saturating_mul(1000);
        let max = self.max_kib_per_sec.saturating_mul(1000);
        if min >= max {
            self.current_speed_bytes_per_sec = max;
            return;
        }
        let span = max - min;
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let max_step = (((span as f64) * MAX_STEP_PER_TICK_RATIO) as u64).max(1);
        let delta = self.source.sample_delta(max_step);
        let next = i128::from(self.current_speed_bytes_per_sec) + i128::from(delta);
        self.current_speed_bytes_per_sec = reflect(next, min, max);
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
        let min = self.min_kib_per_sec.saturating_mul(1000);
        let max = self.max_kib_per_sec.saturating_mul(1000);
        self.current_speed_bytes_per_sec = self.current_speed_bytes_per_sec.clamp(min, max);
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
        delta_bounds: Mutex<Vec<u64>>,
        next_deltas: Mutex<Vec<i64>>,
    }

    impl FixedSource {
        fn new(values: Vec<u64>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                next: Mutex::new(values.into_iter().rev().collect()),
                delta_bounds: Mutex::new(Vec::new()),
                next_deltas: Mutex::new(Vec::new()),
            }
        }

        fn with_deltas(values: Vec<u64>, deltas: Vec<i64>) -> Self {
            let s = Self::new(values);
            *s.next_deltas.lock().unwrap() = deltas.into_iter().rev().collect();
            s
        }
    }

    impl RandomSpeedSource for FixedSource {
        fn sample(&self, min: u64, max: u64) -> u64 {
            self.calls.lock().unwrap().push((min, max));
            self.next.lock().unwrap().pop().unwrap_or(max)
        }

        fn sample_delta(&self, max_abs: u64) -> i64 {
            self.delta_bounds.lock().unwrap().push(max_abs);
            self.next_deltas.lock().unwrap().pop().unwrap_or(0)
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

    // ----- step() / reflected random walk -----

    #[test]
    fn step_keeps_value_within_bounds() {
        // Start mid-range, take a few small symmetric steps — should stay in [min,max].
        let source = Box::new(FixedSource::with_deltas(
            vec![150_000],
            vec![-500, 800, -200, 1_500, -1_200],
        ));
        let mut provider = RandomSpeedProvider::with_source(&config(100, 200), source);
        for _ in 0..5 {
            provider.step();
            let v = provider.current_speed();
            assert!((100_000..=200_000).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn step_reflects_when_crossing_max() {
        // Range [100_000, 200_000] bps, max_step = 2% * 100_000 = 2_000.
        // Start near the top, request a +2_000 delta — should land at max
        // (199_000 + 2_000 = 201_000, reflected -> 199_000).
        let source = Box::new(FixedSource::with_deltas(vec![199_000], vec![2_000]));
        let mut provider = RandomSpeedProvider::with_source(&config(100, 200), source);
        provider.step();
        assert_eq!(provider.current_speed(), 199_000);
    }

    #[test]
    fn step_reflects_when_crossing_min() {
        let source = Box::new(FixedSource::with_deltas(vec![101_000], vec![-2_000]));
        let mut provider = RandomSpeedProvider::with_source(&config(100, 200), source);
        provider.step();
        // 101_000 + (-2_000) = 99_000, reflected at 100_000 -> 101_000.
        assert_eq!(provider.current_speed(), 101_000);
    }

    #[test]
    fn step_is_noop_when_min_equals_max() {
        let source = Box::new(FixedSource::with_deltas(
            vec![50_000],
            vec![999_999, -999_999],
        ));
        let mut provider = RandomSpeedProvider::with_source(&config(50, 50), source);
        provider.step();
        provider.step();
        assert_eq!(provider.current_speed(), 50_000);
    }

    /// Long-run mean ≈ midpoint. A reflected symmetric random walk on
    /// `[min, max]` has a uniform stationary distribution, so over enough
    /// steps the average value sits near `(min+max)/2`. The walk's
    /// autocorrelation time is `~(span/step)^2` steps, so we run for a
    /// solid multiple of that to dampen statistical noise before asserting.
    #[test]
    fn long_run_mean_close_to_midpoint() {
        let cfg = config(100, 200);
        let mut provider = RandomSpeedProvider::new(&cfg);
        let mut sum: u128 = 0;
        let n: u128 = 200_000;
        for _ in 0..n {
            provider.step();
            sum += u128::from(provider.current_speed());
        }
        #[allow(clippy::cast_possible_truncation)]
        let mean = (sum / n) as u64;
        let mid = 150_000_u64;
        // 15% tolerance: ~4σ for the effective sample size (~26 independent
        // samples after dividing by an autocorrelation time of ~7_500).
        let tolerance = mid * 15 / 100;
        assert!(
            mean.abs_diff(mid) <= tolerance,
            "long-run mean {mean} not within 15% of midpoint {mid}",
        );
    }

    #[test]
    fn update_limits_clamps_current_speed_into_new_band() {
        // Start at 180_000 inside [100_000, 200_000], then shrink to [100_000, 150_000].
        let source = Box::new(FixedSource::new(vec![180_000]));
        let mut provider = RandomSpeedProvider::with_source(&config(100, 200), source);
        assert_eq!(provider.current_speed(), 180_000);
        provider.update_limits(&config(100, 150));
        assert_eq!(provider.current_speed(), 150_000);
    }

    #[test]
    fn update_limits_clamps_below_new_min() {
        let source = Box::new(FixedSource::new(vec![60_000]));
        let mut provider = RandomSpeedProvider::with_source(&config(50, 100), source);
        provider.update_limits(&config(80, 100));
        assert_eq!(provider.current_speed(), 80_000);
    }

    #[test]
    fn download_step_stays_zero_when_disabled() {
        let source = Box::new(FixedSource::with_deltas(vec![0], vec![5_000, -5_000]));
        let mut provider = DownloadSpeedProvider::with_source(&config_with_download(0, 0), source);
        provider.step();
        provider.step();
        assert!(!provider.is_enabled());
        assert_eq!(provider.current_speed(), 0);
    }

    #[test]
    fn download_step_walks_when_enabled() {
        // Range 50..100 KiB/s -> 50_000..100_000 bps, max_step = 2% * 50_000 = 1_000.
        let source = Box::new(FixedSource::with_deltas(vec![60_000], vec![700, -300]));
        let mut provider =
            DownloadSpeedProvider::with_source(&config_with_download(50, 100), source);
        assert_eq!(provider.current_speed(), 60_000);
        provider.step();
        assert_eq!(provider.current_speed(), 60_700);
        provider.step();
        assert_eq!(provider.current_speed(), 60_400);
    }

    #[test]
    fn reflect_helper_handles_large_overshoot() {
        // Just past max: bounces back by the overshoot amount.
        assert_eq!(reflect(210_000, 100_000, 200_000), 190_000);
        // Just below min: bounces up.
        assert_eq!(reflect(90_000, 100_000, 200_000), 110_000);
        // 1.5 * span past min: 100_000 + 150_000 = 250_000.
        //   y = (150_000) mod 200_000 = 150_000, which > span(100_000)
        //   → y = 200_000 - 150_000 = 50_000 → result 150_000.
        assert_eq!(reflect(250_000, 100_000, 200_000), 150_000);
        // Values already in range pass through unchanged.
        assert_eq!(reflect(123_456, 100_000, 200_000), 123_456);
    }
}
