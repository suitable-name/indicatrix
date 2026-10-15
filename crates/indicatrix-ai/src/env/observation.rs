//! Observation tensors and feature extraction for gemstone states.
//!
//! # The observation layout (one place)
//!
//! Everything that packs or unpacks an observation goes through this module:
//!
//! - [`GlobalFeature::to_feature_vector`] and [`TierFeature::to_feature_vector`] are the ONLY
//!   places a feature is normalised.
//! - [`GemstoneObservation::global_token`] and [`GemstoneObservation::tier_tokens`] hand those
//!   vectors to the pure-Rust set-attention network.
//! - [`GemstoneObservation::flatten`] is exactly `global_token ++ tier_tokens().concat() ++
//!   zero padding`, so a network fed the flat vector sees the same numbers as one fed the
//!   tokens. [`split_flat`] is its inverse; [`flat_len`] its length.
//! - [`OBS_LAYOUT_VERSION`] changes whenever the meaning or order of any feature changes;
//!   checkpoints record it and refuse a mismatch.
//!
//! Flat layout: `[GLOBAL_DIM global | max_tiers * TIER_DIM tiers]`; tier `i` occupies
//! `GLOBAL_DIM + i * TIER_DIM ..`, padded tiers are all zero (a padded tier is recognised by
//! the caller's tier count, never by its values).
//!
//! Angles are signed degrees (crown positive, pavilion negative).
//!
//! The last four global entries ([`WEIGHT_FRACTION_INDEX`]) are the objective's weight
//! fractions ([`weight_fractions`]): the network composes the policy direction from its
//! per-tier component sensitivities with them, so one model serves every weighting.

use indicatrix_cut_core::ObjectiveWeights;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Version of the observation layout.
///
/// - `1`: original scaffold (constant placeholder features; `flatten` normalised differently
///   from the tokens).
/// - `2`: every feature is measured from the design; `flatten` is built from the tokens.
///   Tier token entries 0 and 1 were both `angle_deg / 90`.
/// - `3`: duplicate angle replaced by the episode-start angle; tier index and azimuth added;
///   base score added. The tier token grows from 10 to 13 entries, the global token from 14
///   to 15. Two tiers with the same angle in the same block are now told apart by their
///   position and azimuth, and the policy sees how far a tier has already moved.
/// - `4`: the objective's weight fractions are appended to the global token (entries 15 to
///   18: windowing, extinction, tilt brilliance, yield, each over the sum of the four). The
///   global token grows from 15 to 19 entries. The model no longer has to guess which
///   weighting a stone is judged under; a checkpoint of layout 3 does not know the entries.
pub const OBS_LAYOUT_VERSION: u32 = 4;

/// Width of the global token.
pub const GLOBAL_DIM: usize = GlobalFeature::FEATURE_DIM;
/// Width of one tier token.
pub const TIER_DIM: usize = TierFeature::FEATURE_DIM;

/// Index of the first of the four weight fractions in the global token (they are its last
/// four entries: windowing, extinction, tilt brilliance, yield).
pub const WEIGHT_FRACTION_INDEX: usize = 15;

const _: () = assert!(
    WEIGHT_FRACTION_INDEX + 4 == GlobalFeature::FEATURE_DIM,
    "the weight fractions are the last four global entries"
);

/// What a non-positive weight sum falls back to: the three optical terms equally, no yield
/// (the fallback `ObjectiveWeights::score_with_yield` applies).
const EQUAL_OPTICAL_FRACTIONS: [f32; 4] = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0];

/// The four objective weights as fractions of their sum: `[windowing, extinction, tilt
/// brilliance, yield] / sum` (the face-up tone term is not part of the observation: the
/// environments refuse it).
///
/// Mirrors `ObjectiveWeights::score_with_yield`: when the sum is not above `1e-6` (or not
/// finite) the fractions are `[1/3, 1/3, 1/3, 0]`. A negative weight is clamped to zero and a
/// weight that is not finite counts as zero before the sum is taken, so the result always
/// holds four finite fractions in `0..=1` that add up to one (up to rounding).
#[must_use]
pub fn weight_fractions(weights: &ObjectiveWeights) -> [f32; 4] {
    let clean = |weight: f32| {
        if weight.is_finite() {
            weight.max(0.0)
        } else {
            0.0
        }
    };
    let raw = [
        weights.windowing,
        weights.extinction,
        weights.tilt_brilliance,
        weights.yield_weight,
    ]
    .map(clean);
    let sum: f32 = raw.iter().sum();
    if sum.is_finite() && sum > 1e-6 {
        raw.map(|weight| weight / sum)
    } else {
        EQUAL_OPTICAL_FRACTIONS
    }
}

/// Smallest margin (degrees) a pavilion facet is kept above the critical angle by the policy
/// projection.
pub const PAVILION_CRITICAL_MARGIN_DEG: f32 = 1.0;
/// Steepest pavilion magnitude (degrees) the policy projection allows.
pub const PAVILION_MAX_ANGLE_DEG: f32 = 85.0;
/// Shallowest crown magnitude (degrees) the policy projection allows.
pub const CROWN_MIN_ANGLE_DEG: f32 = 15.0;
/// Steepest crown magnitude (degrees) the policy projection allows.
pub const CROWN_MAX_ANGLE_DEG: f32 = 80.0;

/// The signed `(min, max)` angle envelope the policy projection keeps a tier in.
///
/// `block_type` is `TierFeature::block_type` (crown `+1`, pavilion `-1`, girdle `0`);
/// `critical_angle_deg` is the critical angle of the material the stone is judged in.
/// Pavilion: `(-85, -(crit + 1))`. Crown: `(15, 80)`. Anything else: `(0, 0)`.
#[must_use]
pub const fn angle_envelope(block_type: f32, critical_angle_deg: f32) -> (f32, f32) {
    if block_type < -0.5 {
        (
            -PAVILION_MAX_ANGLE_DEG,
            -(critical_angle_deg + PAVILION_CRITICAL_MARGIN_DEG),
        )
    } else if block_type > 0.5 {
        (CROWN_MIN_ANGLE_DEG, CROWN_MAX_ANGLE_DEG)
    } else {
        (0.0, 0.0)
    }
}

/// The `(lo, hi)` bounds on the angle MAGNITUDE the policy projection keeps a tier in, or `None`
/// when the block has no envelope (girdle: only the step limit applies).
///
/// Derived from [`angle_envelope`]: pavilion `(-85, -(crit + 1))` becomes `[crit + 1, 85]`,
/// crown `(15, 80)` is kept. `lo > hi` means the critical angle leaves no admissible window.
/// The projection ([`GemstoneSetAttentionNetwork::project_actions`]) and
/// [`GemstoneObservation::free_tier_mask`] both read the window from here.
///
/// [`GemstoneSetAttentionNetwork::project_actions`]: crate::model::GemstoneSetAttentionNetwork::project_actions
#[must_use]
pub fn magnitude_window(block_type: f32, target_critical_angle_deg: f32) -> Option<(f64, f64)> {
    let (env_lo, env_hi) = angle_envelope(block_type, target_critical_angle_deg);
    if env_lo == 0.0 && env_hi == 0.0 {
        None
    } else if env_hi < 0.0 {
        Some((f64::from(-env_hi), f64::from(-env_lo)))
    } else {
        Some((f64::from(env_lo), f64::from(env_hi)))
    }
}

/// Whether the policy projection overrides this tier's action instead of using the sample.
///
/// True for a tier outside its magnitude window (it is moved one step toward the window whatever
/// the policy proposed) and for a tier whose window is empty (it is frozen). A tier without an
/// envelope, or inside its window, keeps the sampled action. A tier within one step of the
/// window edge is also masked: the mask does not know the step size, so it errs on the side of
/// not training on an action the projection may replace.
#[must_use]
pub fn projection_overrides_tier(tier: &TierFeature, target_critical_angle_deg: f32) -> bool {
    let Some((lo, hi)) = magnitude_window(tier.block_type, target_critical_angle_deg) else {
        return false;
    };
    let magnitude = f64::from(tier.angle_deg).abs();
    lo > hi || magnitude < lo || magnitude > hi
}

/// Fixed-size feature vector representing a single constraint tier in a gemstone design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierFeature {
    /// Current signed angle scaled by 1/90 (token entry 0).
    pub angle_normalized: f32,
    /// Signed angle in degrees (crown positive, pavilion negative). Not a token entry itself:
    /// the token carries [`Self::angle_normalized`] and [`Self::base_angle_deg`]. The
    /// projection and the free-tier mask read it.
    pub angle_deg: f32,
    /// The tier's signed angle at the start of the episode, degrees (token entry 1, scaled by
    /// 1/90). The difference to [`Self::angle_deg`] is how far the tier has already moved.
    pub base_angle_deg: f32,
    /// Number of facet index instances in this symmetry orbit.
    pub orbit_count: u32,
    /// Block classification: Crown (+1.0), Pavilion (-1.0), Girdle (0.0).
    pub block_type: f32,
    /// Whether this tier is held fixed by the environment (1.0) or can be moved (0.0).
    pub is_pinned: f32,
    /// Whether this tier is flat or vertical (table, culet, girdle wall: 1.0, otherwise 0.0).
    pub is_flat: f32,
    /// Critical angle margin in degrees (only non-zero for pavilion tiers).
    pub critical_margin_deg: f32,
    /// Solved mast of the tier divided by the largest mast of the design (`0..=1`).
    pub mast: f32,
    /// Lower bound of the policy's signed angle envelope, degrees (see [`angle_envelope`]).
    pub min_angle_deg: f32,
    /// Upper bound of the policy's signed angle envelope, degrees (see [`angle_envelope`]).
    pub max_angle_deg: f32,
    /// Position of the tier in the design divided by the tier count (`0..1`): tiers with the
    /// same angle in the same block are different facets, and closure and facet survival
    /// depend on which one moves.
    pub index_frac: f32,
    /// Sine of the azimuth of the tier's first facet index, `2π * indices[0] / gear_teeth`
    /// (`design.meta.gear_teeth`); `0.0` for a tier without an index or a design without gear
    /// teeth.
    pub azimuth_sin: f32,
    /// Cosine of the same azimuth; `1.0` for a tier without an index.
    pub azimuth_cos: f32,
}

impl TierFeature {
    /// Dimension of the standardized per-tier feature representation (13 features).
    pub const FEATURE_DIM: usize = 13;

    /// Return standardized per-tier feature vector for set attention token embedding.
    ///
    /// Order (a contract with the network, which reads entry 3 as the block type):
    /// `[angle / 90, base angle / 90, orbit count / 32, block type, pinned, flat,
    /// critical margin / 45, mast, envelope min / 90, envelope max / 90, index fraction,
    /// azimuth sin, azimuth cos]`.
    #[must_use]
    pub fn to_feature_vector(&self) -> Vec<f32> {
        vec![
            self.angle_normalized,
            self.base_angle_deg / 90.0,
            (self.orbit_count as f32) / 32.0,
            self.block_type,
            self.is_pinned,
            self.is_flat,
            self.critical_margin_deg / 45.0,
            self.mast,
            self.min_angle_deg / 90.0,
            self.max_angle_deg / 90.0,
            self.index_frac,
            self.azimuth_sin,
            self.azimuth_cos,
        ]
    }
}

/// `(sin, cos)` of the azimuth of an index-wheel position: `θ = 2π * index / gear_teeth`.
///
/// `(0.0, 1.0)` (azimuth zero) when the tier has no index, the index is not finite, or the
/// gear has no teeth (`gear_teeth <= 0`). The index wheel is a ring, so position `0` and
/// position `gear_teeth` are the same azimuth; a quarter turn (`gear_teeth / 4`) is
/// `(1.0, ~0.0)`.
#[must_use]
pub(crate) fn azimuth_sin_cos(index: Option<f64>, gear_teeth: i32) -> (f32, f32) {
    match index {
        Some(index) if index.is_finite() && gear_teeth > 0 => {
            let theta = std::f64::consts::TAU * index / f64::from(gear_teeth);
            let (sin, cos) = theta.sin_cos();
            (sin as f32, cos as f32)
        }
        _ => (0.0, 1.0),
    }
}

/// Global features capturing material properties, optics metrics, and search budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalFeature {
    /// Refractive index (sodium D) of the material the design was authored for.
    pub refractive_index: f32,
    /// Refractive index of the material the stone is judged in (identical to the source in
    /// the optimizer, distinct in retarget).
    pub target_refractive_index: f32,
    /// Source material critical angle in degrees.
    pub critical_angle_deg: f32,
    /// Target material critical angle in degrees.
    pub target_critical_angle_deg: f32,
    /// Dispersion of the target material: `n(486.1 nm) - n(656.3 nm)`.
    ///
    /// Exactly `0.0` whenever the material is a flat index: a design without a material preset
    /// (every `.asc` import, which carries only an `I`-line index) and the preset-less species
    /// Fluorite, Beryl and Garnet. Overrides install a constant Cauchy term, so only a named
    /// preset without an index override disperses.
    pub dispersion: f32,
    /// Windowing percentage of the current state (0 to 100).
    pub windowing_pct: f32,
    /// Extinction percentage of the current state (0 to 100).
    pub extinction_pct: f32,
    /// Tilt brilliance percentage of the current state (0 to 100, higher is better).
    pub tilt_brilliance_pct: f32,
    /// Volumetric yield percentage of the current state (0 when unmeasurable).
    pub yield_pct: f32,
    /// Table size as percentage of stone width (0 without a table).
    pub table_pct: f32,
    /// Crown height divided by pavilion depth (0 when either is unmeasurable).
    pub crown_to_pavilion_ratio: f32,
    /// Thinnest girdle band relative to the base design's (1.0 without a girdle band).
    pub min_girdle_ratio: f32,
    /// Best optical score achieved in current episode so far (equals the current score: the
    /// environments only accept improvements).
    pub best_score_so_far: f32,
    /// Normalized step progress within episode (0.0 at start, 1.0 at limit).
    pub progress_ratio: f32,
    /// Optical score of the base design, the score at the start of the episode (0 to 100,
    /// lower is better). With [`Self::best_score_so_far`] it tells the policy how far the
    /// episode has come and how much room is left; scores differ widely between designs.
    pub base_score: f32,
    /// The objective's weights as fractions of their sum, in the order windowing, extinction,
    /// tilt brilliance, yield (see [`weight_fractions`]; token entries 15 to 18, unscaled).
    /// The network weights its per-tier component sensitivities with them.
    pub weight_fractions: [f32; 4],
}

impl GlobalFeature {
    /// Dimension of the standardized global feature representation (19 features).
    pub const FEATURE_DIM: usize = 19;

    /// Return standardized global feature vector for context token embedding.
    ///
    /// Order: `[source index, target index, source critical angle / 90, target critical angle
    /// / 90, dispersion * 50, windowing / 100, extinction / 100, tilt brilliance / 100, yield /
    /// 100, table / 100, crown-to-pavilion, girdle ratio, best score / 100, progress, base
    /// score / 100, weight fractions (4)]`; the fractions start at
    /// [`WEIGHT_FRACTION_INDEX`].
    #[must_use]
    pub fn to_feature_vector(&self) -> Vec<f32> {
        vec![
            self.refractive_index,
            self.target_refractive_index,
            self.critical_angle_deg / 90.0,
            self.target_critical_angle_deg / 90.0,
            self.dispersion * 50.0,
            self.windowing_pct / 100.0,
            self.extinction_pct / 100.0,
            self.tilt_brilliance_pct / 100.0,
            self.yield_pct / 100.0,
            self.table_pct / 100.0,
            self.crown_to_pavilion_ratio,
            self.min_girdle_ratio,
            self.best_score_so_far / 100.0,
            self.progress_ratio,
            self.base_score / 100.0,
            self.weight_fractions[0],
            self.weight_fractions[1],
            self.weight_fractions[2],
            self.weight_fractions[3],
        ]
    }
}

/// Length of the flat observation vector for `max_tiers` tier slots.
#[must_use]
pub const fn flat_len(max_tiers: usize) -> usize {
    GLOBAL_DIM + max_tiers * TIER_DIM
}

/// Splits a flat observation into `(global, tiers)` slices (`tiers` is `max_tiers * TIER_DIM`
/// long). `None` when `flat` does not have exactly <code>[flat_len](max_tiers)</code> entries.
#[must_use]
pub fn split_flat(flat: &[f32], max_tiers: usize) -> Option<(&[f32], &[f32])> {
    (flat.len() == flat_len(max_tiers)).then(|| flat.split_at(GLOBAL_DIM))
}

/// A design has more tiers than the flat layout has slots for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationError {
    /// Tier count of the observation.
    pub tiers: usize,
    /// Slot count of the requested layout.
    pub max_tiers: usize,
}

impl fmt::Display for ObservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the design has {} tiers but the observation layout holds {}",
            self.tiers, self.max_tiers
        )
    }
}

impl std::error::Error for ObservationError {}

/// Complete observation presented to the policy network at each decision step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GemstoneObservation {
    /// Ordered list of per-tier features (shape: [N, `D_tier`]).
    pub tiers: Vec<TierFeature>,
    /// Global stone, material, and performance features (shape: [`D_global`]).
    pub global: GlobalFeature,
}

impl GemstoneObservation {
    /// Return the slice of per-tier feature tokens for the Geometric Set Attention model.
    #[must_use]
    pub fn tier_tokens(&self) -> Vec<Vec<f32>> {
        self.tiers
            .iter()
            .map(TierFeature::to_feature_vector)
            .collect()
    }

    /// Return the global context feature token.
    #[must_use]
    pub fn global_token(&self) -> Vec<f32> {
        self.global.to_feature_vector()
    }

    /// Per-tier mask of the tiers whose sampled action the environment will actually use: `true`
    /// for a free tier, `false` otherwise. Not free: pinned tiers (`is_pinned`), flat and
    /// vertical tiers, and tiers the projection overrides this step because they lie outside
    /// their angle envelope (see [`projection_overrides_tier`], the rule `project_actions`
    /// applies). Training masks the `false` entries out of log-prob, entropy and the action
    /// sample, the same way it masks zero padding. A tier re-enters the mask once it is inside
    /// its envelope.
    #[must_use]
    pub fn free_tier_mask(&self) -> Vec<bool> {
        let crit = self.global.target_critical_angle_deg;
        self.tiers
            .iter()
            .map(|tier| {
                tier.is_pinned < 0.5 && tier.is_flat < 0.5 && !projection_overrides_tier(tier, crit)
            })
            .collect()
    }

    /// Whether the flat layout with `max_tiers` slots holds every tier.
    #[must_use]
    pub const fn fits(&self, max_tiers: usize) -> bool {
        self.tiers.len() <= max_tiers
    }

    /// The flat vector `global_token ++ tier_tokens().concat() ++ zeros`, with exactly
    /// `max_tiers` tier slots.
    ///
    /// # Errors
    ///
    /// [`ObservationError`] when the observation has more than `max_tiers` tiers (it is
    /// never silently truncated here).
    pub fn try_flatten(&self, max_tiers: usize) -> Result<Vec<f32>, ObservationError> {
        if !self.fits(max_tiers) {
            return Err(ObservationError {
                tiers: self.tiers.len(),
                max_tiers,
            });
        }
        Ok(self.flatten(max_tiers))
    }

    /// [`Self::try_flatten`] for a caller that has already checked [`Self::fits`]: tiers
    /// beyond `max_tiers` are DROPPED, so check first (the trainer rejects such designs when
    /// it loads the dataset).
    #[must_use]
    pub fn flatten(&self, max_tiers: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(flat_len(max_tiers));
        out.extend(self.global_token());
        for tier in self.tiers.iter().take(max_tiers) {
            out.extend(tier.to_feature_vector());
        }
        out.resize(flat_len(max_tiers), 0.0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::ObjectivePreset;

    fn sample(tiers: usize) -> GemstoneObservation {
        GemstoneObservation {
            tiers: (0..tiers)
                .map(|i| TierFeature {
                    angle_normalized: i as f32 / 90.0,
                    angle_deg: i as f32,
                    base_angle_deg: i as f32 - 1.0,
                    orbit_count: 8,
                    block_type: if i.is_multiple_of(2) { 1.0 } else { -1.0 },
                    is_pinned: 0.0,
                    is_flat: 0.0,
                    critical_margin_deg: 3.0,
                    mast: 0.5,
                    min_angle_deg: -85.0,
                    max_angle_deg: 80.0,
                    index_frac: i as f32 / tiers as f32,
                    azimuth_sin: 0.0,
                    azimuth_cos: 1.0,
                })
                .collect(),
            global: GlobalFeature {
                refractive_index: 1.54,
                target_refractive_index: 1.77,
                critical_angle_deg: 40.5,
                target_critical_angle_deg: 34.4,
                dispersion: 0.02,
                windowing_pct: 10.0,
                extinction_pct: 20.0,
                tilt_brilliance_pct: 70.0,
                yield_pct: 40.0,
                table_pct: 55.0,
                crown_to_pavilion_ratio: 0.4,
                min_girdle_ratio: 1.0,
                best_score_so_far: 30.0,
                progress_ratio: 0.25,
                base_score: 35.0,
                weight_fractions: [0.5, 0.25, 0.125, 0.125],
            },
        }
    }

    #[test]
    fn flatten_is_global_token_then_tier_tokens_then_zeros() {
        // The widths of layout 4: a 19-entry global token and 13-entry tier tokens.
        assert_eq!(OBS_LAYOUT_VERSION, 4);
        assert_eq!((GLOBAL_DIM, TIER_DIM), (19, 13));
        let obs = sample(3);
        assert_eq!(obs.global_token().len(), 19);
        assert!(obs.tier_tokens().iter().all(|token| token.len() == 13));
        let flat = obs.flatten(5);
        let mut expected = obs.global_token();
        expected.extend(obs.tier_tokens().concat());
        expected.resize(flat_len(5), 0.0);
        assert_eq!(flat, expected);
        assert_eq!(flat.len(), flat_len(5));
        assert_eq!(flat.len(), 19 + 5 * 13);
        // Two padded tier slots, all zero.
        assert!(flat[19 + 3 * 13..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn the_weight_fractions_are_the_last_four_global_entries_unscaled() {
        assert_eq!(WEIGHT_FRACTION_INDEX, 15);
        assert_eq!(WEIGHT_FRACTION_INDEX + 4, GLOBAL_DIM);
        let obs = sample(2);
        let token = obs.global_token();
        assert_eq!(token[WEIGHT_FRACTION_INDEX..], [0.5, 0.25, 0.125, 0.125]);
        // The flat vector carries them at the same place, ahead of the tier tokens.
        let flat = obs.flatten(3);
        assert_eq!(
            flat[WEIGHT_FRACTION_INDEX..GLOBAL_DIM],
            obs.global.weight_fractions
        );
        assert_eq!(
            flat[GLOBAL_DIM..GLOBAL_DIM + TIER_DIM],
            obs.tier_tokens()[0]
        );
    }

    #[test]
    fn the_preset_weights_give_their_fractions() {
        let fractions = |preset: ObjectivePreset| weight_fractions(&preset.weights());
        let close = |actual: [f32; 4], expected: [f32; 4]| {
            actual
                .iter()
                .zip(expected)
                .all(|(a, e)| (a - e).abs() < 1e-6)
        };
        let sixth = 1.0 / 6.0;
        let third = 1.0 / 3.0;
        assert!(close(
            fractions(ObjectivePreset::Balanced),
            [third, third, third, 0.0]
        ));
        assert!(close(
            fractions(ObjectivePreset::Brilliance),
            [sixth, sixth, 4.0 * sixth, 0.0]
        ));
        assert!(close(
            fractions(ObjectivePreset::LowWindowing),
            [4.0 * sixth, sixth, sixth, 0.0]
        ));
        assert!(close(
            fractions(ObjectivePreset::LowExtinction),
            [sixth, 4.0 * sixth, sixth, 0.0]
        ));
        assert!(close(
            fractions(ObjectivePreset::KeepWeight),
            [sixth, sixth, sixth, 3.0 * sixth]
        ));
        // Balanced is exactly a third each: the fraction is the weight over the sum.
        assert_eq!(
            fractions(ObjectivePreset::Balanced),
            [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0]
        );
    }

    #[test]
    fn the_weight_fractions_add_up_to_one_and_scale_free() {
        let weights = ObjectiveWeights {
            windowing: 2.0,
            extinction: 0.5,
            tilt_brilliance: 3.5,
            yield_weight: 1.0,
            ..ObjectiveWeights::default()
        };
        let fractions = weight_fractions(&weights);
        assert!((fractions.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(
            fractions
                .iter()
                .all(|fraction| (0.0..=1.0).contains(fraction))
        );
        assert!((fractions[0] - 2.0 / 7.0).abs() < 1e-6);
        // Scaling every weight by the same factor changes nothing.
        let doubled = ObjectiveWeights {
            windowing: 4.0,
            extinction: 1.0,
            tilt_brilliance: 7.0,
            yield_weight: 2.0,
            ..weights
        };
        let again = weight_fractions(&doubled);
        assert!(
            fractions
                .iter()
                .zip(again)
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
    }

    #[test]
    fn weights_without_a_positive_sum_fall_back_to_the_optical_thirds() {
        let fallback = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0];
        let zero = ObjectiveWeights {
            windowing: 0.0,
            extinction: 0.0,
            tilt_brilliance: 0.0,
            yield_weight: 0.0,
            ..ObjectiveWeights::default()
        };
        assert_eq!(weight_fractions(&zero), fallback);
        // Negative weights are clamped to zero before the sum: nothing positive is left.
        let negative = ObjectiveWeights {
            windowing: -1.0,
            extinction: -2.0,
            tilt_brilliance: -3.0,
            yield_weight: -4.0,
            ..zero
        };
        assert_eq!(weight_fractions(&negative), fallback);
        // A non-finite weight counts as zero; the other weights keep their share.
        let broken = ObjectiveWeights {
            windowing: f32::NAN,
            extinction: f32::INFINITY,
            tilt_brilliance: 3.0,
            yield_weight: 1.0,
            ..zero
        };
        let fractions = weight_fractions(&broken);
        assert!(fractions.iter().all(|fraction| fraction.is_finite()));
        assert_eq!(fractions, [0.0, 0.0, 0.75, 0.25]);
        // One negative weight among positive ones is dropped, not subtracted.
        let mixed = ObjectiveWeights {
            windowing: -5.0,
            extinction: 1.0,
            tilt_brilliance: 1.0,
            yield_weight: 0.0,
            ..zero
        };
        assert_eq!(weight_fractions(&mixed), [0.0, 0.5, 0.5, 0.0]);
    }

    #[test]
    fn the_token_orders_are_the_contract_with_the_network() {
        let tier = TierFeature {
            angle_normalized: 0.5,
            angle_deg: 45.0,
            base_angle_deg: 36.0,
            orbit_count: 16,
            block_type: 1.0,
            is_pinned: 0.0,
            is_flat: 1.0,
            critical_margin_deg: 9.0,
            mast: 0.25,
            min_angle_deg: 18.0,
            max_angle_deg: 72.0,
            index_frac: 0.4,
            azimuth_sin: 0.6,
            azimuth_cos: 0.8,
        };
        // Hand values: 36 / 90, 16 / 32, 9 / 45, 18 / 90, 72 / 90; entry 3 is the block type.
        assert_eq!(
            tier.to_feature_vector(),
            [
                0.5, 0.4, 0.5, 1.0, 0.0, 1.0, 0.2, 0.25, 0.2, 0.8, 0.4, 0.6, 0.8
            ]
        );
        let global = sample(1).global.to_feature_vector();
        // best score 30 / 100, progress 0.25, the base score 35 / 100, then the four weight
        // fractions last (unscaled).
        assert_eq!(global[12..15], [0.3, 0.25, 0.35]);
        assert_eq!(global[15..], [0.5, 0.25, 0.125, 0.125]);
    }

    #[test]
    fn azimuth_of_index_zero_is_sin_0_cos_1_and_of_a_quarter_turn_is_sin_1_cos_0() {
        assert_eq!(azimuth_sin_cos(Some(0.0), 96), (0.0, 1.0));
        // A quarter turn of a 96-tooth wheel is position 24.
        let (sin, cos) = azimuth_sin_cos(Some(24.0), 96);
        assert!((sin - 1.0).abs() < 1e-6 && cos.abs() < 1e-6, "{sin} {cos}");
        // Half a turn: position 48 is (0, -1); an eighth (12) is (sqrt(1/2), sqrt(1/2)).
        let (sin, cos) = azimuth_sin_cos(Some(48.0), 96);
        assert!(sin.abs() < 1e-6 && (cos + 1.0).abs() < 1e-6, "{sin} {cos}");
        let (sin, cos) = azimuth_sin_cos(Some(12.0), 96);
        let eighth = std::f32::consts::FRAC_1_SQRT_2;
        assert!((sin - eighth).abs() < 1e-6 && (cos - eighth).abs() < 1e-6);
        // The wheel is a ring: the closing position is the same azimuth as position 0.
        let (sin, cos) = azimuth_sin_cos(Some(96.0), 96);
        assert!(sin.abs() < 1e-6 && (cos - 1.0).abs() < 1e-6, "{sin} {cos}");
        // No index, no gear teeth or a negative count, or a non-finite index: azimuth zero.
        assert_eq!(azimuth_sin_cos(None, 96), (0.0, 1.0));
        assert_eq!(azimuth_sin_cos(Some(5.0), 0), (0.0, 1.0));
        assert_eq!(azimuth_sin_cos(Some(5.0), -96), (0.0, 1.0));
        assert_eq!(azimuth_sin_cos(Some(f64::NAN), 96), (0.0, 1.0));
    }

    #[test]
    fn split_flat_inverts_flatten() {
        let obs = sample(2);
        let flat = obs.flatten(4);
        let (global, tiers) = split_flat(&flat, 4).expect("length matches");
        assert_eq!(global, obs.global_token().as_slice());
        assert_eq!(tiers.len(), 4 * TIER_DIM);
        assert!(tiers[2 * TIER_DIM..].iter().all(|&v| v == 0.0));
        assert!(split_flat(&flat, 3).is_none());
    }

    #[test]
    fn try_flatten_rejects_overflow_instead_of_truncating() {
        let obs = sample(6);
        assert_eq!(
            obs.try_flatten(4),
            Err(ObservationError {
                tiers: 6,
                max_tiers: 4
            })
        );
        assert!(obs.try_flatten(6).is_ok());
    }

    fn tier_at(angle_deg: f32, block_type: f32) -> TierFeature {
        TierFeature {
            angle_normalized: angle_deg / 90.0,
            angle_deg,
            base_angle_deg: angle_deg,
            orbit_count: 8,
            block_type,
            is_pinned: 0.0,
            is_flat: 0.0,
            critical_margin_deg: 0.0,
            mast: 0.5,
            min_angle_deg: 0.0,
            max_angle_deg: 0.0,
            index_frac: 0.0,
            azimuth_sin: 0.0,
            azimuth_cos: 1.0,
        }
    }

    #[test]
    fn free_tier_mask_excludes_tiers_the_projection_forces() {
        let mut obs = sample(0);
        obs.global.target_critical_angle_deg = 40.0; // pavilion window [41, 85]
        let mut pinned = tier_at(-60.0, -1.0);
        pinned.is_pinned = 1.0;
        let mut flat = tier_at(0.0, 1.0);
        flat.is_flat = 1.0;
        obs.tiers = vec![
            tier_at(-60.0, -1.0), // pavilion inside the window: free
            tier_at(-38.0, -1.0), // pavilion below the critical-angle bound: forced
            tier_at(-88.0, -1.0), // pavilion steeper than 85: forced
            tier_at(50.0, 1.0),   // crown inside [15, 80]: free
            tier_at(10.0, 1.0),   // crown shallower than 15: forced
            tier_at(85.0, 1.0),   // crown steeper than 80: forced
            tier_at(0.0, 0.0),    // girdle: no envelope, free
            pinned,
            flat,
        ];
        assert_eq!(
            obs.free_tier_mask(),
            [true, false, false, true, false, false, true, false, false]
        );

        // The mask and the projection agree: a masked tier is exactly one the projection moves
        // regardless of the sample (here a zero delta still yields a step).
        for (tier, free) in obs.tiers.iter().zip(obs.free_tier_mask()) {
            if tier.is_pinned > 0.5 || tier.is_flat > 0.5 || tier.block_type.abs() < 0.5 {
                continue;
            }
            let mut action = crate::env::ContinuousTierAction::new(vec![0.0]);
            crate::model::GemstoneSetAttentionNetwork::project_actions(
                &mut action,
                std::slice::from_ref(tier),
                40.0,
                0.75,
            );
            assert_eq!(action.deltas_deg[0] != 0.0, !free, "{}", tier.angle_deg);
        }

        // A smaller critical angle moves the pavilion bound: -38 is inside [36, 85] at crit 35.
        obs.global.target_critical_angle_deg = 35.0;
        assert!(obs.free_tier_mask()[1]);
    }

    #[test]
    fn envelope_follows_the_block() {
        assert_eq!(angle_envelope(1.0, 40.0), (15.0, 80.0));
        assert_eq!(angle_envelope(-1.0, 40.0), (-85.0, -41.0));
        assert_eq!(angle_envelope(0.0, 40.0), (0.0, 0.0));
    }
}
