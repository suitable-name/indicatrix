//! The coordinate-descent search itself: [`OptimizeConfig`]/[`SearchHooks`]
//! (user-visible knobs and cancellation), the deterministic tour order
//! ([`seeded_permutation`]), and [`optimize_design`] (with its
//! [`OptimizeOptions`] sibling [`optimize_design_with`]) -- split into
//! [`baseline_report`]/[`run_search`]/[`build_result`], see that function's
//! own doc comment ("Why this is three functions, not one") for why.

use super::{
    candidate::{BaselineWarningCounts, SearchContext, apply_free_angles},
    guard::ShapeGuard,
    objective::{CANONICAL_LIGHTING_PRESET, ObjectiveComponents, ObjectiveWeights},
    options::{OptimizeOptions, OptimizeResult},
    pool::CandidatePool,
    space::{Anchor, SearchSpace},
};
use crate::design::{Design, DesignSolveError};
use indicatrix::{
    color::metrics::FaceUpTone,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use std::collections::BTreeMap;

#[cfg(doc)]
use super::{candidate::evaluate_candidate_pair, objective::ObjectiveFidelity, polish};

mod multi;
mod result;
mod stages;

use super::multistart::{
    effective_starts as effective_starts_for, polished_starts, screening_draws,
};
use result::build_result;
use stages::{StartRun, baseline_report, run_polish_stage, run_search};

/// Which phase of [`optimize_design`] a [`SearchHooks::on_progress`] call reports on.
///
/// This allows a caller's progress ticker to distinguish between "the counter is
/// frozen because nothing is happening" (a real hang) and "the counter is frozen
/// because this stage does not advance it" (the two fixed [`ObjectiveFidelity::Full`]
/// scorings that bracket every run, and the polish stage, which both report their
/// own evaluations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStage {
    /// Scoring `design` exactly as given, at [`ObjectiveFidelity::Full`], before
    /// any candidate is tried -- always exactly one report, evaluations `0`.
    BaselineFull,
    /// The coordinate-descent stage (see [`optimize_design`]'s "Algorithm"
    /// section).
    Coordinate,
    /// The Nelder-Mead polish stage (see [`optimize_design`]'s "Two stages"
    /// section).
    Polish,
    /// Scoring the point the search ended on, at [`ObjectiveFidelity::Full`],
    /// after the last [`Self::Coordinate`]/[`Self::Polish`] report -- exactly one
    /// report for an ordinary run. A run that asked for alternatives
    /// ([`OptimizeOptions::keep_candidates`] above one) scores each alternative at
    /// [`ObjectiveFidelity::Full`] too, one by one, and reports this stage again for
    /// every one of them (so a progress display stays on "scoring the result").
    FinalFull,
    /// A multi-start run ([`OptimizeConfig::starts`] above one) drawing and scoring its
    /// extra starting points at [`ObjectiveFidelity::Fast`], one evaluation per draw,
    /// before any descent. Never reported by a single-start run.
    Screening,
}

impl SearchStage {
    /// A small, stable numeric encoding for a caller that needs to carry a
    /// [`SearchStage`] across a boundary this enum can't cross directly -- e.g. a
    /// shared [`std::sync::atomic::AtomicU8`] a progress ticker thread polls, the
    /// same way a caller might already share the running evaluation count.
    /// Paired with [`Self::from_code`].
    #[must_use]
    pub const fn to_code(self) -> u8 {
        match self {
            Self::BaselineFull => 0,
            Self::Coordinate => 1,
            Self::Polish => 2,
            Self::FinalFull => 3,
            Self::Screening => 4,
        }
    }

    /// The inverse of [`Self::to_code`]. Any value outside `0..=4` (never
    /// produced by [`Self::to_code`] itself) decodes to [`Self::Coordinate`],
    /// the stage a progress reader is safest defaulting to before the first real
    /// report arrives.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::BaselineFull,
            2 => Self::Polish,
            3 => Self::FinalFull,
            4 => Self::Screening,
            _ => Self::Coordinate,
        }
    }
}

/// Cooperative cancellation and real progress reporting for [`optimize_design`].
///
/// Unlike `gui::editor::deep_solve` (which calls into an opaque repair search with
/// no checkpoint to poll, hence its "UI-level abandonment" cancellation model),
/// this module owns its entire search loop -- so a real mid-search checkpoint is
/// possible: `cancel` is polled once per tier decision, and `on_progress` is
/// invoked with the running evaluation count and the active [`SearchStage`] at
/// every point this module's own doc comment on [`SearchStage`] names -- including
/// the polish stage's own evaluations and the two fixed full-fidelity scorings.
///
/// Every field defaults to `None` ([`SearchHooks::default`]) for a caller (e.g. a
/// test) that wants none of them.
#[derive(Default)]
pub struct SearchHooks<'a> {
    /// Flag polled during the search; setting it stops the search early.
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
    /// Callback invoked with the evaluation count and current stage.
    pub on_progress: Option<&'a dyn Fn(usize, SearchStage)>,
    /// Callback invoked by a multi-start run ([`OptimizeConfig::starts`] above one) on
    /// the calling thread, each time the lane the caller's thread runs begins a start
    /// (descent or polish). Never called by a single-start run.
    pub on_start: Option<&'a dyn Fn(StartProgress)>,
}

/// What a multi-start run tells [`SearchHooks::on_start`].
///
/// Which start the calling thread's lane is beginning, out of how many, and the best
/// `Fast` score any finished start has reached so far (the starting design's own `Fast`
/// score before the first wave ends).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StartProgress {
    /// Zero-based index of the start (`0` is the design's own descent).
    pub index: usize,
    /// How many starts are planned or already run (at least `index + 1`).
    pub count: usize,
    /// The best `Fast` score reached so far, lower is better.
    pub best_fast_score: f32,
}

impl SearchHooks<'_> {
    fn is_cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
    }

    fn report(&self, evaluations: usize, stage: SearchStage) {
        if let Some(on_progress) = self.on_progress {
            on_progress(evaluations, stage);
        }
    }
}

/// A deterministic splitmix64 step -- the whole PRNG this module uses, for the one
/// place it needs one: shuffling the free-tier visit order in [`seeded_permutation`].
/// Not cryptographic; a tiny, dependency-free generator sufficient for "pick a
/// deterministic tour order from a seed", avoiding a `rand` dependency for exactly
/// one shuffle.
pub(super) const fn splitmix64_next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A deterministic Fisher-Yates shuffle of `0..len`, seeded by `seed` -- the tour order
/// [`optimize_design`]'s coordinate search visits free tiers in, each sweep (a fresh
/// permutation per sweep, `seed` mixed with the sweep index so consecutive sweeps don't
/// repeat the same order). Plain `Vec<usize>`, never a `HashMap`/`HashSet` -- see the
/// module doc comment's "Determinism" section.
pub(super) fn seeded_permutation(len: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    let mut state = seed;
    for i in (1..len).rev() {
        let r = (splitmix64_next(&mut state) % (i as u64 + 1)) as usize;
        order.swap(i, r);
    }
    order
}

/// User-visible search configuration -- see the module doc comment's "Determinism"
/// section for why `seed` is required rather than defaulted from wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OptimizeConfig {
    /// Weights blended into the objective score.
    pub weights: ObjectiveWeights,
    /// Seed for the search's deterministic pseudo-random choices.
    pub seed: u64,
    /// Approximate cap on the number of candidate evaluations this search will
    /// perform, regardless of whether it has converged -- the one knob that bounds
    /// wall-clock time directly. A soft cap: each tier decision evaluates its
    /// `+step`/`-step` pair together, checked once BEFORE a pair starts, so the
    /// true count can exceed this value by at most one pair (2).
    pub max_evaluations: usize,
    /// Initial per-tier angle step, in degrees, for the coordinate search. Halved each
    /// time a full sweep produces no accepted improvement, down to `min_step_deg`.
    pub initial_step_deg: f64,
    /// Smallest per-tier angle step, in degrees, before the coordinate search stops halving.
    pub min_step_deg: f64,
    /// The coordinate stage's own step threshold for handing off to the polish
    /// stage -- see [`optimize_design`]'s "Two stages" doc section. Once a sweep's
    /// halved `step_deg` first drops below this value, the coordinate stage stops
    /// early (rather than continuing to grind down to `min_step_deg`) and a
    /// deterministic Nelder-Mead simplex search takes over from its best point.
    /// `None` or a non-positive value disables the polish stage entirely, in which
    /// case the coordinate stage runs exactly as it did before this stage existed
    /// (see [`run_search`]'s own doc comment).
    pub polish_start_step_deg: Option<f64>,
    /// Caps the polish stage's own evaluation count, independent of
    /// `max_evaluations` (the coordinate stage's budget). `None` uses `3 *
    /// free_tier_indices(design).len() + 20` -- enough for the initial simplex (one
    /// evaluation per free tier) plus a modest number of reflect/expand/contract
    /// iterations. Has no effect when the polish stage is disabled.
    pub polish_max_evaluations: Option<usize>,
    /// The lighting preset every design is scored under, before, during and after the
    /// search: the metrics describe the image a preset lights, so a stone optimized for
    /// the preset the viewport shows is optimized for what the user sees. Defaults to
    /// [`CANONICAL_LIGHTING_PRESET`].
    pub lighting: LightingPreset,
    /// How many different starting arrangements the search tries; `0` and `1` (the
    /// default) run the single descent from the design itself, bit for bit as before
    /// this field existed. Above one the run screens extra starting points and splits
    /// [`Self::max_evaluations`] between the starts (see the parent module's "Several
    /// starts" section). Reduced automatically by [`effective_starts`] when the budget
    /// is too small to give every start a few sweeps.
    pub starts: usize,
    /// How many starts run side by side on their own threads in a multi-start run;
    /// `0` (the default) picks half of the available parallelism. The result never
    /// depends on this number. Ignored by a single-start run and on `wasm32`.
    pub max_lanes: usize,
}

impl Default for OptimizeConfig {
    fn default() -> Self {
        Self {
            weights: ObjectiveWeights::default(),
            seed: 0,
            max_evaluations: 200,
            initial_step_deg: 2.0,
            min_step_deg: 0.125,
            polish_start_step_deg: Some(0.5),
            polish_max_evaluations: None,
            lighting: CANONICAL_LIGHTING_PRESET,
            starts: 1,
            max_lanes: 0,
        }
    }
}

/// How many starts a run with `config` and `free_tier_count` free tiers really uses.
///
/// [`OptimizeConfig::starts`] capped so every start gets at least `8 * free_tier_count`
/// evaluations (four sweeps), and never below one. `1` means the run is exactly the
/// single-start search.
#[must_use]
pub fn effective_starts(config: &OptimizeConfig, free_tier_count: usize) -> usize {
    effective_starts_for(config.starts, config.max_evaluations, free_tier_count)
}

/// The inclusive evaluation budget across both search stages for a design with
/// `free_tier_count` tiers free to move.
///
/// [`OptimizeConfig::max_evaluations`] (the coordinate stage) plus the polish
/// stage's own cap, computed the same way [`optimize_design`] itself would (`0`
/// when the polish stage is disabled) -- see
/// [`OptimizeConfig::polish_max_evaluations`]'s own default.
///
/// A caller reporting progress via [`SearchHooks::on_progress`]
/// needs this to show an honest "N of ~M evaluations" once the polish stage's own
/// evaluations are included in `N` -- `config.max_evaluations` alone would silently
/// exclude the polish stage's budget, reading as the counter blowing past its own
/// stated maximum.
///
/// For a multi-start run ([`effective_starts`] above one) the figure also holds the
/// screening draws (`4 * (starts - 1)`, at most 64) and one polish budget per polished
/// start; this function assumes one polished start (the default
/// [`OptimizeOptions::keep_candidates`]), [`inclusive_max_evaluations_for`] takes the
/// real one. A single-start run is unchanged.
#[must_use]
pub fn inclusive_max_evaluations(config: &OptimizeConfig, free_tier_count: usize) -> usize {
    inclusive_max_evaluations_for(config, 1, free_tier_count)
}

/// [`inclusive_max_evaluations`] for a request that keeps `keep_candidates` candidates.
///
/// A multi-start run polishes the best `max(1, keep_candidates)` of its starts, each
/// with its own polish budget. Identical to [`inclusive_max_evaluations`] for a
/// single-start run, whatever `keep_candidates` is.
#[must_use]
pub fn inclusive_max_evaluations_for(
    config: &OptimizeConfig,
    keep_candidates: usize,
    free_tier_count: usize,
) -> usize {
    let polish_max = config
        .polish_start_step_deg
        .filter(|&step| step > 0.0)
        .map_or(0, |_| {
            config
                .polish_max_evaluations
                .unwrap_or(3 * free_tier_count + 20)
        });
    let starts = effective_starts(config, free_tier_count);
    if starts <= 1 {
        return config.max_evaluations + polish_max;
    }
    config.max_evaluations
        + screening_draws(starts)
        + polished_starts(starts, keep_candidates) * polish_max
}

/// One accepted change [`optimize_design`] proposes: tier `index`'s angle moves from
/// `from_deg` to `to_deg`. Never applied by this module itself -- see
/// [`super::apply_optimize_outcome`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngleChange {
    /// Tier position this refers to.
    pub index: usize,
    /// Tier angle before the change, in degrees.
    pub from_deg: f64,
    /// Tier angle after the change, in degrees.
    pub to_deg: f64,
}

/// What [`optimize_design`] found.
///
/// The before/after objective (always measured at [`ObjectiveFidelity::Full`],
/// even though the search itself uses [`ObjectiveFidelity::Fast`] -- see the
/// module doc comment's "Cost first" section), how many candidate evaluations it
/// spent getting there, and which tiers it would change.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizeOutcome {
    /// Objective components of the starting design.
    pub before: ObjectiveComponents,
    /// Combined score of the starting design.
    pub before_score: f32,
    /// `100.0 -` the starting design's own
    /// [`crate::yield_metrics::volumetric_yield`] percentage -- the same figure
    /// [`super::objective::ObjectiveWeights::score_with_yield`] blended into
    /// [`Self::before_score`] at [`OptimizeConfig::weights`]'s own
    /// `yield_weight`, reported here too so a before/after report can show the
    /// yield term even when `yield_weight == 0.0` left it out of the score
    /// itself.
    pub before_yield_loss_pct: f32,
    /// Objective components after applying the changes.
    pub after: ObjectiveComponents,
    /// Combined score after applying the changes. Never above [`Self::before_score`]:
    /// when the search's end point does not measure as an improvement at
    /// [`ObjectiveFidelity::Full`], the outcome proposes no change and `after` is
    /// `before`.
    pub after_score: f32,
    /// [`Self::before_yield_loss_pct`]'s counterpart for the design
    /// [`Self::changes`] produced.
    pub after_yield_loss_pct: f32,
    /// Number of candidate evaluations spent.
    pub evaluations: usize,
    /// Proposed angle changes, one per modified tier.
    pub changes: Vec<AngleChange>,
    /// `true` iff [`SearchHooks::cancel`] was observed set before the search would
    /// otherwise have stopped on its own -- `after`/`after_score`/`changes` still
    /// reflect the best state found up to that point, a real partial result never
    /// discarded.
    pub cancelled: bool,
    /// How many of `evaluations` were spent in the polish stage (see
    /// [`OptimizeConfig::polish_start_step_deg`]) -- `0` when the coordinate stage
    /// was cancelled before reaching it, when polishing is disabled, or when a
    /// freshly-imported design has no free tiers at all.
    pub polish_evaluations: usize,
    /// The score improvement (coordinate stage's ending score minus the polish
    /// stage's, `>= 0.0`) actually adopted from the polish stage. `0.0` whenever
    /// `polish_evaluations == 0`, or when the polish stage ran but never beat the
    /// coordinate stage's own result -- the polish stage's point is only ever
    /// adopted when it strictly improves on the score it started from, exactly like
    /// every other candidate this module considers.
    pub polish_improvement: f32,
}

/// Runs a deterministic coordinate (pattern) search over `design`'s free tier angles
/// (see [`super::free_tier_indices`]) to minimize [`super::objective::ObjectiveWeights::score`].
///
/// [`optimize_design_with`] is the same search with an [`OptimizeOptions`] request.
/// Entirely synchronous on the calling thread (aside from the two short-lived
/// worker threads [`evaluate_candidate_pair`] spawns per tier decision) -- a caller
/// that needs the whole search off the UI thread wraps this call the same way
/// `gui::editor::deep_solve` wraps `solve_meet_points_verified`. `hooks` (see
/// [`SearchHooks`]) is that caller's way to observe progress and request
/// cancellation while this runs.
///
/// # Algorithm
///
/// Greedy coordinate descent with a shrinking step, chosen because it needs no
/// gradient (this objective has none -- a ray can flip from TIR to refract-out at
/// an arbitrary threshold angle) and because every evaluation is independently
/// interpretable (accept/reject), which is what makes the "never propose geometry
/// that fails to close" and "manufacturability must not regress" gates
/// straightforward to enforce per-step.
///
/// Each sweep visits every free tier once, in a fresh [`seeded_permutation`] order,
/// and for each tries `angle + step` and `angle - step` concurrently (via
/// [`evaluate_candidate_pair`]), keeping whichever accepted result has the lower
/// score. A sweep that accepts no improvement at all halves `step`; the coordinate
/// stage stops when `step` would drop below `config.min_step_deg`,
/// [`OptimizeConfig::max_evaluations`] is reached, or `hooks.cancel` is observed
/// set, whichever comes first.
///
/// # Two stages
///
/// Axis-aligned moves stall on a ridge where paired angles (e.g. a crown break and
/// a pavilion main) must move together to hold a critical-angle relation -- see the
/// parent module's "Coordinate descent stalls on diagonal ridges" section, carried
/// into [`super::polish`]'s doc comment. Once a coordinate sweep's
/// halved `step_deg` first drops below [`OptimizeConfig::polish_start_step_deg`],
/// the coordinate stage stops early and a deterministic Nelder-Mead simplex search
/// ([`polish::run_polish`]) takes over from its best point, moving every free
/// tier's angle at once. The polish stage's own point is only adopted if it
/// strictly improves on the score the coordinate stage handed it -- see
/// [`OptimizeOutcome::polish_improvement`]. `polish_start_step_deg: None` (or
/// non-positive) skips this stage entirely and the coordinate stage runs to
/// `min_step_deg` exactly as it did before this stage existed.
///
/// # A freshly-imported design has nothing to optimize
///
/// If [`super::free_tier_indices`] is empty, this returns immediately with
/// `evaluations: 0` and `before == after` -- `hooks.cancel` is never consulted,
/// though `hooks.on_progress` still receives [`baseline_report`]'s one
/// [`SearchStage::BaselineFull`] report, since that scoring happens regardless of
/// whether there turns out to be anything free to search over.
///
/// # Errors
///
/// Propagates [`Design::solve`]'s [`DesignSolveError`] if `design` itself (before any
/// candidate is even tried) does not solve.
///
/// # Panics
///
/// Never in practice: the final re-solve of `current` is `.expect()`ed rather than
/// propagated, but `current` only ever advances to a state
/// [`super::candidate::evaluate_candidate`] already proved solves (and closes, and
/// does not regress manufacturability).
///
/// # Why this is more than one function
///
/// Split into [`baseline_report`] (measure the starting point), [`run_search`] (the
/// coordinate-descent stage), [`run_polish_stage`] (the Nelder-Mead hand-off), and
/// [`build_result`] (measure the ending point and diff it against the start) to
/// stay under clippy's `too_many_lines`, with the starting-point measurements
/// bundled into one [`Baseline`] and the two stages' results bundled into
/// [`SearchRunSummary`] so the split does not trade "one long function" for
/// `too_many_arguments` -- no `#[allow]` added.
pub fn optimize_design(
    design: &Design,
    material: &GemMaterial,
    config: &OptimizeConfig,
    hooks: &SearchHooks<'_>,
) -> Result<OptimizeOutcome, DesignSolveError> {
    optimize_design_with(design, material, config, &OptimizeOptions::default(), hooks)
        .map(|result| result.outcome)
}

/// [`optimize_design`] with an [`OptimizeOptions`] request, returning ranked alternatives.
///
/// The search is the same two-stage one (see that function's "Algorithm" and "Two
/// stages" sections), over a space the request may widen or narrow, and it returns the
/// best result plus the alternatives.
///
/// With [`OptimizeOptions::default`] this is exactly [`optimize_design`]: same free
/// tiers, same candidates, same outcome bits. The request may
///
/// - let `ScaleReference` tiers with a hinge move too
///   ([`OptimizeOptions::vary_anchored`]; their masts follow their hinges, reported as
///   [`OptimizeResult::mast_changes`]),
/// - confine tiers to angle ranges ([`OptimizeOptions::angle_bounds`]),
/// - reject candidates that lose the girdle band or a table facet
///   ([`OptimizeOptions::min_girdle_fraction`]),
/// - keep several distinct end points ([`OptimizeOptions::keep_candidates`]), and
/// - favour options that keep the stone's table size and crown-to-pavilion ratio
///   ([`OptimizeOptions::shape_target`], a penalty in every score).
///
/// The weights may also carry a face-up tone term
/// ([`ObjectiveWeights::tone_weight`](super::objective::ObjectiveWeights::tone_weight),
/// `tone_goal`). The result always reports the tones ([`OptimizeResult::tone_before`],
/// [`OptimizeCandidate::tone`](super::OptimizeCandidate::tone)) and the lighting preset they
/// were measured under ([`OptimizeResult::lighting`]); [`OptimizeResult::tone_goal`] is
/// `Some` iff the tone was weighted.
///
/// # Several candidates
///
/// During both stages the run keeps the best distinct states it saw by fast score
/// (distinct: some free angle at least the separation apart, see
/// [`OptimizeOptions::candidate_separation_deg`]). At the end the end point and the
/// best of those states are re-scored at [`ObjectiveFidelity::Full`], one by one
/// (cancellable between scorings; each reports [`SearchStage::FinalFull`]), those that
/// change something and score no worse than the starting design become
/// [`OptimizeResult::candidates`], best full score first, and `outcome` describes the
/// first of them. With one candidate (the default) there is no extra scoring and the
/// list holds the end point alone.
///
/// # Errors
///
/// Propagates [`Design::solve`]'s [`DesignSolveError`] if `design` itself (before any
/// candidate is even tried) does not solve.
///
/// # Panics
///
/// Never in practice; see [`optimize_design`].
pub fn optimize_design_with(
    design: &Design,
    material: &GemMaterial,
    config: &OptimizeConfig,
    options: &OptimizeOptions,
    hooks: &SearchHooks<'_>,
) -> Result<OptimizeResult, DesignSolveError> {
    let baseline = baseline_report(design, material, config, options, hooks)?;

    if baseline.free.is_empty() {
        return Ok(OptimizeResult::unchanged(
            OptimizeOutcome {
                before: baseline.before,
                before_score: baseline.before_score,
                before_yield_loss_pct: baseline.before_yield_loss_pct,
                after: baseline.before,
                after_score: baseline.before_score,
                after_yield_loss_pct: baseline.before_yield_loss_pct,
                evaluations: 0,
                changes: Vec::new(),
                cancelled: false,
                polish_evaluations: 0,
                polish_improvement: 0.0,
            },
            config.lighting,
            baseline.tone_before,
            (config.weights.tone_weight > 0.0).then_some(config.weights.tone_goal),
        ));
    }

    let space = SearchSpace::new(options, baseline.anchors.clone());
    let ctx = SearchContext {
        material,
        weights: &config.weights,
        baseline_warnings: &baseline.warnings,
        lighting: config.lighting,
        space: &space,
        guard: baseline.guard.as_ref(),
        shape_target: options.shape_target,
    };
    let pool = CandidatePool::new(
        options.candidate_capacity(),
        options
            .candidate_separation_deg
            .unwrap_or(config.min_step_deg),
    );
    let starts = effective_starts(config, baseline.free.len());
    let (current, pool, summary) = if starts <= 1 {
        run_single_start(design, config, &ctx, &baseline, pool, hooks)
    } else {
        multi::run_multi_start(
            &multi::MultiRun {
                design,
                config,
                options,
                ctx: &ctx,
                baseline: &baseline,
                starts,
            },
            hooks,
        )
    };

    let best_angles: Vec<f64> = baseline
        .free
        .iter()
        .map(|&i| current.tiers[i].angle_deg)
        .collect();
    let rivals: Vec<Design> = pool
        .rivals(&best_angles)
        .into_iter()
        .filter_map(|entry| apply_free_angles(design, &baseline.free, &entry.angles, &space))
        .collect();

    Ok(build_result(
        design, &current, &rivals, &ctx, &baseline, hooks, &summary,
    ))
}

/// The single-descent search: the coordinate stage from `design` itself, then the polish
/// stage. What [`optimize_design_with`] runs whenever [`effective_starts`] is one.
fn run_single_start(
    design: &Design,
    config: &OptimizeConfig,
    ctx: &SearchContext,
    baseline: &Baseline,
    pool: CandidatePool,
    hooks: &SearchHooks<'_>,
) -> (Design, CandidatePool, SearchRunSummary) {
    let start = StartRun {
        max_evaluations: config.max_evaluations,
        seed: config.seed,
        initial_score: baseline.before_score_fast,
    };
    let coord = run_search(design, config, ctx, baseline, pool, hooks, &start);
    let coord_evaluations = coord.evaluations;
    let coord_cancelled = coord.cancelled;

    let polish = if coord_cancelled {
        PolishStageOutcome {
            current: coord.design,
            evaluations: 0,
            improvement: 0.0,
            cancelled: true,
            pool: coord.pool,
        }
    } else {
        run_polish_stage(design, baseline, config, ctx, hooks, coord)
    };

    let summary = SearchRunSummary {
        evaluations: coord_evaluations + polish.evaluations,
        cancelled: coord_cancelled || polish.cancelled,
        polish_evaluations: polish.evaluations,
        polish_improvement: polish.improvement,
        starts_run: 1,
        best_start: 0,
    };
    (polish.current, polish.pool, summary)
}

/// [`optimize_design`]'s "measure the starting point" half, bundled into one
/// struct: the [`Design::solve`]'d, [`ObjectiveFidelity::Full`]-scored baseline,
/// the [`BaselineWarningCounts`] every candidate gets checked against, and the
/// free tier indices the search is allowed to move.
struct Baseline {
    before: ObjectiveComponents,
    before_score: f32,
    /// The SAME starting design's score, but at [`ObjectiveFidelity::Fast`] --
    /// what [`run_search`] actually seeds `current_score` from. Every
    /// candidate [`super::candidate::evaluate_candidate`] scores is `Fast` too
    /// (see the parent module's "Cost first" doc section), so comparing a
    /// candidate's score against [`Self::before_score`] (`Full`) mixed
    /// fidelities: a `Fast` candidate that merely reproduces the TRUE starting
    /// point's own `Fast` score could still read as an "improvement" over the
    /// `Full` figure whenever `Full` happened to score worse than `Fast` on the
    /// same design (measured: RBC-445's Upper Girdle, `Full` 30.766 vs `Fast`
    /// 21.926) -- accepting a candidate that changed nothing real.
    before_score_fast: f32,
    /// `weights.score_with_yield`
    /// already computes this to fold into [`Self::before_score`] -- stored here
    /// too (rather than recomputed) so [`OptimizeOutcome::before_yield_loss_pct`]
    /// can report the same figure the score itself was blended from.
    before_yield_loss_pct: f32,
    /// The starting design's face-up tone, from the `Full` baseline scoring (always
    /// measured, weighted or not).
    tone_before: Option<FaceUpTone>,
    warnings: BaselineWarningCounts,
    free: Vec<usize>,
    /// Where each anchored free tier turns, read from the starting design's own solve.
    anchors: BTreeMap<usize, Anchor>,
    /// The girdle/table guard measured on the starting design, when the request
    /// asked for one (and the starting design closes, so there is something to keep).
    guard: Option<ShapeGuard>,
}

/// [`run_search`]'s return, bundled into one struct purely to keep
/// [`run_polish_stage`]'s own argument count under clippy's `too_many_arguments`
/// lint (see [`optimize_design`]'s "why this is more than one function" note): the
/// design and score the coordinate stage ended on, how many evaluations it spent,
/// whether [`SearchHooks::cancel`] cut it short, and the candidate pool so far.
struct CoordinateStageOutcome {
    design: Design,
    score: f32,
    evaluations: usize,
    cancelled: bool,
    pool: CandidatePool,
}

/// [`run_polish_stage`]'s return: the design it ended on (whether or not the polish
/// stage's own point was actually adopted), how many evaluations the polish stage
/// spent, the score improvement adopted (`0.0` if none), whether cancellation cut it
/// short, and the candidate pool as it ended. [`build_result`] always re-solves and
/// re-scores `current` at [`ObjectiveFidelity::Full`], so no `Fast` score is kept.
struct PolishStageOutcome {
    current: Design,
    evaluations: usize,
    improvement: f32,
    cancelled: bool,
    pool: CandidatePool,
}

/// Everything about a finished (both-stage) search run that [`build_result`] needs
/// beyond the design it ended on -- bundled solely to keep that function's argument
/// count reasonable (see [`optimize_design`]'s own "why this is more than one
/// function" note).
struct SearchRunSummary {
    evaluations: usize,
    cancelled: bool,
    polish_evaluations: usize,
    polish_improvement: f32,
    /// How many starts ran (`1` for a single-start search).
    starts_run: usize,
    /// Which start's end point is the result (`0` is the design's own descent).
    best_start: usize,
}
