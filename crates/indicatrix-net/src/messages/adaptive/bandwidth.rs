//! Link bandwidth estimation from the duration of blocking writes.
//!
//! [`BandwidthEstimator`] holds no clock: the sender times its own blocking write of one
//! frame and passes the byte count and the [`Duration`] in, so the estimator is a pure,
//! deterministic function of what it was fed.

use std::time::Duration;

/// Frames smaller than this many bytes never feed the estimator: the kernel send buffer
/// swallows a small write at memory speed, so its duration says nothing about the link.
pub const MIN_SAMPLE_BYTES: usize = 256 * 1024;

/// Samples are clamped to this many Mbit/s so a write the clock resolution made look
/// instantaneous cannot push the estimate to infinity.
pub const MAX_PLAUSIBLE_MBPS: f64 = 100_000.0;

/// Smoothing of a [`BandwidthEstimator`].
///
/// A new sample moves the estimate by `alpha * (sample - estimate)`. A sample below
/// `clearly_slower * estimate` uses `alpha_down` (react promptly to a slowing link), any
/// other sample uses `alpha_up` (distrust a lucky fast write).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandwidthConfig {
    /// Smoothing factor for samples at or above `clearly_slower * estimate`, in `(0, 1]`.
    pub alpha_up: f64,
    /// Smoothing factor for clearly slower samples, in `(0, 1]`.
    pub alpha_down: f64,
    /// A sample below this fraction of the estimate counts as clearly slower.
    pub clearly_slower: f64,
}

impl BandwidthConfig {
    /// Slow upward (0.15), fast downward (0.6) once a sample is below 80% of the estimate.
    pub const DEFAULT: Self = Self {
        alpha_up: 0.15,
        alpha_down: 0.6,
        clearly_slower: 0.8,
    };

    /// `self` with every factor forced into its valid range.
    fn sanitised(self) -> Self {
        let alpha = |a: f64| {
            if a.is_finite() {
                a.clamp(0.01, 1.0)
            } else {
                0.15
            }
        };
        Self {
            alpha_up: alpha(self.alpha_up),
            alpha_down: alpha(self.alpha_down),
            clearly_slower: if self.clearly_slower.is_finite() {
                self.clearly_slower.clamp(0.0, 1.0)
            } else {
                0.8
            },
        }
    }
}

impl Default for BandwidthConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// An exponentially weighted estimate of one connection's throughput in Mbit/s.
#[derive(Debug, Clone, PartialEq)]
pub struct BandwidthEstimator {
    config: BandwidthConfig,
    estimate: Option<f64>,
    samples: u64,
}

impl Default for BandwidthEstimator {
    fn default() -> Self {
        Self::new(BandwidthConfig::DEFAULT)
    }
}

impl BandwidthEstimator {
    /// An empty estimator with the given smoothing (invalid factors are clamped).
    #[must_use]
    pub fn new(config: BandwidthConfig) -> Self {
        Self {
            config: config.sanitised(),
            estimate: None,
            samples: 0,
        }
    }

    /// Feeds one measured write: `bytes` written to the socket and how long the blocking
    /// write took. Returns whether the sample was used; writes under
    /// [`MIN_SAMPLE_BYTES`] and zero durations are ignored.
    pub fn record(&mut self, bytes: usize, elapsed: Duration) -> bool {
        if bytes < MIN_SAMPLE_BYTES || elapsed.is_zero() {
            return false;
        }
        let sample = (bytes as f64 * 8.0 / 1.0e6 / elapsed.as_secs_f64()).min(MAX_PLAUSIBLE_MBPS);
        let next = match self.estimate {
            None => sample,
            Some(current) => {
                let alpha = if sample < current * self.config.clearly_slower {
                    self.config.alpha_down
                } else {
                    self.config.alpha_up
                };
                alpha.mul_add(sample - current, current)
            }
        };
        self.estimate = Some(next);
        self.samples += 1;
        true
    }

    /// Starts the estimate at `mbps`, e.g. the last known value of this peer when a new
    /// connection opens. Non-finite and non-positive values are ignored.
    pub fn seed(&mut self, mbps: f64) {
        if mbps.is_finite() && mbps > 0.0 {
            self.estimate = Some(mbps.min(MAX_PLAUSIBLE_MBPS));
        }
    }

    /// Forgets the estimate and the sample count (the smoothing stays).
    pub const fn reset(&mut self) {
        self.estimate = None;
        self.samples = 0;
    }

    /// The current estimate in Mbit/s, or `None` before a sample or seed.
    #[must_use]
    pub const fn estimate(&self) -> Option<f64> {
        self.estimate
    }

    /// How many samples were used (seeds do not count).
    #[must_use]
    pub const fn sample_count(&self) -> u64 {
        self.samples
    }
}
