//! The UI-thread-confined `Runtime` singleton auto-solve's other submodules share,
//! plus the handles [`init`] stashes into it for later retrieval.

use crate::{
    bridge::render_thread::RenderContext,
    gui::{
        editor::{activity::ActivityRegistry, state::EditorState, view::SolidLastSolved},
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_cut_core::Design;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64},
    },
    time::Duration,
};

/// This module's UI-thread-confined state -- see the parent module doc comment for why
/// this is a `thread_local!` rather than a new `EditorState` field.
pub(super) struct Runtime {
    /// Set once by [`init`], from the same `Arc`s `setup_editor_callbacks` already
    /// holds -- lets a completed background solve push into the shared solid preview
    /// exactly like [`super::super::view::refresh_viewport`] does, without this group
    /// needing `EditorState` (or a wider parameter list on
    /// [`super::super::view::refresh_editor_panel_stale`], which many callbacks this
    /// group does not own already call with a fixed signature).
    pub(super) preview_state: Option<Arc<SolidPreviewState>>,
    pub(super) solid_last_solved: Option<SolidLastSolved>,
    /// The shared [`RenderContext`] handle, stashed by
    /// [`stash_render_ctx`] -- see that function's own doc comment for why it is
    /// stashed from an unrelated `setup_*_callback` rather than passed to
    /// [`init`] directly.
    pub(super) render_ctx: Option<Arc<Mutex<RenderContext>>>,
    /// The editor state handle, stashed by [`stash_editor_state`] from
    /// `callbacks::solve_actions::setup_deep_solve_callback` -- lets a `Send`
    /// completion closure (which cannot capture an `Rc`) reach the state back on
    /// the UI thread through [`editor_state`], the same idiom as [`activity`].
    pub(super) editor_state: Option<Rc<RefCell<EditorState>>>,
    /// The shared [`ActivityRegistry`]
    /// handle, stashed by [`init`] the same way [`Self::preview_state`]/
    /// [`Self::solid_last_solved`] are -- lets `dispatch::dispatch_background_solve`/
    /// `dispatch::apply_background_solve_result`/`dispatch::cancel_in_flight_solve`
    /// register/finish the background-solve activity without a new parameter on any
    /// of their own fixed call sites (`gui::editor::setup::setup_solve_cancel_callback`, and
    /// every `callbacks::*` module that calls `dispatch::dispatch_background_solve`
    /// indirectly via `super::super::view::refresh_all`).
    pub(super) activity: Option<Rc<ActivityRegistry>>,
    /// The most recent REAL measured solve wall time (background or synchronous) for
    /// whichever design is currently loaded -- `None` until the first solve since the
    /// last New/Load. Drives `scheduling::should_schedule_auto_solve`.
    pub(super) last_solve: Option<Duration>,
    /// Bumped by every dispatch (explicit-Solve or auto-solve alike); see the parent
    /// module doc comment's "newer background solve" case.
    pub(super) current_seq: u64,
    /// The debounce timer auto-solve restarts on every eligible edit. Replacing this
    /// with a fresh `Timer` cancels whatever the previous one had pending -- Slint
    /// stops a `Timer`'s callback from firing once the `Timer` itself is dropped.
    pub(super) debounce: Option<slint::Timer>,
    /// The design/multi-select snapshot [`super::replan::stash_current_design`] last
    /// recorded, paired with the generation it was cloned at -- see that function's
    /// own doc comment. Read by [`super::replan::take_matching_design`] once a
    /// solid-preview frame lands claiming the SAME generation, so the tier table can
    /// be rebuilt from its already-solved masts instead of triggering a second,
    /// redundant `Design::solve()`.
    ///
    /// `Arc<Design>`, not a plain `Design`: `view::submit_preview_replan_for` builds ONE `Arc<Design>`
    /// snapshot per drained edit-intent frame and hands THIS field a cheap
    /// `Arc::clone` of it -- see that function's own doc comment for the one
    /// remaining deep clone this does not yet eliminate (the solid-preview
    /// worker's own `ReplanRequest::design`, outside this module's files).
    pub(super) current_design: Option<(u64, Arc<Design>, BTreeSet<usize>)>,
    /// True from the moment `dispatch::dispatch_background_solve`
    /// actually spawns a worker thread until that worker's completion is
    /// observed by `dispatch::apply_background_solve_result` -- gates new dispatches
    /// against piling up multiple concurrent multi-second solve threads. See
    /// [`PendingDispatch`] for what happens to a dispatch request that arrives
    /// while this is `true`.
    pub(super) solve_in_flight: bool,
    /// The most recent dispatch request that arrived while [`Runtime::solve_in_flight`]
    /// was `true` -- captured instead of spawning a second concurrent worker.
    /// `dispatch::apply_background_solve_result` takes and re-dispatches this once the
    /// in-flight solve's completion is observed, so a burst of edits against a
    /// slow design collapses to "the one currently running, then the latest
    /// pending one," never an unbounded pile of worker threads. Only ever holds the LATEST request: a second arrival while one is
    /// already pending simply overwrites it.
    pub(super) pending_dispatch: Option<PendingDispatch>,
    /// The debounce timer [`super::replan::schedule_idle_replan_if_stale`]
    /// (re)starts every time a "one edit behind" (partial/subgraph-resolved)
    /// preview frame lands -- same "replacing this cancels whatever was
    /// pending" reasoning as [`Runtime::debounce`].
    pub(super) idle_replan: Option<slint::Timer>,
    /// The cancel flag the CURRENTLY RUNNING worker's `SolveControl::with_cancel`
    /// observes -- `Some` only between the moment `dispatch::dispatch_background_solve`
    /// actually spawns a worker (not merely queues a [`PendingDispatch`]) and the
    /// moment its completion is observed by `dispatch::apply_background_solve_result`,
    /// which clears this back to `None` in the same preamble that frees
    /// [`Runtime::solve_in_flight`]. `dispatch::cancel_in_flight_solve` flips it, which
    /// is what makes "Abandon Solve" actually stop the worker rather than merely
    /// discard its eventual result -- see that function's own doc comment.
    pub(super) current_cancel: Option<Arc<AtomicBool>>,
    /// The [`ActivityRegistry`] id of
    /// the CURRENTLY RUNNING background solve, if any -- same lifetime as
    /// [`Self::current_cancel`] (set only once a worker is actually spawned, cleared
    /// in the same `dispatch::free_in_flight_slot` preamble), so
    /// `dispatch::cancel_in_flight_solve` can finish exactly this activity
    /// immediately, the same "cancel removes it from the list right away" contract
    /// `callbacks::solve_actions`'s `DEEP_SOLVE_ACTIVITY_ID` documents for Deep Solve.
    pub(super) activity_id: Option<u64>,
    /// `true` once
    /// `dispatch::apply_background_solve_result` has already shown the plane-cap
    /// warning toast for the CURRENT unbroken run of over-`MAX_PLANES`
    /// background-solve results, so a design stuck over the cap (every auto-solve
    /// after the first re-reports the same problem) gets exactly one toast, not one
    /// per edit -- the status-strip sentence (`state::status_text_and_is_problem*`,
    /// always current) already says it on every refresh regardless. Reset to `false`
    /// the moment a result comes back that is NOT over the cap, so the toast fires
    /// again if the design goes back over it later, and by
    /// [`EditorState::fresh`]/`replace_wholesale`'s own generation bump indirectly
    /// (a new design's first over-cap result finds this still `false` from the
    /// previous design only if that previous design's own last result happened to
    /// be over-cap too -- a rare double coincidence, not worth a dedicated reset
    /// hook on a struct this module does not own).
    pub(super) too_many_planes_toasted: bool,
}

impl Runtime {
    pub(super) const fn new() -> Self {
        Self {
            preview_state: None,
            solid_last_solved: None,
            render_ctx: None,
            editor_state: None,
            activity: None,
            last_solve: None,
            current_seq: 0,
            debounce: None,
            current_design: None,
            solve_in_flight: false,
            pending_dispatch: None,
            idle_replan: None,
            current_cancel: None,
            activity_id: None,
            too_many_planes_toasted: false,
        }
    }
}

/// A `dispatch::dispatch_background_solve` call suppressed by
/// [`Runtime::solve_in_flight`] -- see that field's own doc comment.
pub(super) struct PendingDispatch {
    pub(super) design: Design,
    /// the value `generation` held at the MOMENT this dispatch was queued
    /// (captured once, by the caller, in `dispatch::queue_or_claim_solve_slot`),
    /// not a value re-read from the live `Arc` later when this gets replayed --
    /// `generation` is the SAME shared counter `EditorState` keeps bumping, so
    /// reading it again at replay time would silently pick up every edit that
    /// landed while this request sat queued and hand `design` back as if it were
    /// current. `apply::apply_background_solve_result`'s own `generation.load(..)
    /// != started_generation` check is what this pairs with once the replay's
    /// own completion arrives.
    pub(super) started_generation: u64,
    pub(super) generation: Arc<AtomicU64>,
    pub(super) multi_selected: BTreeSet<usize>,
}

thread_local! {
    pub(super) static RUNTIME: RefCell<Runtime> = const { RefCell::new(Runtime::new()) };
}

/// Stashes the shared solid-preview handles this module needs -- called once from
/// `super::super::setup_editor_callbacks`, before any callback (and therefore any
/// possible dispatch) is wired up.
pub(in crate::gui::editor) fn init(
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    activity: &Rc<ActivityRegistry>,
) {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.preview_state = Some(Arc::clone(preview_state));
        rt.solid_last_solved = Some(Arc::clone(solid_last_solved));
        rt.activity = Some(Rc::clone(activity));
    });
}

/// Returns the shared solid-preview handle [`init`] stashed, if it has run yet --
/// lets `callbacks::tier_actions`'s hover/click/selection-changed/multi-select
/// callbacks resubmit a merged `FacetOverlay` (see that module's own doc comment)
/// without `gui::editor::mod`'s fixed call sites, which this module does not own,
/// needing a new parameter threaded through them.
pub(in crate::gui::editor) fn preview_state() -> Option<Arc<SolidPreviewState>> {
    RUNTIME.with(|cell| cell.borrow().preview_state.clone())
}

/// Returns the shared "last solved" cache [`init`] stashed, if it has run yet --
/// same reasoning as [`preview_state`], for the callbacks that need to build a
/// `facet_map::FacetMap` against the last rendered frame's masts without a new
/// parameter on their own fixed call sites.
pub(in crate::gui::editor) fn solid_last_solved() -> Option<SolidLastSolved> {
    RUNTIME.with(|cell| cell.borrow().solid_last_solved.clone())
}

/// Returns the shared [`ActivityRegistry`] [`init`] stashed, if it has run yet --
/// same reasoning as [`preview_state`]/[`solid_last_solved`], so
/// `callbacks::solve_actions`'s Deep Solve/Optimize dispatch and completion
/// handlers can register/finish their own activities without a new parameter on
/// their own fixed call sites. `None` only if this is called before
/// [`setup_editor_callbacks`](super::super::setup_editor_callbacks) has run `init`,
/// which cannot happen from any real Slint callback.
#[must_use]
pub(in crate::gui::editor) fn activity() -> Option<Rc<ActivityRegistry>> {
    RUNTIME.with(|cell| cell.borrow().activity.clone())
}

/// Stashes the editor state handle for [`editor_state`] below -- called from
/// `callbacks::solve_actions::setup_deep_solve_callback`, which already holds the
/// `Rc` and runs once on the UI thread before the event loop starts.
pub(in crate::gui::editor) fn stash_editor_state(state: &Rc<RefCell<EditorState>>) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().editor_state = Some(Rc::clone(state));
    });
}

/// Returns the editor state handle [`stash_editor_state`] stashed, if it has run
/// yet. Used by completion closures that must be `Send` and therefore cannot
/// capture the `Rc` themselves; they run on the UI thread, where this is valid.
#[must_use]
pub(in crate::gui::editor) fn editor_state() -> Option<Rc<RefCell<EditorState>>> {
    RUNTIME.with(|cell| cell.borrow().editor_state.clone())
}

/// Stashes the shared [`RenderContext`] handle for [`render_ctx`] below --
/// called from `callbacks::tier_actions::
/// setup_toggle_detach_callback`, one of several `setup_*_callback`s already
/// given this `Arc` directly by `gui::editor::mod::setup_editor_callbacks`
/// (a file this module does not own, so it is not the place to add a NEW parameter to). That call runs
/// synchronously, before `setup_editor_callbacks` returns and the event loop
/// starts, so [`render_ctx`] is already populated by the time a user can hover
/// or click anything -- the ordering among `setup_*_callback` calls within that
/// one function does not matter.
pub(in crate::gui::editor) fn stash_render_ctx(render_ctx: &Arc<Mutex<RenderContext>>) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().render_ctx = Some(Arc::clone(render_ctx));
    });
}

/// Returns the shared [`RenderContext`] handle [`stash_render_ctx`] stashed, if
/// it has run yet -- lets `callbacks::tier_actions`'s Solid-view hover/click
/// callbacks read the configured render resolution without
/// a new parameter on their own fixed call sites in
/// `gui::editor::mod`, which this module does not own.
pub(in crate::gui::editor) fn render_ctx() -> Option<Arc<Mutex<RenderContext>>> {
    RUNTIME.with(|cell| cell.borrow().render_ctx.clone())
}
