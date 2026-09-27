//! Group 1: resolves a design's write-time solve from a matching cached solve, else a
//! background solve on this module's own [`SolveService`] worker -- never inline on
//! the UI thread. [`resolve_solved_then`] is the shared entry point every
//! write/export/autosave/open path in [`super`] uses.

use crate::MainWindow;
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use slint::ComponentHandle;
use std::{cell::RefCell, sync::Arc};

// This module uses a `SolveService` worker -- see `resolve_solved_then`'s own doc
// comment, below.
use crate::gui::editor::solve_service::{SolveKind, SolveOutcome, SolveRequest, SolveService};

// =============================================================================
// Shared infrastructure for write/export/autosave/open paths. Three groups of
// operations each get one shared entry point below:
// 1. Group 1: no inline solves (use cached solve or background solve)
// 2. Group 2: in-window confirmations
// 3. Group 3: file pickers off the UI thread
// =============================================================================

// --- Group 1: a matching cached solve, else a background solve, never inline ---

/// Pure decision half of the cached-solve reuse this group's write paths use in
/// place of a fresh `Design::solve()` on the UI thread: `Some` iff `cached` has one
/// entry per `design.tiers` -- the same alignment contract
/// [`Design::to_asc_schedule_from_solved_with`]/[`Design::planes_from_solved`]
/// document on their own `solved` parameter. Split out from
/// [`cached_solve_matching`] purely so a test can drive it with a plain fixture,
/// with no `auto_solve` runtime involved.
///
/// This is a conservative PROXY for "describes the design as it is right now," not
/// a true generation match: an edit that changes a tier's angle/index without
/// changing the tier COUNT (an ordinary angle nudge, say) leaves a stale-but-
/// same-shaped cached solve looking usable. `auto_solve::solid_last_solved`'s own
/// cache (`crate::gui::editor::auto_solve::solid_last_solved`) carries no generation tag of its
/// own to check against (it is set once per completed background solve and never
/// cleared on an edit -- see that function's own doc comment) -- closing this gap
/// would need `SolidLastSolved` itself, or a paired generation counter, to carry
/// one, which is `auto_solve.rs`'s own type, defined outside this module. Every
/// call site below still treats "no usable cache" the same as "cache empty": it
/// submits a real, off-UI-thread solve
/// rather than trusting a possibly-stale one, so the exposure here is narrower than
/// it sounds -- a false "still matches" only survives until the next background
/// solve completes for this same tier count.
#[must_use]
pub(super) fn solve_matches_design(
    cached: Option<&[SolvedTier]>,
    design: &Design,
) -> Option<Vec<SolvedTier>> {
    let cached = cached?;
    (cached.len() == design.tiers.len()).then(|| cached.to_vec())
}

/// Reads this module's own cached last-completed background solve and returns it
/// only when [`solve_matches_design`] accepts it against `design`. See that
/// function's own doc comment for what "matches" does and does not guarantee.
fn cached_solve_matching(design: &Design) -> Option<Vec<SolvedTier>> {
    let cache = crate::gui::editor::auto_solve::solid_last_solved()?;
    let guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    solve_matches_design(guard.as_deref(), design)
}

/// A stashed [`resolve_solved_then`] continuation -- named purely to keep
/// [`PENDING_SOLVES`]'s own type under clippy's `type_complexity` lint.
type SolveContinuation = Box<dyn FnOnce(&MainWindow, Result<Vec<SolvedTier>, String>)>;

thread_local! {
    /// This module's own persistent [`SolveService`] worker -- created lazily on
    /// first use ([`ensure_solve_service`]). `setup_editor_callbacks` does not own
    /// one to share with this module, and every write/export/autosave site here
    /// needs the same one worker, so this module owns it directly. One worker for
    /// the life of the window, the same reasoning [`AUTOSAVE_TIMER`] documents on
    /// itself.
    static SOLVE_SERVICE: RefCell<Option<SolveService>> = const { RefCell::new(None) };
    /// Continuations for a request submitted to [`SOLVE_SERVICE`], keyed by the
    /// `generation` `SolveRequest`/`SolveResult` echo back -- reused purely as an
    /// opaque continuation key (see `SolveRequest::generation`'s own doc comment:
    /// "opaque to this module"), since none of this module's callers have a real
    /// domain generation counter to compare a background solve against (that
    /// belongs to `EditorState`, in `state/mod.rs`).
    static PENDING_SOLVES: RefCell<std::collections::HashMap<u64, SolveContinuation>> =
        RefCell::new(std::collections::HashMap::new());
    static NEXT_SOLVE_KEY: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Creates [`SOLVE_SERVICE`]'s worker the first time this module ever needs one.
fn ensure_solve_service(ui: &MainWindow) {
    SOLVE_SERVICE.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }
        let service = SolveService::new(
            ui.as_weak(),
            // No progress UI is wired to this module's own write/export/autosave
            // paths yet -- a future change can render `SolveProgressReport` the
            // same way `deep_solve`'s own status line does.
            |_ui, _progress| {},
            |ui, result| {
                let Some(on_done) =
                    PENDING_SOLVES.with(|c| c.borrow_mut().remove(&result.generation))
                else {
                    // Already delivered, or this module never submitted it.
                    return;
                };
                let outcome = match result.outcome {
                    SolveOutcome::Solved(Ok(solved)) => Ok(solved),
                    SolveOutcome::Solved(Err(e)) => Err(e.to_string()),
                    // This module only ever submits `SolveKind::Full` -- the other
                    // kinds are reserved for other future callers, see
                    // `solve_service.rs`'s own doc comment.
                    SolveOutcome::Verified(_) => Err("unexpected solve result kind".to_string()),
                };
                on_done(ui, outcome);
            },
        );
        *cell.borrow_mut() = Some(service);
    });
}

/// Submits a full solve of `design` to this module's own [`SOLVE_SERVICE`],
/// delivering the result to `on_done` once it lands (never inline on the UI
/// thread's own call stack). Used only when [`cached_solve_matching`] has nothing
/// usable.
fn submit_full_solve(
    ui: &MainWindow,
    design: Arc<Design>,
    on_done: impl FnOnce(&MainWindow, Result<Vec<SolvedTier>, String>) + 'static,
) {
    ensure_solve_service(ui);
    let key = NEXT_SOLVE_KEY.with(|c| {
        let key = c.get();
        c.set(key + 1);
        key
    });
    PENDING_SOLVES.with(|cell| {
        cell.borrow_mut().insert(key, Box::new(on_done));
    });
    SOLVE_SERVICE.with(|cell| {
        if let Some(service) = cell.borrow().as_ref() {
            service.submit(SolveRequest {
                design,
                generation: key,
                kind: SolveKind::Full,
            });
        }
    });
}

/// Group 1's entry point: resolves a design's solve either from cache (synchronous)
/// or background solve (asynchronous). Uses [`cached_solve_matching`] when it has
/// an answer (synchronous, no perceptible cost), else [`submit_full_solve`]
/// (asynchronous -- `then` runs once the background worker reports back). Either
/// way `then` runs exactly once, with `design` handed back alongside the result
/// so a caller need not keep its own separate clone around just to reach it from
/// the continuation.
///
/// Reused by `callbacks::retarget_actions::setup_snapshot_callbacks` for its
/// cached-or-background resolution -- see that function's own doc comment.
pub(in crate::gui::editor) fn resolve_solved_then(
    ui: &MainWindow,
    design: Arc<Design>,
    then: impl FnOnce(&MainWindow, Arc<Design>, Result<Vec<SolvedTier>, String>) + 'static,
) {
    if let Some(solved) = cached_solve_matching(&design) {
        then(ui, design, Ok(solved));
        return;
    }
    let for_submit = Arc::clone(&design);
    submit_full_solve(ui, for_submit, move |ui, result| {
        then(ui, design, result);
    });
}
