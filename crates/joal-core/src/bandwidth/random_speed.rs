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
    fn sample_delta(&self, _max_abs_bytes_per_sec: u64) -> i64 {
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
