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
    activity::ActivityRegistry,
    state::{
        EditorState, apply_multi_selection, cutting_schedule_rows, design_to_gpu_planes,
        girdle_and_ratio_texts, manufacturability_warnings_tagged, preform_mm_texts,
        preform_y_offset_mm_text, proportions_texts, push_multi_selected_count, push_tiers,
        status_text_and_is_problem, status_text_and_is_problem_from_solved, tier_items,
        tier_items_from_solved, yield_report_texts, yield_report_texts_from_solved,
    },
    view::{
        ReplanSource, SolidLastSolved, facet_count_from_solved, girdle_and_ratio_texts_from_solved,
        preform_mm_texts_from_solved, proportions_texts_from_solved, scaled_viewport_size,
        submit_preview_replan_for,
    },
};
use crate::{
    AngleItem, EditorModel, EditorTierItem, MainWindow, SolidPreviewModel, TiltModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        render::camera_lighting::contained_request_size,
        show_toast,
        solid_preview::preview_state::{CameraPose, SolidPreviewState},
    },
};
use indicatrix::{
    geometry::{
        GpuFacetPlane,
        meet_solver::{SolveControl, SolveError, SolveStrategy, SolvedTier, solve_meet_points},
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{Design, DesignSolveError};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Solve cost is driven by plane count, not tier count -- a
/// wide-orbit round-brilliant tier emits many planes at once (the corpus's largest
/// measured design is 103 tiers but 210 planes, `mod.rs`'s own baseline), so a small
/// tier count can still be an expensive, UI-thread-blocking solve.
///
/// A rough estimate, not a re-measurement of this crate's own corpus: roughly double
/// a 16-tier reference point at the corpus's own ~2 planes-per-tier ratio, kept
/// comfortably under the 210-plane/5.9s worst case.
const SYNC_SOLVE_PLANE_LIMIT: usize = 32;

/// Once a design has a real measured solve time, that measurement
/// is a far better signal than any plane-count estimate -- a design that solved in
/// under this long most recently is still fine to solve again synchronously,
/// regardless of how many planes it has.
const SYNC_SOLVE_TIME_LIMIT: Duration = Duration::from_millis(500);

/// Whether a solve for a design with `plane_count` planes should still run
/// synchronously on the UI thread (the "New"/Load/explicit-Solve fast path,
/// [`super::view::refresh_all`]'s sync branch) rather than through
/// [`dispatch_background_solve`], which handles a plane-count cutoff instead of a
/// tier-count one. Prefers this design's own last REAL measured solve time
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

/// How long a "one edit behind" preview frame waits for a
/// FOLLOW-UP edit before concluding the solid-preview worker has gone idle and
/// resubmitting a full replan of its own -- see
/// [`schedule_idle_replan_if_stale`]. Longer than [`AUTO_SOLVE_DEBOUNCE`]:
/// this is specifically waiting for the EDIT stream itself to quiet down (not
/// just one keystroke's own burst), so firing on the same short window would
/// often race a still-typing cutter's next nudge.
const IDLE_REPLAN_DEBOUNCE: Duration = Duration::from_millis(400);

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
    /// The shared [`RenderContext`] handle, stashed by
    /// [`stash_render_ctx`] -- see that function's own doc comment for why it is
    /// stashed from an unrelated `setup_*_callback` rather than passed to
    /// [`init`] directly.
    render_ctx: Option<Arc<Mutex<RenderContext>>>,
    /// The editor state handle, stashed by [`stash_editor_state`] from
    /// `callbacks::solve_actions::setup_deep_solve_callback` -- lets a `Send`
    /// completion closure (which cannot capture an `Rc`) reach the state back on
    /// the UI thread through [`editor_state`], the same idiom as [`activity`].
    editor_state: Option<Rc<RefCell<EditorState>>>,
    /// The shared [`ActivityRegistry`]
    /// handle, stashed by [`init`] the same way [`Self::preview_state`]/
    /// [`Self::solid_last_solved`] are -- lets [`dispatch_background_solve`]/
    /// [`apply_background_solve_result`]/[`cancel_in_flight_solve`] register/finish
    /// the background-solve activity without a new parameter on any of their own
    /// fixed call sites (`mod.rs`'s `setup_solve_cancel_callback`, and every
    /// `callbacks::*` module that calls [`dispatch_background_solve`] indirectly via
    /// [`super::view::refresh_all`]).
    activity: Option<Rc<ActivityRegistry>>,
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
    /// `Design::solve()`.
    ///
    /// `Arc<Design>`, not a plain `Design`: `view::submit_preview_replan_for` builds ONE `Arc<Design>`
    /// snapshot per drained edit-intent frame and hands THIS field a cheap
    /// `Arc::clone` of it -- see that function's own doc comment for the one
    /// remaining deep clone this does not yet eliminate (the solid-preview
    /// worker's own `ReplanRequest::design`, outside this module's files).
    current_design: Option<(u64, Arc<Design>, BTreeSet<usize>)>,
    /// True from the moment [`dispatch_background_solve`]
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
    /// pending one," never an unbounded pile of worker threads. Only ever holds the LATEST request: a second arrival while one is
    /// already pending simply overwrites it.
    pending_dispatch: Option<PendingDispatch>,
    /// The debounce timer [`schedule_idle_replan_if_stale`]
    /// (re)starts every time a "one edit behind" (partial/subgraph-resolved)
    /// preview frame lands -- same "replacing this cancels whatever was
    /// pending" reasoning as [`Runtime::debounce`].
    idle_replan: Option<slint::Timer>,
    /// The cancel flag the CURRENTLY RUNNING worker's `SolveControl::with_cancel`
    /// observes -- `Some` only between the moment [`dispatch_background_solve`]
    /// actually spawns a worker (not merely queues a [`PendingDispatch`]) and the
    /// moment its completion is observed by [`apply_background_solve_result`],
    /// which clears this back to `None` in the same preamble that frees
    /// [`Runtime::solve_in_flight`]. [`cancel_in_flight_solve`] flips it, which is
    /// what makes "Abandon Solve" actually stop the worker rather than merely
    /// discard its eventual result -- see that function's own doc comment.
    current_cancel: Option<Arc<AtomicBool>>,
    /// The [`ActivityRegistry`] id of
    /// the CURRENTLY RUNNING background solve, if any -- same lifetime as
    /// [`Self::current_cancel`] (set only once a worker is actually spawned, cleared
    /// in the same [`free_in_flight_slot`] preamble), so [`cancel_in_flight_solve`]
    /// can finish exactly this activity immediately, the same "cancel removes it
    /// from the list right away" contract `callbacks::solve_actions`'s
    /// `DEEP_SOLVE_ACTIVITY_ID` documents for Deep Solve.
    activity_id: Option<u64>,
    /// `true` once
    /// [`apply_background_solve_result`] has already shown the plane-cap warning
    /// toast for the CURRENT unbroken run of over-`MAX_PLANES` background-solve
    /// results, so a design stuck over the cap (every auto-solve after the first
    /// re-reports the same problem) gets exactly one toast, not one per edit --
    /// the status-strip sentence (`state::status_text_and_is_problem*`, always
    /// current) already says it on every refresh regardless. Reset to `false` the
    /// moment a result comes back that is NOT over the cap, so the toast fires
    /// again if the design goes back over it later, and by
    /// [`EditorState::fresh`]/`replace_wholesale`'s own generation bump indirectly
    /// (a new design's first over-cap result finds this still `false` from the
    /// previous design only if that previous design's own last result happened to
    /// be over-cap too -- a rare double coincidence, not worth a dedicated reset
    /// hook on a struct this module does not own).
    too_many_planes_toasted: bool,
}

impl Runtime {
    const fn new() -> Self {
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

/// A [`dispatch_background_solve`] call suppressed by [`Runtime::solve_in_flight`] --
/// see that field's own doc comment.
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
pub(super) fn init(
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

/// Returns the shared [`ActivityRegistry`] [`init`] stashed, if it has run yet --
/// same reasoning as [`preview_state`]/[`solid_last_solved`], so
/// `callbacks::solve_actions`'s Deep Solve/Optimize dispatch and completion
/// handlers can register/finish their own activities without a new parameter on
/// their own fixed call sites. `None` only if this is called before
/// [`setup_editor_callbacks`](super::setup_editor_callbacks) has run `init`, which
/// cannot happen from any real Slint callback.
#[must_use]
pub(super) fn activity() -> Option<Rc<ActivityRegistry>> {
    RUNTIME.with(|cell| cell.borrow().activity.clone())
}

/// Stashes the editor state handle for [`editor_state`] below -- called from
/// `callbacks::solve_actions::setup_deep_solve_callback`, which already holds the
/// `Rc` and runs once on the UI thread before the event loop starts.
pub(super) fn stash_editor_state(state: &Rc<RefCell<EditorState>>) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().editor_state = Some(Rc::clone(state));
    });
}

/// Returns the editor state handle [`stash_editor_state`] stashed, if it has run
/// yet. Used by completion closures that must be `Send` and therefore cannot
/// capture the `Rc` themselves; they run on the UI thread, where this is valid.
#[must_use]
pub(super) fn editor_state() -> Option<Rc<RefCell<EditorState>>> {
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
pub(super) fn stash_render_ctx(render_ctx: &Arc<Mutex<RenderContext>>) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().render_ctx = Some(Arc::clone(render_ctx));
    });
}

/// Returns the shared [`RenderContext`] handle [`stash_render_ctx`] stashed, if
/// it has run yet -- lets `callbacks::tier_actions`'s Solid-view hover/click
/// callbacks read the configured render resolution without
/// a new parameter on their own fixed call sites in
/// `gui::editor::mod`, which this module does not own.
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
/// instead of a second internal [`Design::solve`]. `state::mod.rs`
/// is a file this module does not own, so this one small conversion
/// lives here instead; it mirrors `design_to_gpu_planes`'s own sign-flip convention
/// exactly (see this crate's `mod.rs` doc comment, "Feeding the viewport").
///
/// `pub(super)` (not just private) since `view::refresh_viewport` also needs this,
/// so that call site solves the design exactly once instead of a second,
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

/// [`Design::solve`]'s own cancellable counterpart for `dispatch_background_solve`'s
/// worker specifically -- same legacy [`SolveError::TooManyPlanes`] fallback
/// [`Design::solve`] documents on itself (reproduced here rather than reused,
/// since that method's own `SolveControl::default()` can never observe a real
/// cancel, so it cannot route a caller-supplied `cancel` flag through).
///
/// [`SolveError::Cancelled`] is returned as a real `Err`, not silently swallowed:
/// the caller treats it exactly like any other solve error (`panel_inputs` shows
/// the design as unsolved), and in practice never reaches the screen at all --
/// `cancel_in_flight_solve` ("Abandon Solve") bumps `Runtime::current_seq` on the
/// very same click that sets this flag, so `apply_background_solve_result`'s
/// existing `is_current(seq)` check discards the whole result before any of this
/// would be shown. See `cancel_in_flight_solve`'s own doc comment.
///
/// `pub(super)` (not just private): `solve_service`'s own worker reuses this
/// exact fallback for a Deep Solve run's baseline plain solve
/// (`solve_service::run_solve`'s `SolveKind::Verified` arm), so that baseline never
/// runs on the UI thread either -- see that call site's own comment.
pub(super) fn solve_cancellably(
    design: &Design,
    cancel: &AtomicBool,
) -> Result<Vec<SolvedTier>, DesignSolveError> {
    match design.solve_with(&SolveControl::with_cancel(cancel)) {
        Err(DesignSolveError::Solve(SolveError::TooManyPlanes { .. })) => Ok(solve_meet_points(
            design.meta.gear_teeth_abs(),
            &design.meet_tier_inputs(),
        )),
        other => other,
    }
}

/// `MAX_PLANES` (400,
/// `indicatrix::geometry::meet_solver::MAX_PLANES`) means an over-cap design
/// silently renders as an ordinary (misleading) "Degenerate"/"Unbounded" status --
/// `Design::solve()`/[`solve_cancellably`] both reproduce the solver's own
/// all-`SolveStrategy::Failed` fallback for that case rather than an error, exactly
/// so the rest of this module's background-solve/panel machinery keeps working
/// unchanged for it (see [`solve_cancellably`]'s own doc comment) -- but that also
/// means nothing ever tells the cutter the REAL problem is plane count. Runs
/// `Design::solve_with` (which surfaces the real
/// [`SolveError::TooManyPlanes`] instead of swallowing it) purely to check for this
/// one condition; `Some` with a cutter-actionable sentence a faceter understands
/// ("reduce symmetry or split the design") iff it applies, `None` otherwise
/// (including every ordinary solve error, which the caller's own existing message
/// already covers).
///
/// A CHEAP (no solve) pre-filter for [`too_many_planes_message`]: `true` iff
/// `solved` carries the exact tell [`indicatrix::geometry::meet_solver::solve::
/// SolveContext::failed_solved`] stamps into every non-anchor tier's own
/// [`SolvedTier::detail`] for its all-`SolveStrategy::Failed` over-`MAX_PLANES`
/// fallback ("... above the N-plane cap for candidate-vertex enumeration").
/// [`too_many_planes_message`]'s own `solve_with` re-check is the only
/// AUTHORITATIVE source of `planes`/`max` (this never parses those numbers back
/// out of the detail string -- a private wording this crate does not own, from
/// `crates/indicatrix`, off limits to this module -- so a caller still calls that
/// function for the real numbers), but calling `solve_with` on every ordinary
/// `Degenerate`/`Unbounded` result -- the overwhelming majority of which have
/// nothing to do with the plane cap at all -- would silently double an
/// already-real solve cost for no reason. This lets both `state::
/// status_text_and_is_problem`/`_from_solved` skip that re-check entirely unless
/// `solved` (which either already has, from its own preceding `Design::solve()`)
/// actually shows the tell. A false negative here only means an ordinary
/// (unhelpful but not wrong) "Degenerate"/"Unbounded" message shows instead of
/// the plane-cap one -- never a false plane-cap message shown for an unrelated
/// failure, since [`too_many_planes_message`] itself is still the one that
/// decides.
#[must_use]
pub(super) fn likely_hit_plane_cap(solved: &[SolvedTier]) -> bool {
    solved
        .iter()
        .any(|t| matches!(t.strategy, SolveStrategy::Failed) && t.detail.contains("plane cap"))
}

/// `pub(super)`: called from `state::status_text_and_is_problem`/
/// `_from_solved`'s own `Degenerate`/`Unbounded` arms (`state/mod.rs`, not this
/// module -- neither this module nor `state/mod.rs` is the place to relocate this
/// function outside its own file, so the check itself lives here instead and is
/// called across the module boundary),
/// each of which was ALREADY paying for a second, redundant `design.solve()` call
/// there (to build the suspects/escaping-tier text) -- swapping that call for this
/// one adds no new solve cost for a design under the cap; only a design actually
/// over it (for which neither `degenerate_suspects_note` nor `escaping_tier_text`
/// is meaningful against a fabricated mast list anyway) takes a different path.
#[must_use]
pub(super) fn too_many_planes_message(design: &Design) -> Option<String> {
    match design.solve_with(&SolveControl::default()) {
        Err(DesignSolveError::Solve(SolveError::TooManyPlanes { planes, max })) => Some(format!(
            "This design has {planes} facet planes; the solver supports up to {max} -- reduce \
             symmetry or split the design."
        )),
        _ => None,
    }
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

/// The currently loaded design's last REAL measured solve time, if any -- the
/// production counterpart to the `#[cfg(test)]`-only
/// [`last_measured_solve_duration`] below, exposed so [`super::view::refresh_all`] can pass a real measurement to
/// [`should_solve_synchronously`] for every caller that is NOT replacing
/// `EditorState` wholesale (an explicit Solve/Adopt/Optimize Apply/etc. on the
/// design already loaded) instead of unconditionally wiping it via
/// [`reset_for_new_design`] first, which made that rule unreachable on exactly
/// the actions it exists for.
#[must_use]
pub(super) fn last_solve() -> Option<Duration> {
    RUNTIME.with(|cell| cell.borrow().last_solve)
}

/// This design's last measured solve time -- a test-only window onto [`Runtime`]'s
/// thread-local state, so `record_solve_duration`/`reset_for_new_design` can be
/// asserted on directly. Identical in body to the production [`last_solve`] getter
/// just above; kept as its own `#[cfg(test)]` function (rather than tests calling
/// [`last_solve`] directly) purely so a rename of either one does not silently
/// change what the other means to read.
///
/// `should_schedule_auto_solve` reads `Runtime::last_solve` itself (via [`on_edit`]).
/// `view::refresh_all`'s sync-vs-background decision
/// reads [`last_solve`] for every caller that is NOT replacing `EditorState`
/// wholesale, and passes `None` only for the `wholesale: true` case, right after
/// [`reset_for_new_design`] has cleared the measurement -- reaching back for the
/// value it just cleared would judge a freshly loaded design by the previous one's
/// solve time, precisely the case where the two have nothing to do with each other.
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
/// # Why bumping `current_seq` here still matters
///
/// A background solve captures its own `generation: Arc<AtomicU64>` snapshot at
/// dispatch time (see [`dispatch_background_solve`]). New/Load Selected/Open Native
/// all go through `EditorState::replace_wholesale` (see e.g.
/// `callbacks::tier_actions::apply_loaded_design`) -- which, despite the name,
/// REUSES the very SAME `Arc<AtomicU64>` across the replacement and bumps it once
/// (see `EditorState::replace_wholesale`'s own doc comment). So [`apply_background_solve_result`]'s
/// `generation` check alone WOULD already catch a dispatch from the design being
/// replaced, once that dispatch's (potentially multi-second) completion finally
/// arrives -- but bumping [`Runtime::current_seq`] here retires it immediately
/// instead: every one of this function's callers reaches it (via `refresh_all`,
/// called unconditionally at the top of every New/Load Selected/Open Native path)
/// BEFORE that stale completion's `upgrade_in_event_loop` closure can run (both run
/// on the UI/event-loop thread, so ordering is never racy), so [`is_current`] now
/// correctly fails right away -- both the "Solving..." banner/ticker
/// ([`spawn_solving_ticker`]'s own `is_current(seq)` check) and the eventual
/// completion stop describing the design that was just replaced, rather than the
/// banner sitting on a stale "Solving... (103 tiers)" message until that old
/// dispatch happens to finish and the generation check silently drops it.
///
/// Dropping `debounce` cancels whatever single-shot auto-solve [`on_edit`] had
/// pending against the design being replaced -- Slint stops a `Timer`'s callback
/// once the `Timer` itself is dropped (see [`Runtime::debounce`]'s own doc comment).
/// Dropping `idle_replan` is the identical fix for
/// [`schedule_idle_replan_if_stale`]'s own timer: without it, a partial-frame idle
/// replan armed for the design being replaced would otherwise fire up to
/// [`IDLE_REPLAN_DEBOUNCE`] later and resubmit that OLD design's masts on top of
/// whatever this reset is about to load -- see that function's own doc comment for
/// the epoch check this pairs with.
///
/// Safe to call at any time: bumping `current_seq`/dropping `debounce`/`idle_replan`
/// with no solve in flight is a no-op beyond the wasted counter tick.
pub(super) fn reset_for_new_design() {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.last_solve = None;
        rt.current_seq += 1;
        rt.debounce = None;
        rt.idle_replan = None;
        // Not load-bearing for correctness -- `EditorState::replace_wholesale`
        // reuses and bumps the SAME `Arc<AtomicU64>` (see its own doc comment),
        // so a stash from before this replacement already carries an older
        // generation number that can never again equal the live one. Cleared
        // anyway so a large superseded design isn't held onto for no reason.
        rt.current_design = None;
        // A pending dispatch queued behind an in-flight solve
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

/// Invalidates whatever background solve is currently in flight or queued behind
/// it, and stops the worker -- see [`Runtime::current_cancel`]'s own doc comment.
///
/// Bumping `current_seq` makes the in-flight worker's eventual completion fail its
/// `is_current(seq)` check (the same mechanism [`reset_for_new_design`] already
/// uses for a wholesale design replacement), so [`apply_background_solve_result`]
/// still runs when that worker finishes (freeing [`Runtime::solve_in_flight`] for
/// the next dispatch) but touches nothing else. Setting [`Runtime::current_cancel`]
/// (when one is running -- `None` while nothing is in flight, e.g. a stray click
/// after the last worker already finished) is what makes the worker itself notice:
/// `dispatch_background_solve`'s worker threads `SolveControl::with_cancel` through
/// `Design::solve_with`, which checks the flag at every cancel point
/// `indicatrix::geometry::meet_solver` exposes (per-sweep, per-pipeline-run) --
/// typically single-digit milliseconds, rather than waiting the multiple seconds a
/// full run-to-completion could take. Also drops any [`PendingDispatch`] queued behind the cancelled
/// solve (it named the design as of the click that queued it; a fresh dispatch
/// from the CURRENT design will replace it via the ordinary edit path if one is
/// still needed) and the debounced auto-solve timer [`on_edit`] may have pending.
///
/// Also drops [`Runtime::idle_replan`] for the
/// same reason [`reset_for_new_design`] does: a cancelled solve leaves nothing
/// guaranteeing the solid preview will settle on its own, but a stale partial-frame
/// idle-replan timer left armed from before the cancel would otherwise fire later
/// and resubmit a replan the cutter no longer asked for.
///
/// Deliberately narrower than [`reset_for_new_design`]: `last_solve`/
/// `current_design` are left untouched, since cancelling a solve does not change
/// which design is loaded or invalidate that design's previous measurement.
///
/// The UI-visible half of "Abandon Solve" -- clearing `EditorModel.solve_running`/
/// setting `solve_state`/`status_text` back to an "abandoned" message on the SAME
/// click -- is `gui::editor::mod::setup_solve_cancel_callback`, which calls this
/// function first.
pub(super) fn cancel_in_flight_solve() {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        if let Some(cancel) = rt.current_cancel.as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
        rt.current_seq += 1;
        rt.debounce = None;
        rt.pending_dispatch = None;
        rt.idle_replan = None;
    });
}

/// [`cancel_in_flight_solve`] plus the SAME `EditorModel` reset
/// `mod.rs::setup_solve_cancel_callback` applies on the command bar's own "Abandon
/// Solve" button -- used as the background-solve activity's own
/// [`ActivityRegistry`] cancel closure (see [`dispatch_background_solve`]), so
/// cancelling from the status strip's activity list is indistinguishable from
/// clicking Abandon Solve itself. `mod.rs`'s own callback is left untouched (it is
/// not this module's file to edit beyond a registration line) and keeps doing the
/// same two things separately; this exists only so the SECOND cancel entry point
/// this module adds behaves identically rather than only stopping the worker without
/// resetting what the command bar shows.
pub(super) fn cancel_in_flight_solve_from_activity(ui_weak: &Weak<MainWindow>) {
    cancel_in_flight_solve();
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let model = ui.global::<EditorModel>();
    model.set_solve_running(false);
    model.set_solve_state("stale".into());
    model.set_status_text("Solve abandoned -- click Solve when you are ready.".into());
    model.set_status_is_problem(true);
}

fn is_current(seq: u64) -> bool {
    RUNTIME.with(|cell| cell.borrow().current_seq == seq)
}

/// Records `design`/`multi_selected` alongside the `generation` they were
/// snapshotted at. Called by `view::submit_preview_replan` on every replan, with
/// an `Arc::clone` of the SAME `Arc<Design>` snapshot that drained edit-intent
/// frame built for `ReplanRequest::design` too (one `Design` clone per drained
/// frame, shared via `Arc` rather than cloned again for each consumer) -- see
/// [`take_matching_design`] for the reader half of this mechanism.
pub(super) fn stash_current_design(
    generation: u64,
    design: Arc<Design>,
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
pub(super) fn take_matching_design(generation: u64) -> Option<(Arc<Design>, BTreeSet<usize>)> {
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

/// Once a "one edit behind" preview frame lands (a partial,
/// subgraph-resolved replan -- `SolidPreviewModel.stale`, pushed by
/// `gui::SlintSolidSink::apply` from `PreviewFrame::stale`, a file this module
/// does not own), the cutter would otherwise be stuck reading the stale banner until another
/// edit happened to trigger a fresh replan. Debounces (`IDLE_REPLAN_DEBOUNCE`) a
/// follow-up FULL replan of the SAME design/generation instead, so the preview
/// catches up on its own once the edit stream actually goes idle -- cheap,
/// since the partial resolve's masts are already chained forward; this only
/// asks the worker to verify them properly rather than compute anything new.
///
/// A no-op when [`init`] has not run yet. The staleness check itself
/// deliberately happens only at FIRE time, inside the scheduled closure below,
/// never here at schedule time: `editor::apply_matching_preview_frame` (this
/// function's one caller) runs BEFORE `gui::SlintSolidSink::apply` (a file this
/// module does not own) pushes THIS SAME frame's own `SolidPreviewModel.stale` value --
/// reading it here would see the PREVIOUS frame's staleness instead. Scheduling
/// unconditionally on every matched frame and checking once the debounce
/// elapses costs nothing but a Timer that a closer-together edit would replace
/// anyway (same trade-off [`on_edit`]'s own debounce already makes).
///
/// Called from `editor::apply_matching_preview_frame` right after
/// `view::push_solved_preview`, with the SAME `design`/`multi_selected` that
/// call's own [`take_matching_design`] just handed back -- this module has no
/// other way to reach a `Design` snapshot for `generation` (see the module doc
/// comment, "Why a `thread_local!`, not a new `EditorState` field").
///
/// The "anything newer since" check at fire time deliberately does not compare
/// against a live generation counter (this module holds none for the
/// solid-preview replan path -- see above): instead it re-checks
/// [`Runtime::current_design`], which every [`super::view::submit_preview_replan`]
/// call (the only other writer) repopulates on its own. If a further edit
/// landed after this frame, that edit's own replan already stashed a NEWER
/// entry there, and this fires nothing (a fresher replan is already in flight
/// or has already landed); if it is still empty, no further edit happened, and
/// resubmitting for `generation` -- once `SolidPreviewModel.stale` confirms
/// there is still something to catch up on -- is exactly this frame's own
/// unfinished business.
///
/// # `current_design.is_none()` alone is ambiguous
///
/// `current_design` also reads `None` for a reason that has NOTHING to do with
/// "no further edit happened": [`reset_for_new_design`]/[`cancel_in_flight_solve`]
/// both clear it deliberately, on every New/Load/Cancel. Scenario the bare check
/// missed: a partial frame for design A lands and arms this timer; within
/// [`IDLE_REPLAN_DEBOUNCE`], the cutter loads design B (the background branch,
/// `SolidPreviewModel.stale` stays `true` from A's own partial frame) -- `
/// current_design` is `None` either way, so the old check could not tell "A's edit
/// stream went idle" apart from "A was replaced by B entirely," and would
/// resubmit A's stashed masts/generation on top of B, whose rows/status/warnings/
/// yield A's stale completion would then overwrite until B's own solve lands.
///
/// The fix: snapshot [`Runtime::current_seq`] at SCHEDULE time (`epoch` below) and
/// require it still match at fire time, via [`is_current`] -- the same epoch
/// [`reset_for_new_design`]/[`cancel_in_flight_solve`] already bump (and now also
/// use to drop this very timer, belt-and-braces). An ordinary further edit does
/// NOT bump `current_seq` (only a dispatch/reset/cancel does), so this adds no
/// false negative for the case the bare `current_design` check already handles
/// correctly.
pub(super) fn schedule_idle_replan_if_stale(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    design: &Design,
    multi_selected: &BTreeSet<usize>,
) {
    let Some((preview_state, solid_last_solved)) = RUNTIME.with(|cell| {
        let rt = cell.borrow();
        rt.preview_state.clone().zip(rt.solid_last_solved.clone())
    }) else {
        return;
    };
    let epoch = RUNTIME.with(|cell| cell.borrow().current_seq);
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let design = design.clone();
    let multi_selected = multi_selected.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        IDLE_REPLAN_DEBOUNCE,
        move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let nothing_newer_since = RUNTIME.with(|cell| {
                let rt = cell.borrow();
                rt.current_design.is_none() && rt.current_seq == epoch
            });
            if !nothing_newer_since || !ui.global::<SolidPreviewModel>().get_stale() {
                return;
            }
            submit_preview_replan_for(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                ReplanSource {
                    design: &design,
                    generation,
                    multi_selected: &multi_selected,
                },
                BTreeSet::new(),
                true,
            );
        },
    );
    RUNTIME.with(|cell| cell.borrow_mut().idle_replan = Some(timer));
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
    /// `(tier index, warning text)` pairs, not a flattened
    /// text-only list -- see [`PanelInputs::warnings`]'s own doc comment.
    warnings: Vec<(usize, String)>,
    yield_texts: (String, String, String, String),
    planes: Vec<GpuFacetPlane>,
    solved: Option<Vec<SolvedTier>>,
    /// `true` iff `status_text`
    /// above is [`too_many_planes_message`]'s plane-cap sentence -- read by
    /// [`apply_background_solve_result`] to fire the once-per-design warning
    /// toast. See [`Runtime::too_many_planes_toasted`]'s own doc comment.
    too_many_planes: bool,
    /// Index-wheel tooth count and reference angle, for the diagram view's wheel.
    gear: (u32, f32),
    /// `multi_selected.len()` at dispatch time, for the tier table's "N selected"
    /// header indicator -- read on the UI thread by [`apply_background_solve_result`],
    /// never recomputed from `EditorState` there (a solve completion has no access
    /// to it beyond this snapshot).
    multi_selected_count: usize,
    /// The SAME `Design` snapshot this worker
    /// solved, moved in here rather than dropped once `panel_inputs`/`solved`
    /// are built -- [`apply_background_solve_result`] needs it to build
    /// proportions/girdle-ratio/preform-mm/facet-count/cutting-schedule the same
    /// way [`super::view::refresh_editor_panel_from_solve`] does, so the
    /// background-solve path builds these too instead of leaving them stale
    /// until the next foreground refresh (see that function's own doc comment).
    /// Free: this `Design` was already cloned into the
    /// worker closure for [`Design::solve`]; nothing else in this struct needs
    /// it again after construction, so moving it costs nothing further.
    design: Design,
}

/// Everything [`dispatch_background_solve`]'s worker derives from one solve, so the
/// two branches live in one named place rather than inline in an already-long
/// function.
struct PanelInputs {
    tiers: Vec<EditorTierItem>,
    status_text: String,
    status_is_problem: bool,
    /// `(tier index, warning text)` pairs -- kept tagged
    /// (rather than a flattened `manufacturability_warning_lines`/`_from_solved`
    /// text-only list) so `apply_background_solve_result` can
    /// push `EditorModel.manufacturability_warning_tiers` alongside the text,
    /// the same "which row is this about" attribution
    /// `view::push_stale_content`/`push_solved_preview` already give theirs.
    warnings: Vec<(usize, String)>,
    yield_texts: (String, String, String, String),
    planes: Vec<GpuFacetPlane>,
    /// See [`BackgroundSolveResult::too_many_planes`]'s own doc comment -- the
    /// same flag, computed here (on the worker thread, where a real `solve_with`
    /// re-check is safe) and carried straight through.
    too_many_planes: bool,
}

/// Builds every panel readout from a SINGLE `Design::solve`, avoiding the cost of
/// calling five separate helpers that would each solve internally -- with
/// `status_text_and_is_problem` solving twice on its own, via `status()` and
/// `measure()` -- for five or more solves per dispatch.
///
/// `solved` is `None` only when the design does not solve at all. That case falls
/// back to each helper's own solving form, which costs nothing extra: a design with
/// a block missing its scale-reference anchor fails `Design::solve` before any real
/// meet-solving work (see that method's early `MissingAnchor` check), so the
/// fallback re-pays only the cheap early-out, never the expensive case.
///
/// `custom_materials` is a snapshot of `RenderContext::custom_materials` taken at
/// dispatch time: this avoids calling the built-ins-only
/// `Design::effective_refractive_index`, which would give a design with a custom
/// catalogue material selected (e.g. "Garnet 1.74") every MARGIN/risk badge in
/// this background-solved tier table computed against the wrong (default) RI --
/// silently wrong numbers on the one readout a pavilion-angle decision rests on.
/// `Design::effective_refractive_index_with` resolves the SAME way
/// `EditorMaterialLookup`/the viewport's `resolve_material` already do.
///
/// `custom_sg` is likewise a snapshot of `RenderContext::
/// custom_material_specific_gravity`, taken at the same dispatch point as
/// `custom_materials` -- see [`dispatch_background_solve`]'s own snapshot comment
/// -- and handed straight through to `yield_report_texts`/
/// `yield_report_texts_from_solved`, which have no `RenderContext` access of
/// their own.
fn panel_inputs(
    design: &Design,
    solved: Option<&Vec<SolvedTier>>,
    custom_materials: &[GemMaterial],
    custom_sg: &[(String, f64)],
) -> PanelInputs {
    let n_d = design.effective_refractive_index_with(custom_materials);
    // The same cheap-then-
    // authoritative check `state::status_text_and_is_problem*` runs internally for
    // its OWN status text, run once more here purely to hand
    // `apply_background_solve_result` a plain `bool` for the once-per-design
    // toast -- see `BackgroundSolveResult::too_many_planes`'s own doc comment.
    // Safe to call `too_many_planes_message` (a real `solve_with`) unconditionally
    // behind the cheap pre-filter here: this whole function runs on the
    // background worker thread (`solve_and_build_result`'s own caller), never the
    // UI thread.
    let too_many_planes = solved.is_some_and(|solved| likely_hit_plane_cap(solved))
        && too_many_planes_message(design).is_some();
    solved.map_or_else(
        || {
            let (status_text, status_is_problem) = status_text_and_is_problem(design);
            PanelInputs {
                tiers: tier_items(design, n_d),
                status_text,
                status_is_problem,
                warnings: manufacturability_warnings_tagged(design, None),
                yield_texts: yield_report_texts(design, custom_sg),
                planes: design_to_gpu_planes(design),
                too_many_planes,
            }
        },
        |solved| {
            let (status_text, status_is_problem) =
                status_text_and_is_problem_from_solved(design, solved);
            PanelInputs {
                tiers: tier_items_from_solved(design, solved, n_d),
                status_text,
                status_is_problem,
                warnings: manufacturability_warnings_tagged(design, Some(solved)),
                yield_texts: yield_report_texts_from_solved(design, solved, custom_sg),
                planes: design_to_gpu_planes_from_solved(design, solved),
                too_many_planes,
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
/// Spawns the ticker thread that keeps `dispatch_background_solve`'s
/// "Solving..." banner's elapsed time fresh -- split out purely to keep that
/// function under clippy's function-length lint. Returns the `done` flag the
/// caller must set once the real solve finishes, which stops this thread's loop
/// on its next `TICK_INTERVAL` wakeup.
fn spawn_solving_ticker(ui: &MainWindow, seq: u64, tier_count: usize) -> Arc<AtomicBool> {
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
    done_flag
}

/// [`dispatch_background_solve`]'s "a solve is already in flight" guard: queues
/// `design`/`generation`/`multi_selected` as a
/// [`PendingDispatch`] (overwriting any earlier one still waiting) rather than
/// spawning a second concurrent worker thread, when one is already running.
/// Otherwise claims the in-flight slot for `cancel` and returns `false`. Split
/// out purely to keep [`dispatch_background_solve`] under clippy's
/// function-length lint.
fn queue_or_claim_solve_slot(
    design: &Design,
    generation: &Arc<AtomicU64>,
    multi_selected: &BTreeSet<usize>,
    cancel: &Arc<AtomicBool>,
) -> bool {
    RUNTIME.with(|cell| {
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
            rt.current_cancel = Some(Arc::clone(cancel));
            false
        }
    })
}

/// Registers this dispatch with
/// the shared `ActivityRegistry` (see [`activity`]'s own doc comment for why no
/// new parameter is needed here) so the status strip's activity list shows it
/// alongside Deep Solve/Optimize -- indeterminate (`meet_solver` reports no
/// completion fraction, only elapsed time, same as [`spawn_solving_ticker`]'s own
/// banner). The cancel closure goes through [`cancel_in_flight_solve_from_activity`]
/// (not the bare `Runtime`-only [`cancel_in_flight_solve`]) so cancelling from the
/// activity list also resets `EditorModel.solve_running`/`solve_state`/
/// `status_text` exactly like clicking "Abandon Solve" does. Split out of
/// [`dispatch_background_solve`] purely to keep that function under clippy's
/// function-length lint.
fn register_solve_activity(ui: &MainWindow, tier_count: usize) {
    let activity_id = RUNTIME
        .with(|cell| cell.borrow().activity.clone())
        .map(|a| {
            let ui_weak = ui.as_weak();
            a.start(
                "solve",
                solving_banner(tier_count, Duration::ZERO),
                Some(Box::new(move || {
                    cancel_in_flight_solve_from_activity(&ui_weak);
                })),
            )
        });
    RUNTIME.with(|cell| cell.borrow_mut().activity_id = activity_id);
}

/// The worker thread's own solve-and-build-panel-inputs body -- see the comment
/// inside this function's body for why `design` is solved exactly
/// once here. Split out of [`dispatch_background_solve`]'s worker closure purely
/// to keep that function under clippy's function-length lint.
fn solve_and_build_result(
    design: Design,
    cancel: &AtomicBool,
    multi_selected: &BTreeSet<usize>,
    custom_materials: &[GemMaterial],
    custom_sg: &[(String, f64)],
    start: Instant,
) -> BackgroundSolveResult {
    // `design` is solved exactly ONCE here, and every other
    // conversion below is built from that same `solved` list via its
    // `_from_solved` counterpart -- the same helpers `view::push_solved_preview`
    // already proves out on the solid-preview worker's own replan-completion
    // path. This avoids `tier_items`/
    // `status_text_and_is_problem`/`manufacturability_warning_lines`/
    // `yield_report_texts`/`design_to_gpu_planes` each independently calling
    // `Design::solve` (`status_text_and_is_problem` would even call it twice, via
    // `status()` and `measure()`), which would add up to five or more total
    // solves per background dispatch. `result.solved` below
    // comes from this same single `solved_result`, not a second `design.solve()`.
    //
    // `solve_cancellably` (not a
    // plain `design.solve()`) threads `cancel` through, so `cancel_in_flight_solve`
    // ("Abandon Solve") can actually stop this thread's work within a sweep or
    // pipeline run, rather than only discarding whatever this eventually returns.
    let solved_result = solve_cancellably(&design, cancel);
    let multi_selected_count = multi_selected.len();
    let PanelInputs {
        mut tiers,
        status_text,
        status_is_problem,
        warnings,
        yield_texts,
        planes,
        too_many_planes,
    } = panel_inputs(
        &design,
        solved_result.as_ref().ok(),
        custom_materials,
        custom_sg,
    );
    apply_multi_selection(&mut tiers, multi_selected);
    let solved = solved_result.ok();
    let gear = (
        design.meta.gear_teeth_abs(),
        design.meta.gear_reference_angle as f32,
    );
    BackgroundSolveResult {
        elapsed: start.elapsed(),
        tiers,
        status_text,
        status_is_problem,
        warnings,
        yield_texts,
        planes,
        solved,
        too_many_planes,
        gear,
        multi_selected_count,
        // The same snapshot solved above, moved in last (after
        // every borrow of it -- `panel_inputs`/`gear` -- is done with it).
        design,
    }
}

pub(super) fn dispatch_background_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: Design,
    generation: &Arc<AtomicU64>,
    multi_selected: BTreeSet<usize>,
) {
    // A solve already in flight keeps this request queued
    // (overwriting any earlier one still waiting) rather than spawning a second
    // concurrent worker thread -- `apply_background_solve_result` replays exactly
    // this dispatch once the in-flight one completes. The existing "Solving..."
    // banner/ticker already belongs to that in-flight solve, so nothing else here
    // needs touching.
    // Created fresh on every call,
    // but only actually stored (and therefore only actually observed by anything)
    // when this call goes on to spawn a worker below -- a call that instead
    // queues as a `PendingDispatch` leaves this `Arc` unused and it is simply
    // dropped, harmlessly.
    let cancel = Arc::new(AtomicBool::new(false));
    if queue_or_claim_solve_slot(&design, generation, &multi_selected, &cancel) {
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
    register_solve_activity(ui, tier_count);

    let done_flag = spawn_solving_ticker(ui, seq, tier_count);

    // Snapshotted here (still on the UI thread) rather than
    // read from `render_ctx_for_apply` inside the worker below -- `RenderContext`'s
    // lock is meant for the render thread's frame cadence, not held across a
    // multi-second `Design::solve()`, and `Arc<Vec<GemMaterial>>` clones cheaply
    // (see `RenderContext::custom_materials`'s own doc comment on why it is an
    // `Arc` in the first place). `custom_sg` is snapshotted in
    // this SAME locked read for the identical reason -- see `panel_inputs`'s own
    // doc comment for where it ends up.
    let (custom_materials, custom_sg) = {
        let ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        (
            Arc::clone(&ctx.custom_materials),
            Arc::clone(&ctx.custom_material_specific_gravity),
        )
    };

    let ui_weak = ui.as_weak();
    let render_ctx_for_apply = Arc::clone(render_ctx);
    let generation = Arc::clone(generation);
    thread::spawn(move || {
        let start = Instant::now();
        let result = solve_and_build_result(
            design,
            &cancel,
            &multi_selected,
            &custom_materials,
            &custom_sg,
            start,
        );
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
/// Frees [`Runtime::solve_in_flight`]/[`Runtime::current_cancel`]
/// and takes whatever [`PendingDispatch`] queued up behind this completion --
/// pulled out of [`apply_background_solve_result`] purely to keep that function
/// under clippy's function-length lint. Must run BEFORE any staleness check, so a
/// pending request still gets dispatched even when this particular completion
/// turns out to be stale (see [`Runtime::pending_dispatch`]'s own doc comment for
/// why it must not be dropped in that case).
fn free_in_flight_slot() -> Option<PendingDispatch> {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.solve_in_flight = false;
        // This worker's own cancel flag (if any -- see `Runtime::current_cancel`'s
        // own doc comment) has nothing left to stop; clear it so a later, unrelated
        // "Abandon Solve" click cannot reach back and flip a flag no thread is
        // reading anymore.
        rt.current_cancel = None;
        // Removes this dispatch from
        // the activity list on EVERY completion path (current or stale, cancelled or
        // not) -- see `Runtime::activity_id`'s own doc comment. A click on the
        // activity list's own Cancel button already reset `EditorModel` via
        // `cancel_in_flight_solve_from_activity`; this is what removes the row
        // itself, once the (now near-instantly cancelled, per `solve_cancellably`)
        // worker actually returns.
        if let (Some(activity), Some(id)) = (rt.activity.clone(), rt.activity_id.take()) {
            activity.finish(id);
        }
        rt.pending_dispatch.take()
    })
}

/// Exactly one warning toast
/// for a design stuck over `MAX_PLANES` -- see
/// [`Runtime::too_many_planes_toasted`]'s own doc comment for why this is a
/// one-shot-until-cleared flag rather than a toast per background solve (the
/// status-strip sentence itself, `result.status_text`, already re-states the
/// problem on every refresh regardless). Split out of
/// [`apply_background_solve_result`] purely to keep that function under
/// clippy's function-length lint.
fn maybe_toast_too_many_planes(ui: &MainWindow, result: &BackgroundSolveResult) {
    let already_toasted = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        let already = rt.too_many_planes_toasted;
        rt.too_many_planes_toasted = result.too_many_planes;
        already
    });
    if result.too_many_planes && !already_toasted {
        show_toast(ui, &result.status_text, "warning");
    }
}

fn apply_background_solve_result(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    seq: u64,
    started_generation: u64,
    generation: &Arc<AtomicU64>,
    result: BackgroundSolveResult,
) {
    let pending = free_in_flight_slot();

    if !is_current(seq) {
        // A newer dispatch has already taken over the banner and will apply its own
        // result in turn -- nothing here is still current enough to show.
        // `record_solve_duration` must not run above this check -- a
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

    maybe_toast_too_many_planes(ui, &result);

    // Pushed first, before ANY field of `result` is
    // moved out below (a shared `&result` borrow needs every field still in place) --
    // see `push_solve_dependent_background_fields`'s own doc comment for what this
    // closes.
    push_solve_dependent_background_fields(ui, &result);
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
    // `result.warnings` is `(tier index, text)` pairs -- see
    // `PanelInputs::warnings`'s own doc comment.
    let warning_tiers: Vec<i32> = result
        .warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(
            result
                .warnings
                .into_iter()
                .map(|(_, text)| SharedString::from(text))
                .collect::<Vec<_>>(),
        )));
    ui.global::<EditorModel>()
        .set_manufacturability_warning_tiers(ModelRc::new(VecModel::from(warning_tiers)));
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
    // `started_generation` rather than the live counter: a
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
    // This is the "immediately after the next edit or Solve" call site that
    // `camera_lighting::contained_request_size`'s own doc comment names as still
    // reproducing the picking misregistration if it were skipped: without it, a
    // completed background solve would request the redraw at the viewport's own
    // raw size instead of the Path-traced/Both letterboxed rectangle
    // `resubmit_at_current_pose`/`refresh_viewport` already correct for, so the
    // mismatch a camera drag/zoom/view-mode switch already corrects for would
    // reappear the moment a background Solve completes -- the common path, not
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

    // This background solve just replaced the shared plane
    // slot's contents for THIS design -- see `view::refresh_viewport`'s identical
    // comment for why any material name a PREVIOUS occupant left in
    // `cached_curve_material` must stop being compared against
    // `ctx.material_name` for tilt-dialog staleness the moment that happens.
    ui.global::<TiltModel>()
        .set_cached_curve_material("".into());
    // Geometry just changed under the tilt dialog -- if it's
    // open, its four curves and summary badges are about to describe the
    // pre-solve stone as settled results unless a fresh sweep is requested.
    // `AxesCacheKey` (tilt_profile.rs) already hashes the planes, so this is a
    // no-op resweep whenever nothing about the planes actually moved.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }

    dispatch_pending(ui, render_ctx, pending);
}

/// The proportions/girdle-ratio/preform-mm/facet-count/cutting-schedule push half
/// of [`apply_background_solve_result`] -- split out purely to keep that function
/// under clippy's function-length lint, the same reasoning `view::
/// push_solve_dependent_panel_fields` documents for itself.
///
/// These are the fields every OTHER
/// solve-completion path pushes -- proportions, girdle/ratio texts, preform mm
/// readouts, facet count, the cutting schedule. The standard round brilliant template alone
/// sums ~72 plane indices, comfortably over `SYNC_SOLVE_PLANE_LIMIT` (32), so
/// essentially every real design takes THIS path (background solve) rather than
/// `refresh_editor_panel_from_solve`'s synchronous one; without this function, the
/// Proportions section would read "-", the Schedule tab would stay empty, preform mm
/// readouts would stay blank, and the status strip's facet count would freeze at whatever
/// the last SYNCHRONOUSLY solved design had, for every one of those designs.
/// `result.solved` is the SAME single `Design::solve()` this dispatch already
/// paid for -- every helper below is the `_from_solved`
/// shape that reuses it, exactly like `refresh_editor_panel_from_solve`'s own
/// `push_yield_and_proportions`/`push_manufacturability_and_preform_scratch`
/// (`view.rs`) do, just called directly here since this module has no
/// `EditorState`/`ScratchDelta` of its own to route through those two functions
/// themselves (see the module doc comment, "Why a `thread_local!`, not a new
/// `EditorState` field").
fn push_solve_dependent_background_fields(ui: &MainWindow, result: &BackgroundSolveResult) {
    let solved_slice = result.solved.as_deref();
    let (table_pct, crown_height, pavilion_depth, total_depth, length_to_width) = solved_slice
        .map_or_else(
            || proportions_texts(&result.design),
            |solved| proportions_texts_from_solved(&result.design, solved),
        );
    ui.global::<EditorModel>()
        .set_proportions_table_pct(table_pct.into());
    ui.global::<EditorModel>()
        .set_proportions_crown_height(crown_height.into());
    ui.global::<EditorModel>()
        .set_proportions_pavilion_depth(pavilion_depth.into());
    ui.global::<EditorModel>()
        .set_proportions_total_depth(total_depth.into());
    ui.global::<EditorModel>()
        .set_proportions_length_to_width(length_to_width.into());

    let (girdle_thickness, crown_to_width, pavilion_to_width, girdle_to_width) = solved_slice
        .map_or_else(
            || girdle_and_ratio_texts(&result.design),
            |solved| girdle_and_ratio_texts_from_solved(&result.design, solved),
        );
    ui.global::<EditorModel>()
        .set_girdle_thickness_text(girdle_thickness.into());
    ui.global::<EditorModel>()
        .set_crown_to_width_text(crown_to_width.into());
    ui.global::<EditorModel>()
        .set_pavilion_to_width_text(pavilion_to_width.into());
    ui.global::<EditorModel>()
        .set_girdle_to_width_text(girdle_to_width.into());

    let (preform_half_width_mm, preform_depth_mm) = solved_slice.map_or_else(
        || preform_mm_texts(&result.design),
        |solved| preform_mm_texts_from_solved(&result.design, solved),
    );
    ui.global::<EditorModel>()
        .set_preform_half_width_mm_text(preform_half_width_mm.into());
    ui.global::<EditorModel>()
        .set_preform_depth_mm_text(preform_depth_mm.into());
    let mm_per_unit =
        solved_slice.and_then(|solved| result.design.yield_report(solved).mm_per_unit);
    ui.global::<EditorModel>().set_preform_y_offset_mm(
        preform_y_offset_mm_text(result.design.preform_y_offset, mm_per_unit).into(),
    );

    let facet_count = solved_slice.map_or(0, |solved| {
        i32::try_from(facet_count_from_solved(&result.design, solved)).unwrap_or(i32::MAX)
    });
    ui.global::<EditorModel>().set_facet_count(facet_count);

    if let Some(solved) = solved_slice {
        let rows: Vec<AngleItem> = cutting_schedule_rows(&result.design, solved);
        ui.global::<EditorModel>()
            .set_cutting_rows(ModelRc::new(VecModel::from(rows)));
    } else {
        ui.global::<EditorModel>()
            .set_cutting_rows(ModelRc::new(VecModel::from(Vec::<AngleItem>::new())));
    }
}

/// Re-dispatches whatever [`Runtime::pending_dispatch`] [`apply_background_solve_result`]
/// took, if any -- the "dispatch once on completion" half of the queue-instead-of-
/// spawn-a-second-worker mechanism. A no-op when nothing queued up while the
/// just-finished solve was running.
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

    // --- should_solve_synchronously / last_measured_solve_duration ---

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

    #[test]
    fn last_solve_is_the_production_counterpart_of_last_measured_solve_duration() {
        // `view::refresh_all` reads this getter for
        // every non-wholesale caller -- it must agree with the test-only window
        // onto the same `Runtime` field at every point in the same sequence.
        reset_for_new_design();
        assert_eq!(last_solve(), None);
        record_solve_duration(Duration::from_millis(123));
        assert_eq!(last_solve(), Some(Duration::from_millis(123)));
        assert_eq!(last_solve(), last_measured_solve_duration());
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

    // --- cancel_in_flight_solve ---

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

    // --- idle_replan ---

    #[test]
    fn reset_for_new_design_drops_the_idle_replan_timer() {
        RUNTIME.with(|cell| cell.borrow_mut().idle_replan = Some(slint::Timer::default()));
        reset_for_new_design();
        RUNTIME.with(|cell| {
            assert!(
                cell.borrow().idle_replan.is_none(),
                "a wholesale New/Load must drop a partial-frame idle-replan timer \
                 armed for whatever design it is replacing -- otherwise that timer \
                 can fire later and resubmit the OLD design's stashed masts on top \
                 of the one just loaded"
            );
        });
    }

    #[test]
    fn cancel_in_flight_solve_drops_the_idle_replan_timer() {
        RUNTIME.with(|cell| cell.borrow_mut().idle_replan = Some(slint::Timer::default()));
        cancel_in_flight_solve();
        RUNTIME.with(|cell| {
            assert!(
                cell.borrow().idle_replan.is_none(),
                "cancelling a solve must also drop whatever idle-replan timer was pending"
            );
        });
    }

    #[test]
    fn cancel_in_flight_solve_leaves_the_measurement_and_design_stash_untouched() {
        reset_for_new_design();
        record_solve_duration(Duration::from_millis(250));
        stash_current_design(3, Arc::new(fixture_design()), BTreeSet::new());
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
        stash_current_design(5, Arc::new(fixture_design()), BTreeSet::from([2]));
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
        stash_current_design(11, Arc::new(fixture_design()), BTreeSet::new());
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
        stash_current_design(5, Arc::new(fixture_design()), BTreeSet::new());
        assert!(
            take_matching_design(6).is_none(),
            "a newer generation must not match an older stash -- an edit landed \
             after the frame asking for generation 6 was submitted"
        );
    }

    #[test]
    fn stash_current_design_shares_the_arc_rather_than_deep_cloning() {
        // `stash_current_design`
        // must accept the CALLER's own `Arc<Design>` snapshot (an `Arc::clone`,
        // cheap) rather than taking ownership of a value the caller had to deep-
        // clone again just to hand over -- `Arc::strong_count` rising by exactly
        // one confirms no hidden deep clone happened inside this function.
        let snapshot = Arc::new(fixture_design());
        assert_eq!(Arc::strong_count(&snapshot), 1);
        stash_current_design(7, Arc::clone(&snapshot), BTreeSet::new());
        assert_eq!(
            Arc::strong_count(&snapshot),
            2,
            "the stash must hold a SHARED clone of the same allocation, not a deep copy"
        );
        let (taken, _) = take_matching_design(7).expect("stashed at generation 7");
        assert!(
            Arc::ptr_eq(&snapshot, &taken),
            "take_matching_design must hand back the SAME allocation stash_current_design \
             was given, never a fresh deep clone"
        );
    }

    // --- solve_cancellably / "Abandon Solve" actually stops the worker ---

    /// "PC 05.115 CrackOtto-Step" by Ottorino Invernizzi, 103 tiers -- the same
    /// real fixture `indicatrix-cut-core`'s own `resolve_dirty_speed_on_a_large_real_design`
    /// test measures a 5.9s full solve against. Read from the crate's own fixture
    /// file (not re-embedded) so the two copies can never drift -- see
    /// `solve_service.rs`'s own identical helper for the full provenance comment.
    const CRACKOTTO_STEP_ASC: &str = include_str!(
        "../../../../../crates/indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc"
    );

    /// Rebuilds the "mostly implicit `MeetExisting`, one `ScaleReference` anchor
    /// per block" structure a meaningful cancellation test needs -- see
    /// `solve_service.rs`'s own `crackotto_step_with_real_meet_structure` for why a
    /// plain `Design::from_asc_schedule` (every tier pinned) solves too fast to
    /// prove anything about mid-solve cancellation.
    fn crackotto_step_with_real_meet_structure() -> Design {
        use indicatrix::geometry::meet_solver::{Block, classify_blocks};

        let schedule =
            indicatrix_formats::asc::parse_asc(CRACKOTTO_STEP_ASC).expect("fixture must parse");
        let mut design = Design::from_asc_schedule(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
            &schedule,
        );
        let inputs = design.meet_tier_inputs();
        let blocks = classify_blocks(&inputs);
        let mut anchor_kept = [false; 3];
        let slot = |b: Block| match b {
            Block::Crown => 0,
            Block::Pavilion => 1,
            Block::Girdle => 2,
        };
        for (i, tier) in design.tiers.iter_mut().enumerate() {
            let s = slot(blocks[i]);
            if !anchor_kept[s] {
                anchor_kept[s] = true;
                continue;
            }
            if let Some(original) = tier.imported_meet.clone() {
                tier.constraint = original;
            }
        }
        design
    }

    #[test]
    fn solve_cancellably_stops_within_100ms_on_the_largest_real_fixture() {
        // This test encodes the "Abandon
        // stops the worker (test: cancel flag set -> worker returns within
        // 100 ms)" acceptance criterion. `dispatch_background_solve`'s worker calls exactly this
        // function; `cancel_in_flight_solve` is what flips the flag it reads.
        let design = crackotto_step_with_real_meet_structure();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = Arc::clone(&cancel);
        let worker = thread::spawn(move || solve_cancellably(&design, &cancel_for_thread));

        // Let the solve get well past the cheap missing-anchor check and into real
        // refinement work before asking it to stop.
        thread::sleep(Duration::from_millis(50));
        let cancel_requested_at = Instant::now();
        cancel.store(true, Ordering::Relaxed);
        let result = worker.join().expect("worker thread must not panic");
        let cancel_latency = cancel_requested_at.elapsed();

        assert!(
            cancel_latency < Duration::from_millis(100),
            "cancel took {cancel_latency:?}, want < 100ms (full uncancelled solve is ~5.9s)"
        );
        assert!(
            matches!(result, Err(DesignSolveError::Solve(SolveError::Cancelled))),
            "expected a cancelled solve"
        );
    }

    #[test]
    fn take_matching_design_drops_the_pending_debounce_on_a_match() {
        stash_current_design(9, Arc::new(fixture_design()), BTreeSet::new());
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
