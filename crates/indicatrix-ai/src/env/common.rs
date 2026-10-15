//! The machinery both environments share: scoring a design, the greedy episode core and its
//! description as an observation.
//!
//! The optimizer environment scores a design in its own material; the retarget environment
//! scores it in a target material. Everything else is identical, so both wrap one
//! [`EnvCore`] and only differ in the gem they score with and in how they describe the
//! materials in the observation.
//!
//! The core is spread over four modules that share its crate-visible fields: this one (the
//! state, scoring and the observation), [`super::moves`] (the movable set and how an action
//! becomes a candidate), [`super::stepping`] (the greedy step and its line search) and
//! [`super::teacher`] (the finite-difference labels).

#![allow(
    clippy::redundant_pub_crate,
    reason = "crate-private helper module: `pub(crate)` documents the intended reach of each item"
)]

use super::{
    error::EnvError,
    observation::{
        GemstoneObservation, GlobalFeature, TierFeature, angle_envelope, azimuth_sin_cos,
        weight_fractions,
    },
    reward::{RewardConfig, RewardEvaluator},
    traits::{StepInfo, StepRejection},
};
use glam::DVec3;
use indicatrix::{
    geometry::{
        GpuFacetPlane,
        meet_solver::{Block, classify_blocks},
        stone_metrics::{SolidStatus, StoneProportions, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    BuiltinMaterials, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, DesignSolveError,
    ManufacturabilityWarning, MaterialLookup, MaterialSelection, ObjectiveComponents,
    ObjectiveWeights, ResolvedMaterial, ShapeTarget, critical_angle_deg,
    design::{girdle_band_in, tier_plane_ranges},
    manufacturability::check_manufacturability_with_geometry,
    optics_hints::is_horizontal_angle_deg,
    optimize::{MAX_SAFE_CANDIDATE_ANGLE_DEG, ObjectiveFidelity, evaluate_objective_threaded},
    volumetric_yield,
};

pub(crate) use super::moves::{MoveSpace, apply_action};

/// Threads the `Full` tilt sweep inside one environment step may use. The trainer steps its
/// environments on up to `available_parallelism` scoped threads at once
/// (`indicatrix-trainer`'s `step_envs`), so each sweep stays on its own thread and the
/// environments, not the sweep, fill the cores. `Fast` (the training default) is one pose
/// and ignores it. The sweep result does not depend on the thread count.
const ENV_INNER_THREADS: Option<usize> = Some(1);

/// Default score below which an episode counts as solved: `0.0` = an episode only ends at the
/// step limit; scores are never negative. (A threshold of 5 ended the episodes of designs
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
    /// Solved mast per tier.
    pub(crate) masts: Vec<f64>,
    pub(crate) table_percent: Option<f64>,
    pub(crate) crown_to_pavilion: Option<f64>,
    pub(crate) yield_pct: Option<f32>,
    /// Thinnest girdle gap, design units; `None` without a live girdle wall.
    pub(crate) girdle_thickness: Option<f64>,
    pub(crate) vanishing_facets: usize,
    pub(crate) undersized_facets: usize,
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

/// The yield loss the score and the teacher's yield component use: `100 - yield_pct`, floored
/// at zero.
///
/// Angle moves can push a stone's solid outside its preform (a yield above 100 %; nothing in
/// the environment keeps the stone inside the preform). Such a stone counts as a full yield,
/// not as a gain: without the floor the score could drop below zero and end an episode as
/// "perfect" (`terminal_score` is 0), and a tier would be rewarded for growing the stone past
/// the rough.
#[must_use]
pub(crate) fn yield_loss_pct(yield_pct: f32) -> f32 {
    (100.0 - yield_pct).max(0.0)
}

/// Solves, closes, scores and measures `design` in `gem`.
///
/// `preform_volume` is [`preform_volume`] of `design`: the caller measures it once per
/// base design. The plane list, mesh and solid measurement of the candidate are built once
/// here and shared with the manufacturability check. The score is [`objective_score`]:
/// optical, plus the yield term when the weights ask for it and the yield measures.
pub(crate) fn evaluate_design(
    design: &Design,
    gem: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
    preform_volume: Option<f64>,
) -> Result<DesignEval, EvalFailure> {
    let solved = design.solve().map_err(EvalFailure::Unsolvable)?;
    let planes = design.planes_from_solved(&solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return Err(EvalFailure::NotClosed);
    };

    let components =
        evaluate_objective_threaded(&to_gpu_planes(&planes), gem, fidelity, ENV_INNER_THREADS);
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

    let metrics = measure_solid(&planes);
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
    let score = objective_score(weights, &components, yield_pct);
    if !score.is_finite() {
        return Err(EvalFailure::NonFiniteScore);
    }

    let ranges = tier_plane_ranges(design, &solved);
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
        masts: solved.iter().map(|tier| tier.mast).collect(),
        table_percent,
        crown_to_pavilion,
        yield_pct,
        girdle_thickness,
        vanishing_facets,
        undersized_facets,
    })
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

/// Whether a stone returns no light face-up (tilt brilliance `0 %`): the tracer finds no
/// light, every move scores exactly the same and a step costs 7 ms for nothing. Such a dead
/// stone is refused at construction ([`EnvError::NoLightReturn`]).
pub(crate) const fn returns_no_light(components: &ObjectiveComponents) -> bool {
    components.tilt_brilliance_pct <= 0.0
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
        if self.weights.tone_weight > 0.0 {
            return Err(EnvError::InvalidConfig(
                "the face-up tone objective is not supported",
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
    /// [`EnvError::InvalidConfig`] for an out-of-range setting or a face-up tone weight (the
    /// environments do not trace the face-up tone); [`EnvError::Unsolvable`],
    /// [`EnvError::NotClosed`] or [`EnvError::NonFiniteScore`] when the base design cannot be
    /// scored; [`EnvError::NoLightReturn`] when it scores but returns no light face-up (a dead
    /// stone: no move changes its score, see [`returns_no_light`]); [`EnvError::NoMovableTiers`]
    /// when no tier can move.
    pub(crate) fn new(
        design: Design,
        gem: GemMaterial,
        settings: CoreSettings,
        reward_config: RewardConfig,
    ) -> Result<Self, EnvError> {
        settings.validate()?;
        let preform_volume = preform_volume(&design);
        let base_eval = evaluate_design(
            &design,
            &gem,
            &settings.weights,
            settings.fidelity,
            preform_volume,
        )?;
        if returns_no_light(&base_eval.components) {
            return Err(EnvError::NoLightReturn);
        }
        let space = MoveSpace::measure(&design, settings.vary_anchored)?;
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
                    angle_envelope(block, target_crit)
                };
                let mast = match eval.masts.get(index) {
                    Some(mast) if max_mast > 0.0 => (mast.abs() / max_mast) as f32,
                    _ => 0.0,
                };
                TierFeature {
                    angle_normalized: angle / 90.0,
                    angle_deg: angle,
                    base_angle_deg: base_angle,
                    orbit_count: tier.indices.len() as u32,
                    block_type: block,
                    is_pinned: if self.space.is_free(index) { 0.0 } else { 1.0 },
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
                weight_fractions: weight_fractions(&self.settings.weights),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::ObjectivePreset;

    fn fixture_settings() -> CoreSettings {
        CoreSettings {
            max_steps: 60,
            max_step_deg: 0.5,
            weights: ObjectiveWeights::default(),
            fidelity: ObjectiveFidelity::Fast,
            terminal_score: DEFAULT_TERMINAL_SCORE,
            min_girdle_ratio: DEFAULT_MIN_GIRDLE_RATIO,
            vary_anchored: true,
        }
    }

    #[test]
    fn source_material_reads_the_design_refractive_index() {
        let material = design_source_material(&Design::concave_fixture());
        assert!((material.n_d - 1.54).abs() < 1e-9, "{}", material.n_d);
        assert!((material.critical_angle_deg - critical_angle_deg(1.54)).abs() < 1e-9);
    }

    #[test]
    fn source_material_of_a_design_without_a_material_takes_the_i_line_index() {
        // Every `.asc` import carries `MaterialSelection::none()`: the index comes from the
        // design metadata, as a flat (dispersion-free) material.
        let mut design = Design::concave_fixture();
        design.material = MaterialSelection::none();
        let material = design_source_material(&design);
        assert!((material.n_d - 1.54).abs() < 1e-9, "{}", material.n_d);
        assert!((material.critical_angle_deg - critical_angle_deg(1.54)).abs() < 1e-9);
        let at_d = f64::from(material.gem.dispersion.evaluate(589.3));
        assert!((at_d - 1.54).abs() < 1e-4, "{at_d}");
        let spread =
            material.gem.dispersion.evaluate(486.1) - material.gem.dispersion.evaluate(656.3);
        assert!(
            spread.abs() < 1e-6,
            "flat index has no dispersion: {spread}"
        );
    }

    #[test]
    fn a_dead_stone_is_refused() {
        let components = |tilt_brilliance_pct| ObjectiveComponents {
            windowing_pct: 30.0,
            extinction_pct: 40.0,
            tilt_brilliance_pct,
        };
        // Brilliance exactly 0 (the `shah` stone of the review: every move scores 0.00) is
        // dead; a tenth of a percent is not.
        assert!(returns_no_light(&components(0.0)));
        assert!(!returns_no_light(&components(0.1)));
        assert!(!returns_no_light(&components(70.0)));
        assert_eq!(DEFAULT_TERMINAL_SCORE, 0.0);
    }

    #[test]
    fn the_fixture_returns_light_so_the_core_accepts_it() {
        let design = Design::concave_fixture();
        let material = design_source_material(&design);
        let core = EnvCore::new(
            design,
            material.gem,
            fixture_settings(),
            RewardConfig::default(),
        )
        .expect("the fixture returns light face-up");
        assert!(core.base_components().tilt_brilliance_pct > 0.0);
        assert_eq!(core.weights(), &ObjectiveWeights::default());
    }

    #[test]
    fn girdle_gate_rules() {
        assert!(girdle_ok(None, None, 0.5));
        assert!(!girdle_ok(Some(1.0), None, 0.5));
        assert!(girdle_ok(Some(1.0), Some(0.6), 0.5));
        assert!(!girdle_ok(Some(1.0), Some(0.4), 0.5));
        assert!(!girdle_ok(Some(1.0), Some(0.0), 0.0));
    }

    #[test]
    fn a_tone_weight_is_refused_at_construction() {
        let design = Design::concave_fixture();
        let material = design_source_material(&design);
        for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
            let settings = CoreSettings {
                weights: preset.weights(),
                ..fixture_settings()
            };
            let refused = EnvCore::new(
                design.clone(),
                material.gem.clone(),
                settings,
                RewardConfig::default(),
            );
            assert!(
                matches!(
                    refused,
                    Err(EnvError::InvalidConfig(text)) if text.contains("tone")
                ),
                "{preset:?}"
            );
        }
        // Every other preset builds.
        let settings = CoreSettings {
            weights: ObjectivePreset::KeepWeight.weights(),
            ..fixture_settings()
        };
        assert!(
            EnvCore::new(design, material.gem, settings, RewardConfig::default()).is_ok(),
            "a yield weight is supported"
        );
    }

    #[test]
    fn the_score_carries_the_yield_only_when_it_is_weighted_and_measurable() {
        let design = Design::concave_fixture();
        let gem = design_source_material(&design).gem;
        let volume = preform_volume(&design);
        assert!(volume.is_some(), "the fixture's block preform measures");
        let evaluate = |weights: &ObjectiveWeights, volume| {
            evaluate_design(&design, &gem, weights, ObjectiveFidelity::Fast, volume)
                .expect("the fixture scores")
        };

        // Weight zero: the optical score, bit for bit, whatever the yield is.
        let optical_weights = ObjectiveWeights::default();
        let optical = evaluate(&optical_weights, volume);
        assert_eq!(optical.score, optical_weights.score(&optical.components));
        assert!(optical.yield_pct.is_some());

        // A positive weight and a measurable yield: the yield loss joins the score.
        let weights = ObjectivePreset::KeepWeight.weights();
        let weighted = evaluate(&weights, volume);
        let yield_pct = weighted.yield_pct.expect("measurable");
        assert!(yield_pct > 0.0 && yield_pct.is_finite());
        assert_eq!(
            weighted.score,
            weights.score_with_yield(&weighted.components, 100.0 - yield_pct)
        );
        assert_ne!(weighted.score, weights.score(&weighted.components));
        // The optical measurements do not depend on the weights.
        assert_eq!(weighted.components, optical.components);

        // No preform volume: the yield cannot be measured and the optics score alone.
        let blind = evaluate(&weights, None);
        assert_eq!(blind.yield_pct, None);
        assert_eq!(blind.score, weights.score(&blind.components));
    }

    #[test]
    fn objective_score_falls_back_to_the_optics() {
        let components = ObjectiveComponents {
            windowing_pct: 20.0,
            extinction_pct: 30.0,
            tilt_brilliance_pct: 70.0,
        };
        let weights = ObjectivePreset::KeepWeight.weights();
        assert_eq!(
            objective_score(&weights, &components, None),
            weights.score(&components)
        );
        assert_eq!(
            objective_score(&weights, &components, Some(40.0)),
            weights.score_with_yield(&components, 60.0)
        );
        let plain = ObjectiveWeights::default();
        assert_eq!(
            objective_score(&plain, &components, Some(40.0)),
            plain.score(&components)
        );
    }
}
