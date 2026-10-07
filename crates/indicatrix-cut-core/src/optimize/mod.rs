//! Optimizing a [`crate::design::Design`]'s authored facet *angles* against the optical
//! metrics `indicatrix::color::metrics` already measures.
//!
//! Windowing, extinction, and the tilt-performance sweep, instead of only reporting
//! them after the fact.
//!
//! # Cost first (measured, not assumed)
//!
//! Every candidate needs a real, solved geometry (see [`evaluate_objective`]'s doc
//! comment), so every evaluation pays for a [`crate::design::Design::solve`] --
//! never [`crate::design::Design::resolve_dirty`], which buys nothing once a design
//! has more than one non-`ScaleReference` tier (see [`crate::resolve`]'s module doc
//! comment), exactly the situation this optimizer's free-variable set creates.
//!
//! [`cost_probe`] (`#[cfg(test)]`, `#[ignore]`d: `cargo test -p indicatrix-cut-core
//! --release --lib cost_probe -- --ignored --nocapture`) measures one full objective
//! evaluation end to end. Measured numbers (release, single-threaded, this machine):
//!
//! | fixture | tiers | solve | Fast metrics | Full metrics (724 samples) |
//! |---|---|---|---|---|
//! | RBC-445 | 12 | 5.1 ms | 2.2 ms | 1.281 s |
//! | CrackOtto-Step | 103 | 6.200 s | 0.11 ms | 145.9 ms |
//!
//! CrackOtto-Step's metrics cost is an artifact of the cost-probe's arbitrary,
//! un-calibrated fixture (an under-filled camera frustum means cheap early ray
//! misses) -- RBC-445's 1.281 s is the trustworthy figure. Net effect at
//! [`ObjectiveFidelity::Fast`] (what [`optimize_design`]'s search loop actually
//! spends its budget on): one evaluation is solve-dominated the moment a design has
//! real meet-derived structure -- **~7 ms/eval on a small design like RBC-445 (200
//! evaluations in ~1.4 s, interactive), versus ~6.2 s/eval on a large, heavily
//! meet-derived design like CrackOtto-Step (200 evaluations would be ~20 minutes,
//! entirely re-solving)**. There is no cheap incremental resolve to fall back on
//! once tiers are genuinely free.
//!
//! Asking for several candidates ([`OptimizeOptions::keep_candidates`]) adds one more
//! `Full`-fidelity scoring per extra candidate (the last column above: 1.281 s on
//! RBC-445), once, at the end; the search loop itself pays only the bookkeeping of
//! its pool. The girdle guard ([`OptimizeOptions::min_girdle_fraction`]) adds one
//! `measure_solid` per candidate that closes, plus a plane-boundary pass and one walk
//! over the girdle walls' rings (the thinnest-point measure). Neither of those two
//! extras was measured separately.
//!
//! # Which tiers are free (never moves a pinned facet)
//!
//! [`free_tier_indices`] returns every tier index whose current constraint is *not*
//! [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`] -- a
//! `ScaleReference` tier's mast is never solver-derived, so its plane should never
//! be an optimizer-derived guess either. Import pins every tier to `ScaleReference`
//! (see [`crate::design::Design::from_asc_schedule`]), so a design fresh off the
//! catalogue has **zero** free tiers until the user adopts one back to its real meet
//! constraint via [`crate::edit::Edit::SetConstraint`]; this optimizer cannot
//! second-guess which imported tiers are adoptable meet structure versus genuinely
//! authored scale references, so [`optimize_design`] on a freshly-imported design
//! returns an [`OptimizeOutcome`] with zero evaluations and an unchanged score --
//! an honest "nothing to do here yet", not an error. (A request can opt in to the
//! other answer, see "Varying anchored tiers" below.)
//!
//! Two further exclusions apply to every tier, whatever its constraint. A
//! **horizontal** tier (`|angle| < 1e-9`: the table at `+0.0`, the culet at `-0.0`)
//! has no angle to optimize, and a step off zero would put the facet on the wrong
//! side of the girdle. A **vertical** tier (`|angle| >= 89.5` degrees, a real girdle
//! facet at `-90.0`) can never pass the safety gate in either direction, and one such
//! tier in the free set would make the polish stage reject every point it proposes.
//! See [`free_tier_indices`]'s own doc comment.
//!
//! A tier's `angle_deg` is the only field this module ever changes on a free tier --
//! never `indices`/`detached`/`name` -- and, for an anchored tier only, the mast inside
//! its `ScaleReference` constraint, which follows the angle (see below). Since all of a
//! [`crate::design::ConstraintTier`]'s `indices` share one `angle_deg`, moving that
//! shared field moves every orbit member together by construction, keeping symmetry
//! intact without this module ever touching `crate::orbit`'s membership operations.
//!
//! # Varying anchored tiers (opt-in)
//!
//! With [`OptimizeOptions::vary_anchored`] on, a `ScaleReference` tier that has a
//! *hinge* in [`OptimizeOptions::anchor_hinges`] is free too, which is what makes an
//! imported design (every tier pinned) searchable at all. The hinge is the point of the
//! tier's first facet that stays put when the facet turns, normally a vertex it shares
//! with a neighbour; the caller supplies the points (the optimizer does not derive
//! them). Whenever such a tier's angle changes its mast is recomputed so the plane
//! still passes through the hinge: `mast = n . hinge`, with `n` the facet normal
//! `(sin(theta) cos(phi), +-cos(theta), sin(theta) sin(phi))` built exactly as the
//! schedule path builds it (`phi` from the tier's first index and the gear reference
//! angle; the tier's cheater offset included). The changed masts come back as
//! [`MastChange`]s next to the [`AngleChange`]s. A tier with a tier target is never
//! varied this way: the solver takes its mast from the target. Every `ScaleReference`
//! tier stays a `ScaleReference` tier, so the block anchors the solver needs are
//! never lost.
//!
//! # Bounds, guards and candidates
//!
//! [`OptimizeOptions::angle_bounds`] confines a tier to a signed angle range: a
//! coordinate step that overshoots is clamped to the range edge (as long as that still
//! moves the tier), and a polish point outside it is rejected. With
//! [`OptimizeOptions::min_girdle_fraction`] set, a candidate whose girdle band vanishes
//! or thins below that fraction of the starting band is rejected, and so is one whose
//! band runs to a knife edge anywhere around the outline (the thinnest point is measured
//! too, see [`crate::design::girdle_band_in`]: a facet turned about its girdle edge keeps
//! the extreme vertices the overall thickness is read from, so that figure alone cannot
//! see a corner pinch out), or that loses a girdle wall or the table facet the starting
//! design had (found through the table tier's own plane, not the solid's table
//! percentage). The thinnest point takes the same fraction unless
//! [`OptimizeOptions::min_girdle_thinnest_fraction`] gives it one of its own.
//!
//! [`OptimizeOptions::keep_candidates`] keeps a bounded list of the best *distinct*
//! states the search saw (some free angle differing by at least
//! [`OptimizeConfig::min_step_deg`], or [`OptimizeOptions::candidate_separation_deg`]).
//! At the end each is re-scored at full fidelity and returned, best first, as
//! [`OptimizeResult::candidates`]; the plain [`OptimizeOutcome`] describes the first.
//! [`OptimizeOptions::shape_target`] keeps the stone's look: every score, the starting
//! design's included, adds a penalty for drifting from a table size and a
//! crown-to-pavilion ratio ([`ShapeTarget`]); `None` changes nothing.
//! Named objective weightings for a front end are [`ObjectivePreset`].
//!
//! # Face-up tone
//!
//! The objective has a fifth, optional term: the colour the stone shows face-up
//! ([`FaceUpTone`]: CIELAB `L*`, chroma, hue and an sRGB swatch of the returned light),
//! with a goal ([`ToneGoal`]: lighter or deeper) at [`ObjectiveWeights::tone_weight`].
//! Weight `0` (the default) measures nothing in the search loop and every result stays
//! bit for bit what it was. Cost: about +0.2 ms per weighted `Fast` evaluation and about
//! +2.4 ms per `Full` scoring. Every `Full` scoring (the start, the end point, each
//! alternative) measures and reports the tone whether it is weighted or not.
//!
//! The tone is the table-up pose, under [`OptimizeConfig::lighting`]'s own light and
//! white point (a UV lamp has no visible white and falls back to D65). The stone size
//! enters through [`indicatrix::optics::materials::GemMaterial::absorption_path_scale`]
//! (the caller sizes the material, see the editor's `sized_material_for_optimize`). A
//! head-shadow setting would enter only the Returned classification of the rays.
//! [`OptimizeResult::lighting`] records the preset a run scored under.
//!
//! # Where the compute runs, and why not remote
//!
//! The search loop is plain, synchronous Rust with no threading of its own, so
//! `indicatrix-cut` wraps it in a cancellable background thread like
//! `gui::editor::deep_solve`. Its remote-worker infrastructure can't help: a worker
//! only knows already-solved plane geometry, not `Design`/`Design::solve`
//! (`indicatrix-cut-core` is intentionally not a dependency of it), and solving --
//! not the optics metrics a worker can already compute -- is the dominant cost, so
//! dispatching candidates remotely would never reach the expensive part.
//!
//! **Local parallelism** IS reusable: a coordinate search's two candidates per free
//! tier per sweep (`angle + step`, `angle - step`) are independent `Design::solve`
//! calls with no shared state, so [`optimize_design`] evaluates the pair
//! concurrently via `std::thread::scope` (see
//! [`candidate::evaluate_candidate_pair`]), roughly halving each tier's decision
//! cost on two free cores -- the only parallelism available, since sweep N+1's
//! starting point depends on which of sweep N's candidates were accepted. Moving it
//! into `meet_solver`'s own phase-3 scan instead was tried and reverted: flat at 2-4
//! threads, a regression at `available_parallelism()` (see the NOTE in
//! `indicatrix::geometry::meet_solver::candidates`'s module doc comment).
//!
//! # Never proposes geometry that fails to close, or moves a tier past its own block boundary
//!
//! Every candidate is solved and meshed before it is scored; one that fails to
//! solve or does not close into [`indicatrix::geometry::stone_metrics::SolidStatus::Closed`]
//! is rejected outright and never scored (see [`candidate::evaluate_candidate`]). A
//! candidate is also rejected before solving if the proposed angle would change sign
//! or land on zero, for every tier alike -- a measured hazard: a 0.5-degree edit
//! crossing the girdle plane can remove a block's own anchor by reclassifying the
//! tier's crown/pavilion/girdle [`indicatrix::geometry::meet_solver::Block`] entirely
//! (see [`candidate::candidate_angle_is_safe`]).
//!
//! # Manufacturability must not regress
//!
//! An accepted candidate's [`crate::manufacturability::check_manufacturability`]
//! warning counts for the two mesh-dependent checks (vanishing/undersized facets --
//! the only two an angle-only edit can change) must not exceed the running
//! baseline's own counts. See `candidate::manufacturability_regressed`.
//!
//! # `History` remains the sole mutator
//!
//! [`optimize_design`] takes a `&Design` and returns a plain, inert
//! [`OptimizeOutcome`] describing which tiers it would change and to what angle --
//! it never mutates a [`Design`] itself, and neither does [`optimize_design_with`].
//! [`apply_optimize_outcome`] turns an outcome into one [`crate::edit::Edit::Batch`]
//! of `ModifyTier` sub-edits applied via a single [`crate::edit::History::apply`]
//! call, so an applied optimization is undoable as ONE step regardless of how many
//! tiers it touched. [`apply_optimize_result`] and [`apply_optimize_candidate`] do the
//! same for a result with mast changes, or for one of its ranked candidates: angles and
//! masts of one tier travel in the same `ModifyTier`, the stale guard compares both
//! bit for bit, and one undo restores the design exactly.
//!
//! # Determinism
//!
//! [`OptimizeConfig::seed`] is a required, explicit `u64`: the only
//! nondeterministic-looking part of a coordinate search is the order free tiers are
//! visited each sweep, and that comes from `search::seeded_permutation`, a
//! deterministic splitmix64-based Fisher-Yates shuffle. Identical
//! `(Design, OptimizeConfig, OptimizeOptions)` input always produces an identical
//! [`OptimizeResult`], candidates and mast changes included.
//!
//! # Why the new knobs are not fields of `OptimizeConfig`
//!
//! `OptimizeConfig` is `Copy` and is stored in `indicatrix-editor`'s `RetargetMode`
//! (also `Copy`), and the web crates build [`OptimizeOutcome`] by full struct literal;
//! the extended request and answer therefore live in their own types
//! ([`OptimizeOptions`], [`OptimizeResult`]) and the old types and functions are
//! unchanged. See [`options`]'s module docs.
//!
//! # Coordinate descent stalls on diagonal ridges
//!
//! [`search`]'s coordinate descent tries one free tier's angle at a time; optical
//! objectives (windowing, extinction, tilt brilliance) have ridges where paired
//! angles (e.g. a crown break and a pavilion main) must move together to hold a
//! critical-angle relation, and axis-aligned moves stall there. [`polish`] is the
//! second stage that picks up once the coordinate stage's own step has shrunk small
//! enough that further halving is exactly this stalling behavior: a deterministic
//! Nelder-Mead simplex search over the whole free-angle vector at once, which can
//! move along a ridge no single axis move can climb. See [`search::optimize_design`]'s
//! own doc comment ("Two stages") and [`polish::run_polish`]'s for the algorithm and
//! knobs ([`search::OptimizeConfig::polish_start_step_deg`]/
//! [`search::OptimizeConfig::polish_max_evaluations`]).
//!
//! # Several starts
//!
//! The objective the search descends on is a quantised landscape of cliffs (critical
//! angles, lit/unlit exits) and ridges, so one coordinate descent stalls on the first
//! plateau. With [`OptimizeConfig::starts`] above one, [`optimize_design_with`] runs
//! several descents and keeps the best:
//!
//! 1. **Screen.** Draw `4 * (starts - 1)` points (at most 64) from the search box (each
//!    free tier's bounds, or +-5 degrees around its angle, see
//!    `SearchSpace::start_box`) with a scrambled Halton sequence; every third
//!    draw is local (30 % of the box around the design). Each draw is one `Fast`
//!    evaluation, reported as [`SearchStage::Screening`]. The best distinct draws become
//!    the extra starts; the design itself is always start 0 and sweeps with the run's own
//!    seed.
//! 2. **Descend.** Each start gets `max_evaluations / starts` evaluations and its own
//!    sweep seed. Descents that stop early on a plateau leave budget behind; while at
//!    least `8 * free` evaluations are left the next best screened draws run in another
//!    wave, until a whole wave improves nothing.
//! 3. **Polish** the best `max(1, keep_candidates)` starts (not all: polishing every
//!    start would cost more than the screening and never reach the candidate list).
//! 4. **Merge** every start's candidate pool in start order and finish exactly as the
//!    single-start search does (the `Full` scorings and the `<= before` gate).
//!
//! Cost rule: [`effective_starts`] caps the starts at `max_evaluations / (8 * free)` so
//! each gets four sweeps; a large, slow design therefore degrades to one start on its
//! own, and `starts <= 1` is bit for bit the single-start search.
//!
//! Determinism: per-start budgets, seeds and start points are fixed before a wave
//! begins, results are collected by start index and merged in that order, so the result
//! is the same for any [`OptimizeConfig::max_lanes`]. Lanes are `std::thread::scope`
//! threads (the calling thread runs lane 0 and forwards progress; each lane's own
//! candidate pair still uses two solve threads); `wasm32` runs the starts in sequence.
//! A cancelled run returns the merged best so far, as today. [`SearchHooks::on_start`]
//! tells the caller which start its thread's lane is on; [`OptimizeResult::starts_run`]
//! and [`OptimizeResult::best_start`] say what ran and what won.
//! [`inclusive_max_evaluations_for`] extends the honest evaluation total with the
//! screening draws and one polish budget per polished start.
//!
//! # Module layout
//!
//! [`objective`] (the optical score itself), [`candidate`] (per-candidate
//! solve/close/guard/manufacturability gates, the `+step`/`-step` pair, and building a
//! candidate from a whole free-angle vector), [`search`] (the coordinate-descent
//! stage and its user-visible config/outcome types), [`polish`] (the Nelder-Mead
//! second stage, generic over the scoring function so it is testable without a real
//! [`crate::design::Design`]), [`options`] (the extended request and result types),
//! [`space`] (angle bounds and the hinge geometry), [`guard`] (the girdle/table
//! guard), [`pool`] (the bounded list of distinct candidates), `multistart` (the generic
//! multi-start driver; the design-specific engine is `search::multi`), [`preset`] (named
//! objective weightings), and [`apply`] (turning an outcome into real `History`
//! edits).

mod apply;
mod candidate;
mod guard;
mod multistart;
mod objective;
mod options;
mod polish;
mod pool;
mod preset;
mod search;
mod space;
#[cfg(test)]
mod tests;

pub use apply::{apply_optimize_candidate, apply_optimize_outcome, apply_optimize_result};
pub use candidate::{
    FinishedScore, MAX_SAFE_CANDIDATE_ANGLE_DEG, angle_is_variable, free_tier_indices,
    free_tier_indices_with, score_finished_design,
};
pub use indicatrix::color::metrics::FaceUpTone;
pub use objective::{
    CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW, CANONICAL_LIGHTING_PRESET, ObjectiveComponents,
    ObjectiveFidelity, ObjectiveWeights, ToneGoal, evaluate_objective, evaluate_objective_under,
    evaluate_objective_with_tone, evaluate_objective_with_tone_under,
};
pub use options::{MastChange, OptimizeCandidate, OptimizeOptions, OptimizeResult, ShapeTarget};
pub use preset::ObjectivePreset;
pub use search::{
    AngleChange, OptimizeConfig, OptimizeOutcome, SearchHooks, SearchStage, StartProgress,
    effective_starts, inclusive_max_evaluations, inclusive_max_evaluations_for, optimize_design,
    optimize_design_with,
};
