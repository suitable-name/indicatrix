//! The first two stages of [`optimize_design_with`](super::optimize_design_with): measuring
//! the starting point ([`baseline_report`]), the coordinate-descent stage ([`run_search`]) and
//! the Nelder-Mead hand-off ([`run_polish_stage`]). Split from the parent module so the file
//! stays readable; the three functions are unchanged.

use super::{
    Baseline, CoordinateStageOutcome, OptimizeConfig, PolishStageOutcome, SearchHooks, SearchStage,
    seeded_permutation,
};
use crate::{
    design::{Design, DesignSolveError},
    manufacturability::{DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_manufacturability},
    optimize::{
        candidate::{
            BaselineWarningCounts, CandidateOutcome, SearchContext, apply_free_angles,
            build_free_angle_candidate, evaluate_candidate_pair, evaluate_in_context,
            free_tier_indices_with, measure_with_tone, shape_penalty_for_planes, yield_loss_pct,
        },
        guard::ShapeGuard,
        objective::{ObjectiveFidelity, to_gpu_planes},
        options::OptimizeOptions,
        polish,
        pool::CandidatePool,
        space::measure_anchors,
    },
};
use indicatrix::{
    geometry::stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    optics::materials::GemMaterial,
};

#[cfg(doc)]
use super::optimize_design;

/// What one coordinate descent owns when several share a config: see [`run_search`].
pub(super) struct StartRun {
    /// The evaluation budget of this descent (a soft cap, checked per tier decision).
    pub(super) max_evaluations: usize,
    /// The seed of this descent's sweep orders.
    pub(super) seed: u64,
    /// The `Fast` score of the design the descent starts from.
    pub(super) initial_score: f32,
}

/// Builds [`optimize_design`]'s [`Baseline`].
///
/// Reports [`SearchStage::BaselineFull`] (evaluations `0`, since none of
/// `config`'s budget is spent here) through `hooks` before running the one
/// [`ObjectiveFidelity::Full`] scoring this function performs -- that scoring
/// alone measures ~1.3s on a small real design (see the parent
/// module's "Cost first" doc section); without this report, a caller's progress
/// display would read as a frozen counter before the search had even started.
///
/// # Errors
///
/// Propagates [`Design::solve`]'s [`DesignSolveError`].
pub(super) fn baseline_report(
    design: &Design,
    material: &GemMaterial,
    config: &OptimizeConfig,
    options: &OptimizeOptions,
    hooks: &SearchHooks<'_>,
) -> Result<Baseline, DesignSolveError> {
    let weights = &config.weights;
    let lighting = config.lighting;
    hooks.report(0, SearchStage::BaselineFull);
    let baseline_solved = design.solve()?;
    let baseline_planes = design.planes_from_solved(&baseline_solved);
    let warnings = BaselineWarningCounts::count(&check_manufacturability(
        design,
        &baseline_solved,
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    ));
    let (before, tone_before, before_tone_loss) = measure_with_tone(
        &to_gpu_planes(&baseline_planes),
        material,
        weights,
        ObjectiveFidelity::Full,
        lighting,
    );
    let baseline_metrics = measure_solid(&baseline_planes);
    let before_yield_loss_pct = yield_loss_pct(design, baseline_metrics.as_ref());
    // The starting design pays the same shape penalty its candidates do (zero when the
    // target was read off it), so every score in the run stays comparable.
    let before_penalty = shape_penalty_for_planes(
        options.shape_target,
        baseline_metrics.as_ref(),
        &baseline_planes,
    );
    let before_score =
        weights.score_with_tone(&before, before_yield_loss_pct, before_tone_loss) + before_penalty;
    // Same planes, same yield figure -- just re-scored at `Fast` so `run_search`
    // has a same-fidelity baseline to compare its `Fast`-scored candidates
    // against (see `Baseline::before_score_fast`'s own doc comment). The tone is
    // measured here only when it is weighted, exactly like every candidate's.
    let (before_fast, _, before_fast_tone_loss) = measure_with_tone(
        &to_gpu_planes(&baseline_planes),
        material,
        weights,
        ObjectiveFidelity::Fast,
        lighting,
    );
    let before_score_fast =
        weights.score_with_tone(&before_fast, before_yield_loss_pct, before_fast_tone_loss)
            + before_penalty;
    let mut free = free_tier_indices_with(design, options);
    let anchors = measure_anchors(design, options, &free, &baseline_solved, &baseline_planes);
    // An anchored tier with no facet plane to read an azimuth from cannot turn about its
    // hinge, so it is not part of this run.
    free.retain(|&tier| {
        options.anchored_hinge(design, tier).is_none() || anchors.contains_key(&tier)
    });
    let guard = options.min_girdle_fraction.and_then(|fraction| {
        // Nothing closes without a closed start, so nothing needs guarding.
        let SolidStatus::Closed(mesh) = build_solid_mesh(&baseline_planes) else {
            return None;
        };
        Some(
            ShapeGuard::measure(design, &baseline_solved, &baseline_planes, &mesh, fraction)
                .with_thinnest_fraction(options.min_girdle_thinnest_fraction),
        )
    });
    Ok(Baseline {
        before,
        before_score,
        before_score_fast,
        before_yield_loss_pct,
        tone_before,
        warnings,
        free,
        anchors,
        guard,
    })
}

/// [`optimize_design`]'s coordinate-descent stage itself -- see that function's own
/// doc comment for the algorithm. Returns the design the stage ended on, that
/// design's own [`ObjectiveFidelity::Fast`] score (so [`run_polish_stage`] does not
/// have to re-evaluate the point it starts from), how many candidate evaluations
/// were spent, and whether [`SearchHooks::cancel`] cut it short.
///
/// When [`OptimizeConfig::polish_start_step_deg`] is disabled (`None` or
/// non-positive), this behaves bit-for-bit as it did before the polish stage
/// existed: the one extra check this function performs (below the `if
/// !improved_this_sweep` halving) is a no-op in that case, never changing which
/// candidates are tried or accepted.
///
/// `pool` collects every accepted candidate that beats the starting design's own
/// `Fast` score (see [`CandidatePool`]); a disabled pool ignores the offers, so the
/// single-result path is unchanged.
///
/// `start` carries the three values a run owns when several starts share one config:
/// its slice of the evaluation budget, its sweep seed, and the `Fast` score of `design`
/// (the point it starts from). A single-start search passes the config's own values and
/// the baseline's `Fast` score, so nothing changes for it.
pub(super) fn run_search(
    design: &Design,
    config: &OptimizeConfig,
    ctx: &SearchContext,
    baseline: &Baseline,
    mut pool: CandidatePool,
    hooks: &SearchHooks<'_>,
    start: &StartRun,
) -> CoordinateStageOutcome {
    let free = &baseline.free;
    let mut current = design.clone();
    // Mirrors `current`'s free angles, in `free` order, so a pool entry can be built
    // without re-reading the design.
    let mut free_angles: Vec<f64> = free.iter().map(|&i| current.tiers[i].angle_deg).collect();
    // Seeded from the `Fast`-fidelity baseline, not `baseline.before_score`
    // (`Full`) -- every candidate this loop compares against `current_score` is
    // itself scored at `Fast` (see `evaluate_candidate`), so mixing in a `Full`
    // starting figure could accept a candidate that only reproduces the true
    // starting point, never actually improving anything. See
    // `Baseline::before_score_fast`'s own doc comment for the measured case this
    // fixes.
    let mut current_score = start.initial_score;
    let mut evaluations = 0usize;
    let mut step_deg = config.initial_step_deg;
    let mut sweep_index = 0u64;
    let mut cancelled = false;

    'search: while step_deg >= config.min_step_deg && evaluations < start.max_evaluations {
        let order = seeded_permutation(
            free.len(),
            start.seed ^ sweep_index.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );
        sweep_index += 1;
        let mut improved_this_sweep = false;

        for &free_slot in &order {
            if hooks.is_cancelled() {
                cancelled = true;
                break 'search;
            }
            if evaluations >= start.max_evaluations {
                break;
            }
            let tier_index = free[free_slot];
            let original_deg = current.tiers[tier_index].angle_deg;

            let pair = evaluate_candidate_pair(
                &current,
                tier_index,
                original_deg,
                step_deg,
                ctx,
                current_score,
            );
            evaluations += pair.evaluations;
            hooks.report(evaluations, SearchStage::Coordinate);

            // Offered relative to the state BEFORE this tier's move is adopted: every
            // accepted direction is a neighbour of it, whether or not it won.
            if pool.is_enabled() {
                for &(candidate_deg, score) in &pair.accepted {
                    if score < baseline.before_score_fast {
                        let mut angles = free_angles.clone();
                        angles[free_slot] = candidate_deg;
                        pool.offer(angles, score);
                    }
                }
            }

            if let Some((new_deg, new_score)) = pair.best {
                let written = ctx.space.write_angle(&mut current, tier_index, new_deg);
                debug_assert!(written, "an accepted candidate already had a usable mast");
                free_angles[free_slot] = new_deg;
                current_score = new_score;
                improved_this_sweep = true;
            }
        }

        if !improved_this_sweep {
            step_deg /= 2.0;
            // Hand off to the polish stage once the coordinate stage's own step has
            // become fine enough that further axis-aligned halving is exactly the
            // grinding-on-a-ridge behavior `optimize_design`'s "Two stages" doc
            // section describes -- a no-op check when polishing is disabled.
            let should_hand_off = config
                .polish_start_step_deg
                .is_some_and(|threshold| threshold > 0.0 && step_deg < threshold);
            if should_hand_off {
                break;
            }
        }
    }

    CoordinateStageOutcome {
        design: current,
        score: current_score,
        evaluations,
        cancelled,
        pool,
    }
}

/// [`optimize_design`]'s "best point in, best point out" half for the polish stage:
/// builds the Fast-fidelity scoring closure [`polish::run_polish`] needs (via
/// [`build_free_angle_candidate`]/[`evaluate_in_context`] -- the exact same
/// solve/close/guard/manufacturability gate the coordinate stage's candidates go
/// through) and, if the polish stage's own result strictly improves on
/// `current_score`, applies it to `current`'s free tiers. Never touches a tier outside
/// the free set (`baseline.free`). Every point the closure scores finite and below the
/// starting design's `Fast` score is also offered to the candidate pool.
///
/// Disabled ([`OptimizeConfig::polish_start_step_deg`] is `None` or non-positive)
/// short-circuits to a zero-evaluation, unchanged [`PolishStageOutcome`] before ever
/// building the closure or calling [`polish::run_polish`].
///
/// `coord`'s own `evaluations` is only for progress reporting: each call the
/// `evaluate` closure below makes reports `hooks.report` with
/// [`SearchStage::Polish`] and a running total that STARTS from
/// `coord.evaluations` rather than from zero, so a caller's own evaluation
/// counter keeps climbing smoothly across the coordinate-to-polish hand-off
/// instead of resetting or freezing for the whole polish stage. Only ever
/// called with `coord.cancelled == false` -- see [`optimize_design`]'s own call
/// site.
pub(super) fn run_polish_stage(
    design: &Design,
    baseline: &Baseline,
    config: &OptimizeConfig,
    ctx: &SearchContext,
    hooks: &SearchHooks<'_>,
    coord: CoordinateStageOutcome,
) -> PolishStageOutcome {
    let free = &baseline.free;
    let CoordinateStageOutcome {
        design: current,
        score: current_score,
        evaluations: coord_evaluations,
        cancelled: _,
        mut pool,
    } = coord;
    let Some(start_step) = config.polish_start_step_deg.filter(|&step| step > 0.0) else {
        return PolishStageOutcome {
            current,
            evaluations: 0,
            improvement: 0.0,
            cancelled: false,
            pool,
        };
    };

    let starting_point: Vec<f64> = free.iter().map(|&i| current.tiers[i].angle_deg).collect();
    let max_evaluations = config
        .polish_max_evaluations
        .unwrap_or_else(|| 3 * free.len() + 20);
    let min_spread = config.min_step_deg / 2.0;

    let mut polish_evaluations_done = 0usize;
    let evaluate = |angles: &[f64]| -> f32 {
        let score = build_free_angle_candidate(design, free, &starting_point, angles, ctx.space)
            .map_or(f32::INFINITY, |candidate| {
                match evaluate_in_context(&candidate, ctx) {
                    CandidateOutcome::Rejected => f32::INFINITY,
                    CandidateOutcome::Accepted { score } => score,
                }
            });
        if pool.is_enabled() && score < baseline.before_score_fast {
            pool.offer(angles.to_vec(), score);
        }
        polish_evaluations_done += 1;
        hooks.report(
            coord_evaluations + polish_evaluations_done,
            SearchStage::Polish,
        );
        score
    };

    let result = polish::run_polish(
        &starting_point,
        current_score,
        start_step,
        max_evaluations,
        min_spread,
        &|| hooks.is_cancelled(),
        evaluate,
    );

    let adopted = if result.score < current_score {
        apply_free_angles(&current, free, &result.point, ctx.space)
    } else {
        None
    };
    let (current, improvement) = adopted.map_or((current, 0.0), |improved| {
        (improved, current_score - result.score)
    });
    PolishStageOutcome {
        current,
        evaluations: result.evaluations,
        improvement,
        cancelled: result.cancelled,
        pool,
    }
}
