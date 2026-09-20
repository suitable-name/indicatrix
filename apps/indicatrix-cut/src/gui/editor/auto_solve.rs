//! Background solving for the "Edit" sub-tab: moves `Design::solve` off the UI thread
//! for the explicit "Solve" action (button and F5, via [`super::view::refresh_all`]'s
//! large-design path) and, when the design is cheap enough, schedules the SAME
//! machinery automatically after an edit ("auto-solve") so small/medium designs get a
//! fresh path-traced view and masts without a click. See `mod.rs`'s "Never block the
//! UI thread with a solve" section for the rule this exists to uphold.
//!
//! # Why a `thread_local!`, not a new `EditorState` field
//!
//! `deep_solve`/`optimize`'s in-flight handles live directly on `EditorState`
//! (`state/mod.rs`), which is NOT this group's file to edit. Everything this module
//! needs beyond `EditorState::design`/`generation` (both already readable through a
//! plain `&EditorState`) -- the debounce timer, the running request counter, the last
//! measured solve time, and the shared solid-preview handles a completed background
//! solve pushes into -- lives instead in [`Runtime`], a `thread_local!` singleton.
//! That is sound for the identical reason `EditorState` itself gets away with a plain
//! `Rc<RefCell<..>>` rather than `Arc<Mutex<..>>` (see that struct's own doc comment):
//! every function in this module that touches `Runtime` runs on the UI/event-loop
//! thread, either directly from a Slint callback or from inside a
//! `Weak::upgrade_in_event_loop` closure. The worker threads this module spawns never
//! touch `Runtime` themselves -- they only compute pure `Design` -> data conversions
//! and hand the result back across the event-loop boundary.
//!
//! # Two kinds of "stale" a completed background solve must detect
//!
//! A background solve is dispatched against a snapshot: a cloned [`Design`] plus the
//! [`EditorState::generation`] counter's value at that moment. By the time it
//! completes, either or both of the following can have happened, and
//! [`apply_background_solve_result`] must not let either corrupt the display:
//!
//! 1. **A newer background solve was dispatched** (the user clicked Solve again, or
//!    another auto-solve fired) before this one returned. [`Runtime::current_seq`] is
//!    bumped on every dispatch; a completion only touches the UI at all while its own
//!    sequence number still matches -- an older one simply has nothing left to do,
//!    since the newer dispatch already owns the "Solving..." banner and will apply its
//!    own result in turn.
//! 2. **The design changed without a new dispatch** -- possible when auto-solve is
//!    disabled (or this design's own measured cost exceeds the budget) and an edit
//!    lands while an explicit Solve from BEFORE that edit is still running. Here this
//!    result's own sequence number is still current (nothing superseded it), but
//!    `generation` has moved past what was captured at dispatch time. The edit's own
//!    [`super::view::refresh_editor_panel_stale`] call already repainted the banner
//!    correctly at edit time, so this arm only needs to clear `editor_solve_running`
//!    (nothing else will) and otherwise touch nothing.
//!
//! Both checks are cheap `u64` comparisons; nothing here ever tries to reinterpret a
//! stale result's tier indices against a design it no longer describes.

use super::{
    state::{
        EditorState, apply_multi_selection, design_to_gpu_planes, manufacturability_warning_lines,
        manufacturability_warning_lines_from_solved, push_multi_selected_count, push_tiers,
        status_text_and_is_problem, status_text_and_is_problem_from_solved, tier_items,
        tier_items_from_solved, yield_report_texts, yield_report_texts_from_solved,
    },
    view::{SolidLastSolved, scaled_viewport_size},
};
use crate::{
    EditorModel, EditorTierItem, MainWindow, SolidPreviewModel, TiltModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        render::camera_lighting::contained_request_size,
        solid_preview::preview_state::{CameraPose, SolidPreviewState},
    },
};
use indicatrix::geometry::{GpuFacetPlane, meet_solver::SolvedTier};
use indicatrix_cut_core::Design;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// CAD audit item 113. Solve cost is driven by plane count, not tier count -- a
/// wide-orbit round-brilliant tier emits many planes at once (the corpus's largest
/// measured design is 103 tiers but 210 planes, `mod.rs`'s own baseline), so a small
/// tier count can still be an expensive, UI-thread-blocking solve. This replaced a
/// bare `SYNC_SOLVE_TIER_LIMIT` of 16 tiers.
///
/// A rough estimate, not a re-measurement of this crate's own corpus: roughly double
/// that old tier count at the corpus's own ~2 planes-per-tier ratio, kept
/// comfortably under the 210-plane/5.9s worst case.
const SYNC_SOLVE_PLANE_LIMIT: usize = 32;

/// CAD audit item 113: once a design has a real measured solve time, that measurement
/// is a far better signal than any plane-count estimate -- a design that solved in
/// under this long most recently is still fine to solve again synchronously,
/// regardless of how many planes it has.
const SYNC_SOLVE_TIME_LIMIT: Duration = Duration::from_millis(500);

/// Whether a solve for a design with `plane_count` planes should still run
/// synchronously on the UI thread (the "New"/Load/explicit-Solve fast path,
/// [`super::view::refresh_all`]'s sync branch) rather than through
/// [`dispatch_background_solve`] -- CAD audit item 113's replacement for a bare
/// tier-count cutoff. Prefers this design's own last REAL measured solve time
/// (see [`Runtime::last_solve`]) when one exists, since a real measurement beats
/// any estimate; falls back to `plane_count` only for a design that has never
/// solved yet (a fresh "New" design, most commonly).
///
/// Pure and unit tested directly, mirroring [`should_schedule_auto_solve`]'s own
/// "measurement first, otherwise estimate" shape -- `last_solve` is a parameter
/// here (rather than read from `Runtime` internally) for exactly the same
/// testability reason that function documents on itself.
#[must_use]
pub(super) fn should_solve_synchronously(plane_count: usize, last_solve: Option<Duration>) -> bool {
    last_solve.map_or(plane_count <= SYNC_SOLVE_PLANE_LIMIT, |last| {
        last <= SYNC_SOLVE_TIME_LIMIT
    })
}

/// Debounce delay between an edit landing and an eligible auto-solve actually
/// dispatching -- short enough that a burst of keystrokes (typing an angle, say) only
/// ever triggers the LAST one, long enough that it reads as "just happened," not
/// "laggy."
const AUTO_SOLVE_DEBOUNCE: Duration = Duration::from_millis(150);

/// How often a running background solve's ticker updates the "Solving... (N tiers)"
/// banner with fresh elapsed time -- matches `deep_solve::TICK_INTERVAL`/
/// `optimize_solve::TICK_INTERVAL`'s own value and rationale.
const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// This module's UI-thread-confined state -- see the module doc comment for why this
/// is a `thread_local!` rather than a new `EditorState` field.
struct Runtime {
    /// Set once by [`init`], from the same `Arc`s `setup_editor_callbacks` already
    /// holds -- lets a completed background solve push into the shared solid preview
    /// exactly like [`super::view::refresh_viewport`] does, without this module
    /// needing `EditorState` (or a wider parameter list on
    /// [`super::view::refresh_editor_panel_stale`], which many callbacks this group
    /// does not own already call with a fixed signature).
    preview_state: Option<Arc<SolidPreviewState>>,
    solid_last_solved: Option<SolidLastSolved>,
    /// CAD audit item 117: the shared [`RenderContext`] handle, stashed by
    /// [`stash_render_ctx`] -- see that function's own doc comment for why it is
    /// stashed from an unrelated `setup_*_callback` rather than passed to
    /// [`init`] directly.
    render_ctx: Option<Arc<Mutex<RenderContext>>>,
    /// The most recent REAL measured solve wall time (background or synchronous) for
    /// whichever design is currently loaded -- `None` until the first solve since the
    /// last New/Load. Drives [`should_schedule_auto_solve`].
    last_solve: Option<Duration>,
    /// Bumped by every dispatch (explicit-Solve or auto-solve alike); see the module
    /// doc comment's "newer background solve" case.
    current_seq: u64,
    /// The debounce timer auto-solve restarts on every eligible edit. Replacing this
    /// with a fresh `Timer` cancels whatever the previous one had pending -- Slint
    /// stops a `Timer`'s callback from firing once the `Timer` itself is dropped.
    debounce: Option<slint::Timer>,
    /// The design/multi-select snapshot [`stash_current_design`] last recorded,
    /// paired with the generation it was cloned at -- see that function's own doc
    /// comment. Read by [`take_matching_design`] once a solid-preview frame lands
    /// claiming the SAME generation, so the tier table can be rebuilt from its
    /// already-solved masts instead of triggering a second, redundant
    /// `Design::solve()` (`cad_todo.md` #73).
    current_design: Option<(u64, Design, BTreeSet<usize>)>,
    /// CAD audit item 112: true from the moment [`dispatch_background_solve`]
    /// actually spawns a worker thread until that worker's completion is
    /// observed by [`apply_background_solve_result`] -- gates new dispatches
    /// against piling up multiple concurrent multi-second solve threads. See
    /// [`PendingDispatch`] for what happens to a dispatch request that arrives
    /// while this is `true`.
    solve_in_flight: bool,
    /// The most recent dispatch request that arrived while [`Runtime::solve_in_flight`]
    /// was `true` -- captured instead of spawning a second concurrent worker.
    /// [`apply_background_solve_result`] takes and re-dispatches this once the
    /// in-flight solve's completion is observed, so a burst of edits against a
    /// slow design collapses to "the one currently running, then the latest
    /// pending one," never an unbounded pile of worker threads (CAD audit item
    /// 112). Only ever holds the LATEST request: a second arrival while one is
    /// already pending simply overwrites it.
    pending_dispatch: Option<PendingDispatch>,
}

impl Runtime {
    const fn new() -> Self {
        Self {
            preview_state: None,
            solid_last_solved: None,
            render_ctx: None,
            last_solve: None,
            current_seq: 0,
            debounce: None,
            current_design: None,
            solve_in_flight: false,
            pending_dispatch: None,
        }
    }
}

/// A [`dispatch_background_solve`] call suppressed by [`Runtime::solve_in_flight`] --
/// see that field's own doc comment (CAD audit item 112).
struct PendingDispatch {
    design: Design,
    generation: Arc<AtomicU64>,
    multi_selected: BTreeSet<usize>,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = const { RefCell::new(Runtime::new()) };
}

/// Stashes the shared solid-preview handles this module needs -- called once from
/// [`super::setup_editor_callbacks`], before any callback (and therefore any possible
/// dispatch) is wired up.
pub(super) fn init(preview_state: &Arc<SolidPreviewState>, solid_last_solved: &SolidLastSolved) {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.preview_state = Some(Arc::clone(preview_state));
        rt.solid_last_solved = Some(Arc::clone(solid_last_solved));
    });
}

/// Returns the shared solid-preview handle [`init`] stashed, if it has run yet --
/// lets `callbacks::tier_actions`'s hover/click/selection-changed/multi-select
/// callbacks resubmit a merged `FacetOverlay` (see that module's own doc comment)
/// without `gui::editor::mod`'s fixed, not-owned-by-this-lane call sites needing a
/// new parameter threaded through them.
pub(super) fn preview_state() -> Option<Arc<SolidPreviewState>> {
    RUNTIME.with(|cell| cell.borrow().preview_state.clone())
}

/// Returns the shared "last solved" cache [`init`] stashed, if it has run yet --
/// same reasoning as [`preview_state`], for the callbacks that need to build a
/// `facet_map::FacetMap` against the last rendered frame's masts without a new
/// parameter on their own fixed call sites.
pub(super) fn solid_last_solved() -> Option<SolidLastSolved> {
    RUNTIME.with(|cell| cell.borrow().solid_last_solved.clone())
}

/// Stashes the shared [`RenderContext`] handle for [`render_ctx`] below (CAD
/// audit item 117) -- called from `callbacks::tier_actions::
/// setup_toggle_detach_callback`, one of several `setup_*_callback`s already
/// given this `Arc` directly by `gui::editor::mod::setup_editor_callbacks`
/// (which is not this lane's file to add a NEW parameter to). That call runs
/// synchronously, before `setup_editor_callbacks` returns and the event loop
/// starts, so [`render_ctx`] is already populated by the time a user can hover
/// or click anything -- the ordering among `setup_*_callback` calls within that
/// one function does not matter.
pub(super) fn stash_render_ctx(render_ctx: &Arc<Mutex<RenderContext>>) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().render_ctx = Some(Arc::clone(render_ctx));
    });
}

/// Returns the shared [`RenderContext`] handle [`stash_render_ctx`] stashed, if
/// it has run yet -- lets `callbacks::tier_actions`'s Solid-view hover/click
/// callbacks read the configured render resolution (CAD audit item 117) without
/// a new parameter on their own fixed, not-owned-by-this-lane call sites in
/// `gui::editor::mod`.
pub(super) fn render_ctx() -> Option<Arc<Mutex<RenderContext>>> {
    RUNTIME.with(|cell| cell.borrow().render_ctx.clone())
}

/// Whether an edit that just landed should schedule a debounced auto-solve --
/// `budget` `0` disables auto-solve outright (today's stale-marker-only behaviour);
/// otherwise, a design with no measurement YET (`last_solve: None` -- nothing has
/// solved since the last New/Load) is scheduled optimistically: a fresh design's
/// schedule starts empty and solves near-instantly, so refusing to try would only
/// delay that design's very first auto-solve for no reason. Once a real measurement
/// exists, it alone decides.
///
/// Pure and unit tested directly -- the one genuinely decision-shaped piece of logic
/// in this module.
pub(super) const fn should_schedule_auto_solve(
    last_solve: Option<Duration>,
    budget: Duration,
) -> bool {
    if budget.is_zero() {
        return false;
    }
    match last_solve {
        Some(last) => last.as_millis() < budget.as_millis(),
        None => true,
    }
}

/// The banner text shown while auto-solve is disabled FOR THIS DESIGN specifically
/// (as opposed to the user having set the budget to `0` outright) -- this design's own
/// last measured solve already exceeds the configured budget.
pub(super) fn auto_solve_off_note(last_solve: Duration) -> String {
    format!(
        "Auto-solve off for this design: last solve took {:.1}s.",
        last_solve.as_secs_f32()
    )
}

/// [`super::state::design_to_gpu_planes`]'s counterpart for a caller that already has
/// an up-to-date `solved` mast list on hand -- built from [`Design::planes_from_solved`]
/// instead of a second internal [`Design::solve`] (CAD audit item 111). `state::mod.rs`
/// is not this lane's file to add a new function to, so this one small conversion
/// lives here instead; it mirrors `design_to_gpu_planes`'s own sign-flip convention
/// exactly (see this crate's `mod.rs` doc comment, "Feeding the viewport").
///
/// `pub(super)` (not just private) since CAD audit item 111 also needs this from
/// `view::refresh_viewport`, to avoid that call site's own former SECOND
/// independent `Design::solve()`.
pub(super) fn design_to_gpu_planes_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> Vec<GpuFacetPlane> {
    design
        .planes_from_solved(solved)
        .into_iter()
        .map(|(normal, offset)| GpuFacetPlane::new(normal.as_vec3(), -offset as f32))
        .collect()
}

/// The "Solving..." banner text a running background solve shows, ticked forward by
/// [`dispatch_background_solve`]'s ticker thread.
fn solving_banner(tier_count: usize, elapsed: Duration) -> String {
    format!(
        "Solving... ({tier_count} tier{}) -- {:.1}s elapsed",
        if tier_count == 1 { "" } else { "s" },
        elapsed.as_secs_f32()
    )
}

/// Records a just-completed solve's wall time (background or synchronous) as this
/// design's new estimate for [`should_schedule_auto_solve`]'s next decision.
pub(super) fn record_solve_duration(elapsed: Duration) {
    RUNTIME.with(|cell| cell.borrow_mut().last_solve = Some(elapsed));
}

/// This design's last measured solve time -- a test-only window onto [`Runtime`]'s
/// thread-local state, so `record_solve_duration`/`reset_for_new_design` can be
/// asserted on directly.
///
/// Deliberately NOT used in production: `should_schedule_auto_solve` reads
/// `Runtime::last_solve` itself, and `view::refresh_all`'s sync-vs-background
/// decision passes `None` to [`should_solve_synchronously`] on purpose, because
/// `reset_for_new_design` has just cleared the measurement and reaching back for the
/// value it cleared would judge a freshly loaded design by the previous one's solve
/// time -- precisely the case where the two have nothing to do with each other.
#[cfg(test)]
#[must_use]
fn last_measured_solve_duration() -> Option<Duration> {
    RUNTIME.with(|cell| cell.borrow().last_solve)
}

/// Clears the running estimate, drops any pending debounced auto-solve, and
/// invalidates every still-in-flight background solve -- called whenever
/// `EditorState` itself is replaced wholesale (New/Load Selected/Open Native, all via
/// [`super::view::refresh_all`]), since a different design's solve cost, debounce
/// timer, and any dispatch still running against the OLD design have nothing to do
/// with the one that just replaced it.
///
/// # Why this alone fixes a stale-design race none of its three callers need to know about
///
/// A background solve captures its own `generation: Arc<AtomicU64>` snapshot at
/// dispatch time (see [`dispatch_background_solve`]), but New/Load Selected/Open
/// Native don't just bump `EditorState::generation` -- they replace `EditorState`
/// wholesale with a BRAND NEW `Arc<AtomicU64>` (see e.g.
/// `callbacks::tier_actions::apply_loaded_design`). A dispatch from the design being
/// replaced is comparing against the OLD `Arc`, which nothing ever increments again,
/// so [`apply_background_solve_result`]'s `generation` check alone can never detect
/// this case -- it would apply a 103-tier design's stale completion on top of a
/// 5-tier design just loaded over it. Bumping [`Runtime::current_seq`] here closes
/// that gap: every one of this function's callers reaches it (via `refresh_all`,
/// called unconditionally at the top of every New/Load Selected/Open Native path)
/// BEFORE that stale completion's `upgrade_in_event_loop` closure can run (both run
/// on the UI/event-loop thread, so ordering is never racy), so its `is_current(seq)`
/// check now correctly fails and it returns having touched nothing.
///
/// Dropping `debounce` cancels whatever single-shot auto-solve [`on_edit`] had
/// pending against the design being replaced -- Slint stops a `Timer`'s callback
/// once the `Timer` itself is dropped (see [`Runtime::debounce`]'s own doc comment).
///
/// Safe to call at any time: bumping `current_seq`/dropping `debounce` with no solve
/// in flight is a no-op beyond the wasted counter tick.
pub(super) fn reset_for_new_design() {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.last_solve = None;
        rt.current_seq += 1;
        rt.debounce = None;
        // Not load-bearing for correctness -- `EditorState::replace_wholesale`
        // reuses and bumps the SAME `Arc<AtomicU64>` (see its own doc comment),
        // so a stash from before this replacement already carries an older
        // generation number that can never again equal the live one. Cleared
        // anyway so a large superseded design isn't held onto for no reason.
        rt.current_design = None;
        // CAD audit item 112: a pending dispatch queued behind an in-flight solve
        // named the OLD `Design`/generation -- it must not be replayed once this
        // reset has moved on to a different one entirely.
        rt.pending_dispatch = None;
        // Deliberately NOT reset to `false` here: whatever worker
        // `dispatch_background_solve` already spawned against the OLD design (if
        // any) is still physically running and will still call
        // `apply_background_solve_result`, which clears this flag itself the
        // moment that completion is observed (staleness or not) -- see that
        // function's own doc comment. Clearing it early here would let a NEW
        // dispatch for the just-loaded design start a second worker thread while
        // the stale one is still finishing, which is exactly the "unbounded
        // concurrent solve threads" failure mode this mechanism exists to
        // prevent.
    });
}

/// CAD audit item 112: invalidates whatever background solve is currently in
/// flight or queued behind it -- the `Runtime`-only half of what a "Cancel Solve"
/// action needs. Bumping `current_seq` makes the in-flight worker's eventual
/// completion fail its `is_current(seq)` check (the same mechanism
/// [`reset_for_new_design`] already uses for a wholesale design replacement), so
/// [`apply_background_solve_result`] still runs when that worker finishes
/// (freeing [`Runtime::solve_in_flight`] for the next dispatch) but touches
/// nothing else -- the abandoned worker keeps running to completion in the
/// background; its result is simply dropped, costing CPU only, never
/// correctness, same as every other abandonment in this module. Also drops any
/// [`PendingDispatch`] queued behind the cancelled solve (it named the design as
/// of the click that queued it; a fresh dispatch from the CURRENT design will
/// replace it via the ordinary edit path if one is still needed) and the
/// debounced auto-solve timer [`on_edit`] may have pending.
///
/// Deliberately narrower than [`reset_for_new_design`]: `last_solve`/
/// `current_design` are left untouched, since cancelling a solve does not change
/// which design is loaded or invalidate that design's previous measurement.
///
/// # Handoff
/// The UI-visible half of "Cancel Solve" -- clearing `EditorModel.solve_running`/
/// setting `solve_state`/`status_text` back to a "cancelled" message on the SAME
/// click, exactly like `callbacks::solve_actions::setup_deep_solve_cancel_callback`
/// already does for Deep Solve -- needs a new `EditorModel.solve_cancel` callback
/// (`ui/models/editor.slint`, not owned by this lane) plus a `setup_solve_cancel_callback`
/// (`gui::editor::mod`, also not owned by this lane) that calls this function
/// first, then:
/// ```ignore
/// ui.global::<EditorModel>().on_solve_cancel(move || {
///     auto_solve::cancel_in_flight_solve();
///     ui.global::<EditorModel>().set_solve_running(false);
///     ui.global::<EditorModel>().set_solve_state("stale".into());
///     ui.global::<EditorModel>().set_status_text("Solve cancelled.".into());
///     ui.global::<EditorModel>().set_status_is_problem(true);
/// });
/// ```
/// plus a "Cancel Solve" button in `editor_command_bar.slint`'s `SolveGroup`
/// (this lane's own file), shown only while `EditorModel.solve_running` is
/// `true` -- mirroring `AnalysisGroup`'s existing "Cancel Deep Solve"/"Cancel
/// Optimize" buttons in the same file. Left unwired here: this lane's brief
/// forbids referencing a Slint global callback that does not exist yet, and
/// `EditorModel.solve_cancel`/the button's own call site both need that new
/// callback declared first.
pub(super) fn cancel_in_flight_solve() {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.current_seq += 1;
        rt.debounce = None;
        rt.pending_dispatch = None;
    });
}

fn is_current(seq: u64) -> bool {
    RUNTIME.with(|cell| cell.borrow().current_seq == seq)
}

/// Records `design`/`multi_selected` (plain clones -- see `EditorState::design`'s
/// own `Rc<RefCell<..>>` wrapper, which cannot cross into `gui::SlintSolidSink::
/// apply`) alongside the `generation` they were cloned at. Called by
/// `view::submit_preview_replan` on every replan, using the SAME `Design` clone
/// already going into that call's own `ReplanRequest::design` -- see
/// [`take_matching_design`] for the reader half of this `cad_todo.md` #73
/// mechanism.
pub(super) fn stash_current_design(
    generation: u64,
    design: Design,
    multi_selected: BTreeSet<usize>,
) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().current_design = Some((generation, design, multi_selected));
    });
}

/// Called once a solid-preview frame lands naming `generation`
/// (`solid_preview::preview_state::PreviewFrame::generation`) -- see
/// `gui::SlintSolidSink::apply`'s own doc comment for why THIS module (`Runtime`
/// is confined to the UI/event-loop thread, exactly like that frame-apply
/// closure once it hops back via `slint::Weak::upgrade_in_event_loop`) is the
/// only safe place to make the comparison.
///
/// Returns the `(Design, multi_selected)` [`stash_current_design`] last recorded
/// for EXACTLY this generation -- `None` when an edit has moved the live design
/// past it (an older frame still finishing after a newer edit landed, which
/// itself already called [`stash_current_design`] again and overwrote this
/// field), in which case the caller must change nothing, matching every other
/// staleness check in this module.
///
/// # Consumes the match -- a `take`, not a `peek`
///
/// A successful match also REMOVES the stash (`Option::take`), not merely reads
/// it: `solid_preview::preview_state::PreviewFrame::solved`/`generation` are
/// carried forward unchanged by every subsequent `Reproject` frame too (an
/// ordinary camera orbit re-issues the SAME masts/generation the last `Replan`
/// produced -- see `WorkerMemory::generation`'s own doc comment), so without
/// this, every single orbit/zoom frame after an edit would re-trigger a full
/// tier-table rebuild for no reason, and possibly stutter a drag. The FIRST
/// frame to ever carry a given generation is always that generation's own
/// `Replan` result (`memory.generation` is set inside `resolve_replan_state`,
/// and the very frame built right after is what reaches the sink first), so
/// consuming it here costs nothing: no genuine result is ever missed, only the
/// redundant repeats are skipped.
///
/// On a match, ALSO drops whatever debounced auto-solve [`on_edit`] scheduled:
/// that timer would only recompute the exact masts the caller is about to push
/// from the frame's own `solved` anyway, so letting it fire would still pay the
/// redundant `Design::solve()` this whole mechanism exists to avoid. A debounce
/// that survives to see a MATCHING generation here can only be the one this SAME
/// edit's own [`on_edit`] call scheduled: no newer edit landed (that would have
/// bumped the generation and re-stashed, failing the filter above), so no newer
/// debounce could have replaced it either. This never touches an in-flight
/// [`dispatch_background_solve`] that already fired before this frame landed --
/// [`apply_background_solve_result`]'s own `generation`/sequence checks still
/// guard that arrival exactly as before.
pub(super) fn take_matching_design(generation: u64) -> Option<(Design, BTreeSet<usize>)> {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        let is_match = rt
            .current_design
            .as_ref()
            .is_some_and(|(stashed_generation, ..)| *stashed_generation == generation);
        if !is_match {
            return None;
        }
        let (_, design, multi_selected) = rt.current_design.take()?;
        rt.debounce = None;
        Some((design, multi_selected))
    })
}

/// Called at the end of [`super::view::refresh_editor_panel_stale`] -- every edit
/// callback's stale-refresh path calls that function with a fixed, already-established
/// signature, so this hook reads everything it needs (the budget property, `state`'s
/// design/generation) rather than asking for new parameters. See
/// [`should_schedule_auto_solve`] for the eligibility rule.
pub(super) fn on_edit(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    let budget_ms =
        u64::try_from(ui.global::<EditorModel>().get_auto_solve_budget_ms()).unwrap_or(0);
    let budget = Duration::from_millis(budget_ms);
    let last_solve = RUNTIME.with(|cell| cell.borrow().last_solve);

    if !should_schedule_auto_solve(last_solve, budget) {
        // Only worth a dedicated note when THIS design is the reason (its own last
        // solve exceeded a real, nonzero budget) -- a `0` budget is a deliberate,
        // global "auto-solve off" choice that the ordinary stale banner already
        // covers without needing an extra explanation.
        if !budget.is_zero()
            && let Some(last) = last_solve
        {
            ui.global::<EditorModel>()
                .set_status_text(auto_solve_off_note(last).into());
            ui.global::<EditorModel>().set_status_is_problem(true);
        }
        return;
    }

    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let design = state.design.clone();
    let generation = Arc::clone(&state.generation);
    let multi_selected = state.multi_selected.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        AUTO_SOLVE_DEBOUNCE,
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                dispatch_background_solve(
                    &ui,
                    &render_ctx,
                    design.clone(),
                    &generation,
                    multi_selected.clone(),
                );
            }
        },
    );
    RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(timer));
}

/// Bundles a completed background solve's results -- kept as one struct (rather than
/// threading each value through as its own parameter) purely to keep
/// [`apply_background_solve_result`] under clippy's argument-count lint without a new
/// `#[allow]`.
struct BackgroundSolveResult {
    elapsed: Duration,
    tiers: Vec<EditorTierItem>,
    status_text: String,
    status_is_problem: bool,
    warnings: Vec<String>,
    yield_texts: (String, String, String, String),
    planes: Vec<GpuFacetPlane>,
    solved: Option<Vec<SolvedTier>>,
    /// Index-wheel tooth count and reference angle, for the diagram view's wheel.
    gear: (u32, f32),
    /// `multi_selected.len()` at dispatch time, for the tier table's "N selected"
    /// header indicator -- read on the UI thread by [`apply_background_solve_result`],
    /// never recomputed from `EditorState` there (a solve completion has no access
    /// to it beyond this snapshot).
    multi_selected_count: usize,
}

/// Everything [`dispatch_background_solve`]'s worker derives from one solve, so the
/// two branches live in one named place rather than inline in an already-long
/// function.
struct PanelInputs {
    tiers: Vec<EditorTierItem>,
    status_text: String,
    status_is_problem: bool,
    warnings: Vec<String>,
    yield_texts: (String, String, String, String),
    planes: Vec<GpuFacetPlane>,
}

/// Builds every panel readout from a SINGLE `Design::solve` (CAD audit item 111).
///
/// This used to call five separate helpers that each solved internally -- and
/// `status_text_and_is_problem` solved twice on its own, via `status()` and
/// `measure()` -- for five or more solves per dispatch, which this module's own
/// former comment admitted to and declined to fix.
///
/// `solved` is `None` only when the design does not solve at all. That case falls
/// back to each helper's own solving form, which costs nothing extra: a design with
/// a block missing its scale-reference anchor fails `Design::solve` before any real
/// meet-solving work (see that method's early `MissingAnchor` check), so the
/// fallback re-pays only the cheap early-out, never the expensive case.
fn panel_inputs(design: &Design, solved: Option<&Vec<SolvedTier>>) -> PanelInputs {
    let n_d = design.effective_refractive_index();
    solved.map_or_else(
        || {
            let (status_text, status_is_problem) = status_text_and_is_problem(design);
            PanelInputs {
                tiers: tier_items(design, n_d),
                status_text,
                status_is_problem,
                warnings: manufacturability_warning_lines(design),
                yield_texts: yield_report_texts(design),
                planes: design_to_gpu_planes(design),
            }
        },
        |solved| {
            let (status_text, status_is_problem) =
                status_text_and_is_problem_from_solved(design, solved);
            PanelInputs {
                tiers: tier_items_from_solved(design, solved, n_d),
                status_text,
                status_is_problem,
                warnings: manufacturability_warning_lines_from_solved(design, solved),
                yield_texts: yield_report_texts_from_solved(design, solved),
                planes: design_to_gpu_planes_from_solved(design, solved),
            }
        },
    )
}

/// Spawns a background solve of `design` (a snapshot taken the moment this is called)
/// plus a ticker that keeps the "Solving..." banner's elapsed time fresh, following
/// `deep_solve::spawn_deep_solve`'s own `thread::spawn` + `Weak::upgrade_in_event_loop`
/// shape. Shared by [`super::view::refresh_all`]'s large-design path and this module's
/// own debounced auto-solve dispatch from [`on_edit`] -- both just need a `Design`
/// snapshot and the `generation` counter to check it against on arrival.
///
/// See the module doc comment for exactly what the completion handler does and does
/// not apply.
///
/// `multi_selected` is a snapshot of `EditorState::multi_selected` taken at dispatch
/// time -- cloned in (a plain `BTreeSet<usize>` is `Send`, unlike the `Rc<RefCell<..>>`
/// wrapping `EditorState` itself) so the worker thread can bake the live multi-select
/// highlight into `tier_items`' result via `apply_multi_selection`, matching every
/// other full tier-list rebuild (`view::refresh_editor_panel`/`push_stale_content`).
/// A toggle that lands after dispatch but before this solve completes is not
/// reflected in that particular frame -- no worse than the existing generation-based
/// staleness this module already accepts elsewhere, and the next refresh (of any
/// kind) reads the current selection fresh.
pub(super) fn dispatch_background_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: Design,
    generation: &Arc<AtomicU64>,
    multi_selected: BTreeSet<usize>,
) {
    // CAD audit item 112: a solve already in flight keeps this request queued
    // (overwriting any earlier one still waiting) rather than spawning a second
    // concurrent worker thread -- `apply_background_solve_result` replays exactly
    // this dispatch once the in-flight one completes. The existing "Solving..."
    // banner/ticker already belongs to that in-flight solve, so nothing else here
    // needs touching.
    let queued = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        if rt.solve_in_flight {
            rt.pending_dispatch = Some(PendingDispatch {
                design: design.clone(),
                generation: Arc::clone(generation),
                multi_selected: multi_selected.clone(),
            });
            true
        } else {
            rt.solve_in_flight = true;
            false
        }
    });
    if queued {
        return;
    }

    let tier_count = design.tiers.len();
    let started_generation = generation.load(Ordering::Relaxed);
    let seq = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.current_seq += 1;
        rt.current_seq
    });

    ui.global::<EditorModel>().set_solve_running(true);
    ui.global::<EditorModel>().set_solve_state("solving".into());
    ui.global::<EditorModel>()
        .set_status_text(solving_banner(tier_count, Duration::ZERO).into());
    ui.global::<EditorModel>().set_status_is_problem(false);

    let done_flag = Arc::new(AtomicBool::new(false));
    let done_ticker = Arc::clone(&done_flag);
    let ticker_ui = ui.as_weak();
    thread::spawn(move || {
        let start = Instant::now();
        loop {
            thread::sleep(TICK_INTERVAL);
            if done_ticker.load(Ordering::Relaxed) {
                break;
            }
            let elapsed = start.elapsed();
            let _ = ticker_ui.upgrade_in_event_loop(move |ui| {
                // A superseded dispatch's ticker has nothing left worth touching --
                // the newer dispatch owns the banner now.
                if is_current(seq) {
                    ui.global::<EditorModel>()
                        .set_status_text(solving_banner(tier_count, elapsed).into());
                }
            });
        }
    });

    let ui_weak = ui.as_weak();
    let render_ctx_for_apply = Arc::clone(render_ctx);
    let generation = Arc::clone(generation);
    thread::spawn(move || {
        let start = Instant::now();
        // CAD audit item 111: `design` is solved exactly ONCE here, and every other
        // conversion below is built from that same `solved` list via its
        // `_from_solved` counterpart -- the same helpers `view::push_solved_preview`
        // already proves out on the solid-preview worker's own replan-completion
        // path (`cad_todo.md` #73). Before this, `tier_items`/
        // `status_text_and_is_problem`/`manufacturability_warning_lines`/
        // `yield_report_texts`/`design_to_gpu_planes` each independently called
        // `Design::solve` (`status_text_and_is_problem` even called it twice, via
        // `status()` and `measure()`), for five or more total solves per background
        // dispatch -- the exact redundancy this module's own former comment here
        // used to say was deliberately NOT addressed. `result.solved` below now
        // comes from this same single `solved_result`, not a second `design.solve()`.
        let solved_result = design.solve();
        let multi_selected_count = multi_selected.len();
        let PanelInputs {
            mut tiers,
            status_text,
            status_is_problem,
            warnings,
            yield_texts,
            planes,
        } = panel_inputs(&design, solved_result.as_ref().ok());
        apply_multi_selection(&mut tiers, &multi_selected);
        let solved = solved_result.ok();
        let gear = (
            design.meta.gear_teeth_abs(),
            design.meta.gear_reference_angle as f32,
        );
        let result = BackgroundSolveResult {
            elapsed: start.elapsed(),
            tiers,
            status_text,
            status_is_problem,
            warnings,
            yield_texts,
            planes,
            solved,
            gear,
            multi_selected_count,
        };
        done_flag.store(true, Ordering::Relaxed);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            apply_background_solve_result(
                &ui,
                &render_ctx_for_apply,
                seq,
                started_generation,
                &generation,
                result,
            );
        });
    });
}

/// Applies a completed background solve's result -- see the module doc comment for
/// the two staleness checks this performs before touching anything, and `mod.rs`'s
/// "Feeding the viewport" section for the `GpuFacetPlane` sign convention
/// `refresh_viewport` already documents (this mirrors it exactly, just deferred to a
/// worker-computed `Vec<GpuFacetPlane>` instead of computing it inline).
fn apply_background_solve_result(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    seq: u64,
    started_generation: u64,
    generation: &Arc<AtomicU64>,
    result: BackgroundSolveResult,
) {
    // CAD audit item 112: this worker is done either way -- free the in-flight
    // slot and take whatever request queued up behind it BEFORE any of the
    // staleness checks below, so a pending request still gets dispatched even
    // when this particular completion turns out to be stale (see
    // `Runtime::pending_dispatch`'s own doc comment for why it must not be
    // dropped in that case).
    let pending = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.solve_in_flight = false;
        rt.pending_dispatch.take()
    });

    if !is_current(seq) {
        // A newer dispatch has already taken over the banner and will apply its own
        // result in turn -- nothing here is still current enough to show. CAD audit
        // item 238: `record_solve_duration` must not run above this check -- a
        // superseded result's elapsed time is not a measurement of the design that
        // is actually current, and must never set `should_schedule_auto_solve`'s
        // budget baseline.
        dispatch_pending(ui, render_ctx, pending);
        return;
    }
    record_solve_duration(result.elapsed);
    ui.global::<EditorModel>().set_solve_running(false);
    if generation.load(Ordering::Relaxed) != started_generation {
        // The design changed after this solve was dispatched, without a newer
        // dispatch superseding it (auto-solve disabled, or this design's own budget
        // was already exceeded) -- the edit that changed it already repainted the
        // banner correctly via `refresh_editor_panel_stale`. Only the running flag
        // above needed clearing.
        dispatch_pending(ui, render_ctx, pending);
        return;
    }

    push_tiers(ui, result.tiers);
    push_multi_selected_count(ui, result.multi_selected_count);
    ui.global::<EditorModel>()
        .set_status_text(result.status_text.into());
    ui.global::<EditorModel>()
        .set_status_is_problem(result.status_is_problem);
    // Below the staleness early returns above, same as the line it follows: a
    // superseded run must not relabel the strip for a design it no longer describes.
    ui.global::<EditorModel>().set_solve_state(
        if result.status_is_problem {
            "failed"
        } else {
            "solved"
        }
        .into(),
    );
    ui.global::<EditorModel>()
        .set_last_solve_duration_ms(i32::try_from(result.elapsed.as_millis()).unwrap_or(i32::MAX));
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(
            result
                .warnings
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    let (vol_yield_text, carat_text, sg_used_text, fit_text) = result.yield_texts;
    ui.global::<EditorModel>()
        .set_volumetric_yield_text(vol_yield_text.into());
    ui.global::<EditorModel>()
        .set_carat_weight_text(carat_text.into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text(sg_used_text.into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning(fit_text.into());
    // `editor_deep_solve_available`/`editor_optimize_available` (and their hints)
    // depend only on `state.printed_proportions`/free-tier membership, neither of
    // which a solve can change -- and `generation` matching above already confirms
    // no edit touched them since `refresh_editor_panel_stale` last pushed them at
    // edit time. Nothing to refresh here.

    let planes = Arc::new(result.planes);
    let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
    // CAD audit item 58. `started_generation` rather than the live counter: a
    // background solve that finished against an older design must not out-rank the
    // claim a newer edit already made, which is exactly what `may_claim_active_planes`
    // compares. A refused claim here means a newer editor state owns the planes and
    // this result is stale -- the `is_current(seq)` guard above should already have
    // returned, so this is the belt to that braces.
    if !ctx.claim_active_planes(
        Arc::clone(&planes),
        Some(result.gear),
        PlanesOwner::Editor {
            generation: started_generation,
        },
    ) {
        drop(ctx);
        dispatch_pending(ui, render_ctx, pending);
        return;
    }
    ctx.dirty = true;
    let design_gear = ctx.design_gear;
    let camera = CameraPose {
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
    };
    let render_size = (ctx.width, ctx.height);
    drop(ctx);
    let redraw_planes: Vec<(glam::Vec3, f32)> = planes
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    // CAD audit item 117 (HANDOFF from `camera_lighting::contained_request_size`'s
    // own doc comment): this is the "immediately after the next edit or Solve"
    // call site that comment names as still reproducing the picking
    // misregistration -- a completed background solve used to request the redraw
    // at the viewport's own raw size instead of the Path-traced/Both letterboxed
    // rectangle `resubmit_at_current_pose`/`refresh_viewport` already correct for,
    // so the mismatch this fix addresses for a camera drag/zoom/view-mode switch
    // reappeared the moment a background Solve completed -- the common path, not
    // an edge case.
    let size = contained_request_size(view_mode, scaled_viewport_size(ui), render_size);
    let (preview_state, solid_last_solved) = RUNTIME.with(|cell| {
        let rt = cell.borrow();
        (rt.preview_state.clone(), rt.solid_last_solved.clone())
    });
    if let Some(preview_state) = preview_state {
        preview_state.request_redraw_with_gear(redraw_planes, camera, size, view_mode, design_gear);
    }
    if let (Some(solved), Some(cache)) = (result.solved, solid_last_solved) {
        *cache.lock().unwrap_or_else(PoisonError::into_inner) = Some(solved);
    }

    // CAD audit item 141: this background solve just replaced the shared plane
    // slot's contents for THIS design -- see `view::refresh_viewport`'s identical
    // comment for why any material name a PREVIOUS occupant left in
    // `cached_curve_material` must stop being compared against
    // `ctx.material_name` for tilt-dialog staleness the moment that happens.
    ui.global::<TiltModel>()
        .set_cached_curve_material("".into());
    // CAD audit item 147: geometry just changed under the tilt dialog -- if it's
    // open, its four curves and summary badges are about to describe the
    // pre-solve stone as settled results unless a fresh sweep is requested.
    // `AxesCacheKey` (tilt_profile.rs) already hashes the planes, so this is a
    // no-op resweep whenever nothing about the planes actually moved.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }

    dispatch_pending(ui, render_ctx, pending);
}

/// Re-dispatches whatever [`Runtime::pending_dispatch`] [`apply_background_solve_result`]
/// took, if any -- the "dispatch once on completion" half of CAD audit item 112. A
/// no-op when nothing queued up while the just-finished solve was running.
fn dispatch_pending(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    pending: Option<PendingDispatch>,
) {
    if let Some(pending) = pending {
        dispatch_background_solve(
            ui,
            render_ctx,
            pending.design,
            &pending.generation,
            pending.multi_selected,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- should_schedule_auto_solve ---

    #[test]
    fn zero_budget_disables_auto_solve_regardless_of_measurement() {
        assert!(!should_schedule_auto_solve(None, Duration::ZERO));
        assert!(!should_schedule_auto_solve(
            Some(Duration::from_millis(1)),
            Duration::ZERO
        ));
    }

    #[test]
    fn no_measurement_yet_is_scheduled_optimistically() {
        // A fresh design (or one just New/Loaded) has no measurement -- see this
        // function's own doc comment for why that is treated as "try it," not
        // "refuse until proven cheap."
        assert!(should_schedule_auto_solve(None, Duration::from_millis(300)));
    }

    #[test]
    fn a_measurement_under_budget_is_scheduled() {
        assert!(should_schedule_auto_solve(
            Some(Duration::from_millis(120)),
            Duration::from_millis(300)
        ));
    }

    #[test]
    fn a_measurement_at_or_over_budget_is_not_scheduled() {
        assert!(!should_schedule_auto_solve(
            Some(Duration::from_millis(300)),
            Duration::from_millis(300)
        ));
        assert!(!should_schedule_auto_solve(
            Some(Duration::from_secs(6)),
            Duration::from_millis(300)
        ));
    }

    // --- should_solve_synchronously / last_measured_solve_duration (CAD audit item 113) ---

    #[test]
    fn a_fresh_design_with_few_planes_solves_synchronously() {
        assert!(should_solve_synchronously(4, None));
    }

    #[test]
    fn a_fresh_design_with_many_planes_does_not_solve_synchronously() {
        assert!(!should_solve_synchronously(210, None));
    }

    #[test]
    fn a_real_fast_measurement_wins_over_a_high_plane_count() {
        // A design can have many planes yet still measure fast (or vice versa) --
        // once a real measurement exists it alone decides, exactly like
        // `should_schedule_auto_solve`.
        assert!(should_solve_synchronously(
            210,
            Some(Duration::from_millis(50))
        ));
    }

    #[test]
    fn a_real_slow_measurement_loses_even_with_few_planes() {
        assert!(!should_solve_synchronously(4, Some(Duration::from_secs(2))));
    }

    #[test]
    fn last_measured_solve_duration_reflects_record_solve_duration() {
        reset_for_new_design();
        assert_eq!(last_measured_solve_duration(), None);
        record_solve_duration(Duration::from_millis(77));
        assert_eq!(
            last_measured_solve_duration(),
            Some(Duration::from_millis(77))
        );
        reset_for_new_design();
        assert_eq!(last_measured_solve_duration(), None);
    }

    // --- is_current / the sequence-number epoch ---

    #[test]
    fn a_freshly_recorded_sequence_number_is_current() {
        let seq = RUNTIME.with(|cell| {
            let mut rt = cell.borrow_mut();
            rt.current_seq += 1;
            rt.current_seq
        });
        assert!(is_current(seq));
    }

    #[test]
    fn an_older_sequence_number_is_superseded_by_a_newer_dispatch() {
        let old_seq = RUNTIME.with(|cell| {
            let mut rt = cell.borrow_mut();
            rt.current_seq += 1;
            rt.current_seq
        });
        // A second dispatch bumps the counter again, exactly like
        // `dispatch_background_solve` does on every call.
        RUNTIME.with(|cell| cell.borrow_mut().current_seq += 1);
        assert!(!is_current(old_seq));
    }

    // --- cancel_in_flight_solve (CAD audit item 112) ---

    #[test]
    fn cancel_in_flight_solve_supersedes_whatever_sequence_was_current() {
        let seq = RUNTIME.with(|cell| {
            let mut rt = cell.borrow_mut();
            rt.current_seq += 1;
            rt.current_seq
        });
        assert!(is_current(seq));
        cancel_in_flight_solve();
        assert!(
            !is_current(seq),
            "cancelling must supersede whatever solve was in flight, so its \
             eventual completion is dropped as stale"
        );
    }

    #[test]
    fn cancel_in_flight_solve_drops_a_queued_pending_dispatch() {
        RUNTIME.with(|cell| {
            cell.borrow_mut().pending_dispatch = Some(PendingDispatch {
                design: fixture_design(),
                generation: Arc::new(AtomicU64::new(0)),
                multi_selected: BTreeSet::new(),
            });
        });
        cancel_in_flight_solve();
        RUNTIME.with(|cell| {
            assert!(
                cell.borrow().pending_dispatch.is_none(),
                "a cancel must drop whatever dispatch was queued behind the \
                 cancelled solve, not replay it once the in-flight worker \
                 eventually frees the slot"
            );
        });
    }

    #[test]
    fn cancel_in_flight_solve_drops_the_pending_debounce() {
        RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(slint::Timer::default()));
        cancel_in_flight_solve();
        RUNTIME.with(|cell| {
            assert!(
                cell.borrow().debounce.is_none(),
                "a cancel must also drop whatever debounced auto-solve was pending"
            );
        });
    }

    #[test]
    fn cancel_in_flight_solve_leaves_the_measurement_and_design_stash_untouched() {
        reset_for_new_design();
        record_solve_duration(Duration::from_millis(250));
        stash_current_design(3, fixture_design(), BTreeSet::new());
        cancel_in_flight_solve();
        assert_eq!(
            last_measured_solve_duration(),
            Some(Duration::from_millis(250)),
            "cancelling an in-flight solve must not discard this design's own \
             last REAL measurement -- unlike reset_for_new_design, no different \
             design has just replaced it"
        );
        assert!(
            take_matching_design(3).is_some(),
            "cancelling an in-flight solve must not clear an unrelated stash \
             from a concurrent edit's own submit_preview_replan"
        );
    }

    // --- record_solve_duration / reset_for_new_design ---

    #[test]
    fn record_solve_duration_is_visible_to_the_next_schedule_decision() {
        record_solve_duration(Duration::from_millis(42));
        let last = RUNTIME.with(|cell| cell.borrow().last_solve);
        assert_eq!(last, Some(Duration::from_millis(42)));
        reset_for_new_design();
        let last = RUNTIME.with(|cell| cell.borrow().last_solve);
        assert_eq!(last, None);
    }

    // --- banner text ---

    #[test]
    fn solving_banner_pluralizes_the_tier_count() {
        assert!(solving_banner(1, Duration::ZERO).contains("1 tier)"));
        assert!(solving_banner(2, Duration::ZERO).contains("2 tiers)"));
    }

    #[test]
    fn auto_solve_off_note_reports_the_measured_seconds() {
        let text = auto_solve_off_note(Duration::from_millis(5900));
        assert!(text.contains("5.9s"));
    }

    // --- stash_current_design / take_matching_design ---

    /// A minimal, cheap-to-build fixture -- content never matters to these tests,
    /// only that a `Design` value round-trips through the stash unchanged.
    fn fixture_design() -> Design {
        Design::fresh(
            indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
            96,
            8,
            1.54,
        )
    }

    #[test]
    fn take_matching_design_returns_none_when_nothing_stashed() {
        // `reset_for_new_design` (not just letting the field default) since this
        // module's `RUNTIME` is a `thread_local!` a prior test on the same pooled
        // test thread may have already stashed into.
        reset_for_new_design();
        assert!(take_matching_design(1).is_none());
    }

    #[test]
    fn take_matching_design_returns_the_stash_for_its_own_generation() {
        stash_current_design(5, fixture_design(), BTreeSet::from([2]));
        let (design, multi_selected) = take_matching_design(5).expect("stashed at generation 5");
        assert!(
            design.tiers.is_empty(),
            "a fresh design starts with no tiers"
        );
        assert_eq!(multi_selected, BTreeSet::from([2]));
    }

    #[test]
    fn take_matching_design_only_matches_once_per_stash() {
        // A camera-drag `Reproject` frame carries the SAME generation/masts
        // forward as the `Replan` that produced them (see
        // `solid_preview::preview_state::WorkerMemory::generation`'s own doc
        // comment) -- so a second call for the identical generation, with no
        // new `stash_current_design` in between, must find nothing left to
        // give, or every orbit frame after an edit would re-trigger a full
        // tier-table rebuild.
        stash_current_design(11, fixture_design(), BTreeSet::new());
        assert!(
            take_matching_design(11).is_some(),
            "the first call must match"
        );
        assert!(
            take_matching_design(11).is_none(),
            "a second call for the same generation must not match again"
        );
    }

    #[test]
    fn take_matching_design_rejects_a_superseded_generation() {
        stash_current_design(5, fixture_design(), BTreeSet::new());
        assert!(
            take_matching_design(6).is_none(),
            "a newer generation must not match an older stash -- an edit landed \
             after the frame asking for generation 6 was submitted"
        );
    }

    #[test]
    fn take_matching_design_drops_the_pending_debounce_on_a_match() {
        stash_current_design(9, fixture_design(), BTreeSet::new());
        RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(slint::Timer::default()));
        assert!(take_matching_design(9).is_some());
        RUNTIME.with(|cell| {
            assert!(
                cell.borrow().debounce.is_none(),
                "a matching frame must cancel whatever debounced auto-solve was pending"
            );
        });
    }
}
