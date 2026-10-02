//! The bandwidth tier ladder, the 50% rule that maps an estimate onto it, and the
//! hysteresis that keeps a link hovering at a boundary from flipping the tier every frame.

use super::bandwidth::{BandwidthConfig, BandwidthEstimator};
use std::time::Duration;

pub use crate::messages::encoding_matrix::TIERS_MBPS;

/// Number of rungs on the ladder.
pub const TIER_COUNT: usize = TIERS_MBPS.len();

/// Index of the tier a connection starts on before any bandwidth is known (300 Mbit/s,
/// the middle of the measured range: neither Raw nor the slowest-link codec).
pub const DEFAULT_TIER_INDEX: usize = 2;

/// Default hysteresis: 15% of the gap between two tiers on either side of the 50% line.
pub const DEFAULT_HYSTERESIS: f64 = 0.15;

/// One rung of the [`TIERS_MBPS`] ladder: a bandwidth the benchmark measured codecs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BandwidthTier(usize);

impl BandwidthTier {
    /// The slowest tier.
    pub const LOWEST: Self = Self(0);
    /// The fastest tier.
    pub const HIGHEST: Self = Self(TIER_COUNT - 1);
    /// The tier of a connection with no bandwidth information yet.
    pub const DEFAULT: Self = Self(DEFAULT_TIER_INDEX);

    /// The tier at `index`, clamped onto the ladder.
    #[must_use]
    pub const fn from_index(index: usize) -> Self {
        if index < TIER_COUNT {
            Self(index)
        } else {
            Self::HIGHEST
        }
    }

    /// Position on the ladder, 0 = slowest; indexes the encoding matrix columns.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }

    /// The tier's bandwidth in Mbit/s.
    #[must_use]
    pub const fn mbps(self) -> f64 {
        TIERS_MBPS[self.0]
    }

    /// Every tier, slowest first.
    #[must_use]
    pub fn all() -> [Self; TIER_COUNT] {
        std::array::from_fn(Self)
    }

    /// The tier for `mbps` by the plain 50% rule, without hysteresis: with the estimate
    /// between tier `i` and tier `i + 1`, tier `i + 1` once it is MORE THAN halfway from
    /// `i` to `i + 1`, else tier `i`. Below the ladder is the lowest tier, above it the
    /// highest; a non-finite or non-positive estimate is the lowest.
    #[must_use]
    pub fn for_estimate(mbps: f64) -> Self {
        if !mbps.is_finite() || mbps <= 0.0 {
            return Self::LOWEST;
        }
        let passed = (0..TIER_COUNT - 1)
            .take_while(|&i| mbps > gap_point(i, 0.5))
            .count();
        Self(passed)
    }
}

/// The bandwidth `fraction` of the way from tier `i` to tier `i + 1`.
fn gap_point(i: usize, fraction: f64) -> f64 {
    fraction.mul_add(TIERS_MBPS[i + 1] - TIERS_MBPS[i], TIERS_MBPS[i])
}

/// Holds the current tier of one connection and moves it with hysteresis.
///
/// Moving up from tier `i` needs the estimate above `0.5 + h` of the gap to tier `i + 1`;
/// moving down needs it below `0.5 - h` of the gap from tier `i - 1`. The first estimate
/// ever seen (or the first after [`Self::reset`]) places the tier directly by the plain
/// 50% rule.
#[derive(Debug, Clone, PartialEq)]
pub struct TierSelector {
    hysteresis: f64,
    current: BandwidthTier,
    settled: bool,
    switches: u32,
}

impl Default for TierSelector {
    fn default() -> Self {
        Self::new(DEFAULT_HYSTERESIS)
    }
}

impl TierSelector {
    /// A selector on [`BandwidthTier::DEFAULT`]. `hysteresis` (a fraction of the gap
    /// between tiers) is clamped to `[0, 0.45]`.
    #[must_use]
    pub const fn new(hysteresis: f64) -> Self {
        let h = if hysteresis.is_finite() {
            hysteresis.clamp(0.0, 0.45)
        } else {
            DEFAULT_HYSTERESIS
        };
        Self {
            hysteresis: h,
            current: BandwidthTier::DEFAULT,
            settled: false,
            switches: 0,
        }
    }

    /// The tier currently selected.
    #[must_use]
    pub const fn tier(&self) -> BandwidthTier {
        self.current
    }

    /// How many times [`Self::update`] changed the tier after the first placement.
    #[must_use]
    pub const fn switches(&self) -> u32 {
        self.switches
    }

    /// Back to the default tier, unplaced, with the switch counter cleared.
    pub const fn reset(&mut self) {
        self.current = BandwidthTier::DEFAULT;
        self.settled = false;
        self.switches = 0;
    }

    /// Moves the tier towards what `estimate` (Mbit/s) says and returns it. `None` and
    /// unusable estimates leave the tier unchanged.
    pub fn update(&mut self, estimate: Option<f64>) -> BandwidthTier {
        let Some(e) = estimate.filter(|e| e.is_finite() && *e > 0.0) else {
            return self.current;
        };
        if !self.settled {
            self.settled = true;
            self.current = BandwidthTier::for_estimate(e);
            return self.current;
        }
        let mut i = self.current.0;
        while i + 1 < TIER_COUNT && e > gap_point(i, 0.5 + self.hysteresis) {
            i += 1;
        }
        while i > 0 && e < gap_point(i - 1, 0.5 - self.hysteresis) {
            i -= 1;
        }
        if i != self.current.0 {
            self.current = BandwidthTier(i);
            self.switches += 1;
        }
        self.current
    }
}

/// The estimator and the tier selector of one connection, wired together: what a sender
/// owns per peer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkAdapter {
    estimator: BandwidthEstimator,
    selector: TierSelector,
}

impl LinkAdapter {
    /// An adapter with the given smoothing and hysteresis.
    #[must_use]
    pub fn new(config: BandwidthConfig, hysteresis: f64) -> Self {
        Self {
            estimator: BandwidthEstimator::new(config),
            selector: TierSelector::new(hysteresis),
        }
    }

    /// Feeds one measured blocking write (bytes written, how long it took) and returns the
    /// tier to use for the next frame. See [`BandwidthEstimator::record`].
    pub fn observe(&mut self, bytes: usize, elapsed: Duration) -> BandwidthTier {
        self.estimator.record(bytes, elapsed);
        self.selector.update(self.estimator.estimate())
    }

    /// Starts from a known bandwidth (the last value of this peer) and places the tier at
    /// once; returns that tier. Unusable values change nothing.
    pub fn seed(&mut self, mbps: f64) -> BandwidthTier {
        if mbps.is_finite() && mbps > 0.0 {
            self.estimator.seed(mbps);
            self.selector.reset();
            self.selector.update(self.estimator.estimate());
        }
        self.selector.tier()
    }

    /// The tier to use for the next frame.
    #[must_use]
    pub const fn tier(&self) -> BandwidthTier {
        self.selector.tier()
    }

    /// The bandwidth estimate in Mbit/s, if any; store it to seed the next connection.
    #[must_use]
    pub const fn estimate(&self) -> Option<f64> {
        self.estimator.estimate()
    }

    /// How many times the tier changed (a flapping indicator).
    #[must_use]
    pub const fn tier_switches(&self) -> u32 {
        self.selector.switches()
    }
}
