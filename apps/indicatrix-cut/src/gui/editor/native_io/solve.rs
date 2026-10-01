//! Group 1: resolves a design's write-time solve from a matching cached solve, else a
//! background solve on this module's own [`SolveService`] worker -- never inline on
//! the UI thread. [`resolve_solve_at`] is `save`'s generation-safe entry point with a
//! typed [`SolveFailure`]; [`resolve_solved_then`] is the plain one, still used by
//! `retarget_actions::setup_snapshot_callbacks`.

use crate::MainWindow;
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    fmt,
    sync::Arc,
};

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

/// Why a write-time solve produced no masts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::gui::editor) enum SolveFailure {
    /// The worker's result described a request that a later one displaced, and the
    /// re-issue allowance ran out. Says nothing about the design's geometry, so a
    /// caller must not word it as "not a closed solid" nor stamp it into a file.
    Superseded,
    /// The design itself does not solve; carries the solver's message.
    Failed(String),
}

impl fmt::Display for SolveFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Superseded => f.write_str("the solve was displaced by a newer request"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// Maps a delivered worker result onto the typed outcome a caller acts on. A result
/// the worker flagged `superseded` is never trusted, whatever `outcome` holds: it
/// describes a request that is no longer the newest.
///
/// This module only ever submits [`SolveKind::Full`], so a `Verified` result is a
/// bookkeeping error reported as [`SolveFailure::Failed`].
pub(super) fn classify_solve_result(
    superseded: bool,
    outcome: SolveOutcome,
) -> Result<Vec<SolvedTier>, SolveFailure> {
    if superseded {
        return Err(SolveFailure::Superseded);
    }
    match outcome {
        SolveOutcome::Solved(result) => result.map_err(|e| SolveFailure::Failed(e.to_string())),
        SolveOutcome::Verified(_) => Err(SolveFailure::Failed(
            "unexpected solve result kind".to_string(),
        )),
    }
}

/// The FIFO of submitted solve keys plus the single key currently on the worker.
///
/// [`SolveService`]'s mailbox holds one request and overwrites anything the worker has
/// not picked up yet, so two requests in flight at once strand or displace each other.
/// This lane hands the worker one request at a time instead: a key starts only after
/// the previous one finished, so the worker never sees a newer request arrive while
/// it is still computing an older one.
#[derive(Debug, Default)]
pub(super) struct SolveLane {
    /// Keys waiting for the worker, oldest first.
    queued: VecDeque<u64>,
    /// The key the worker is computing, if any.
    running: Option<u64>,
}

impl SolveLane {
    /// Queues `key` behind everything already waiting.
    pub(super) fn enqueue(&mut self, key: u64) {
        self.queued.push_back(key);
    }

    /// Queues `key` ahead of everything waiting -- a request that must run again
    /// keeps its place in line.
    pub(super) fn requeue_front(&mut self, key: u64) {
        self.queued.push_front(key);
    }

    /// The next key to hand to the worker: `None` while one is still running or
    /// nothing waits. Marks the returned key as running.
    pub(super) fn start_next(&mut self) -> Option<u64> {
        if self.running.is_some() {
            return None;
        }
        let key = self.queued.pop_front()?;
        self.running = Some(key);
        Some(key)
    }

    /// Records that the worker delivered `key`'s result. `false` (and no state
    /// change) when `key` was not the running one.
    pub(super) fn finish(&mut self, key: u64) -> bool {
        if self.running == Some(key) {
            self.running = None;
            true
        } else {
            false
        }
    }
}

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
/// same-shaped cached solve looking usable. Every call site below still treats
/// "no usable cache" the same as "cache empty": it submits a real, off-UI-thread
/// solve rather than trusting a possibly-stale one, so the exposure here is
/// narrower than it sounds -- a false "still matches" only survives until the next
/// background solve completes for this same tier count.
#[must_use]
pub(super) fn solve_matches_design(
    cached: Option<&[SolvedTier]>,
    design: &Design,
) -> Option<Vec<SolvedTier>> {
    let cached = cached?;
    (cached.len() == design.tiers.len()).then(|| cached.to_vec())
}

/// Reads the shared last-completed background solve and returns it only when
/// [`solve_matches_design`] accepts it against `design`. See that function's own
/// doc comment for what "matches" does and does not guarantee.
///
/// # The shared cache is tagged, but this reader cannot use the tag
///
/// `auto_solve::solid_last_solved`'s cache is an
/// `Arc<Mutex<Option<(u64, Vec<SolvedTier>)>>>`: every completed background solve
/// stores the `EditorState::generation` it was solved against next to the masts.
/// This function has no generation of its own to compare that tag with (its one
/// caller, `retarget_actions::setup_snapshot_callbacks`, goes through
/// [`resolve_solved_then`], whose signature carries none), so it drops the tag and
/// falls back to the tier-count proxy. A Save/Export landing right after an edit can
/// therefore reuse a cache entry that describes the design one or more edits ago.
/// [`resolve_solve_at`] is the fix for the write paths: it never reads THIS cache,
/// only a LOCAL one this module tags with the caller's generation itself (see
/// [`LOCAL_SOLVE_CACHE`]) and matches exactly -- at the cost of a design's first
/// save or export in a session always submitting a fresh solve.
fn cached_solve_matching(design: &Design) -> Option<Vec<SolvedTier>> {
    let cache = crate::gui::editor::auto_solve::solid_last_solved()?;
    let guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    solve_matches_design(guard.as_ref().map(|(_, solved)| solved.as_slice()), design)
}

thread_local! {
    /// [`resolve_solve_at`]'s own generation-tagged cache -- see
    /// [`cached_solve_matching`]'s own doc comment for why this module needs one
    /// of its own rather than trusting the shared one's tier-count proxy. Written only
    /// by [`resolve_solve_at`]'s own background-solve completion, tagged with
    /// the generation that request was submitted for; read only by
    /// [`local_cache_matching`], which requires an EXACT match against the
    /// generation it was called with.
    static LOCAL_SOLVE_CACHE: RefCell<Option<(u64, Vec<SolvedTier>)>> = const { RefCell::new(None) };
}

/// [`LOCAL_SOLVE_CACHE`]'s read half: `Some` only when the cache holds a solve
/// for EXACTLY `generation` and its tier count still matches `design` (the
/// second check catches the (rare) case where `generation` was somehow reused
/// against a structurally different design -- cheap, and matches
/// [`solve_matches_design`]'s own existing discipline of never trusting length
/// alone as a first-order signal without at least a re-check here).
fn local_cache_matching(design: &Design, generation: u64) -> Option<Vec<SolvedTier>> {
    LOCAL_SOLVE_CACHE.with(|cell| {
        let cached = cell.borrow();
        let (cached_generation, solved) = cached.as_ref()?;
        (*cached_generation == generation)
            .then(|| solve_matches_design(Some(solved.as_slice()), design))?
    })
}

/// A stashed solve continuation -- named purely to keep [`PendingSolve`]'s own type
/// under clippy's `type_complexity` lint.
type SolveContinuation = Box<dyn FnOnce(&MainWindow, Result<Vec<SolvedTier>, SolveFailure>)>;

/// How many times a displaced request is re-issued before its caller is told
/// [`SolveFailure::Superseded`]. The [`SolveLane`] keeps the worker to one request at
/// a time, so a displacement should not occur at all; this bounds the recovery if it
/// ever does.
const SUPERSEDED_REISSUES: u8 = 2;

/// One submitted solve awaiting its result.
struct PendingSolve {
    /// The design to (re-)submit.
    design: Arc<Design>,
    /// Runs exactly once with the final outcome.
    on_done: SolveContinuation,
    /// Re-issues left if the worker reports this request displaced.
    reissues_left: u8,
}

thread_local! {
    /// This module's own persistent [`SolveService`] worker -- created lazily on
    /// first use ([`ensure_solve_service`]). `setup_editor_callbacks` does not own
    /// one to share with this module, and every write/export/autosave site here
    /// needs the same one worker, so this module owns it directly. One worker for
    /// the life of the window.
    static SOLVE_SERVICE: RefCell<Option<SolveService>> = const { RefCell::new(None) };
    /// Submitted solves keyed by the `generation` `SolveRequest`/`SolveResult` echo
    /// back -- reused purely as an opaque continuation key (see
    /// `SolveRequest::generation`'s own doc comment: "opaque to this module"), since
    /// none of this module's callers have a real domain generation counter to compare
    /// a background solve against (that belongs to `EditorState`).
    static PENDING_SOLVES: RefCell<HashMap<u64, PendingSolve>> = RefCell::new(HashMap::new());
    /// Which pending keys wait for the worker and which one is on it.
    static SOLVE_LANE: RefCell<SolveLane> = RefCell::new(SolveLane::default());
    static NEXT_SOLVE_KEY: Cell<u64> = const { Cell::new(0) };
}

/// Hands the worker the next queued request, if it is idle.
fn dispatch_next_solve() {
    loop {
        let Some(key) = SOLVE_LANE.with(|lane| lane.borrow_mut().start_next()) else {
            return;
        };
        let design = PENDING_SOLVES.with(|pending| {
            pending
                .borrow()
                .get(&key)
                .map(|entry| Arc::clone(&entry.design))
        });
        if let Some(design) = design {
            SOLVE_SERVICE.with(|cell| {
                if let Some(service) = cell.borrow().as_ref() {
                    service.submit(SolveRequest {
                        design,
                        generation: key,
                        kind: SolveKind::Full,
                    });
                }
            });
            return;
        }
        // The continuation is gone (already delivered): free the worker's slot and
        // look at the next key.
        SOLVE_LANE.with(|lane| lane.borrow_mut().finish(key));
    }
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
                let key = result.generation;
                SOLVE_LANE.with(|lane| lane.borrow_mut().finish(key));
                let Some(mut pending) = PENDING_SOLVES.with(|c| c.borrow_mut().remove(&key)) else {
                    // Already delivered, or this module never submitted it.
                    dispatch_next_solve();
                    return;
                };
                // A displaced request is still delivered (see
                // `solve_service::SolveResult::superseded`) so this `remove` always
                // runs and the continuation is never stranded. Its `outcome` describes
                // a request that is no longer the newest, so it is never trusted;
                // the request is re-issued instead of surfaced as a geometry problem.
                let outcome = classify_solve_result(result.superseded, result.outcome);
                if matches!(outcome, Err(SolveFailure::Superseded)) && pending.reissues_left > 0 {
                    pending.reissues_left -= 1;
                    PENDING_SOLVES.with(|c| c.borrow_mut().insert(key, pending));
                    SOLVE_LANE.with(|lane| lane.borrow_mut().requeue_front(key));
                    dispatch_next_solve();
                    return;
                }
                // The worker is idle again: start the next queued request before the
                // continuation runs, which may itself submit more.
                dispatch_next_solve();
                (pending.on_done)(ui, outcome);
            },
        );
        *cell.borrow_mut() = Some(service);
    });
}

/// Submits a full solve of `design` to this module's own [`SOLVE_SERVICE`] behind any
/// request already waiting, delivering the result to `on_done` once it lands (never
/// inline on the UI thread's own call stack). Used only when a cache has nothing
/// usable.
fn submit_full_solve(
    ui: &MainWindow,
    design: Arc<Design>,
    on_done: impl FnOnce(&MainWindow, Result<Vec<SolvedTier>, SolveFailure>) + 'static,
) {
    ensure_solve_service(ui);
    let key = NEXT_SOLVE_KEY.with(|c| {
        let key = c.get();
        c.set(key + 1);
        key
    });
    PENDING_SOLVES.with(|cell| {
        cell.borrow_mut().insert(
            key,
            PendingSolve {
                design,
                on_done: Box::new(on_done),
                reissues_left: SUPERSEDED_REISSUES,
            },
        );
    });
    SOLVE_LANE.with(|lane| lane.borrow_mut().enqueue(key));
    dispatch_next_solve();
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
        then(ui, design, result.map_err(|failure| failure.to_string()));
    });
}

/// [`resolve_solved_then`]'s generation-tracked, typed sibling -- for the write paths
/// (`save`), which can cheaply name the design's live `EditorState::generation` at the
/// same moment they clone `design` out of it. Identical shape and guarantee (`then`
/// runs exactly once, cache or background solve, never inline), except the cache it
/// may answer from is [`LOCAL_SOLVE_CACHE`] -- tagged with `generation` -- never the
/// shared one [`cached_solve_matching`] reads, and the error is a [`SolveFailure`]:
/// a caller can tell a design that does not solve from a solve that was displaced.
pub(in crate::gui::editor) fn resolve_solve_at(
    ui: &MainWindow,
    design: Arc<Design>,
    generation: u64,
    then: impl FnOnce(&MainWindow, Arc<Design>, Result<Vec<SolvedTier>, SolveFailure>) + 'static,
) {
    if let Some(solved) = local_cache_matching(&design, generation) {
        then(ui, design, Ok(solved));
        return;
    }
    let for_submit = Arc::clone(&design);
    submit_full_solve(ui, for_submit, move |ui, result| {
        if let Ok(solved) = &result {
            LOCAL_SOLVE_CACHE.with(|cell| *cell.borrow_mut() = Some((generation, solved.clone())));
        }
        then(ui, design, result);
    });
}
