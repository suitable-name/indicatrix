//! The Gaussian policy distribution and the composed policy mean.
//!
//! Conventions shared with the environments and the trainer:
//! - Angles are in degrees and **signed**: crown tiers are positive, pavilion tiers negative.
//! - The policy outputs one Gaussian per tier. Its `mean` is in **pre-squash** units and bounded to
//!   `[-MEAN_BOUND, MEAN_BOUND]`: the network predicts, per tier, the slope of each objective
//!   component and [`compose_mean`] turns the slopes into a direction with the objective's weight
//!   fractions, scaled so the steepest tier sits at `-+MEAN_BOUND = -+atanh(0.95)`. Sampling draws a
//!   *raw* (pre-squash) value around that mean and [`GaussianPolicy::squash`] maps it through `tanh`
//!   into a degree step, so the deterministic steepest tier moves 95 % of the step limit.
//! - `log_std` is **state-independent**: one learnable value per block (crown, pavilion, girdle or
//!   other, chosen by the tier token's `block_type`), squashed smoothly into
//!   `[LOG_STD_MIN, LOG_STD_MAX]` by [`bounded_log_std`]. The old per-tier head with a hard `clamp`
//!   had a dead gradient outside its window and froze both runs of 2026-10-12. The sampler and the
//!   trainer use `std = exp(log_std)`; the clamp in [`GaussianPolicy`] is a harmless safety net.

use super::rng::Prng;
use crate::env::{ContinuousTierAction, MEAN_BOUND, N_COMPONENTS, SLOPE_NORM_FLOOR};

/// Lower bound of the policy's `log_std` (std 0.05 in pre-squash units).
pub const LOG_STD_MIN: f32 = -3.0;
/// Upper bound of the policy's `log_std` (std 1.65 in pre-squash units).
pub const LOG_STD_MAX: f32 = 0.5;
/// The bounded log-std every block starts at (std 0.5 in pre-squash units).
pub const LOG_STD_INIT: f32 = -0.7;

/// `0.5 * ln(2 * pi)`, the per-dimension constant of the Gaussian log-density.
pub const HALF_LN_2PI: f32 = 0.918_938_5;
/// `0.5 * ln(2 * pi * e)`, the entropy of a unit Gaussian per dimension.
pub const HALF_LN_2PI_E: f32 = 1.418_938_5;

/// Index of the block feature (`block_type`: +1 crown, -1 pavilion, 0 otherwise) in a tier token;
/// the first ten entries keep their order across observation layouts.
pub const BLOCK_TYPE_INDEX: usize = 3;
/// Number of learnable log-std values: crown, pavilion, girdle or other.
pub const LOG_STD_SLOTS: usize = 3;

/// `LOG_STD_MIN + (LOG_STD_MAX - LOG_STD_MIN) * sigmoid(raw)`: smooth, bounded, never a dead gradient.
///
/// At `raw = 0` the result is the midpoint of the range (-1.25); the gradient peaks there (0.875 per
/// unit of `raw`) and decays only exponentially, where the old `clamp` cut it to exactly zero.
#[must_use]
pub fn bounded_log_std(raw: f32) -> f32 {
    (LOG_STD_MAX - LOG_STD_MIN).mul_add(1.0 / (1.0 + (-raw).exp()), LOG_STD_MIN)
}

/// The raw value whose [`bounded_log_std`] is [`LOG_STD_INIT`] (the inverse sigmoid, computed, not
/// hard-coded: `ln(p / (1 - p))` with `p = (LOG_STD_INIT - LOG_STD_MIN) / (LOG_STD_MAX - LOG_STD_MIN)`).
#[must_use]
pub fn log_std_raw_init() -> f32 {
    let p = (LOG_STD_INIT - LOG_STD_MIN) / (LOG_STD_MAX - LOG_STD_MIN);
    (p / (1.0 - p)).ln()
}

/// Which of the three `log_std_raw` entries a tier uses: crown (`block_type > 0.5`) → 0, pavilion
/// (`< -0.5`) → 1, anything else (girdle, or a NaN) → 2.
#[must_use]
pub const fn block_slot(block_type: f32) -> usize {
    if block_type > 0.5 {
        0
    } else if block_type < -0.5 {
        1
    } else {
        2
    }
}

/// The policy mean from per-tier component sensitivities and the objective's weight fractions.
///
/// Per tier `k`, the objective's slope is `g_k = sum_c fractions[c] * sensitivities[k][c]`
/// (accumulated for `c = 0..N_COMPONENTS` in order with `mul_add`, starting at `0.0`); with
/// `g_max = max_k |g_k|` over mobile tiers, the mean is
/// `mean_k = -MEAN_BOUND * (g_k / max(g_max, SLOPE_NORM_FLOOR))`. This preserves exact scale-free
/// direction above the documented score-points-per-degree floor and smoothly reduces the action
/// magnitude below it, bounding the composition Jacobian near flat scores. All-zero slopes yield zero.
///
/// The Burn twin and the finite-difference oracle (`direction_from_slopes`) use the same operation
/// order; a test compares them.
#[must_use]
pub fn compose_mean(
    sensitivities: &[[f32; N_COMPONENTS]],
    fractions: &[f32; N_COMPONENTS],
) -> Vec<f32> {
    compose_mean_masked(sensitivities, fractions, &vec![true; sensitivities.len()])
}

/// Compose sensitivity rows while excluding coordinates the environment cannot move.
/// Immobile tiers remain part of attention, but cannot set the normalization scale or receive an
/// action. A short mask treats missing entries as immobile.
#[must_use]
pub fn compose_mean_masked(
    sensitivities: &[[f32; N_COMPONENTS]],
    fractions: &[f32; N_COMPONENTS],
    mobile: &[bool],
) -> Vec<f32> {
    let slopes: Vec<f32> = sensitivities
        .iter()
        .map(|row| {
            fractions
                .iter()
                .zip(row)
                .fold(0.0f32, |acc, (&frac, &sens)| frac.mul_add(sens, acc))
        })
        .collect();
    let g_max = slopes.iter().enumerate().fold(0.0f32, |acc, (i, g)| {
        if mobile.get(i).copied().unwrap_or(false) {
            acc.max(g.abs())
        } else {
            acc
        }
    });
    slopes
        .iter()
        .enumerate()
        .map(|(i, &g)| {
            let is_mobile = mobile.get(i).copied().unwrap_or(false);
            if is_mobile {
                (g / g_max.max(SLOPE_NORM_FLOOR)) * -MEAN_BOUND
            } else {
                0.0
            }
        })
        .collect()
}

/// Whether tier `i` is free under an optional mask (`None` = every tier; a short mask = not free).
fn is_free(free: Option<&[bool]>, i: usize) -> bool {
    free.is_none_or(|f| f.get(i).copied().unwrap_or(false))
}

/// Gaussian policy distribution parameters (one entry per tier).
#[derive(Debug, Clone, PartialEq)]
pub struct GaussianPolicy {
    /// Action mean vector in pre-squash units: bounded to `[-MEAN_BOUND, MEAN_BOUND]` when produced
    /// by the network (see [`compose_mean`]); [`Self::squash`] applies the `tanh`.
    pub mean: Vec<f32>,
    /// Action log standard deviation vector (within `[LOG_STD_MIN, LOG_STD_MAX]` when produced by the
    /// network: one [`bounded_log_std`] value per tier, shared by all tiers of a block).
    pub log_std: Vec<f32>,
}

impl GaussianPolicy {
    /// Standard deviation of dimension `i`: `exp(log_std)` with the single shared clamp.
    #[must_use]
    pub fn std(&self, i: usize) -> f32 {
        self.log_std[i].clamp(LOG_STD_MIN, LOG_STD_MAX).exp()
    }

    /// Draw one raw (pre-`tanh`) action per tier from `N(mean, std^2)` and return it together with its
    /// summed log-probability under this policy.
    #[allow(
        clippy::suboptimal_flops,
        reason = "operation order mirrors the Burn model; the equivalence test depends on it"
    )]
    pub fn sample_raw(&self, rng: &mut Prng) -> (Vec<f32>, f32) {
        self.sample_raw_inner(rng, None)
    }

    /// As [`Self::sample_raw`], but only the tiers with `free[i] == true` are sampled.
    ///
    /// A tier the environment will not move (pinned or flat) takes its mean as the raw value, draws no
    /// random number and contributes nothing to the returned log-probability, matching the masked
    /// objective of the trainer. `free` must have one entry per tier (a shorter slice marks the missing
    /// tiers as not free).
    pub fn sample_raw_masked(&self, rng: &mut Prng, free: &[bool]) -> (Vec<f32>, f32) {
        self.sample_raw_inner(rng, Some(free))
    }

    #[allow(
        clippy::suboptimal_flops,
        reason = "operation order mirrors the Burn model; the equivalence test depends on it"
    )]
    fn sample_raw_inner(&self, rng: &mut Prng, free: Option<&[bool]>) -> (Vec<f32>, f32) {
        let mut raw = Vec::with_capacity(self.mean.len());
        for (i, &m) in self.mean.iter().enumerate() {
            if is_free(free, i) {
                raw.push(m + self.std(i) * rng.standard_normal());
            } else {
                raw.push(m);
            }
        }
        let log_prob = self.log_prob_inner(&raw, free);
        (raw, log_prob)
    }

    /// Summed Gaussian log-density of a raw (pre-`tanh`) action under this policy.
    #[must_use]
    pub fn log_prob(&self, raw: &[f32]) -> f32 {
        self.log_prob_inner(raw, None)
    }

    /// As [`Self::log_prob`], summed over the tiers with `free[i] == true` only (see
    /// [`Self::sample_raw_masked`]).
    #[must_use]
    pub fn log_prob_masked(&self, raw: &[f32], free: &[bool]) -> f32 {
        self.log_prob_inner(raw, Some(free))
    }

    #[allow(
        clippy::suboptimal_flops,
        reason = "operation order mirrors the Burn model; the equivalence test depends on it"
    )]
    fn log_prob_inner(&self, raw: &[f32], free: Option<&[bool]>) -> f32 {
        let mut total = 0.0f32;
        for (i, (&x, &m)) in raw.iter().zip(&self.mean).enumerate() {
            if !is_free(free, i) {
                continue;
            }
            let log_std = self.log_std[i].clamp(LOG_STD_MIN, LOG_STD_MAX);
            let z = (x - m) / log_std.exp();
            total += -0.5 * z * z - log_std - HALF_LN_2PI;
        }
        total
    }

    /// Differential entropy of the Gaussian, **averaged** over the tiers with `free[i] == true`
    /// (`0.0` when none is free); the twin of the masked per-tier entropy the Burn trainer maximises.
    ///
    /// A mean rather than a sum, so a 25-tier design does not get 25 times the bonus of a one-tier
    /// design and `entropy_coeff` means the same on every stone.
    #[must_use]
    pub fn entropy_masked(&self, free: &[bool]) -> f32 {
        let (sum, count) = self
            .log_std
            .iter()
            .enumerate()
            .filter(|&(i, _)| is_free(Some(free), i))
            .fold((0.0f32, 0usize), |(sum, count), (_, &ls)| {
                (
                    sum + (ls.clamp(LOG_STD_MIN, LOG_STD_MAX) + HALF_LN_2PI_E),
                    count + 1,
                )
            });
        if count == 0 { 0.0 } else { sum / count as f32 }
    }

    /// Map a raw action into per-tier degree steps: `tanh(raw) * max_step_deg`.
    ///
    /// A raw value of [`MEAN_BOUND`] (the steepest tier of a composed mean) maps to 95 % of
    /// `max_step_deg`.
    #[must_use]
    pub fn squash(raw: &[f32], max_step_deg: f64) -> ContinuousTierAction {
        ContinuousTierAction::new(
            raw.iter()
                .map(|&r| f64::from(r.tanh()) * max_step_deg)
                .collect(),
        )
    }

    /// Convenience wrapper: the deterministic mode (`deterministic == true`, uses the mean) or one
    /// stochastic draw seeded by `rng_seed`, squashed into degrees.
    #[must_use]
    pub fn sample(
        &self,
        rng_seed: u64,
        deterministic: bool,
        max_step_deg: f64,
    ) -> ContinuousTierAction {
        if deterministic {
            Self::squash(&self.mean, max_step_deg)
        } else {
            let mut rng = Prng::new(rng_seed);
            let (raw, _) = self.sample_raw(&mut rng);
            Self::squash(&raw, max_step_deg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row4(a: f32, b: f32, c: f32, d: f32) -> [f32; N_COMPONENTS] {
        let mut row = [0.0; N_COMPONENTS];
        row[..4].copy_from_slice(&[a, b, c, d]);
        row
    }

    fn weights4(a: f32, b: f32, c: f32, d: f32) -> [f32; N_COMPONENTS] {
        row4(a, b, c, d)
    }

    fn policy() -> GaussianPolicy {
        GaussianPolicy {
            mean: vec![0.1, -0.2, 0.3, 0.0],
            log_std: vec![-0.5, 0.2, -1.0, 0.7],
        }
    }

    #[test]
    fn masked_log_prob_ignores_tiers_that_are_not_free() {
        let p = policy();
        let free = [true, false, true, false];
        let a = [0.4f32, 5.0, -0.3, -7.0];
        let b = [0.4f32, -9.0, -0.3, 3.0];
        assert_eq!(p.log_prob_masked(&a, &free), p.log_prob_masked(&b, &free));

        // Equals the plain log-prob of the free tiers alone.
        let sub = GaussianPolicy {
            mean: vec![0.1, 0.3],
            log_std: vec![-0.5, -1.0],
        };
        let want = sub.log_prob(&[0.4, -0.3]);
        assert!((p.log_prob_masked(&a, &free) - want).abs() < 1e-6);

        // An all-free mask is the unmasked value.
        assert_eq!(p.log_prob_masked(&a, &[true; 4]), p.log_prob(&a));
        // Nothing free: zero.
        assert_eq!(p.log_prob_masked(&a, &[false; 4]), 0.0);
    }

    #[test]
    fn masked_sampling_keeps_the_mean_on_frozen_tiers_and_is_deterministic() {
        let p = policy();
        let free = [true, false, true, false];
        let (raw1, lp1) = p.sample_raw_masked(&mut Prng::new(7), &free);
        let (raw2, lp2) = p.sample_raw_masked(&mut Prng::new(7), &free);
        assert_eq!(raw1, raw2);
        assert_eq!(lp1, lp2);
        assert_eq!(raw1[1], p.mean[1]);
        assert_eq!(raw1[3], p.mean[3]);
        assert_ne!(raw1[0], p.mean[0]);
        // The reported log-prob is the masked log-prob of the draw.
        assert!((lp1 - p.log_prob_masked(&raw1, &free)).abs() < 1e-6);
        // An all-free mask reproduces the unmasked sampler.
        let (all, lp_all) = p.sample_raw_masked(&mut Prng::new(7), &[true; 4]);
        let (plain, lp_plain) = p.sample_raw(&mut Prng::new(7));
        assert_eq!(all, plain);
        assert_eq!(lp_all, lp_plain);
    }

    #[test]
    fn masked_entropy_is_the_mean_over_the_free_tiers() {
        let p = policy();
        let free = [true, false, true, false];
        // Free tiers 0 and 2 (log_std -0.5 and -1.0): the mean of the two, not their sum.
        let want = ((-0.5 + HALF_LN_2PI_E) + (-1.0 + HALF_LN_2PI_E)) / 2.0;
        assert!((p.entropy_masked(&free) - want).abs() < 1e-6);
        assert!((want - (-0.75 + HALF_LN_2PI_E)).abs() < 1e-6);
        // A single free tier: exactly its own entropy.
        let one = p.entropy_masked(&[false, true, false, false]);
        assert!((one - (0.2 + HALF_LN_2PI_E)).abs() < 1e-6);
        // Nothing free: zero, not NaN.
        assert_eq!(p.entropy_masked(&[false; 4]), 0.0);
    }

    #[test]
    fn bounded_log_std_starts_at_the_init_value_and_approaches_the_bounds() {
        let at_init = bounded_log_std(log_std_raw_init());
        assert!((at_init - LOG_STD_INIT).abs() < 1e-5, "{at_init}");
        // sigmoid(0) = 0.5: the midpoint of [-3.0, 0.5] is -1.25.
        assert!((bounded_log_std(0.0) - (-1.25)).abs() < 1e-6);
        // Far out the value saturates at the bounds without a NaN or an overflow.
        assert!((bounded_log_std(60.0) - LOG_STD_MAX).abs() < 1e-5);
        assert!((bounded_log_std(-60.0) - LOG_STD_MIN).abs() < 1e-5);
        assert!(bounded_log_std(f32::MAX).is_finite());
        assert!(bounded_log_std(f32::MIN).is_finite());
        // Strictly increasing in a window around the start: the gradient is never zero there.
        assert!(bounded_log_std(-1.0) < bounded_log_std(0.0));
        assert!(bounded_log_std(0.0) < bounded_log_std(1.0));
        // Hand value: sigmoid(0.8) = 0.689_974_5 -> -3 + 3.5 * 0.689_974_5 = -0.585_089_3.
        assert!((bounded_log_std(0.8) - (-0.585_089_3)).abs() < 1e-5);
    }

    #[test]
    fn block_slot_maps_crown_pavilion_and_the_rest() {
        assert_eq!(block_slot(1.0), 0);
        assert_eq!(block_slot(0.6), 0);
        assert_eq!(block_slot(-1.0), 1);
        assert_eq!(block_slot(-0.6), 1);
        assert_eq!(block_slot(0.0), 2);
        // The thresholds themselves and NaN belong to the girdle/other slot.
        assert_eq!(block_slot(0.5), 2);
        assert_eq!(block_slot(-0.5), 2);
        assert_eq!(block_slot(f32::NAN), 2);
    }

    #[test]
    fn squash_of_the_mean_bound_is_95_percent_of_the_step() {
        let action = GaussianPolicy::squash(&[MEAN_BOUND, -MEAN_BOUND, 0.0], 0.5);
        let d = &action.deltas_deg;
        assert!((d[0] - 0.475).abs() < 1e-4, "{}", d[0]);
        assert!((d[1] + 0.475).abs() < 1e-4, "{}", d[1]);
        assert_eq!(d[2], 0.0);
    }

    /// Reference composition in `f64`, written independently of [`compose_mean`].
    fn compose_f64(sens: &[[f32; N_COMPONENTS]], frac: &[f32; N_COMPONENTS]) -> Vec<f64> {
        let g: Vec<f64> = sens
            .iter()
            .map(|row| {
                row.iter()
                    .zip(frac)
                    .map(|(&s, &f)| f64::from(f) * f64::from(s))
                    .sum()
            })
            .collect();
        let g_max = g.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        let denom = g_max.max(f64::from(SLOPE_NORM_FLOOR));
        g.iter()
            .map(|v| -f64::from(MEAN_BOUND) * (v / denom))
            .collect()
    }

    #[test]
    fn compose_mean_puts_the_steepest_tier_at_the_bound_and_scales_the_rest() {
        // Windowing only: g = [1, 2, -4], g_max = 4.
        let sens = [
            row4(1.0, 0.0, 0.0, 0.0),
            row4(2.0, 0.0, 0.0, 0.0),
            row4(-4.0, 0.0, 0.0, 0.0),
        ];
        let mean = compose_mean(&sens, &weights4(1.0, 0.0, 0.0, 0.0));
        let want = [-MEAN_BOUND / 4.0, -MEAN_BOUND / 2.0, MEAN_BOUND];
        for (k, (got, want)) in mean.iter().zip(want).enumerate() {
            assert!((got - want).abs() < 1e-5, "tier {k}: {got} vs {want}");
        }
        assert!((mean[2] - MEAN_BOUND).abs() < 1e-5);
    }

    #[test]
    fn compose_mean_follows_the_objective_weights_without_new_weights() {
        let sens = [row4(1.0, 0.0, 0.0, -1.0), row4(-0.5, 0.0, 0.0, 1.0)];
        // Windowing only: g = [1, -0.5].
        let windowing = compose_mean(&sens, &weights4(1.0, 0.0, 0.0, 0.0));
        assert!((windowing[0] + MEAN_BOUND).abs() < 1e-5);
        assert!((windowing[1] - MEAN_BOUND / 2.0).abs() < 1e-5);
        // Yield only: g = [-1, 1], the direction flips with the weights alone.
        let yield_only = compose_mean(&sens, &weights4(0.0, 0.0, 0.0, 1.0));
        assert!((yield_only[0] - MEAN_BOUND).abs() < 1e-5);
        assert!((yield_only[1] + MEAN_BOUND).abs() < 1e-5);
        // A mix: g = 0.4 * 1 + 0.1 * -1 = 0.3 and 0.4 * -0.5 + 0.1 * 1 = -0.1.
        let mixed = compose_mean(&sens, &weights4(0.4, 0.3, 0.2, 0.1));
        assert!((mixed[0] + MEAN_BOUND).abs() < 1e-5);
        assert!((mixed[1] - MEAN_BOUND / 3.0).abs() < 1e-5);
    }

    #[test]
    fn compose_mean_equal_tiers_sit_at_the_bound_with_balanced_fractions() {
        let balanced = weights4(1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0);
        let sens = [row4(0.9, 0.6, 0.3, 0.5); 2];
        let mean = compose_mean(&sens, &balanced);
        // g = 0.6 on both tiers, so g_max exceeds the floor and the mean is -MEAN_BOUND.
        for m in mean {
            assert!((m + MEAN_BOUND).abs() < 1e-5, "{m}");
        }
    }

    #[test]
    fn compose_mean_of_nothing_or_of_zeros_is_zero_and_never_nan() {
        assert!(compose_mean(&[], &[0.25; N_COMPONENTS]).is_empty());
        let zeros = compose_mean(&[[0.0; N_COMPONENTS]; 3], &[0.25; N_COMPONENTS]);
        assert_eq!(zeros.len(), 3);
        assert!(zeros.iter().all(|&m| m == 0.0 && !m.is_nan()));
        // Weights of zero silence the objective the same way.
        let silent = compose_mean(&[row4(1.0, 2.0, 3.0, 4.0)], &[0.0; N_COMPONENTS]);
        assert!(silent.iter().all(|&m| m == 0.0));
    }

    #[test]
    fn immobile_coordinates_neither_set_scale_nor_receive_a_mean() {
        let mut mobile = [0.0; N_COMPONENTS];
        mobile[0] = 2.0;
        let mut pinned = [0.0; N_COMPONENTS];
        pinned[0] = 200.0;
        let got = compose_mean_masked(&[mobile, pinned], &[1.0; N_COMPONENTS], &[true, false]);
        assert!((got[0] + MEAN_BOUND).abs() < 1e-5, "{}", got[0]);
        assert_eq!(got[1], 0.0);

        let all_pinned = compose_mean_masked(&[pinned], &[1.0; N_COMPONENTS], &[false]);
        assert_eq!(all_pinned, [0.0]);
    }

    #[test]
    fn compose_mean_is_bounded_scale_free_and_matches_an_f64_reference() {
        let sens: Vec<[f32; N_COMPONENTS]> = (0..9)
            .map(|k| {
                let x = k as f32;
                row4(
                    (x * 0.7 + 0.1).sin(),
                    (x * 1.3 + 0.2).cos() * 0.5,
                    (x * 0.4 - 0.3).sin() * 2.0,
                    (x * 0.9 + 0.5).cos(),
                )
            })
            .collect();
        let frac = weights4(0.4, 0.3, 0.2, 0.1);
        let mean = compose_mean(&sens, &frac);
        let reference = compose_f64(&sens, &frac);
        for (k, (got, want)) in mean.iter().zip(&reference).enumerate() {
            assert!(
                (f64::from(*got) - want).abs() < 1e-5,
                "tier {k}: {got} vs {want}"
            );
            assert!(got.abs() <= MEAN_BOUND + 1e-6);
        }
        let peak = mean.iter().fold(0.0f32, |a, m| a.max(m.abs()));
        assert!((peak - MEAN_BOUND).abs() < 1e-4, "{peak}");

        // Doubling every sensitivity leaves the direction unchanged.
        let doubled: Vec<[f32; N_COMPONENTS]> = sens.iter().map(|r| r.map(|v| 2.0 * v)).collect();
        for (a, b) in mean.iter().zip(compose_mean(&doubled, &frac)) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn slope_floor_scales_near_zero_predictions_and_bounds_the_mean() {
        let mut row = [0.0; N_COMPONENTS];
        row[0] = SLOPE_NORM_FLOOR * 0.25;
        let mean = compose_mean(&[row], &[1.0; N_COMPONENTS]);
        assert!((mean[0] + MEAN_BOUND * 0.25).abs() < 1e-6, "{}", mean[0]);
        let h = SLOPE_NORM_FLOOR * 0.001;
        let mut upper = row;
        let mut lower = row;
        upper[0] += h;
        lower[0] -= h;
        let upper_mean = compose_mean(&[upper], &[1.0; N_COMPONENTS])[0];
        let lower_mean = compose_mean(&[lower], &[1.0; N_COMPONENTS])[0];
        let derivative = (upper_mean - lower_mean) / (2.0 * h);
        let expected_derivative = -MEAN_BOUND / SLOPE_NORM_FLOOR;
        assert!(
            (derivative - expected_derivative).abs() < 1e-3,
            "{derivative} vs {expected_derivative}"
        );

        row[0] = SLOPE_NORM_FLOOR;
        let at_floor = compose_mean(&[row], &[1.0; N_COMPONENTS]);
        assert!((at_floor[0] + MEAN_BOUND).abs() < 1e-6);

        row[0] = 2.0 * SLOPE_NORM_FLOOR;
        let above_floor = compose_mean(&[row], &[1.0; N_COMPONENTS]);
        assert!((above_floor[0] + MEAN_BOUND).abs() < 1e-6);
    }
}
