//! Per-candidate evaluation: free-tier selection, the closes/manufacturability
//! gates a candidate must pass before it is even scored, and evaluating a
//! tier's `+step`/`-step` pair. See the parent module's doc comment ("Never
//! proposes geometry that fails to close", "Manufacturability must not
//! regress", "Where the compute runs") for why these gates and this
//! parallelism shape exist.

use super::objective::{ObjectiveFidelity, ObjectiveWeights, evaluate_objective, to_gpu_planes};
use crate::{
    design::Design,
    manufacturability::{
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, ManufacturabilityWarning, check_manufacturability,
    },
};
use glam::DVec3;
use indicatrix::{
    geometry::{
        meet_solver::MeetConstraint,
        stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
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
pub(super) fn yield_loss_pct(design: &Design, finished_planes: &[(DVec3, f64)]) -> f32 {
    let finished = measure_solid(finished_planes);
    let preform = measure_solid(&design.preform.planes());
    let yield_fraction = match (&finished, &preform) {
        (Some(f), Some(p)) => crate::yield_metrics::volumetric_yield(f.volume, p.volume),
        _ => None,
    };
    yield_fraction.map_or(0.0, |y| 100.0_f64.mul_add(-y, 100.0) as f32)
}

/// Every tier index this optimizer is allowed to move: not a
/// [`MeetConstraint::ScaleReference`] (see the module doc comment's "Which tiers are
/// free" section) and not already vertical.
///
/// The vertical exclusion matters more than it looks. A tier authored at or beyond
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
    design
        .tiers
        .iter()
        .enumerate()
        .filter(|(_, tier)| {
            !matches!(tier.constraint, MeetConstraint::ScaleReference(_))
                && tier.angle_deg.abs() <= MAX_SAFE_CANDIDATE_ANGLE_DEG
        })
        .map(|(i, _)| i)
        .collect()
}

/// Facet angles at or beyond this magnitude are close enough to vertical that
/// `classify_blocks` (which classifies a tier as crown/pavilion/girdle from the
/// magnitude of `angle_deg.cos()`, not just its sign) would reclassify the tier
/// into [`indicatrix::geometry::meet_solver::Block::Girdle`], silently losing
/// whatever crown/pavilion anchor it had -- the same class of anchor loss
/// [`candidate_angle_is_safe`]'s zero-crossing check guards against on the
/// opposite boundary. `89.5` leaves the same half-degree margin there.
pub(super) const MAX_SAFE_CANDIDATE_ANGLE_DEG: f64 = 89.5;

/// Whether nudging tier `index`'s angle from `original_deg` to `candidate_deg` is
/// safe to even attempt solving. Unsafe in any of three ways: the candidate's
/// magnitude reaches or passes [`MAX_SAFE_CANDIDATE_ANGLE_DEG`], the candidate
/// lands exactly on the girdle plane (`0.0`, either sign), or it otherwise crosses
/// that plane while the original angle did not start there -- `Block` classifies a
/// tier as crown/pavilion purely from the SIGN BIT of `angle_deg` at that boundary
/// (`f64::is_sign_negative`, exactly like
/// `indicatrix::geometry::meet_solver::blocks`'s own girdle classifier), and
/// crossing (or landing on) it can silently remove a block's only scale anchor or
/// reassign a facet to the wrong block. A tier legitimately authored AT `+0.0` (a
/// table facet, with no side yet) is left free to move to either side once -- but
/// `-0.0` is not `+0.0` here: this crate's own templates author a pavilion culet at
/// `-0.0` specifically to record its side (see
/// `crate::design::ConstraintTier::standard_round_brilliant`), so only a truly
/// side-less `+0.0` origin gets the either-direction exemption; a candidate must
/// never land exactly on `0.0` regardless of sign, since `signum`/`==` treat `-0.0`
/// and landing-on-zero inconsistently -- exactly the failure mode this check
/// exists to prevent.
#[must_use]
pub(super) fn candidate_angle_is_safe(original_deg: f64, candidate_deg: f64) -> bool {
    if candidate_deg.abs() > MAX_SAFE_CANDIDATE_ANGLE_DEG {
        return false;
    }
    if original_deg == 0.0 && !original_deg.is_sign_negative() {
        return true;
    }
    candidate_deg != 0.0 && candidate_deg.is_sign_negative() == original_deg.is_sign_negative()
}

/// One candidate's fully-evaluated outcome: either rejected outright (unsolvable,
/// non-closed, or a manufacturability regression), or accepted with its
/// [`ObjectiveFidelity::Fast`] score. Deliberately does not carry the candidate's
/// solved masts: the search always re-solves the next candidate from scratch (see
/// the module doc comment's "Cost first" section), so there is no "previous" this
/// outcome would ever be threaded into.
pub(super) enum CandidateOutcome {
    Rejected,
    Accepted { score: f32 },
}

/// Solves `design`, checks closure and manufacturability against `baseline_warnings`,
/// and (only if both pass) scores it at [`ObjectiveFidelity::Fast`] -- the one
/// function [`super::optimize_design`]'s search loop calls for every candidate.
pub(super) fn evaluate_candidate(
    design: &Design,
    material: &GemMaterial,
    weights: &ObjectiveWeights,
    baseline_warnings: &BaselineWarningCounts,
) -> CandidateOutcome {
    let Ok(solved) = design.solve() else {
        return CandidateOutcome::Rejected;
    };
    let planes = design.planes_from_solved(&solved);
    if !matches!(build_solid_mesh(&planes), SolidStatus::Closed(_)) {
        return CandidateOutcome::Rejected;
    }
    let warnings = check_manufacturability(design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
    if manufacturability_regressed(baseline_warnings, &warnings) {
        return CandidateOutcome::Rejected;
    }
    let gpu_planes = to_gpu_planes(&planes);
    let components = evaluate_objective(&gpu_planes, material, ObjectiveFidelity::Fast);
    let score = weights.score_with_yield(&components, yield_loss_pct(design, &planes));
    CandidateOutcome::Accepted { score }
}

/// Builds a candidate [`Design`] from `base` with every tier index in `free` set to
/// the correspondingly-positioned angle in `angles` -- [`super::polish::run_polish`]'s
/// only way to turn one simplex vertex (a plain angle vector, positionally paired
/// with `free`) into something [`evaluate_candidate`] can score.
///
/// Every candidate angle is checked against [`candidate_angle_is_safe`] relative to
/// its own entry in `reference` (the angle vector the polish stage started from, not
/// necessarily `base`'s own current angle for that tier) BEFORE `base` is ever
/// cloned -- `None` if any one of them is unsafe, matching the parent module's
/// "Angle bounds" requirement that an out-of-bounds simplex point is rejected
/// outright, never clamped.
#[must_use]
pub(super) fn build_free_angle_candidate(
    base: &Design,
    free: &[usize],
    reference: &[f64],
    angles: &[f64],
) -> Option<Design> {
    let all_safe = angles
        .iter()
        .zip(reference)
        .all(|(&candidate_deg, &reference_deg)| {
            candidate_angle_is_safe(reference_deg, candidate_deg)
        });
    if !all_safe {
        return None;
    }
    let mut candidate = base.clone();
    for (&tier_index, &angle) in free.iter().zip(angles) {
        candidate.tiers[tier_index].angle_deg = angle;
    }
    Some(candidate)
}

/// Everything [`evaluate_candidate_pair`] needs that stays fixed across the whole
/// search, bundled purely to keep that function's (and [`super::optimize_design`]'s)
/// argument count reasonable.
pub(super) struct SearchContext<'a> {
    pub(super) material: &'a GemMaterial,
    pub(super) weights: &'a ObjectiveWeights,
    pub(super) baseline_warnings: &'a BaselineWarningCounts,
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
pub(super) fn select_candidate_directions(
    current: &Design,
    tier_index: usize,
    original_deg: f64,
    step_deg: f64,
) -> Vec<(f64, Design)> {
    [step_deg, -step_deg]
        .into_iter()
        .filter_map(|delta| {
            let candidate_deg = original_deg + delta;
            candidate_angle_is_safe(original_deg, candidate_deg).then(|| {
                let mut candidate = current.clone();
                candidate.tiers[tier_index].angle_deg = candidate_deg;
                (candidate_deg, candidate)
            })
        })
        .collect()
}

/// Evaluates every surviving `(candidate_deg, Design)` from
/// [`select_candidate_directions`] and reduces them to the `(best,
/// evaluations_spent)` shape [`evaluate_candidate_pair`] documents. Threading is
/// shaped by how many survivors there are: two are solved concurrently via
/// `std::thread::scope`; exactly one is solved inline, no scope, no spawn; zero
/// (both directions rejected before either was solved) returns `(None, 0)`
/// immediately, starting no threads at all.
pub(super) fn evaluate_survivors(
    survivors: Vec<(f64, Design)>,
    ctx: &SearchContext,
    current_score: f32,
) -> (Option<(f64, f32)>, usize) {
    let outcomes: Vec<(f64, CandidateOutcome)> = if let [(deg_a, design_a), (deg_b, design_b)] =
        survivors.as_slice()
    {
        let (outcome_a, outcome_b) = std::thread::scope(|scope| {
            let handle_a = scope.spawn(|| {
                evaluate_candidate(design_a, ctx.material, ctx.weights, ctx.baseline_warnings)
            });
            let handle_b = scope.spawn(|| {
                evaluate_candidate(design_b, ctx.material, ctx.weights, ctx.baseline_warnings)
            });
            (
                handle_a
                    .join()
                    .expect("candidate evaluation thread must not panic"),
                handle_b
                    .join()
                    .expect("candidate evaluation thread must not panic"),
            )
        });
        vec![(*deg_a, outcome_a), (*deg_b, outcome_b)]
    } else {
        survivors
            .into_iter()
            .map(|(deg, design)| {
                let outcome =
                    evaluate_candidate(&design, ctx.material, ctx.weights, ctx.baseline_warnings);
                (deg, outcome)
            })
            .collect()
    };

    let mut evaluations = 0usize;
    let mut best: Option<(f64, f32)> = None;
    for (candidate_deg, outcome) in outcomes {
        evaluations += 1;
        if let CandidateOutcome::Accepted { score } = outcome
            && score < current_score
            && best.is_none_or(|(_, best_score)| score < best_score)
        {
            best = Some((candidate_deg, score));
        }
    }
    (best, evaluations)
}

/// Evaluates a free tier's two candidate directions (`original_deg + step_deg`,
/// `original_deg - step_deg`) -- see [`select_candidate_directions`] for how the
/// unsafe ones are filtered out and [`evaluate_survivors`] for how the survivors
/// are run (concurrently on two OS threads only when both survive -- see the
/// module doc comment's "Where the compute runs" section). `std::thread::scope`
/// guarantees both spawned threads finish before this function returns, so there
/// is no lifetime hazard in borrowing `current`/`ctx` from the calling thread's
/// stack.
///
/// Returns `(best, evaluations_spent)`: `best` is `Some((candidate_deg, score))` for
/// whichever direction is [`CandidateOutcome::Accepted`] AND scores below
/// `current_score`, preferring the lower score when both qualify; `None` if neither
/// does. `evaluations_spent` counts every direction actually solved (0, 1, or 2) --
/// a direction rejected by [`candidate_angle_is_safe`] before ever calling
/// [`Design::solve`] is not counted.
pub(super) fn evaluate_candidate_pair(
    current: &Design,
    tier_index: usize,
    original_deg: f64,
    step_deg: f64,
    ctx: &SearchContext,
    current_score: f32,
) -> (Option<(f64, f32)>, usize) {
    let survivors = select_candidate_directions(current, tier_index, original_deg, step_deg);
    evaluate_survivors(survivors, ctx, current_score)
}

/// Running counts of the two mesh-dependent [`ManufacturabilityWarning`] variants an
/// angle-only edit can actually change. `FractionalIndex`/`OutOfOrderMeet`/
/// `MeetNameNotAscSafe` are excluded: they depend on `indices`/`constraint`, which
/// this module never touches (it only ever rewrites a free tier's `angle_deg`), so
/// their counts are invariant across every candidate by construction.
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
                | ManufacturabilityWarning::MeetNameNotAscSafe { .. } => {}
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
