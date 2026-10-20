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
//! The last seven global entries ([`WEIGHT_FRACTION_INDEX`]) are coefficients for the
//! objective components in sensitivity-label order.

use indicatrix_cut_core::{ObjectiveWeights, optimize::MAX_SAFE_CANDIDATE_ANGLE_DEG};
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
/// - `4` (historical layout): four objective weight fractions were appended to the global token
///   (entries 15 to 18: windowing, extinction, tilt brilliance and yield, each over their sum).
///   The global token grew from 15 to 19 entries; layout 5 below has seven fractions.
/// - `5`: seven objective coefficients and measured material, lighting, geometry, mobility,
///   constraint and action-context features were added; the global and tier widths are 41 and 26.
pub const OBS_LAYOUT_VERSION: u32 = 5;

/// Width of the global token.
pub const GLOBAL_DIM: usize = GlobalFeature::FEATURE_DIM;
/// Width of one tier token.
pub const TIER_DIM: usize = TierFeature::FEATURE_DIM;

/// Index of the first of seven global objective coefficients (the last seven entries).
pub const WEIGHT_FRACTION_INDEX: usize = 34;
/// Tier-token index of mobility (`1.0` movable, `0.0` otherwise).
pub const MOBILITY_INDEX: usize = 13;
/// Backwards-readable alias for the same token position; `0.0` means immobile.
pub const IMMOBILE_INDEX: usize = MOBILITY_INDEX;
/// Global-token index of task kind (`0` optimizer, `1` retarget).
pub const TASK_KIND_INDEX: usize = 15;

const _: () = assert!(
    WEIGHT_FRACTION_INDEX + 7 == GlobalFeature::FEATURE_DIM,
    "the objective coefficients are the last seven global entries"
);

/// What a non-positive weight sum falls back to: the three optical terms equally, no yield
/// (the fallback `ObjectiveWeights::score_with_yield` applies).
///
/// Seven objective coefficients in sensitivity order. Yield is omitted from normalization when
/// unavailable; tone goals are separate and shape remains additive, matching the shared scorer.
#[must_use]
pub fn weight_fractions(weights: &ObjectiveWeights) -> [f32; 7] {
    objective_coefficients(weights, true, 0.0)
}

/// Seven coefficients matching score composition for measured yield availability and shape.
/// Yield is removed from the denominator when it cannot be measured; shape stays additive.
#[must_use]
pub fn objective_coefficients(
    weights: &ObjectiveWeights,
    yield_available: bool,
    shape_weight: f32,
) -> [f32; 7] {
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
        if yield_available {
            weights.yield_weight
        } else {
            0.0
        },
        if weights.tone_goal == indicatrix_cut_core::ToneGoal::Lighter {
            weights.tone_weight
        } else {
            0.0
        },
        if weights.tone_goal == indicatrix_cut_core::ToneGoal::Deeper {
            weights.tone_weight
        } else {
            0.0
        },
    ]
    .map(clean);
    let sum: f32 = raw.iter().sum();
    if sum.is_finite() && sum > 1e-6 {
        [
            raw[0] / sum,
            raw[1] / sum,
            raw[2] / sum,
            raw[3] / sum,
            raw[4] / sum,
            raw[5] / sum,
            clean(shape_weight),
        ]
    } else {
        [
            1.0 / 3.0,
            1.0 / 3.0,
            1.0 / 3.0,
            0.0,
            0.0,
            0.0,
            clean(shape_weight),
        ]
    }
}

/// Smallest margin (degrees) a pavilion facet is kept above the critical angle by the policy
/// projection.
pub const PAVILION_CRITICAL_MARGIN_DEG: f32 = 1.0;
/// Steepest pavilion magnitude (degrees) the policy projection allows.
pub const PAVILION_MAX_ANGLE_DEG: f32 = MAX_SAFE_CANDIDATE_ANGLE_DEG as f32;
/// Shallowest crown magnitude (degrees) the policy projection allows.
pub const CROWN_MIN_ANGLE_DEG: f32 = 1.0;
/// Steepest crown magnitude (degrees) the policy projection allows.
pub const CROWN_MAX_ANGLE_DEG: f32 = MAX_SAFE_CANDIDATE_ANGLE_DEG as f32;

/// The signed `(min, max)` angle envelope the policy projection keeps a tier in.
///
/// `block_type` is `TierFeature::block_type` (crown `+1`, pavilion `-1`, girdle `0`);
/// The critical angle is retained as context but does not autonomously alter a coordinate.
/// Crown: `(1, MAX_SAFE)`. Pavilion: `(-MAX_SAFE, -1)`. Anything else: `(0, 0)`.
#[must_use]
pub const fn angle_envelope(block_type: f32, _critical_angle_deg: f32) -> (f32, f32) {
    if block_type < -0.5 {
        (-PAVILION_MAX_ANGLE_DEG, -1.0)
    } else if block_type > 0.5 {
        (CROWN_MIN_ANGLE_DEG, CROWN_MAX_ANGLE_DEG)
    } else {
        (0.0, 0.0)
    }
}

/// The `(lo, hi)` bounds on the angle MAGNITUDE the policy projection keeps a tier in, or `None`
/// when the block has no envelope (girdle: only the step limit applies).
///
/// Free coordinates stay within the shared sign-preserving candidate safety interval, then
/// intersect that interval with caller bounds. Critical margin is observed but never enforced.
/// `lo > hi` means no candidate is reachable.
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
    /// Whether this coordinate can move (`1.0`) or is immobile (`0.0`).
    pub is_mobile: f32,
    /// Orbit harmonics from authored index positions and cheater offsets: mean over the
    /// tier's facets of the sine of twice the facet azimuth.
    pub orbit_sin_2: f32,
    /// Mean cosine of twice the facet azimuth (see [`Self::orbit_sin_2`]).
    pub orbit_cos_2: f32,
    /// Mean sine of four times the facet azimuth (see [`Self::orbit_sin_2`]).
    pub orbit_sin_4: f32,
    /// Mean cosine of four times the facet azimuth (see [`Self::orbit_sin_2`]).
    pub orbit_cos_4: f32,
    /// Live surface area fraction of the closed solid.
    pub live_area_fraction: f32,
    /// Live facet area-weighted centroid normalized by original width.
    pub live_centroid_x: f32,
    /// Y component of the live centroid (see [`Self::live_centroid_x`]).
    pub live_centroid_y: f32,
    /// Z component of the live centroid (see [`Self::live_centroid_x`]).
    pub live_centroid_z: f32,
    /// `1.0` if a live facet area/centroid was measured.
    pub geometry_available: f32,
    /// Constraint category: MeetExisting=0, MeetNamed=0.5, ScaleReference=1.
    pub constraint_kind: f32,
    /// Whether a relation drives this tier's angle.
    pub relation_driven: f32,
    /// Whether the tier has a dimensional target.
    pub target_driven: f32,
}

impl TierFeature {
    /// Whether this coordinate has a non-empty, reachable signed-angle interval.
    #[must_use]
    pub fn is_mobile(&self) -> bool {
        self.is_mobile > 0.5
    }

    /// Dimension of the standardized per-tier feature representation.
    pub const FEATURE_DIM: usize = 26;

    /// Return standardized per-tier feature vector for set attention token embedding.
    ///
    /// Order (a contract with the network, which reads entry 3 as the block type): geometry,
    /// mobility at [`MOBILITY_INDEX`], orbit harmonics, live mesh descriptors and constraints.
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
            self.is_mobile,
            self.orbit_sin_2,
            self.orbit_cos_2,
            self.orbit_sin_4,
            self.orbit_cos_4,
            self.live_area_fraction,
            self.live_centroid_x,
            self.live_centroid_y,
            self.live_centroid_z,
            self.geometry_available,
            self.constraint_kind,
            self.relation_driven,
            self.target_driven,
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
    /// Task kind (`0` optimizer, `1` retarget).
    pub task_kind: f32,
    /// Current lighter-tone loss (0 to 100).
    pub tone_lighter_loss_pct: f32,
    /// Current deeper-tone loss (0 to 100).
    pub tone_deeper_loss_pct: f32,
    /// Shape distance from the target or baseline, times 100.
    pub shape_distance_pct: f32,
    /// Configured table target, or zero when absent.
    pub shape_target_table_pct: f32,
    /// Configured crown to pavilion target, or zero when absent.
    pub shape_target_ratio: f32,
    /// Shape penalty weight, or zero when disabled.
    pub shape_weight: f32,
    /// Probe displacement in degrees.
    pub probe_scale_deg: f32,
    /// Maximum action displacement in degrees.
    pub action_step_deg: f32,
    /// Scoring fidelity (`0` Fast, `1` Full).
    pub fidelity: f32,
    /// Whether yield is measurable.
    pub yield_available: f32,
    /// Whether an explicit shape target is configured.
    pub shape_target_available: f32,
    /// Measured target-material absorption at 450 nm (mean principal-channel coefficient).
    pub absorption_450: f32,
    /// Measured target-material absorption at 550 nm.
    pub absorption_550: f32,
    /// Measured target-material absorption at 650 nm.
    pub absorption_650: f32,
    /// Target material's model-unit absorption scale multiplied by measured stone width.
    pub absorption_path_scale: f32,
    /// Target material scattering coefficient.
    pub scattering_sigma_s: f32,
    /// Stable normalized lighting preset index.
    pub lighting_index: f32,
    /// Whether tone was measured for this observation.
    pub tone_available: f32,
    /// Seven objective coefficients in sensitivity-label order.
    pub weight_fractions: [f32; 7],
}

impl GlobalFeature {
    /// Dimension of the standardized global feature representation.
    pub const FEATURE_DIM: usize = 41;

    /// Return standardized global feature vector for context token embedding.
    ///
    /// Measured optical, material, task and search context followed by seven objective
    /// coefficients starting at [`WEIGHT_FRACTION_INDEX`].
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
            self.task_kind,
            self.tone_lighter_loss_pct / 100.0,
            self.tone_deeper_loss_pct / 100.0,
            self.shape_distance_pct / 100.0,
            self.shape_target_table_pct / 100.0,
            self.shape_target_ratio,
            self.shape_weight,
            self.probe_scale_deg / 90.0,
            self.action_step_deg / 90.0,
            self.fidelity,
            self.yield_available,
            self.shape_target_available,
            self.absorption_450,
            self.absorption_550,
            self.absorption_650,
            self.absorption_path_scale,
            self.scattering_sigma_s,
            self.lighting_index,
            self.tone_available,
            self.weight_fractions[0],
            self.weight_fractions[1],
            self.weight_fractions[2],
            self.weight_fractions[3],
            self.weight_fractions[4],
            self.weight_fractions[5],
            self.weight_fractions[6],
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
#[path = "observation_tests.rs"]
mod tests;
