//! The last solve of the design and the rules around running one.
//!
//! # The flow
//!
//! 1. The front end asks [`begin_solve`] for a [`SolveJob`]: the design as it is now,
//!    stamped with the session's generation. `None` means the design is already solved
//!    (or failed) at that generation, or a job for it is running.
//! 2. It runs [`run_solve`] on this thread (a tiny design, see
//!    [`should_solve_synchronously`]) or on a worker thread with a cancel flag.
//! 3. It hands the answer to [`install`] on the UI thread. An answer for a generation the
//!    design has moved past is dropped.
//!
//! # Stale rows
//!
//! After an edit the tier list keeps the previous solve's masts for the tiers the edit
//! did not touch (marked stale) and blanks the touched ones, exactly as the desktop and
//! web tier tables do. [`SolveState::note_edit`] keeps that dirty set: it is exact while
//! every edit goes through [`crate::design_state`]'s edit functions, and any edit it did
//! not see (another lane's drag, a file replacing the design) makes every tier dirty.

use std::{collections::BTreeSet, sync::atomic::AtomicBool, time::Duration};

use indicatrix::geometry::{GpuFacetPlane, meet_solver::SolvedTier};
use indicatrix_cut_core::Design;
use indicatrix_editor::solve_policy::{
    AUTO_SOLVE_DEBOUNCE, SolveCostEstimate, should_schedule_auto_solve,
    should_solve_synchronously_for,
};
use indicatrix_web_core::solve::{
    SolveOutcome, SolveResponse, TierWarning, solve_design, to_solved_tiers,
};

use crate::app_state::AppState;

/// A solve that succeeded (the solid may still not close; `status` and `problem` say so).
#[derive(Debug, Clone)]
pub struct SolvedSnapshot {
    /// `EditorSession::current_generation` when the solve started.
    pub generation: u64,
    /// One entry per tier, in tier order.
    pub tiers: Vec<SolvedTier>,
    /// The solid-status line (`status_text_and_is_problem_from_solved`).
    pub status: String,
    /// Whether `status` describes a problem.
    pub problem: bool,
    /// Wall time of the solve.
    pub took: Duration,
    /// The viewport planes built from `tiers`.
    pub planes: Vec<GpuFacetPlane>,
    /// The tier-tagged manufacturability warnings of this solve.
    pub warnings: Vec<TierWarning>,
}

/// A solve that did not produce masts (a missing scale anchor, an unresolvable target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedSolve {
    /// `EditorSession::current_generation` when the solve started.
    pub generation: u64,
    /// The solver's own message.
    pub message: String,
}

/// Which tiers an edit made untrustworthy, for the rows' stale masts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyScope {
    /// Every tier (undo, redo, move, remove: their blast radius is not tracked).
    All,
    /// Just these tiers.
    Tiers(BTreeSet<usize>),
}

impl DirtyScope {
    /// Just tier `index`.
    #[must_use]
    pub fn one(index: usize) -> Self {
        Self::Tiers(BTreeSet::from([index]))
    }
}

/// The tiers whose last solved mast the edits since that solve made untrustworthy, valid
/// for one design generation.
#[derive(Debug, Clone, Default)]
struct DirtyTiers {
    /// The generation these sets describe; a different current generation means an edit
    /// that was not reported, so everything is dirty.
    generation: u64,
    all: bool,
    tiers: BTreeSet<usize>,
}

/// The last solve of the current design. See the module docs.
#[derive(Debug, Default)]
pub struct SolveState {
    solved: Option<SolvedSnapshot>,
    failed: Option<FailedSolve>,
    /// The generation of the job that is running, if any.
    in_flight: Option<u64>,
    /// How long the newest finished solve took, successful or not.
    last_took: Option<Duration>,
    dirty: DirtyTiers,
    /// Bumped by every change a front end may need to re-push; a cache key.
    revision: u64,
}

impl SolveState {
    /// The newest successful solve, whatever generation it was for.
    #[must_use]
    pub const fn solved(&self) -> Option<&SolvedSnapshot> {
        self.solved.as_ref()
    }

    /// The solved tiers, only if the solve is of `generation`.
    #[must_use]
    pub fn current_tiers(&self, generation: u64) -> Option<&[SolvedTier]> {
        self.solved
            .as_ref()
            .filter(|solved| solved.generation == generation)
            .map(|solved| solved.tiers.as_slice())
    }

    /// The tiers of the newest successful solve, any generation (for stale display).
    #[must_use]
    pub fn last_tiers(&self) -> Option<&[SolvedTier]> {
        self.solved.as_ref().map(|solved| solved.tiers.as_slice())
    }

    /// The viewport planes, only if the solve is of `generation`.
    #[must_use]
    pub fn current_planes(&self, generation: u64) -> Option<&[GpuFacetPlane]> {
        self.solved
            .as_ref()
            .filter(|solved| solved.generation == generation)
            .map(|solved| solved.planes.as_slice())
    }

    /// The manufacturability warnings, only if the solve is of `generation`.
    #[must_use]
    pub fn current_warnings(&self, generation: u64) -> &[TierWarning] {
        self.solved
            .as_ref()
            .filter(|solved| solved.generation == generation)
            .map_or(&[], |solved| solved.warnings.as_slice())
    }

    /// Why the solve of `generation` failed, if it did.
    #[must_use]
    pub fn failure(&self, generation: u64) -> Option<&str> {
        self.failed
            .as_ref()
            .filter(|failed| failed.generation == generation)
            .map(|failed| failed.message.as_str())
    }

    /// Whether there is an answer (masts or a failure) for `generation`.
    #[must_use]
    pub fn is_current(&self, generation: u64) -> bool {
        self.current_tiers(generation).is_some() || self.failure(generation).is_some()
    }

    /// The status line of the newest result, whatever generation it is for: "Solving..."
    /// while a job runs, otherwise the newest solve's line and whether it is a problem.
    /// A front end that knows the design's generation uses
    /// [`crate::design_state::status_line`] instead, which also says "stale".
    #[must_use]
    pub fn status(&self) -> (String, bool) {
        if self.in_flight.is_some() {
            return ("Solving...".to_owned(), false);
        }
        let newer_failure = self.failed.as_ref().filter(|failed| {
            self.solved
                .as_ref()
                .is_none_or(|solved| failed.generation >= solved.generation)
        });
        if let Some(failed) = newer_failure {
            return (failed.message.clone(), true);
        }
        self.solved.as_ref().map_or_else(
            || ("Not solved yet.".to_owned(), false),
            |solved| (solved.status.clone(), solved.problem),
        )
    }

    /// Whether a solve job is running.
    #[must_use]
    pub const fn is_solving(&self) -> bool {
        self.in_flight.is_some()
    }

    /// The generation of the running job.
    #[must_use]
    pub const fn in_flight_generation(&self) -> Option<u64> {
        self.in_flight
    }

    /// How long the newest finished solve took.
    #[must_use]
    pub const fn last_took(&self) -> Option<Duration> {
        self.last_took
    }

    /// A counter that changes whenever anything a front end shows from this state changes.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Forgets every result (a different design replaced the old one). The running job,
    /// if any, is forgotten too: its answer will not match the new design's generation.
    pub fn invalidate(&mut self) {
        *self = Self {
            revision: self.revision.wrapping_add(1),
            ..Self::default()
        };
    }

    /// Records that the design moved from `before` to `after` by an edit that made `scope`
    /// untrustworthy. An edit chain with a gap (the dirty set was not valid at `before`)
    /// makes every tier dirty.
    pub fn note_edit(&mut self, before: u64, after: u64, scope: DirtyScope) {
        if self.dirty.generation != before {
            self.dirty.all = true;
        }
        match scope {
            DirtyScope::All => self.dirty.all = true,
            DirtyScope::Tiers(tiers) => self.dirty.tiers.extend(tiers),
        }
        self.dirty.generation = after;
        self.revision = self.revision.wrapping_add(1);
    }

    /// The tiers (of `tier_count`) whose solved mast cannot be trusted at `generation`.
    #[must_use]
    pub fn dirty_tiers(&self, generation: u64, tier_count: usize) -> BTreeSet<usize> {
        if self.dirty.all || self.dirty.generation != generation {
            (0..tier_count).collect()
        } else {
            self.dirty.tiers.clone()
        }
    }
}

/// A design to solve, stamped with the generation it was taken at.
#[derive(Debug, Clone)]
pub struct SolveJob {
    /// The session generation the design was cloned at.
    pub generation: u64,
    /// A copy of the design, so the solve runs without borrowing the state.
    pub design: Design,
}

/// What [`install`] did with an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// The state holds the answer now (masts, or a failure for that generation).
    Applied,
    /// The design moved on while the job ran; the answer was dropped.
    Stale,
    /// The job was cancelled; nothing changed except that it is no longer running.
    Cancelled,
}

/// Starts a solve of the current design: marks the job as running and returns it.
///
/// `None` when the design already has an answer at this generation, or a job for this
/// generation is running. A job for an older generation does not block a new one (the
/// caller cancels the old job's flag).
pub fn begin_solve(state: &mut AppState) -> Option<SolveJob> {
    let generation = state.session.current_generation();
    if state.solve.is_current(generation) || state.solve.in_flight == Some(generation) {
        return None;
    }
    state.solve.in_flight = Some(generation);
    state.solve.revision = state.solve.revision.wrapping_add(1);
    Some(SolveJob {
        generation,
        design: state.session.design.clone(),
    })
}

/// Solves `job`'s design. Thread-safe and free of state; `cancel` is polled by the solver
/// and a set flag gives [`SolveResponse::Cancelled`].
#[must_use]
pub fn run_solve(job: &SolveJob, cancel: &AtomicBool) -> SolveResponse {
    solve_design(&job.design, cancel)
}

/// Installs the answer to the job of `generation`. See [`Installed`].
pub fn install(
    state: &mut AppState,
    generation: u64,
    response: SolveResponse,
    took: Duration,
) -> Installed {
    if state.solve.in_flight == Some(generation) {
        state.solve.in_flight = None;
        state.solve.revision = state.solve.revision.wrapping_add(1);
    }
    if matches!(response, SolveResponse::Cancelled) {
        return Installed::Cancelled;
    }
    if generation != state.session.current_generation() {
        return Installed::Stale;
    }
    match response {
        SolveResponse::Solved(outcome) => {
            install_outcome(&mut state.solve, generation, outcome, took);
        }
        SolveResponse::InvalidDesign { message } => {
            fail(
                &mut state.solve,
                generation,
                format!("The solver could not read the design: {message}"),
                took,
            );
        }
        _ => fail(
            &mut state.solve,
            generation,
            "The solver answered a different request.".to_owned(),
            took,
        ),
    }
    Installed::Applied
}

/// Records that the job of `generation` ended without an answer.
///
/// The solver thread panicked: the design then reads as failed with `message`, instead of
/// "solving" forever. Like [`install`], an answer for a generation the design left is dropped.
pub fn install_failure(
    state: &mut AppState,
    generation: u64,
    message: String,
    took: Duration,
) -> Installed {
    if state.solve.in_flight == Some(generation) {
        state.solve.in_flight = None;
        state.solve.revision = state.solve.revision.wrapping_add(1);
    }
    if generation != state.session.current_generation() {
        return Installed::Stale;
    }
    fail(&mut state.solve, generation, message, took);
    Installed::Applied
}

/// Stores a finished [`SolveOutcome`].
fn install_outcome(solve: &mut SolveState, generation: u64, outcome: SolveOutcome, took: Duration) {
    let SolveOutcome {
        solved,
        error,
        status_text,
        status_is_problem,
        planes,
        warnings,
        ..
    } = outcome;
    let Some(solved) = solved else {
        let message = error.unwrap_or(status_text);
        fail(solve, generation, message, took);
        return;
    };
    solve.solved = Some(SolvedSnapshot {
        generation,
        tiers: to_solved_tiers(&solved),
        status: status_text,
        problem: status_is_problem,
        took,
        planes: planes.into_iter().map(GpuFacetPlane::from).collect(),
        warnings,
    });
    solve.failed = None;
    solve.last_took = Some(took);
    solve.dirty = DirtyTiers {
        generation,
        all: false,
        tiers: BTreeSet::new(),
    };
    solve.revision = solve.revision.wrapping_add(1);
}

/// Records a failed solve of `generation`.
fn fail(solve: &mut SolveState, generation: u64, message: String, took: Duration) {
    solve.failed = Some(FailedSolve {
        generation,
        message,
    });
    solve.last_took = Some(took);
    solve.revision = solve.revision.wrapping_add(1);
}

/// Whether the current design is small enough to solve on the UI thread (the shared
/// policy: few planes, few meet-derived tiers, no tier targets, and a last solve under
/// half a second).
#[must_use]
pub fn should_solve_synchronously(state: &AppState) -> bool {
    should_solve_synchronously_for(
        SolveCostEstimate::of(&state.session.design),
        state.solve.last_took,
    )
}

/// After an edit: how long to wait before solving automatically.
///
/// `None` when no automatic solve should run (auto-solve is off, the design already has
/// an answer, or its last solve took longer than the budget).
#[must_use]
pub fn should_auto_solve(state: &AppState) -> Option<Duration> {
    let budget_ms = state.settings.auto_solve_budget_ms;
    if budget_ms == 0 || state.solve.is_current(state.session.current_generation()) {
        return None;
    }
    let budget = Duration::from_millis(u64::from(budget_ms));
    (should_solve_synchronously(state) || should_schedule_auto_solve(state.solve.last_took, budget))
        .then_some(AUTO_SOLVE_DEBOUNCE)
}
