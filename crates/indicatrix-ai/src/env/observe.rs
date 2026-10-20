//! The observation of the core's current state: what [`EnvCore`] tells the policy.
//!
//! One further `impl` block of the core (see [`super::common`]): [`EnvCore::observation`]
//! measures every tier and global feature of [`GemstoneObservation`] from the current design,
//! its evaluation and the episode's settings. The feature meanings and their order in the
//! tokens are the contract of [`super::observation`]; nothing is normalised here.

#![allow(
    clippy::redundant_pub_crate,
    reason = "crate-private helper module: `pub(crate)` documents the intended reach of each item"
)]

use super::{
    common::{EnvCore, girdle_ratio},
    features::{absorption_summary, orbit_harmonics},
    observation::{
        GemstoneObservation, GlobalFeature, TierFeature, angle_envelope, azimuth_sin_cos,
        objective_coefficients,
    },
};
use indicatrix::geometry::meet_solver::{Block, MeetConstraint, classify_blocks};
use indicatrix_cut_core::{
    ResolvedMaterial,
    optics_hints::is_horizontal_angle_deg,
    optimize::{MAX_SAFE_CANDIDATE_ANGLE_DEG, ObjectiveFidelity},
};

impl EnvCore {
    /// Describes the current state. `source` is the material the design was authored for,
    /// `target` the one the stone is judged in (the same for the optimizer).
    pub(crate) fn observation(
        &self,
        source: &ResolvedMaterial,
        target: &ResolvedMaterial,
    ) -> GemstoneObservation {
        let design = &self.current_design;
        let eval = &self.current_eval;
        let target_crit = target.critical_angle_deg as f32;
        let blocks = classify_blocks(&design.meet_tier_inputs());
        let max_mast = eval
            .masts
            .iter()
            .fold(0.0_f64, |max, mast| max.max(mast.abs()));

        let tier_count = design.tiers.len() as f32;
        let tiers = design
            .tiers
            .iter()
            .enumerate()
            .map(|(index, tier)| {
                let angle = tier.angle_deg as f32;
                // The base and the current design have the same tiers (a step only moves
                // angles and masts), so the base tier at `index` always exists.
                let base_angle = self.base_design.tiers[index].angle_deg as f32;
                let (azimuth_sin, azimuth_cos) =
                    azimuth_sin_cos(tier.indices.first().copied(), design.meta.gear_teeth);
                let block = match blocks.get(index).copied() {
                    Some(Block::Crown) => 1.0_f32,
                    Some(Block::Pavilion) => -1.0,
                    _ => 0.0,
                };
                let flat = is_horizontal_angle_deg(tier.angle_deg)
                    || tier.angle_deg.abs() >= MAX_SAFE_CANDIDATE_ANGLE_DEG;
                let (min_angle_deg, max_angle_deg) = if flat {
                    (0.0, 0.0)
                } else {
                    let (safe_lo, safe_hi) = angle_envelope(block, target_crit);
                    let (user_lo, user_hi) = self
                        .settings
                        .angle_bounds
                        .get(&index)
                        .copied()
                        .unwrap_or((f64::from(safe_lo), f64::from(safe_hi)));
                    let lo = f64::from(safe_lo).max(user_lo);
                    let hi = f64::from(safe_hi).min(user_hi);
                    if lo <= hi {
                        (lo as f32, hi as f32)
                    } else {
                        (0.0, 0.0)
                    }
                };
                let mobile = self.space.is_free(index)
                    && min_angle_deg <= max_angle_deg
                    && angle >= min_angle_deg
                    && angle <= max_angle_deg;
                let mast = match eval.masts.get(index) {
                    Some(mast) if max_mast > 0.0 => (mast.abs() / max_mast) as f32,
                    _ => 0.0,
                };
                let (orbit_sin_2, orbit_cos_2, orbit_sin_4, orbit_cos_4) =
                    orbit_harmonics(design, index);
                let live = eval
                    .live_tier_geometry
                    .get(index)
                    .copied()
                    .unwrap_or_default();
                let width = self.base_eval.width_axis.unwrap_or(0.0);
                TierFeature {
                    angle_normalized: angle / 90.0,
                    angle_deg: angle,
                    base_angle_deg: base_angle,
                    orbit_count: tier.indices.len() as u32,
                    block_type: block,
                    is_pinned: if mobile { 0.0 } else { 1.0 },
                    is_flat: if flat { 1.0 } else { 0.0 },
                    critical_margin_deg: if block < 0.0 {
                        angle.abs() - target_crit
                    } else {
                        0.0
                    },
                    mast,
                    min_angle_deg,
                    max_angle_deg,
                    index_frac: index as f32 / tier_count,
                    azimuth_sin,
                    azimuth_cos,
                    is_mobile: if mobile { 1.0 } else { 0.0 },
                    orbit_sin_2,
                    orbit_cos_2,
                    orbit_sin_4,
                    orbit_cos_4,
                    live_area_fraction: live.area_fraction,
                    live_centroid_x: if live.available && width > 0.0 {
                        (live.centroid[0] / width) as f32
                    } else {
                        0.0
                    },
                    live_centroid_y: if live.available && width > 0.0 {
                        (live.centroid[1] / width) as f32
                    } else {
                        0.0
                    },
                    live_centroid_z: if live.available && width > 0.0 {
                        (live.centroid[2] / width) as f32
                    } else {
                        0.0
                    },
                    geometry_available: if live.available { 1.0 } else { 0.0 },
                    constraint_kind: match &tier.constraint {
                        MeetConstraint::MeetExisting => 0.0,
                        MeetConstraint::MeetNamed(_) => 0.5,
                        MeetConstraint::ScaleReference(_) => 1.0,
                    },
                    relation_driven: if design.is_tier_driven(index) {
                        1.0
                    } else {
                        0.0
                    },
                    target_driven: if design.tier_target(index).is_some() {
                        1.0
                    } else {
                        0.0
                    },
                }
            })
            .collect();

        GemstoneObservation {
            tiers,
            global: GlobalFeature {
                refractive_index: source.n_d as f32,
                target_refractive_index: target.n_d as f32,
                critical_angle_deg: source.critical_angle_deg as f32,
                target_critical_angle_deg: target_crit,
                dispersion: target.gem.dispersion.evaluate(486.1)
                    - target.gem.dispersion.evaluate(656.3),
                windowing_pct: eval.components.windowing_pct,
                extinction_pct: eval.components.extinction_pct,
                tilt_brilliance_pct: eval.components.tilt_brilliance_pct,
                yield_pct: eval.yield_pct.unwrap_or(0.0),
                table_pct: eval.table_percent.unwrap_or(0.0) as f32,
                crown_to_pavilion_ratio: eval.crown_to_pavilion.unwrap_or(0.0) as f32,
                min_girdle_ratio: girdle_ratio(
                    self.base_eval.girdle_thickness,
                    eval.girdle_thickness,
                ),
                best_score_so_far: self.best_score,
                progress_ratio: self.step_count as f32 / self.settings.max_steps as f32,
                base_score: self.base_eval.score,
                weight_fractions: objective_coefficients(
                    &self.settings.weights,
                    eval.yield_pct.is_some(),
                    self.settings
                        .shape_target
                        .map_or(0.0, |target| target.weight),
                ),
                task_kind: self.settings.task_kind,
                tone_lighter_loss_pct: eval.tone_lighter_loss_pct.unwrap_or(0.0),
                tone_deeper_loss_pct: eval.tone_deeper_loss_pct.unwrap_or(0.0),
                shape_distance_pct: if eval.shape_distance_pct.is_finite() {
                    eval.shape_distance_pct
                } else {
                    0.0
                },
                shape_target_table_pct: self
                    .settings
                    .shape_target
                    .and_then(|t| t.table_percent)
                    .unwrap_or(0.0) as f32,
                shape_target_ratio: self
                    .settings
                    .shape_target
                    .and_then(|t| t.crown_to_pavilion)
                    .unwrap_or(0.0) as f32,
                shape_weight: self.settings.shape_target.map_or(0.0, |t| t.weight),
                probe_scale_deg: self.settings.probe_scale_deg as f32,
                action_step_deg: self.settings.max_step_deg as f32,
                fidelity: if self.settings.fidelity == ObjectiveFidelity::Full {
                    1.0
                } else {
                    0.0
                },
                yield_available: if eval.yield_pct.is_some() { 1.0 } else { 0.0 },
                shape_target_available: if self.settings.shape_target.is_some() {
                    1.0
                } else {
                    0.0
                },
                absorption_450: absorption_summary(&target.gem, 450.0),
                absorption_550: absorption_summary(&target.gem, 550.0),
                absorption_650: absorption_summary(&target.gem, 650.0),
                absorption_path_scale: target.gem.absorption_path_scale
                    * eval.width_axis.unwrap_or(0.0) as f32,
                scattering_sigma_s: target.gem.scattering_sigma_s,
                lighting_index: self.settings.lighting.index() as f32 / 14.0,
                tone_available: 1.0,
            },
        }
    }
}
