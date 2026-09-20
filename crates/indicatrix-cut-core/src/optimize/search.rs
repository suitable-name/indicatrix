//! The coordinate-descent search itself: [`OptimizeConfig`]/[`SearchHooks`]
//! (user-visible knobs and cancellation), the deterministic tour order
//! ([`seeded_permutation`]), and [`optimize_design`] -- split into
//! [`baseline_report`]/[`run_search`]/[`build_outcome`], see that function's
//! own doc comment ("Why this is three functions, not one") for why.

use super::{
    candidate::{
        BaselineWarningCounts, CandidateOutcome, SearchContext, build_free_angle_candidate,
        evaluate_candidate, evaluate_candidate_pair, free_tier_indices,
    },
    objective::{
        ObjectiveComponents, ObjectiveFidelity, ObjectiveWeights, evaluate_objective, to_gpu_planes,
    },
    polish,
};
use crate::{
    design::{Design, MissingAnchor},
    manufacturability::{DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_manufacturability},
};
use indicatrix::optics::materials::GemMaterial;

/// Which phase of [`optimize_design`] a [`SearchHooks::on_progress`] call reports on.
///
/// CAD audit items 153/161: without this, a caller's progress ticker had no way
/// to tell "the counter is frozen because nothing is happening" (a real hang)
/// apart from "the counter is frozen because this stage does not advance it" (the
/// two fixed [`ObjectiveFidelity::Full`] scorings that bracket every run, and --
/// before this type existed -- the polish stage too, which never reported at all).
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
    /// after the last [`Self::Coordinate`]/[`Self::Polish`] report -- always
    /// exactly one report.
    FinalFull,
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
        }
    }

    /// The inverse of [`Self::to_code`]. Any value outside `0..=3` (never
    /// produced by [`Self::to_code`] itself) decodes to [`Self::Coordinate`],
    /// the stage a progress reader is safest defaulting to before the first real
    /// report arrives.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::BaselineFull,
            2 => Self::Polish,
            3 => Self::FinalFull,
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
/// the polish stage's own evaluations and the two fixed full-fidelity scorings,
/// neither of which used to report at all (CAD audit items 153/161).
///
/// Both fields default to `None` ([`SearchHooks::default`]) for a caller (e.g. a
/// test) that wants neither.
#[derive(Default)]
pub struct SearchHooks<'a> {
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
    pub on_progress: Option<&'a dyn Fn(usize, SearchStage)>,
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
    pub weights: ObjectiveWeights,
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
        }
    }
}

/// The inclusive evaluation budget across both search stages for a design with
/// `free_tier_count` tiers free to move.
///
/// [`OptimizeConfig::max_evaluations`] (the coordinate stage) plus the polish
/// stage's own cap, computed the same way [`optimize_design`] itself would (`0`
/// when the polish stage is disabled) -- see
/// [`OptimizeConfig::polish_max_evaluations`]'s own default.
///
/// CAD audit item 153: a caller reporting progress via [`SearchHooks::on_progress`]
/// needs this to show an honest "N of ~M evaluations" once the polish stage's own
/// evaluations are included in `N` -- `config.max_evaluations` alone silently
/// excluded the polish stage's budget, reading as the counter blowing past its own
/// stated maximum.
#[must_use]
pub fn inclusive_max_evaluations(config: &OptimizeConfig, free_tier_count: usize) -> usize {
    let polish_max = config
        .polish_start_step_deg
        .filter(|&step| step > 0.0)
        .map_or(0, |_| {
            config
                .polish_max_evaluations
                .unwrap_or(3 * free_tier_count + 20)
        });
    config.max_evaluations + polish_max
}

/// One accepted change [`optimize_design`] proposes: tier `index`'s angle moves from
/// `from_deg` to `to_deg`. Never applied by this module itself -- see
/// [`super::apply_optimize_outcome`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngleChange {
    pub index: usize,
    pub from_deg: f64,
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
    pub before: ObjectiveComponents,
    pub before_score: f32,
    pub after: ObjectiveComponents,
    pub after_score: f32,
    pub evaluations: usize,
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
/// (see [`free_tier_indices`]) to minimize [`super::objective::ObjectiveWeights::score`].
///
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
/// If [`free_tier_indices`] is empty, this returns immediately with
/// `evaluations: 0` and `before == after` -- `hooks.cancel` is never consulted,
/// though `hooks.on_progress` still receives [`baseline_report`]'s one
/// [`SearchStage::BaselineFull`] report, since that scoring happens regardless of
/// whether there turns out to be anything free to search over.
///
/// # Errors
///
/// Propagates [`Design::solve`]'s [`MissingAnchor`] if `design` itself (before any
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
/// [`build_outcome`] (measure the ending point and diff it against the start) to
/// stay under clippy's `too_many_lines`, with the starting-point measurements
/// bundled into one [`Baseline`] and the two stages' results bundled into
/// [`SearchRunSummary`] so the split does not trade "one long function" for
/// `too_many_arguments` -- no `#[allow]` added.
pub fn optimize_design(
    design: &Design,
    material: &GemMaterial,
    config: &OptimizeConfig,
    hooks: &SearchHooks<'_>,
) -> Result<OptimizeOutcome, MissingAnchor> {
    let baseline = baseline_report(design, material, &config.weights, hooks)?;

    if baseline.free.is_empty() {
        return Ok(OptimizeOutcome {
            before: baseline.before,
            before_score: baseline.before_score,
            after: baseline.before,
            after_score: baseline.before_score,
            evaluations: 0,
            changes: Vec::new(),
            cancelled: false,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        });
    }

    let ctx = SearchContext {
        material,
        weights: &config.weights,
        baseline_warnings: &baseline.warnings,
    };
    let coord = run_search(design, config, &ctx, &baseline, hooks);
    let coord_evaluations = coord.evaluations;
    let coord_cancelled = coord.cancelled;

    let polish = if coord_cancelled {
        PolishStageOutcome {
            current: coord.design,
            evaluations: 0,
            improvement: 0.0,
            cancelled: true,
        }
    } else {
        run_polish_stage(design, &baseline.free, config, &ctx, hooks, coord)
    };

    let summary = SearchRunSummary {
        evaluations: coord_evaluations + polish.evaluations,
        cancelled: coord_cancelled || polish.cancelled,
        polish_evaluations: polish.evaluations,
        polish_improvement: polish.improvement,
    };

    Ok(build_outcome(
        design,
        &polish.current,
        &ctx,
        &baseline,
        hooks,
        &summary,
    ))
}

/// [`optimize_design`]'s "measure the starting point" half, bundled into one
/// struct: the [`Design::solve`]'d, [`ObjectiveFidelity::Full`]-scored baseline,
/// the [`BaselineWarningCounts`] every candidate gets checked against, and the
/// free tier indices the search is allowed to move.
struct Baseline {
    before: ObjectiveComponents,
    before_score: f32,
    warnings: BaselineWarningCounts,
    free: Vec<usize>,
}

/// Builds [`optimize_design`]'s [`Baseline`].
///
/// Reports [`SearchStage::BaselineFull`] (evaluations `0`, since none of
/// `config`'s budget is spent here) through `hooks` before running the one
/// [`ObjectiveFidelity::Full`] scoring this function performs -- CAD audit item
/// 161: that scoring alone measures ~1.3s on a small real design (see the parent
/// module's "Cost first" doc section), and used to report nothing at all, reading
/// as a frozen counter before the search had even started.
///
/// # Errors
///
/// Propagates [`Design::solve`]'s [`MissingAnchor`].
fn baseline_report(
    design: &Design,
    material: &GemMaterial,
    weights: &ObjectiveWeights,
    hooks: &SearchHooks<'_>,
) -> Result<Baseline, MissingAnchor> {
    hooks.report(0, SearchStage::BaselineFull);
    let baseline_solved = design.solve()?;
    let baseline_planes = design.planes_from_solved(&baseline_solved);
    let warnings = BaselineWarningCounts::count(&check_manufacturability(
        design,
        &baseline_solved,
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    ));
    let before = evaluate_objective(
        &to_gpu_planes(&baseline_planes),
        material,
        ObjectiveFidelity::Full,
    );
    let before_score = weights.score(&before);
    let free = free_tier_indices(design);
    Ok(Baseline {
        before,
        before_score,
        warnings,
        free,
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
fn run_search(
    design: &Design,
    config: &OptimizeConfig,
    ctx: &SearchContext,
    baseline: &Baseline,
    hooks: &SearchHooks<'_>,
) -> CoordinateStageOutcome {
    let free = &baseline.free;
    let before_score = baseline.before_score;
    let mut current = design.clone();
    let mut current_score = before_score;
    let mut evaluations = 0usize;
    let mut step_deg = config.initial_step_deg;
    let mut sweep_index = 0u64;
    let mut cancelled = false;

    'search: while step_deg >= config.min_step_deg && evaluations < config.max_evaluations {
        let order = seeded_permutation(
            free.len(),
            config.seed ^ sweep_index.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );
        sweep_index += 1;
        let mut improved_this_sweep = false;

        for &free_slot in &order {
            if hooks.is_cancelled() {
                cancelled = true;
                break 'search;
            }
            if evaluations >= config.max_evaluations {
                break;
            }
            let tier_index = free[free_slot];
            let original_deg = current.tiers[tier_index].angle_deg;

            let (best, spent) = evaluate_candidate_pair(
                &current,
                tier_index,
                original_deg,
                step_deg,
                ctx,
                current_score,
            );
            evaluations += spent;
            hooks.report(evaluations, SearchStage::Coordinate);

            if let Some((new_deg, new_score)) = best {
                current.tiers[tier_index].angle_deg = new_deg;
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
    }
}

/// [`run_search`]'s return, bundled into one struct purely to keep
/// [`run_polish_stage`]'s own argument count under clippy's `too_many_arguments`
/// lint (see [`optimize_design`]'s "why this is more than one function" note): the
/// design and score the coordinate stage ended on, how many evaluations it spent,
/// and whether [`SearchHooks::cancel`] cut it short.
struct CoordinateStageOutcome {
    design: Design,
    score: f32,
    evaluations: usize,
    cancelled: bool,
}

/// [`optimize_design`]'s "best point in, best point out" half for the polish stage:
/// builds the Fast-fidelity scoring closure [`polish::run_polish`] needs (via
/// [`build_free_angle_candidate`]/[`evaluate_candidate`] -- the exact same
/// solve/close/manufacturability gate the coordinate stage's candidates go through)
/// and, if the polish stage's own result strictly improves on `current_score`,
/// applies it to `current`'s free tiers. Never touches a tier outside `free`.
///
/// Disabled ([`OptimizeConfig::polish_start_step_deg`] is `None` or non-positive)
/// short-circuits to a zero-evaluation, unchanged [`PolishStageOutcome`] before ever
/// building the closure or calling [`polish::run_polish`].
///
/// `coord`'s own `evaluations` is only for progress reporting (CAD audit item
/// 153): each call the `evaluate` closure below makes reports `hooks.report` with
/// [`SearchStage::Polish`] and a running total that STARTS from
/// `coord.evaluations` rather than from zero, so a caller's own evaluation
/// counter keeps climbing smoothly across the coordinate-to-polish hand-off
/// instead of resetting or (as it did before this reporting existed at all)
/// freezing for the whole polish stage. Only ever called with `coord.cancelled ==
/// false` -- see [`optimize_design`]'s own call site.
fn run_polish_stage(
    design: &Design,
    free: &[usize],
    config: &OptimizeConfig,
    ctx: &SearchContext,
    hooks: &SearchHooks<'_>,
    coord: CoordinateStageOutcome,
) -> PolishStageOutcome {
    let current = coord.design;
    let current_score = coord.score;
    let coord_evaluations = coord.evaluations;
    let Some(start_step) = config.polish_start_step_deg.filter(|&step| step > 0.0) else {
        return PolishStageOutcome {
            current,
            evaluations: 0,
            improvement: 0.0,
            cancelled: false,
        };
    };

    let starting_point: Vec<f64> = free.iter().map(|&i| current.tiers[i].angle_deg).collect();
    let max_evaluations = config
        .polish_max_evaluations
        .unwrap_or_else(|| 3 * free.len() + 20);
    let min_spread = config.min_step_deg / 2.0;

    let mut polish_evaluations_done = 0usize;
    let evaluate = |angles: &[f64]| -> f32 {
        let score = build_free_angle_candidate(design, free, &starting_point, angles).map_or(
            f32::INFINITY,
            |candidate| match evaluate_candidate(
                &candidate,
                ctx.material,
                ctx.weights,
                ctx.baseline_warnings,
            ) {
                CandidateOutcome::Rejected => f32::INFINITY,
                CandidateOutcome::Accepted { score } => score,
            },
        );
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

    if result.score < current_score {
        let mut improved = current;
        for (&tier_index, &angle) in free.iter().zip(&result.point) {
            improved.tiers[tier_index].angle_deg = angle;
        }
        PolishStageOutcome {
            current: improved,
            evaluations: result.evaluations,
            improvement: current_score - result.score,
            cancelled: result.cancelled,
        }
    } else {
        PolishStageOutcome {
            current,
            evaluations: result.evaluations,
            improvement: 0.0,
            cancelled: result.cancelled,
        }
    }
}

/// [`run_polish_stage`]'s return: the design it ended on (whether or not the polish
/// stage's own point was actually adopted -- its score is not carried separately,
/// since [`build_outcome`] always re-solves and re-scores `current` at
/// [`ObjectiveFidelity::Full`] regardless), how many evaluations the polish stage
/// spent, the score improvement adopted (`0.0` if none), and whether cancellation
/// cut it short.
struct PolishStageOutcome {
    current: Design,
    evaluations: usize,
    improvement: f32,
    cancelled: bool,
}

/// Everything about a finished (both-stage) search run that [`build_outcome`] needs
/// beyond the design it ended on -- bundled solely to keep that function's argument
/// count reasonable (see [`optimize_design`]'s own "why this is more than one
/// function" note).
struct SearchRunSummary {
    evaluations: usize,
    cancelled: bool,
    polish_evaluations: usize,
    polish_improvement: f32,
}

/// [`optimize_design`]'s "measure the ending point" half: re-solves `current` (see
/// that function's own `# Panics` section for why the `.expect()` inside this is
/// safe), scores it at [`ObjectiveFidelity::Full`], and diffs its tier angles against
/// `design`'s original ones to build the [`AngleChange`] list.
///
/// Reports [`SearchStage::FinalFull`] through `hooks` (evaluations
/// `summary.evaluations`, unchanged by this call -- same reasoning as
/// [`baseline_report`]'s own report) before running its own
/// [`ObjectiveFidelity::Full`] scoring -- CAD audit item 161's second bracketing
/// hang: this call is as expensive as the baseline's, and used to leave a
/// caller's progress ticker stuck on its last coordinate/polish reading through
/// the whole final scoring, reading as the run having already finished when it
/// had not.
fn build_outcome(
    design: &Design,
    current: &Design,
    ctx: &SearchContext,
    baseline: &Baseline,
    hooks: &SearchHooks<'_>,
    summary: &SearchRunSummary,
) -> OptimizeOutcome {
    hooks.report(summary.evaluations, SearchStage::FinalFull);
    let final_solved = current
        .solve()
        .expect("current was only ever advanced via evaluate_candidate-accepted, solvable states");
    let final_planes = current.planes_from_solved(&final_solved);
    let after = evaluate_objective(
        &to_gpu_planes(&final_planes),
        ctx.material,
        ObjectiveFidelity::Full,
    );
    let after_score = ctx.weights.score(&after);

    let changes: Vec<AngleChange> = design
        .tiers
        .iter()
        .zip(&current.tiers)
        .enumerate()
        .filter_map(|(index, (before_tier, after_tier))| {
            (before_tier.angle_deg != after_tier.angle_deg).then_some(AngleChange {
                index,
                from_deg: before_tier.angle_deg,
                to_deg: after_tier.angle_deg,
            })
        })
        .collect();

    OptimizeOutcome {
        before: baseline.before,
        before_score: baseline.before_score,
        after,
        after_score,
        evaluations: summary.evaluations,
        changes,
        cancelled: summary.cancelled,
        polish_evaluations: summary.polish_evaluations,
        polish_improvement: summary.polish_improvement,
    }
}
