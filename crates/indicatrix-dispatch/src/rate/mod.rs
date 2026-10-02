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
//! worker's rendering rate: the samples between its first and its last progress report
//! that advanced the count, over the time between those two reports, so connection
//! setup, the final frame's encode and upload and the merge are all excluded; a chunk
//! too short to have two such reports reports its whole first-progress-to-done rate
//! instead), otherwise `done / wall time` of the whole chunk ([`marginal_rate`]).
//!
//! # Staggered lanes
//!
//! Two lanes backed by the same worker would otherwise start together with equal
//! chunks, finish together and upload their frames together, leaving the worker's GPU
//! idle during the shared upload. A model built with [`RateModel::staggered`] carries a
//! one-shot scale ([`RateModel::take_first_chunk_scale`]) that the pool applies to the
//! lane's first calibrated chunk only, so that lane starts half a chunk out of phase:
//! one lane renders while the other uploads, for the rest of the run.

use std::time::Duration;

#[cfg(test)]
mod tests;

/// Weight of a new measurement in the moving average: 0.3 new, 0.7 old, the same
/// blend the hybrid CPU/GPU split uses.
pub const DEFAULT_SMOOTHING: f64 = 0.3;

/// The first-chunk factor of a [`RateModel::staggered`] lane: half a chunk, so two lanes
/// of one worker end up half a chunk apart.
const STAGGER_SCALE: f64 = 0.5;

/// The worker's hidden per-request cap: a chunk sent to a worker must never be larger,
/// or the worker rejects the whole request.
///
/// The one definition: `indicatrix-worker`'s request validation
/// (`MAX_SAMPLES_PER_REQUEST`) and the desktop's live remote lane
/// (`LIVE_CHUNK_MAX_SAMPLES`) re-export it under their own names.
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
        self.clamp_samples(rate * self.target.as_secs_f64())
    }

    /// `samples` multiplied by `scale`, rounded and clamped to `[min_samples,
    /// max_samples]` exactly like [`Self::samples_for_rate`]; `min_samples` for a
    /// non-finite product. Never `0`.
    #[must_use]
    pub(crate) fn scaled_samples(&self, samples: u32, scale: f64) -> u32 {
        self.clamp_samples(f64::from(samples) * scale)
    }

    /// `wanted` rounded to a whole chunk size inside `(min, max)` (see [`Self::bounds`]);
    /// `min` for a non-finite or too-small value, `max` for a too-large one.
    fn clamp_samples(&self, wanted: f64) -> u32 {
        let (min, max) = self.bounds();
        if !wanted.is_finite() || wanted < f64::from(min) {
            return min;
        }
        if wanted >= f64::from(max) {
            return max;
        }
        (wanted.round() as u32).clamp(min, max)
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

    /// Tail-aware, share-aware chunk size for a lane measured at `rate`, given the
    /// run's whole outstanding sample count `remaining` (every lane, not just this one)
    /// and `sum_rates`, the sum of every lane's current rate (guess or estimate):
    ///
    /// ```text
    /// want = min(rate * target_secs, ceil(remaining * rate / sum_rates))
    /// ```
    ///
    /// clamped to `[min_samples, max_samples]` exactly like [`Self::samples_for_rate`].
    /// The first term is the plain target-duration chunk; the second is this lane's
    /// proportional share of what is left. Taking the smaller keeps ordinary chunk
    /// sizing unchanged while samples are plentiful (the share term is then far
    /// larger than the target term) and only shrinks the LAST few chunks of a run, so a
    /// slow lane can no longer claim a disproportionate slice of a small tail and force
    /// a fast lane sitting idle in [`crate::pool::epoch::Epoch::claim`] to wait out that
    /// whole oversized chunk.
    ///
    /// Falls back to plain [`Self::samples_for_rate`] when `remaining` is `0` or
    /// `sum_rates` is non-finite/non-positive (a single lane's `sum_rates` is its own
    /// `rate`, so this only ever changes anything once `remaining` gets small relative
    /// to that lane's own target-duration chunk -- exactly the run's tail).
    #[must_use]
    pub fn tail_aware_samples(&self, rate: f64, remaining: u32, sum_rates: f64) -> u32 {
        let by_target = self.samples_for_rate(rate);
        if remaining == 0 || !sum_rates.is_finite() || sum_rates <= 0.0 {
            return by_target;
        }
        let share = (f64::from(remaining) * rate / sum_rates).ceil();
        if !share.is_finite() {
            return by_target;
        }
        let (min, max) = self.bounds();
        let share_samples = if share <= 0.0 {
            min
        } else if share >= f64::from(max) {
            max
        } else {
            (share as u32).clamp(min, max)
        };
        by_target.min(share_samples).max(min)
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
    /// A one-shot factor for the lane's first calibrated chunk (see the module doc's
    /// "Staggered lanes"); `None` once taken, and for every model not built by
    /// [`Self::staggered`].
    first_chunk_scale: Option<f64>,
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
            first_chunk_scale: None,
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

    /// [`Self::calibrated`] at `rate`, whose first chunk is only half the size its rate
    /// asks for ([`Self::take_first_chunk_scale`] yields `Some(0.5)` once), so this lane
    /// runs half a chunk out of phase with another lane of the same worker that starts
    /// from [`Self::calibrated`]. An unusable `rate` yields an uncalibrated model with no
    /// scale: there is no calibrated first chunk to halve.
    #[must_use]
    pub fn staggered(rate: f64) -> Self {
        let mut model = Self::calibrated(rate);
        if model.is_calibrated() {
            model.first_chunk_scale = Some(STAGGER_SCALE);
        }
        model
    }

    /// Returns the factor to apply to this lane's first calibrated chunk and clears it,
    /// so every later call yields `None`. Always `None` for a model not built by
    /// [`Self::staggered`].
    pub const fn take_first_chunk_scale(&mut self) -> Option<f64> {
        self.first_chunk_scale.take()
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
