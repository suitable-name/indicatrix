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
//! an honest "nothing to do here yet", not an error.
//!
//! A tier's `angle_deg` is the only field this module ever changes on a free tier --
//! never `indices`/`detached`/`name`/`constraint`. Since all of a
//! [`crate::design::ConstraintTier`]'s `indices` share one `angle_deg`, moving that
//! shared field moves every orbit member together by construction, keeping symmetry
//! intact without this module ever touching `crate::orbit`'s membership operations.
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
//! candidate is also rejected before solving if the proposed angle would cross zero
//! -- a measured hazard: a 0.5-degree edit crossing the girdle plane can remove a
//! block's own anchor by reclassifying the tier's crown/pavilion/girdle
//! [`indicatrix::geometry::meet_solver::Block`] entirely (see
//! [`candidate::candidate_angle_is_safe`]).
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
//! it never mutates a [`Design`] itself. [`apply_optimize_outcome`] turns that
//! outcome into real edits via [`crate::edit::History::apply`], once per changed
//! tier, so an applied optimization is undoable one tier at a time like any other
//! editor action (there is no batched `History` entry point to group them).
//!
//! # Determinism
//!
//! [`OptimizeConfig::seed`] is a required, explicit `u64`: the only
//! nondeterministic-looking part of a coordinate search is the order free tiers are
//! visited each sweep, and that comes from `search::seeded_permutation`, a
//! deterministic splitmix64-based Fisher-Yates shuffle. Identical
//! `(Design, OptimizeConfig)` input always produces an identical [`OptimizeOutcome`].
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
//! # Module layout
//!
//! [`objective`] (the optical score itself), [`candidate`] (per-candidate
//! solve/close/manufacturability gates, the `+step`/`-step` pair, and building a
//! candidate from a whole free-angle vector), [`search`] (the coordinate-descent
//! stage and its user-visible config/outcome types), [`polish`] (the Nelder-Mead
//! second stage, generic over the scoring function so it is testable without a real
//! [`crate::design::Design`]), and [`apply`] (turning an outcome into real `History`
//! edits).

mod apply;
mod candidate;
mod objective;
mod polish;
mod search;
#[cfg(test)]
mod tests;

pub use apply::apply_optimize_outcome;
pub use candidate::free_tier_indices;
pub use objective::{
    CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW, ObjectiveComponents, ObjectiveFidelity,
    ObjectiveWeights, evaluate_objective,
};
pub use search::{AngleChange, OptimizeConfig, OptimizeOutcome, SearchHooks, optimize_design};
