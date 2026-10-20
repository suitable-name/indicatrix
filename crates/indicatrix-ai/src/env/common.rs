//! The machinery both environments share: scoring a design, the greedy episode core and its
//! description as an observation.
//!
//! The optimizer environment scores a design in its own material; the retarget environment
//! scores it in a target material. Everything else is identical, so both wrap one
//! [`EnvCore`] and only differ in the gem they score with and in how they describe the
//! materials in the observation.
//!
//! The core is spread over five modules that share its crate-visible fields: this one (the
//! state and scoring), [`super::observe`] (the observation of the current state),
//! [`super::moves`] (the movable set and how an action becomes a candidate),
//! [`super::stepping`] (the greedy step and its line search) and [`super::teacher`] (the
//! finite-difference labels).

#![allow(
    clippy::redundant_pub_crate,
    reason = "crate-private helper module: `pub(crate)` documents the intended reach of each item"
)]

use super::{
    error::EnvError,
    reward::{RewardConfig, RewardEvaluator},
    traits::{StepInfo, StepRejection},
};
use glam::DVec3;
use indicatrix::{
    color::metrics::FaceUpTone,
    geometry::{
        GpuFacetPlane, StoneGeometry,
        stone_metrics::{
            SolidMesh, SolidStatus, StoneProportions, build_solid_mesh_geom, measure_solid,
        },
    },
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_cut_core::{
    BuiltinMaterials, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, DesignSolveError,
    ManufacturabilityWarning, MaterialLookup, MaterialSelection, ObjectiveComponents,
    ObjectiveWeights, ResolvedMaterial, ShapeTarget, critical_angle_deg,
    design::{girdle_band_in, tier_plane_ranges},
    manufacturability::check_manufacturability_with_geometry,
    optimize::{ObjectiveFidelity, evaluate_objective_with_tone_geom_under_threaded},
    volumetric_yield,
};
use std::sync::Arc;

pub(crate) use super::moves::{MoveSpace, apply_action};

/// Threads the `Full` tilt sweep inside one environment step may use. The trainer steps its
/// environments on up to `available_parallelism` scoped threads at once
/// (`indicatrix-trainer`'s `step_envs`), so each sweep stays on its own thread and the
/// environments, not the sweep, fill the cores. `Fast` (the training default) is one pose
/// and ignores it. The sweep result does not depend on the thread count.
const ENV_INNER_THREADS: Option<usize> = Some(1);

/// Default score below which an episode counts as solved: `0.0`, so ordinary positive-score
/// episodes end at the step limit. (A threshold of 5 ended the episodes of designs
/// whose base score was already below it after their first step, producing one-step
/// episodes that taught nothing.)
pub(crate) const DEFAULT_TERMINAL_SCORE: f32 = 0.0;

/// Default fraction of the base design's thinnest girdle band a candidate must keep (the
/// value the cut-core optimizer documents for "at least half the girdle thickness").
pub(crate) const DEFAULT_MIN_GIRDLE_RATIO: f32 = 0.5;

/// The solved facet planes in the GPU plane convention, shared by the environments and the
/// inference report.
pub(crate) fn to_gpu_planes(planes: &[(DVec3, f64)]) -> Vec<GpuFacetPlane> {
    // `planes_from_solved` yields n.x <= m; the GPU convention is n.x + d <= 0, so d = -m.
    planes
        .iter()
        .map(|&(normal, mast)| GpuFacetPlane::new(normal.as_vec3(), -mast as f32))
        .collect()
}

/// The material a design was authored for, as the editor and the optimizer see it.
///
/// The built-in preset named by the selection (diamond when there is none), then the
/// selection's refractive-index and colour overrides; with no recognised preset and no
/// override, the design's effective refractive index ([`Design::effective_refractive_index`],
/// so an `.asc` `I` line) as a flat index.
#[must_use]
pub fn design_source_material(design: &Design) -> ResolvedMaterial {
    let mut selection: MaterialSelection = design.material.clone();
    let looked_up = selection
        .name
        .as_deref()
        .and_then(|name| BuiltinMaterials.lookup(name));
    if looked_up.is_none() && selection.refractive_index_override.is_none() {
        selection.refractive_index_override = Some(design.effective_refractive_index());
    }
    let gem = selection.apply_overrides(looked_up.unwrap_or_else(GemMaterial::diamond));
    let n_d = selection
        .refractive_index_override
        .unwrap_or_else(|| f64::from(gem.dispersion.evaluate(589.3)));
    ResolvedMaterial {
        critical_angle_deg: critical_angle_deg(n_d),
        n_d,
        gem,
    }
}

/// Why a candidate design could not be scored.
#[derive(Debug)]
pub(crate) enum EvalFailure {
    Unsolvable(DesignSolveError),
    NotClosed,
    NonFiniteScore,
}

impl EvalFailure {
    pub(crate) const fn rejection(&self) -> StepRejection {
        match self {
            Self::Unsolvable(_) => StepRejection::Unsolvable,
            Self::NotClosed => StepRejection::NotClosed,
            Self::NonFiniteScore => StepRejection::NonFiniteScore,
        }
    }
}

impl From<EvalFailure> for EnvError {
    fn from(failure: EvalFailure) -> Self {
        match failure {
            EvalFailure::Unsolvable(error) => Self::Unsolvable(error),
            EvalFailure::NotClosed => Self::NotClosed,
            EvalFailure::NonFiniteScore => Self::NonFiniteScore,
        }
    }
}

/// Everything measured on one solved, closed design.
#[derive(Debug, Clone)]
pub(crate) struct DesignEval {
    pub(crate) score: f32,
    pub(crate) components: ObjectiveComponents,
    pub(crate) tone_lighter_loss_pct: Option<f32>,
    pub(crate) tone_deeper_loss_pct: Option<f32>,
    pub(crate) tone: FaceUpTone,
    pub(crate) shape_distance_pct: f32,
    /// Solved mast per tier.
    pub(crate) masts: Vec<f64>,
    pub(crate) table_percent: Option<f64>,
    pub(crate) crown_to_pavilion: Option<f64>,
    pub(crate) yield_pct: Option<f32>,
    /// Thinnest girdle gap, design units; `None` without a live girdle wall.
    pub(crate) girdle_thickness: Option<f64>,
    pub(crate) vanishing_facets: usize,
    pub(crate) undersized_facets: usize,
    pub(crate) width_axis: Option<f64>,
    pub(crate) live_tier_geometry: Vec<LiveTierGeometry>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LiveTierGeometry {
    pub(crate) area_fraction: f32,
    pub(crate) centroid: [f64; 3],
    pub(crate) available: bool,
}

/// The volume of `design`'s preform, `None` when it does not measure.
///
/// A pure function of `design.preform`, which neither environment ever writes (they move
/// only `angle_deg` and `constraint`), so [`EnvCore`] measures it once and hands it to
/// every [`evaluate_design`] call instead of re-measuring per step.
pub(crate) fn preform_volume(design: &Design) -> Option<f64> {
    measure_solid(&design.preform.planes()).map(|metrics| metrics.volume)
}

/// The figure the environments minimise (`0..=100`, lower is better).
///
/// The optical score ([`ObjectiveWeights::score`]), unless the objective weighs the yield
/// (`yield_weight > 0`) and the yield is measurable (`yield_pct` is `Some`): then the yield
/// loss [`yield_loss_pct`] joins it ([`ObjectiveWeights::score_with_yield`]). A stone with no
/// measurable yield is scored on its optics alone, and a zero yield weight reproduces the
/// optical score bit for bit.
#[cfg(test)]
#[must_use]
pub(crate) fn objective_score(
    weights: &ObjectiveWeights,
    components: &ObjectiveComponents,
    yield_pct: Option<f32>,
) -> f32 {
    match yield_pct {
        Some(yield_pct) if weights.yield_weight > 0.0 => {
            weights.score_with_yield(components, yield_loss_pct(yield_pct))
        }
        _ => weights.score(components),
    }
}

/// The yield loss the score and the teacher's yield component use: exactly `100 - yield_pct`,
/// matching the shared cut-core objective, including when measured yield exceeds 100 %.
#[must_use]
pub(crate) fn yield_loss_pct(yield_pct: f32) -> f32 {
    100.0 - yield_pct
}

/// Solves, closes, scores and measures `design` in `gem`.
///
/// `preform_volume` is [`preform_volume`] of `design`: the caller measures it once per
/// base design. The plane list, mesh and solid measurement of the candidate are built once
/// here and shared with the manufacturability check. The score uses the same core objective
/// path as optimization, including measured yield when requested and available.
pub(crate) fn evaluate_design(
    design: &Design,
    gem: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
    preform_volume: Option<f64>,
    lighting: LightingPreset,
    shape_target: Option<ShapeTarget>,
) -> Result<DesignEval, EvalFailure> {
    let solved = design.solve().map_err(EvalFailure::Unsolvable)?;
    let planes = design.planes_from_solved(&solved);
    let tools = design.tools_for_scoring(&planes);
    let gpu_planes = to_gpu_planes(&planes);
    let geometry = StoneGeometry {
        planes: &gpu_planes,
        tools: &tools,
    };
    let SolidStatus::Closed(mesh) = build_solid_mesh_geom(&planes, &tools) else {
        return Err(EvalFailure::NotClosed);
    };

    let (components, tone) = evaluate_objective_with_tone_geom_under_threaded(
        geometry,
        gem,
        fidelity,
        lighting,
        ENV_INNER_THREADS,
    );
    // The components are checked before the solid is measured, so a dead measurement exits
    // early; the final score is checked once the yield term (if any) is in.
    let finite = [
        components.windowing_pct,
        components.extinction_pct,
        components.tilt_brilliance_pct,
    ]
    .iter()
    .all(|value| value.is_finite());
    if !finite {
        return Err(EvalFailure::NonFiniteScore);
    }

    let flat_metrics = measure_solid(&planes);
    let metrics = Design::carved_metrics(flat_metrics, &planes, &tools);
    let (table_percent, crown_to_pavilion) = metrics.as_ref().map_or((None, None), |metrics| {
        let proportions = StoneProportions::from_solid(metrics, &mesh, &planes);
        let ratio = match (proportions.crown_height, proportions.pavilion_depth) {
            (Some(crown), Some(depth)) if depth > 0.0 => Some(crown / depth),
            _ => None,
        };
        (proportions.table_percent, ratio)
    });
    let yield_pct = metrics
        .as_ref()
        .and_then(|finished| volumetric_yield(finished.volume, preform_volume?))
        .map(|fraction| (fraction * 100.0) as f32);
    let tone_lighter_loss = indicatrix_cut_core::ToneGoal::Lighter.loss_pct(&tone);
    let tone_deeper_loss = indicatrix_cut_core::ToneGoal::Deeper.loss_pct(&tone);
    let tone_lighter_loss_pct = Some(tone_lighter_loss);
    let tone_deeper_loss_pct = Some(tone_deeper_loss);
    let effective_yield_weight = if yield_pct.is_some() {
        weights.yield_weight
    } else {
        0.0
    };
    let effective_weights = ObjectiveWeights {
        yield_weight: effective_yield_weight,
        ..*weights
    };
    let tone_loss = match weights.tone_goal {
        indicatrix_cut_core::ToneGoal::Lighter => tone_lighter_loss,
        indicatrix_cut_core::ToneGoal::Deeper => tone_deeper_loss,
    };
    let mut score = effective_weights.score_with_tone(
        &components,
        yield_pct.map_or(0.0, yield_loss_pct),
        tone_loss,
    );
    let shape_distance_pct = shape_target.map_or(f32::NAN, |target| {
        (100.0 * target.distance(table_percent, crown_to_pavilion)) as f32
    });
    if let Some(target) = shape_target {
        score += target.penalty(table_percent, crown_to_pavilion);
    }
    if !score.is_finite() {
        return Err(EvalFailure::NonFiniteScore);
    }

    let ranges = tier_plane_ranges(design, &solved);
    let live_tier_geometry = live_geometry_by_tier(&ranges, &mesh);
    let girdle_thickness =
        girdle_band_in(design, &ranges, &planes, &mesh).map(|band| band.min_thickness);

    let warnings = check_manufacturability_with_geometry(
        design,
        &solved,
        Some(&mesh),
        metrics.as_ref(),
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    );
    let vanishing_facets = warnings
        .iter()
        .filter(|warning| matches!(warning, ManufacturabilityWarning::VanishingFacet { .. }))
        .count();
    let undersized_facets = warnings
        .iter()
        .filter(|warning| matches!(warning, ManufacturabilityWarning::UndersizedFacet { .. }))
        .count();

    Ok(DesignEval {
        score,
        components,
        tone_lighter_loss_pct,
        tone_deeper_loss_pct,
        tone,
        shape_distance_pct,
        masts: solved.iter().map(|tier| tier.mast).collect(),
        table_percent,
        crown_to_pavilion,
        yield_pct,
        girdle_thickness,
        vanishing_facets,
        undersized_facets,
        width_axis: metrics.as_ref().map(|m| m.width_axis),
        live_tier_geometry,
    })
}

fn live_geometry_by_tier(
    ranges: &[std::ops::Range<usize>],
    mesh: &SolidMesh,
) -> Vec<LiveTierGeometry> {
    let mut areas = vec![0.0_f64; ranges.len()];
    let mut moments = vec![[0.0_f64; 3]; ranges.len()];
    let mut total = 0.0_f64;
    for (facet, ring) in &mesh.rings {
        let Some((tier_index, _)) = ranges.iter().enumerate().find(|(_, r)| r.contains(facet))
        else {
            continue;
        };
        if ring.len() < 3 {
            continue;
        }
        let origin = ring[0];
        let mut area = 0.0;
        let mut moment = [0.0; 3];
        for i in 1..ring.len() - 1 {
            let a = ring[i] - origin;
            let b = ring[i + 1] - origin;
            let triangle_area = 0.5 * a.cross(b).length();
            if !triangle_area.is_finite() {
                continue;
            }
            let center = (origin + ring[i] + ring[i + 1]) / 3.0;
            area += triangle_area;
            moment[0] += center.x * triangle_area;
            moment[1] += center.y * triangle_area;
            moment[2] += center.z * triangle_area;
        }
        areas[tier_index] += area;
        for axis in 0..3 {
            moments[tier_index][axis] += moment[axis];
        }
        total += area;
    }
    ranges
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let area = areas[i];
            LiveTierGeometry {
                area_fraction: if total > 0.0 {
                    (area / total) as f32
                } else {
                    0.0
                },
                centroid: if area > 0.0 {
                    [
                        moments[i][0] / area,
                        moments[i][1] / area,
                        moments[i][2] / area,
                    ]
                } else {
                    [0.0; 3]
                },
                available: area > 0.0 && total > 0.0,
            }
        })
        .collect()
}

/// Whether a candidate keeps the girdle: not lost, not a knife edge, and at least
/// `min_ratio` of the base design's thinnest gap. A base design without a girdle band has
/// nothing to keep.
fn girdle_ok(base: Option<f64>, candidate: Option<f64>, min_ratio: f32) -> bool {
    match (base, candidate) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(base), Some(candidate)) => {
            candidate >= f64::from(min_ratio) * base && (candidate > 0.0 || base <= 0.0)
        }
    }
}

/// Thinnest girdle gap of `current` relative to `base` (`1.0` when there is nothing to
/// compare).
pub(crate) const fn girdle_ratio(base: Option<f64>, current: Option<f64>) -> f32 {
    match (base, current) {
        (Some(base), Some(current)) if base > 0.0 => (current / base) as f32,
        _ => 1.0,
    }
}

/// Silhouette drift of `candidate` from `base` in percent: `100 *`
/// [`ShapeTarget::distance`] over table size and crown-to-pavilion ratio.
pub(crate) fn shape_drift_pct(base: &DesignEval, candidate: &DesignEval) -> f32 {
    let target = ShapeTarget {
        table_percent: base.table_percent,
        crown_to_pavilion: base.crown_to_pavilion,
        weight: 1.0,
    };
    (100.0 * target.distance(candidate.table_percent, candidate.crown_to_pavilion)) as f32
}

/// Episode settings the core needs (the part of the two config types they share).
#[derive(Debug, Clone)]
pub(crate) struct CoreSettings {
    pub(crate) max_steps: usize,
    pub(crate) max_step_deg: f64,
    pub(crate) weights: ObjectiveWeights,
    pub(crate) fidelity: ObjectiveFidelity,
    pub(crate) terminal_score: f32,
    pub(crate) min_girdle_ratio: f32,
    pub(crate) vary_anchored: bool,
    pub(crate) task_kind: f32,
    pub(crate) lighting: LightingPreset,
    pub(crate) shape_target: Option<ShapeTarget>,
    pub(crate) probe_scale_deg: f64,
    pub(crate) only_tiers: Option<std::collections::BTreeSet<usize>>,
    pub(crate) angle_bounds: std::collections::BTreeMap<usize, (f64, f64)>,
}

impl CoreSettings {
    fn validate(&self) -> Result<(), EnvError> {
        if self.max_steps == 0 {
            return Err(EnvError::InvalidConfig("max_steps must be at least 1"));
        }
        if !(self.max_step_deg.is_finite() && self.max_step_deg > 0.0) {
            return Err(EnvError::InvalidConfig(
                "max_step_deg must be finite and positive",
            ));
        }
        if !self.terminal_score.is_finite() {
            return Err(EnvError::InvalidConfig("terminal_score must be finite"));
        }
        if !(self.min_girdle_ratio.is_finite() && (0.0..=1.0).contains(&self.min_girdle_ratio)) {
            return Err(EnvError::InvalidConfig(
                "min_girdle_ratio must lie in 0..=1",
            ));
        }
        if !self.probe_scale_deg.is_finite() || self.probe_scale_deg <= 0.0 {
            return Err(EnvError::InvalidConfig(
                "probe_scale_deg must be finite and positive",
            ));
        }
        if [
            self.weights.windowing,
            self.weights.extinction,
            self.weights.tilt_brilliance,
            self.weights.yield_weight,
            self.weights.tone_weight,
        ]
        .iter()
        .any(|w| !w.is_finite())
        {
            return Err(EnvError::InvalidConfig("objective weights must be finite"));
        }
        if self.shape_target.is_some_and(|target| {
            !target.weight.is_finite()
                || target.weight < 0.0
                || target.table_percent.is_some_and(|value| !value.is_finite())
                || target
                    .crown_to_pavilion
                    .is_some_and(|value| !value.is_finite())
        }) {
            return Err(EnvError::InvalidConfig(
                "shape target values must be finite and its weight non-negative",
            ));
        }
        if self
            .angle_bounds
            .values()
            .any(|(lo, hi)| !lo.is_finite() || !hi.is_finite() || lo > hi)
        {
            return Err(EnvError::InvalidConfig(
                "angle bounds must be finite inclusive ranges",
            ));
        }
        Ok(())
    }
}

/// What one core step produced (the environment adds the observation).
pub(crate) struct CoreStep {
    pub(crate) reward: f32,
    pub(crate) terminated: bool,
    pub(crate) truncated: bool,
    pub(crate) info: StepInfo,
}

/// The greedy episode: a base design, the current accepted design, and the scoring gem.
///
/// The fields are crate-visible because the step ([`super::stepping`]) and the teacher
/// ([`super::teacher`]) are further `impl` blocks of this type.
pub(crate) struct EnvCore {
    pub(crate) settings: CoreSettings,
    pub(crate) gem: GemMaterial,
    pub(crate) reward: RewardEvaluator,
    pub(crate) base_design: Design,
    /// [`preform_volume`] of the base design, measured once: the base design is fixed for
    /// the core's life (a new design is a new core), so it never goes stale.
    pub(crate) preform_volume: Option<f64>,
    pub(crate) current_design: Design,
    pub(crate) space: MoveSpace,
    pub(crate) base_eval: DesignEval,
    pub(crate) current_eval: DesignEval,
    pub(crate) best_score: f32,
    pub(crate) step_count: usize,
    pub(crate) last_components: Option<ObjectiveComponents>,
}

impl EnvCore {
    /// Builds the core for `design` scored in `gem`.
    ///
    /// # Errors
    ///
    /// [`EnvError::InvalidConfig`] for an out-of-range setting; [`EnvError::Unsolvable`],
    /// [`EnvError::NotClosed`] or [`EnvError::NonFiniteScore`] when the base design cannot be
    /// scored; [`EnvError::NoMovableTiers`]
    /// when no tier can move.
    pub(crate) fn new(
        design: Design,
        gem: GemMaterial,
        settings: CoreSettings,
        reward_config: RewardConfig,
    ) -> Result<Self, EnvError> {
        settings.validate()?;
        reward_config.validate().map_err(EnvError::InvalidConfig)?;
        if settings
            .only_tiers
            .as_ref()
            .is_some_and(|tiers| tiers.iter().any(|&i| i >= design.tiers.len()))
            || settings.angle_bounds.iter().any(|(&i, &(lo, hi))| {
                design
                    .tiers
                    .get(i)
                    .is_none_or(|tier| tier.angle_deg < lo || tier.angle_deg > hi)
            })
        {
            return Err(EnvError::InvalidConfig(
                "tier scope or angle bounds do not contain the design",
            ));
        }
        let preform_volume = preform_volume(&design);
        let base_eval = evaluate_design(
            &design,
            &gem,
            &settings.weights,
            settings.fidelity,
            preform_volume,
            settings.lighting,
            settings.shape_target,
        )?;
        let policy = if settings.task_kind > 0.5 {
            indicatrix_cut_core::meet_preservation::PreservationPolicy::RETARGET
        } else {
            indicatrix_cut_core::meet_preservation::PreservationPolicy::OPTIMIZE
        };
        let space = MoveSpace::measure_with(
            &design,
            settings.vary_anchored,
            settings.only_tiers.as_ref(),
            policy,
            &settings.angle_bounds,
        )?;
        Ok(Self {
            best_score: base_eval.score,
            current_eval: base_eval.clone(),
            base_eval,
            settings,
            gem,
            reward: RewardEvaluator::new(reward_config),
            base_design: design.clone(),
            preform_volume,
            current_design: design,
            space,
            step_count: 0,
            last_components: None,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.current_design = self.base_design.clone();
        self.current_eval = self.base_eval.clone();
        self.best_score = self.base_eval.score;
        self.step_count = 0;
        self.last_components = None;
    }

    pub(crate) fn free_tiers(&self) -> &[usize] {
        self.space.free()
    }

    pub(crate) const fn base_design(&self) -> &Design {
        &self.base_design
    }

    pub(crate) const fn current_design(&self) -> &Design {
        &self.current_design
    }

    /// The objective weights the core scores with.
    pub(crate) const fn weights(&self) -> &ObjectiveWeights {
        &self.settings.weights
    }

    pub(crate) const fn base_score(&self) -> f32 {
        self.base_eval.score
    }

    pub(crate) const fn current_score(&self) -> f32 {
        self.current_eval.score
    }

    pub(crate) fn preservation_facts(
        &self,
    ) -> Result<indicatrix_cut_core::meet_preservation::PreservationFacts, String> {
        self.space.certify(&self.current_design)
    }

    pub(crate) fn certify_design(
        &self,
        design: &Design,
    ) -> Result<indicatrix_cut_core::meet_preservation::PreservationFacts, String> {
        self.space.certify(design)
    }

    pub(crate) fn preservation_baseline(
        &self,
    ) -> Arc<indicatrix_cut_core::meet_preservation::MeetPreservationBaseline> {
        self.space.baseline()
    }

    pub(crate) const fn best_score(&self) -> f32 {
        self.best_score
    }

    pub(crate) const fn base_components(&self) -> ObjectiveComponents {
        self.base_eval.components
    }

    pub(crate) const fn current_components(&self) -> ObjectiveComponents {
        self.current_eval.components
    }

    pub(crate) const fn last_components(&self) -> Option<ObjectiveComponents> {
        self.last_components
    }

    pub(crate) const fn step_count(&self) -> usize {
        self.step_count
    }

    /// Whether `eval` keeps the girdle of the base design (see [`girdle_ok`]); the gate the
    /// step and the teacher probes both apply.
    pub(crate) fn girdle_gate(&self, eval: &DesignEval) -> bool {
        girdle_ok(
            self.base_eval.girdle_thickness,
            eval.girdle_thickness,
            self.settings.min_girdle_ratio,
        )
    }

    /// Whether `eval` has no more vanishing or undersized facets than the base design.
    pub(crate) const fn manufacturing_gate(&self, eval: &DesignEval) -> bool {
        eval.vanishing_facets <= self.base_eval.vanishing_facets
            && eval.undersized_facets <= self.base_eval.undersized_facets
    }
}

#[cfg(test)]
#[path = "common_tests.rs"]
mod tests;
