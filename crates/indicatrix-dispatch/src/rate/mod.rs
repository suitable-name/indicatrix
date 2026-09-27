//! [`RateModel`]: one lane's throughput estimate, and [`ChunkPolicy`]: how a chunk is
//! sized from it.
//!
//! Generalised from the desktop export's remote lane (`remote_chunk_samples`,
//! `remote_marginal_rate`, `calibrate_remote_rate`, the per-video rate carry) and the
//! hybrid split's 0.7-old/0.3-new moving average:
//!
//! 1. **Initial guess.** A lane that has never been measured starts from a guess and
//!    is handed a small calibration chunk ([`ChunkPolicy::calibration_samples`]) rather
//!    than a chunk sized from the guess, so a wildly wrong guess costs one short chunk.
//! 2. **First-chunk calibration.** The first valid measurement REPLACES the guess
//!    outright; a guess is not evidence worth averaging with.
//! 3. **Blending.** Every later measurement is folded in with an exponential moving
//!    average ([`DEFAULT_SMOOTHING`] weight on the new sample), so thermal drift and load
//!    changes are followed without one noisy chunk swinging the next chunk's size.
//!
//! A measurement is the lane's own reported marginal rate when it has one (a remote
//! worker's steady-state rate, excluding connection setup and upload), otherwise
//! `done / wall time` of the whole chunk ([`marginal_rate`]).

use std::time::Duration;

#[cfg(test)]
mod tests;

/// Weight of a new measurement in the moving average: 0.3 new, 0.7 old, the same
/// blend the hybrid CPU/GPU split uses.
pub const DEFAULT_SMOOTHING: f64 = 0.3;

/// The worker's hidden per-request cap (`MAX_SAMPLES_PER_REQUEST` in
/// `indicatrix-worker`'s request validation): a chunk sent to a worker must never be
/// larger, or the worker rejects the whole request.
pub const DEFAULT_MAX_CHUNK_SAMPLES: u32 = 65_536;

/// Throughput in samples per second: `delta_samples` traced over `elapsed`. `None`
/// when nothing was traced or the span is zero, so a degenerate measurement never
/// reaches a [`RateModel`].
#[must_use]
pub fn marginal_rate(delta_samples: u32, elapsed: Duration) -> Option<f64> {
    if delta_samples == 0 {
        return None;
    }
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    Some(f64::from(delta_samples) / secs)
}

/// How chunks are sized for a [`crate::LanePool`] run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPolicy {
    /// Target wall-clock duration of one chunk. Long enough that per-request overhead
    /// (handshake, upload, the final transfer) is negligible against it; short enough
    /// that a slow lane never holds the tail of the image hostage while every other
    /// lane idles.
    pub target: Duration,
    /// Floor for a rate-sized chunk: a zero, tiny or non-finite rate must not collapse
    /// the chunk to something overhead dominates.
    pub min_samples: u32,
    /// Ceiling for any chunk ([`DEFAULT_MAX_CHUNK_SAMPLES`] for remote workers).
    pub max_samples: u32,
    /// Size of an uncalibrated lane's first chunk (see the module doc).
    pub calibration_samples: u32,
}

impl ChunkPolicy {
    /// The still export and tilt video: about 22 s per chunk, a 32-sample floor
    /// (below that a remote round trip costs more than it saves), an 8-sample
    /// calibration probe -- the desktop export's existing numbers.
    pub const EXPORT: Self = Self {
        target: Duration::from_secs(22),
        min_samples: 32,
        max_samples: DEFAULT_MAX_CHUNK_SAMPLES,
        calibration_samples: 8,
    };

    /// Interactive (live-view) requests: about 1.5 s per chunk so progress arrives
    /// often, single-sample floor and calibration.
    pub const INTERACTIVE: Self = Self {
        target: Duration::from_millis(1500),
        min_samples: 1,
        max_samples: DEFAULT_MAX_CHUNK_SAMPLES,
        calibration_samples: 1,
    };

    /// Every chunk (calibration included) is exactly `samples` long, whatever the rate.
    /// The chunk partition of a run is then independent of timing (see
    /// [`crate::Merger`]'s determinism notes). `0` is treated as `1`.
    #[must_use]
    pub const fn fixed(samples: u32) -> Self {
        let samples = if samples == 0 { 1 } else { samples };
        Self {
            target: Duration::ZERO,
            min_samples: samples,
            max_samples: samples,
            calibration_samples: samples,
        }
    }

    /// The chunk size for a lane measured at `rate` samples per second: `rate *
    /// target`, rounded, clamped to `[min_samples, max_samples]`; `min_samples` for a
    /// non-finite or non-positive rate. Never `0`.
    #[must_use]
    pub fn samples_for_rate(&self, rate: f64) -> u32 {
        let (min, max) = self.bounds();
        let target = rate * self.target.as_secs_f64();
        if !target.is_finite() || target < f64::from(min) {
            return min;
        }
        if target >= f64::from(max) {
            return max;
        }
        (target.round() as u32).clamp(min, max)
    }

    /// `(min, max)` with both at least `1` and `min <= max`.
    fn bounds(&self) -> (u32, u32) {
        let max = self.max_samples.max(1);
        (self.min_samples.clamp(1, max), max)
    }

    /// The first chunk of an uncalibrated lane, clamped to `[1, max_samples]`.
    #[must_use]
    pub fn first_chunk_samples(&self) -> u32 {
        self.calibration_samples.clamp(1, self.bounds().1)
    }
}

/// One lane's throughput estimate in samples per second. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateModel {
    /// Used until the first measurement.
    guess: f64,
    /// `None` until calibrated.
    estimate: Option<f64>,
    /// Weight of a new measurement, in `(0, 1]`.
    smoothing: f64,
    /// Measurements folded in so far.
    observations: u32,
}

impl RateModel {
    /// An uncalibrated model starting from `initial_guess` samples per second. The
    /// guess only matters for reporting ([`Self::rate`]); an uncalibrated lane's first
    /// chunk is [`ChunkPolicy::first_chunk_samples`] long regardless.
    #[must_use]
    pub const fn new(initial_guess: f64) -> Self {
        Self {
            guess: initial_guess,
            estimate: None,
            smoothing: DEFAULT_SMOOTHING,
            observations: 0,
        }
    }

    /// A model already calibrated at `rate` (carried over from an earlier epoch of the
    /// same lane, e.g. the previous tilt-video frame), so no calibration chunk is spent.
    /// A non-finite or non-positive `rate` yields an uncalibrated model instead.
    #[must_use]
    pub fn calibrated(rate: f64) -> Self {
        let mut model = Self::new(rate);
        if valid_rate(rate) {
            model.estimate = Some(rate);
        }
        model
    }

    /// Replaces the moving-average weight of a new measurement; clamped to
    /// `[0.01, 1.0]` (`1.0` means "always trust the latest chunk").
    #[must_use]
    pub const fn with_smoothing(mut self, weight_of_new: f64) -> Self {
        self.smoothing = if weight_of_new.is_finite() {
            weight_of_new.clamp(0.01, 1.0)
        } else {
            DEFAULT_SMOOTHING
        };
        self
    }

    /// Whether at least one real measurement has replaced the guess.
    #[must_use]
    pub const fn is_calibrated(&self) -> bool {
        self.estimate.is_some()
    }

    /// The current estimate, or the initial guess while uncalibrated.
    #[must_use]
    pub fn rate(&self) -> f64 {
        self.estimate.unwrap_or(self.guess)
    }

    /// The calibrated estimate only (`None` while uncalibrated) -- what a caller
    /// carries into the next epoch with [`Self::calibrated`].
    #[must_use]
    pub const fn estimate(&self) -> Option<f64> {
        self.estimate
    }

    /// Measurements folded in so far.
    #[must_use]
    pub const fn observations(&self) -> u32 {
        self.observations
    }

    /// Folds in one finished chunk: `done` samples in `elapsed` wall time, with the
    /// lane's own `reported` marginal rate preferred when it is valid. Returns the
    /// measurement used, or `None` (model unchanged) when there was nothing to measure.
    pub fn observe(&mut self, done: u32, elapsed: Duration, reported: Option<f64>) -> Option<f64> {
        let measured = reported
            .filter(|&rate| valid_rate(rate))
            .or_else(|| marginal_rate(done, elapsed))
            .filter(|&rate| valid_rate(rate))?;
        self.estimate = Some(
            self.estimate
                .map_or(measured, |old| self.smoothing.mul_add(measured - old, old)),
        );
        self.observations += 1;
        Some(measured)
    }

    /// The size of this lane's next chunk under `policy`: the calibration chunk while
    /// uncalibrated, otherwise [`ChunkPolicy::samples_for_rate`] of the estimate.
    #[must_use]
    pub fn chunk_samples(&self, policy: &ChunkPolicy) -> u32 {
        self.estimate.map_or_else(
            || policy.first_chunk_samples(),
            |rate| policy.samples_for_rate(rate),
        )
    }
}

/// A usable throughput: finite and strictly positive.
fn valid_rate(rate: f64) -> bool {
    rate.is_finite() && rate > 0.0
}
