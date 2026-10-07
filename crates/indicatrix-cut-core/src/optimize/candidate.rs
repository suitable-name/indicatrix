//! Per-candidate evaluation: free-tier selection, the closes/manufacturability
//! gates a candidate must pass before it is even scored, and evaluating a
//! tier's `+step`/`-step` pair. See the parent module's doc comment ("Never
//! proposes geometry that fails to close", "Manufacturability must not
//! regress", "Where the compute runs") for why these gates and this
//! parallelism shape exist.

use super::{
    guard::ShapeGuard,
    objective::{
        ObjectiveComponents, ObjectiveFidelity, ObjectiveWeights, evaluate_objective_under,
        evaluate_objective_with_tone_under, to_gpu_planes,
    },
    options::{OptimizeOptions, ShapeTarget},
    space::SearchSpace,
};
use crate::{
    design::Design,
    manufacturability::{
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, ManufacturabilityWarning, check_manufacturability,
    },
};
use glam::DVec3;
use indicatrix::{
    color::metrics::FaceUpTone,
    geometry::{
        meet_solver::MeetConstraint,
        stone_metrics::{
            SolidMesh, SolidMetrics, SolidStatus, StoneProportions, build_solid_mesh, measure_solid,
        },
    },
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};

/// A candidate's yield loss (`0.0` to `100.0`, LOWER is better), for
/// [`ObjectiveWeights::score_with_yield`] -- `100.0` minus
/// [`crate::yield_metrics::volumetric_yield`]'s own percentage. `0.0` (no penalty
/// at all) whenever either the finished or the preform solid fails to measure,
/// matching `volumetric_yield`'s own `None` case -- a candidate that already
/// failed to close was rejected before this is ever called (see
/// [`evaluate_candidate`]), so `finished` here always measures in practice; the
/// fallback exists only so this can never itself become a reason to reject a
/// candidate that DID close.
///
/// Takes the finished solid already measured (`measure_solid` of the finished planes), so a
/// caller that also needs the stone's shape figures measures once.
pub(super) fn yield_loss_pct(design: &Design, finished: Option<&SolidMetrics>) -> f32 {
    let preform = measure_solid(&design.preform.planes());
    let yield_fraction = match (finished, &preform) {
        (Some(f), Some(p)) => crate::yield_metrics::volumetric_yield(f.volume, p.volume),
        _ => None,
    };
    yield_fraction.map_or(0.0, |y| 100.0_f64.mul_add(-y, 100.0) as f32)
}

/// A stone's table percent and crown-height-to-pavilion-depth ratio, the two figures a
/// [`ShapeTarget`] holds a search to. The ratio is `None` unless both heights are known
/// and the depth is positive.
pub(super) fn shape_figures(
    metrics: &SolidMetrics,
    mesh: &SolidMesh,
    planes: &[(DVec3, f64)],
) -> (Option<f64>, Option<f64>) {
    let proportions = StoneProportions::from_solid(metrics, mesh, planes);
    let ratio = match (proportions.crown_height, proportions.pavilion_depth) {
        (Some(crown), Some(depth)) if depth > 0.0 => Some(crown / depth),
        _ => None,
    };
    (proportions.table_percent, ratio)
}

/// The score penalty of a stone whose solid is `metrics` and `mesh`: `0.0` without a
/// target.
pub(super) fn shape_penalty(
    target: Option<ShapeTarget>,
    metrics: Option<&SolidMetrics>,
    mesh: &SolidMesh,
    planes: &[(DVec3, f64)],
) -> f32 {
    match (target, metrics) {
        (Some(target), Some(metrics)) => {
            let (table, ratio) = shape_figures(metrics, mesh, planes);
            target.penalty(table, ratio)
        }
        _ => 0.0,
    }
}

/// [`shape_penalty`] for a caller without the mesh: builds it, and only when there is a
/// target to read it for. `0.0` when the planes do not close.
pub(super) fn shape_penalty_for_planes(
    target: Option<ShapeTarget>,
    metrics: Option<&SolidMetrics>,
    planes: &[(DVec3, f64)],
) -> f32 {
    if target.is_none() {
        return 0.0;
    }
    match build_solid_mesh(planes) {
        SolidStatus::Closed(mesh) => shape_penalty(target, metrics, &mesh, planes),
        _ => 0.0,
    }
}

/// A finished stone measured the way the optimizer reports its results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FinishedScore {
    /// The optical measurements at the requested fidelity.
    pub after: ObjectiveComponents,
    /// [`ObjectiveWeights::score_with_yield`] of `after` and `yield_loss_pct`; lower is better.
    pub score: f32,
    /// The stone's yield loss, `0.0` to `100.0`, lower is better.
    pub yield_loss_pct: f32,
    /// The face-up tone at the table-up pose: always `Some` at
    /// [`ObjectiveFidelity::Full`], at `Fast` only when the weights carry a tone term.
    pub tone: Option<FaceUpTone>,
}

/// The objective components of a finished stone and, when wanted, its face-up tone with
/// the tone loss for `weights`' goal (`0.0` when the tone is not measured).
///
/// The tone is measured when `fidelity` is `Full` (always reported) or the weights carry a
/// tone term; without either it costs nothing and the components are exactly
/// [`evaluate_objective_under`]'s.
pub(super) fn measure_with_tone(
    planes: &[indicatrix::geometry::GpuFacetPlane],
    material: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
    lighting: LightingPreset,
) -> (ObjectiveComponents, Option<FaceUpTone>, f32) {
    let weighted = weights.tone_weight > 0.0;
    if weighted || fidelity == ObjectiveFidelity::Full {
        let (components, tone) =
            evaluate_objective_with_tone_under(planes, material, fidelity, lighting);
        let loss = if weighted {
            weights.tone_goal.loss_pct(&tone)
        } else {
            0.0
        };
        (components, Some(tone), loss)
    } else {
        (
            evaluate_objective_under(planes, material, fidelity, lighting),
            None,
            0.0,
        )
    }
}

/// Scores a design whose planes the caller already has, exactly as the optimizer scores
/// the end points it reports (`fidelity` [`ObjectiveFidelity::Full`] for those).
///
/// For a caller that changes a design after the search, for example by re-anchoring tiers
/// that follow a relation, and wants numbers that describe the design it actually delivers.
/// `planes` are the design's own ([`Design::planes_from_solved`]); `weights` are the ones
/// the search ran with, so the score is comparable with the search's. With a
/// `shape_target` (the one the search ran with) the score carries the same penalty for
/// drifting from the target's table size and crown-to-pavilion ratio.
#[must_use]
pub fn score_finished_design(
    design: &Design,
    planes: &[(DVec3, f64)],
    material: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
    lighting: LightingPreset,
    shape_target: Option<ShapeTarget>,
) -> FinishedScore {
    let (after, tone, tone_loss) = measure_with_tone(
        &to_gpu_planes(planes),
        material,
        weights,
        fidelity,
        lighting,
    );
    let metrics = measure_solid(planes);
    let loss = yield_loss_pct(design, metrics.as_ref());
    let mut score = weights.score_with_tone(&after, loss, tone_loss);
    if shape_target.is_some() {
        score += shape_penalty_for_planes(shape_target, metrics.as_ref(), planes);
    }
    FinishedScore {
        after,
        score,
        yield_loss_pct: loss,
        tone,
    }
}

/// Every tier index this optimizer may move under the default [`OptimizeOptions`].
///
/// A free tier is not a [`MeetConstraint::ScaleReference`] (see the module doc
/// comment's "Which tiers are free" section), not horizontal and not vertical.
/// [`free_tier_indices_with`] is the same question for a request that also varies
/// anchored tiers.
///
/// **Horizontal tiers** (the table at `+0.0`, the culet at `-0.0`) are never free: a
/// horizontal facet has no angle to optimize, and a step off zero would move the
/// facet to a side of the girdle it was not authored on.
///
/// **Vertical tiers** matter more than they look. A tier authored at or beyond
/// [`MAX_SAFE_CANDIDATE_ANGLE_DEG`] -- a real girdle facet at `-90.0`, which most
/// `.asc` files carry -- is one [`candidate_angle_is_safe`] can never accept any
/// angle for, in either direction. [`build_free_angle_candidate`] is all-or-nothing
/// across the whole angle vector, so leaving such a tier in `free` made it reject
/// every simplex point [`super::polish::run_polish`] proposes, silently disabling
/// the polish stage for the whole design rather than just for that one facet. A
/// vertical facet is the girdle by definition, so there was never anything to
/// optimize there.
#[must_use]
pub fn free_tier_indices(design: &Design) -> Vec<usize> {
    free_tier_indices_with(design, &OptimizeOptions::default())
}

/// [`free_tier_indices`] for a request that may also vary anchored tiers.
///
/// With [`OptimizeOptions::vary_anchored`] on, a `ScaleReference` tier that has a hinge
/// in [`OptimizeOptions::anchor_hinges`] (and no tier target) is free too. The angle
/// rule is the same for every tier: horizontal and vertical tiers never move.
///
/// A tier whose angle follows a relation ([`Design::is_tier_driven`]) is never free,
/// whatever its constraint: its angle is the result of other tiers' angles, so the search
/// moves the tiers it reads and the relation moves it.
#[must_use]
pub fn free_tier_indices_with(design: &Design, options: &OptimizeOptions) -> Vec<usize> {
    design
        .tiers
        .iter()
        .enumerate()
        .filter(|(index, tier)| {
            angle_is_variable(tier.angle_deg)
                && !design.is_tier_driven(*index)
                && (!matches!(tier.constraint, MeetConstraint::ScaleReference(_))
                    || options.anchored_hinge(design, *index).is_some())
        })
        .map(|(i, _)| i)
        .collect()
}

/// Facet angles at or beyond this magnitude are close enough to vertical to be unsafe.
///
/// `classify_blocks` (which classifies a tier as crown/pavilion/girdle from the
/// magnitude of `angle_deg.cos()`, not just its sign) would reclassify the tier
/// into [`indicatrix::geometry::meet_solver::Block::Girdle`], silently losing
/// whatever crown/pavilion anchor it had -- the same class of anchor loss
/// [`candidate_angle_is_safe`]'s zero-crossing check guards against on the
/// opposite boundary. `89.5` leaves the same half-degree margin there.
pub const MAX_SAFE_CANDIDATE_ANGLE_DEG: f64 = 89.5;

/// Angles closer to zero than this are horizontal (the table, the culet): never free.
pub(super) const MIN_VARIABLE_ANGLE_DEG: f64 = 1e-9;

/// Whether a tier authored at `angle_deg` may be moved at all: neither horizontal
/// (`|angle| < 1e-9`) nor vertical (`|angle| >= 89.5`). A non-finite angle may not.
///
/// Public so a front end can list the tiers a run could move (an angle-range table)
/// without repeating the rule.
#[must_use]
pub fn angle_is_variable(angle_deg: f64) -> bool {
    (MIN_VARIABLE_ANGLE_DEG..MAX_SAFE_CANDIDATE_ANGLE_DEG).contains(&angle_deg.abs())
}

/// Whether nudging a tier's angle from `original_deg` to `candidate_deg` is safe to
/// even attempt solving. Unsafe in any of three ways: the candidate is not finite or
/// its magnitude passes [`MAX_SAFE_CANDIDATE_ANGLE_DEG`], the candidate lands exactly
/// on the girdle plane (`0.0`, either sign), or its sign differs from the original's.
///
/// `Block` classifies a tier as crown/pavilion purely from the SIGN BIT of
/// `angle_deg` at that boundary (`f64::is_sign_negative`, exactly like
/// `indicatrix::geometry::meet_solver::blocks`'s own girdle classifier), so crossing
/// (or landing on) it can silently remove a block's only scale anchor or reassign a
/// facet to the wrong block. That holds for EVERY tier, a table authored at `+0.0`
/// included: it stays on the crown side (a candidate must be positive), and a culet
/// authored at `-0.0` stays on the pavilion side. (Horizontal tiers are never free,
/// see [`free_tier_indices`], so this matters only to a caller that asks anyway.)
#[must_use]
pub(super) fn candidate_angle_is_safe(original_deg: f64, candidate_deg: f64) -> bool {
    if !candidate_deg.is_finite() || candidate_deg.abs() > MAX_SAFE_CANDIDATE_ANGLE_DEG {
        return false;
    }
    candidate_deg != 0.0 && candidate_deg.is_sign_negative() == original_deg.is_sign_negative()
}

/// One candidate's fully-evaluated outcome: either rejected outright (unsolvable,
/// non-closed, a manufacturability regression, or a lost girdle), or accepted with
/// its [`ObjectiveFidelity::Fast`] score. Deliberately does not carry the candidate's
/// solved masts: the search always re-solves the next candidate from scratch (see
/// the module doc comment's "Cost first" section), so there is no "previous" this
/// outcome would ever be threaded into.
pub(super) enum CandidateOutcome {
    Rejected,
    Accepted { score: f32 },
}

/// Solves `design`, checks closure, the shape guard and manufacturability against
/// `baseline_warnings`, and (only if all pass) scores it at
/// [`ObjectiveFidelity::Fast`] under `lighting` -- the one function
/// [`super::optimize_design`]'s search loop calls for every candidate.
pub(super) fn evaluate_candidate(
    design: &Design,
    material: &GemMaterial,
    weights: &ObjectiveWeights,
    baseline_warnings: &BaselineWarningCounts,
    lighting: LightingPreset,
    guard: Option<&ShapeGuard>,
    shape_target: Option<ShapeTarget>,
) -> CandidateOutcome {
    let Ok(solved) = design.solve() else {
        return CandidateOutcome::Rejected;
    };
    let planes = design.planes_from_solved(&solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return CandidateOutcome::Rejected;
    };
    if let Some(guard) = guard
        && !guard.admits(design, &solved, &planes, &mesh)
    {
        return CandidateOutcome::Rejected;
    }
    let warnings = check_manufacturability(design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
    if manufacturability_regressed(baseline_warnings, &warnings) {
        return CandidateOutcome::Rejected;
    }
    let gpu_planes = to_gpu_planes(&planes);
    let (components, _, tone_loss) = measure_with_tone(
        &gpu_planes,
        material,
        weights,
        ObjectiveFidelity::Fast,
        lighting,
    );
    let metrics = measure_solid(&planes);
    let mut score = weights.score_with_tone(
        &components,
        yield_loss_pct(design, metrics.as_ref()),
        tone_loss,
    );
    if shape_target.is_some() {
        score += shape_penalty(shape_target, metrics.as_ref(), &mesh, &planes);
    }
    CandidateOutcome::Accepted { score }
}

/// [`evaluate_candidate`] with every fixed input taken from `ctx`.
pub(super) fn evaluate_in_context(design: &Design, ctx: &SearchContext) -> CandidateOutcome {
    evaluate_candidate(
        design,
        ctx.material,
        ctx.weights,
        ctx.baseline_warnings,
        ctx.lighting,
        ctx.guard,
        ctx.shape_target,
    )
}

/// Builds a candidate [`Design`] from `base` with every tier index in `free` set to
/// the correspondingly-positioned angle in `angles` (an anchored tier's mast follows
/// its hinge, see [`SearchSpace::write_angle`]). `None` if some tier has no usable
/// mast. No safety or bounds check: the caller vouches for `angles`.
#[must_use]
pub(super) fn apply_free_angles(
    base: &Design,
    free: &[usize],
    angles: &[f64],
    space: &SearchSpace,
) -> Option<Design> {
    let mut candidate = base.clone();
    for (&tier_index, &angle) in free.iter().zip(angles) {
        if !space.write_angle(&mut candidate, tier_index, angle) {
            return None;
        }
    }
    Some(candidate)
}

/// Builds a candidate [`Design`] from `base` with every tier index in `free` set to
/// the correspondingly-positioned angle in `angles` -- [`super::polish::run_polish`]'s
/// only way to turn one simplex vertex (a plain angle vector, positionally paired
/// with `free`) into something [`evaluate_candidate`] can score.
///
/// Every candidate angle is checked against [`candidate_angle_is_safe`] relative to
/// its own entry in `reference` (the angle vector the polish stage started from, not
/// necessarily `base`'s own current angle for that tier) and against the tier's angle
/// bounds BEFORE `base` is ever cloned -- `None` if any one of them is unsafe or out
/// of bounds, matching the parent module's "Angle bounds" requirement that an
/// out-of-bounds simplex point is rejected outright, never clamped.
#[must_use]
pub(super) fn build_free_angle_candidate(
    base: &Design,
    free: &[usize],
    reference: &[f64],
    angles: &[f64],
    space: &SearchSpace,
) -> Option<Design> {
    let all_safe = angles.iter().zip(reference).zip(free).all(
        |((&candidate_deg, &reference_deg), &tier_index)| {
            candidate_angle_is_safe(reference_deg, candidate_deg)
                && space.contains(tier_index, candidate_deg)
        },
    );
    if !all_safe {
        return None;
    }
    apply_free_angles(base, free, angles, space)
}

/// Everything [`evaluate_candidate_pair`] needs that stays fixed across the whole
/// search, bundled purely to keep that function's (and [`super::optimize_design`]'s)
/// argument count reasonable.
pub(super) struct SearchContext<'a> {
    pub(super) material: &'a GemMaterial,
    pub(super) weights: &'a ObjectiveWeights,
    pub(super) baseline_warnings: &'a BaselineWarningCounts,
    /// The lighting preset every candidate is scored under.
    pub(super) lighting: LightingPreset,
    /// The angle bounds and hinges of this run.
    pub(super) space: &'a SearchSpace,
    /// The girdle/table guard, when the request asked for one.
    pub(super) guard: Option<&'a ShapeGuard>,
    /// The table size and ratio every score is penalised for drifting from, when the
    /// request asked for it.
    pub(super) shape_target: Option<ShapeTarget>,
}

/// Builds the surviving `original_deg + step_deg` / `original_deg - step_deg`
/// candidate [`Design`]s -- exactly the directions [`candidate_angle_is_safe`]
/// allows -- paired with the absolute angle each represents. Split out of
/// [`evaluate_survivors`] so that "which directions are even worth solving" (cheap:
/// a clone and a field write, no [`Design::solve`]) is a separate, directly
/// testable step from "solve the survivors" -- letting
/// [`evaluate_candidate_pair`] skip `std::thread::scope` entirely when nothing
/// survives. Order is always `[+step_deg, -step_deg]` with rejected directions
/// simply absent.
///
/// A tier with angle bounds never proposes outside them: a step that overshoots is
/// clamped to the bound, and dropped when the clamp leaves the tier where it is. If
/// both directions clamp to the same angle only the first is kept.
pub(super) fn select_candidate_directions(
    current: &Design,
    tier_index: usize,
    original_deg: f64,
    step_deg: f64,
    space: &SearchSpace,
) -> Vec<(f64, Design)> {
    let mut survivors: Vec<(f64, Design)> = Vec::with_capacity(2);
    for delta in [step_deg, -step_deg] {
        let proposed_deg = original_deg + delta;
        let candidate_deg = space.clamp(tier_index, proposed_deg);
        let clamped_in_place = candidate_deg.to_bits() != proposed_deg.to_bits()
            && candidate_deg.to_bits() == original_deg.to_bits();
        if clamped_in_place
            || !candidate_angle_is_safe(original_deg, candidate_deg)
            || survivors.iter().any(|(deg, _)| *deg == candidate_deg)
        {
            continue;
        }
        let mut candidate = current.clone();
        if space.write_angle(&mut candidate, tier_index, candidate_deg) {
            survivors.push((candidate_deg, candidate));
        }
    }
    survivors
}

/// Evaluates both surviving candidates from [`select_candidate_directions`]'s
/// two-element case, on two OS threads via `std::thread::scope` natively.
///
/// `wasm32-unknown-unknown` has no OS thread to spawn (`std::thread::Scope::spawn`
/// panics there), so that target's build gets a separate, sequential definition of
/// this function below, evaluating `design_a` then `design_b` in the same order --
/// each candidate is a pure function of its own `Design`, so the result is identical
/// either way. Two `cfg`-gated definitions, like `renderer::tonemap`'s
/// `effective_thread_count`, rather than one function with an internal `#[cfg]`
/// block on the body.
#[cfg(not(target_arch = "wasm32"))]
fn evaluate_two_survivors(
    design_a: &Design,
    design_b: &Design,
    ctx: &SearchContext,
) -> (CandidateOutcome, CandidateOutcome) {
    std::thread::scope(|scope| {
        let handle_a = scope.spawn(|| evaluate_in_context(design_a, ctx));
        let handle_b = scope.spawn(|| evaluate_in_context(design_b, ctx));
        (
            handle_a
                .join()
                .expect("candidate evaluation thread must not panic"),
            handle_b
                .join()
                .expect("candidate evaluation thread must not panic"),
        )
    })
}

/// The `wasm32` arm of [`evaluate_two_survivors`] -- see that function's doc comment.
#[cfg(target_arch = "wasm32")]
fn evaluate_two_survivors(
    design_a: &Design,
    design_b: &Design,
    ctx: &SearchContext,
) -> (CandidateOutcome, CandidateOutcome) {
    let outcome_a = evaluate_in_context(design_a, ctx);
    let outcome_b = evaluate_in_context(design_b, ctx);
    (outcome_a, outcome_b)
}

/// What [`evaluate_survivors`] (and so [`evaluate_candidate_pair`]) found.
pub(super) struct SurvivorEvaluation {
    /// `Some((candidate_deg, score))` for the best accepted direction that also beats
    /// the score it was compared against; `None` if no direction does.
    pub(super) best: Option<(f64, f32)>,
    /// How many directions were actually solved (0, 1 or 2).
    pub(super) evaluations: usize,
    /// Every accepted direction with its score, whether or not it beat the comparison
    /// score -- what a run that keeps several candidates feeds its pool.
    pub(super) accepted: Vec<(f64, f32)>,
}

/// Evaluates every surviving `(candidate_deg, Design)` from
/// [`select_candidate_directions`] and reduces them to the [`SurvivorEvaluation`]
/// [`evaluate_candidate_pair`] documents. Threading is
/// shaped by how many survivors there are: two are solved concurrently via
/// `std::thread::scope` natively (`wasm32` solves them sequentially instead, in the
/// same `[a, b]` order -- see [`evaluate_two_survivors`]); exactly one is solved
/// inline, no scope, no spawn; zero (both directions rejected before either was
/// solved) returns an empty evaluation immediately, starting no threads at all.
pub(super) fn evaluate_survivors(
    survivors: Vec<(f64, Design)>,
    ctx: &SearchContext,
    current_score: f32,
) -> SurvivorEvaluation {
    let outcomes: Vec<(f64, CandidateOutcome)> =
        if let [(deg_a, design_a), (deg_b, design_b)] = survivors.as_slice() {
            let (outcome_a, outcome_b) = evaluate_two_survivors(design_a, design_b, ctx);
            vec![(*deg_a, outcome_a), (*deg_b, outcome_b)]
        } else {
            survivors
                .into_iter()
                .map(|(deg, design)| (deg, evaluate_in_context(&design, ctx)))
                .collect()
        };

    let mut evaluations = 0usize;
    let mut best: Option<(f64, f32)> = None;
    let mut accepted = Vec::new();
    for (candidate_deg, outcome) in outcomes {
        evaluations += 1;
        if let CandidateOutcome::Accepted { score } = outcome {
            accepted.push((candidate_deg, score));
            if score < current_score && best.is_none_or(|(_, best_score)| score < best_score) {
                best = Some((candidate_deg, score));
            }
        }
    }
    SurvivorEvaluation {
        best,
        evaluations,
        accepted,
    }
}

/// Evaluates a free tier's two candidate directions (`original_deg + step_deg`,
/// `original_deg - step_deg`) -- see [`select_candidate_directions`] for how the
/// unsafe ones are filtered out and [`evaluate_survivors`] for how the survivors
/// are run (concurrently on two OS threads only when both survive, natively -- see
/// the module doc comment's "Where the compute runs" section; sequentially, same
/// order, on `wasm32` -- see [`evaluate_two_survivors`]). `std::thread::scope`
/// guarantees both spawned threads finish before this function returns, so there
/// is no lifetime hazard in borrowing `current`/`ctx` from the calling thread's
/// stack.
///
/// The returned [`SurvivorEvaluation::best`] is `Some((candidate_deg, score))` for
/// whichever direction is [`CandidateOutcome::Accepted`] AND scores below
/// `current_score`, preferring the lower score when both qualify; `None` if neither
/// does. [`SurvivorEvaluation::evaluations`] counts every direction actually solved
/// (0, 1, or 2) -- a direction rejected by [`candidate_angle_is_safe`] before ever
/// calling [`Design::solve`] is not counted.
pub(super) fn evaluate_candidate_pair(
    current: &Design,
    tier_index: usize,
    original_deg: f64,
    step_deg: f64,
    ctx: &SearchContext,
    current_score: f32,
) -> SurvivorEvaluation {
    let survivors =
        select_candidate_directions(current, tier_index, original_deg, step_deg, ctx.space);
    evaluate_survivors(survivors, ctx, current_score)
}

/// Running counts of the two mesh-dependent [`ManufacturabilityWarning`] variants an
/// angle-only edit can actually change. `FractionalIndex`/`OutOfOrderMeet`/
/// `MeetNameNotAscSafe` are excluded: they depend on `indices`/`constraint`, which
/// this module never touches (it only ever rewrites a free tier's `angle_deg`, and
/// an anchored tier's mast), so their counts are invariant across every candidate by
/// construction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct BaselineWarningCounts {
    vanishing: usize,
    undersized: usize,
}

impl BaselineWarningCounts {
    pub(super) fn count(warnings: &[ManufacturabilityWarning]) -> Self {
        let mut counts = Self::default();
        for warning in warnings {
            match warning {
                ManufacturabilityWarning::VanishingFacet { .. } => counts.vanishing += 1,
                ManufacturabilityWarning::UndersizedFacet { .. } => counts.undersized += 1,
                ManufacturabilityWarning::FractionalIndex { .. }
                | ManufacturabilityWarning::OutOfOrderMeet { .. }
                | ManufacturabilityWarning::MeetNameNotAscSafe { .. }
                | ManufacturabilityWarning::ConcaveTiersOmittedFromExport { .. }
                | ManufacturabilityWarning::ToolMissesStone { .. }
                | ManufacturabilityWarning::ToolBreaksThrough { .. }
                | ManufacturabilityWarning::ToolRemovesMeet { .. }
                | ManufacturabilityWarning::ToolsOverlap { .. }
                | ManufacturabilityWarning::ConcaveSliver { .. }
                | ManufacturabilityWarning::ToolRemovesHullVertex { .. }
                | ManufacturabilityWarning::ToolEnclosed { .. } => {}
            }
        }
        counts
    }
}

/// `true` iff `candidate` has strictly more vanishing OR undersized facets than
/// `baseline` -- the "must not regress" gate [`evaluate_candidate`] applies to every
/// candidate. Equal or fewer warnings than baseline is accepted (an optimizer that
/// only ever matches the starting design's manufacturability is not a regression, even
/// if it does not improve it either).
fn manufacturability_regressed(
    baseline: &BaselineWarningCounts,
    candidate_warnings: &[ManufacturabilityWarning],
) -> bool {
    let candidate = BaselineWarningCounts::count(candidate_warnings);
    candidate.vanishing > baseline.vanishing || candidate.undersized > baseline.undersized
}
