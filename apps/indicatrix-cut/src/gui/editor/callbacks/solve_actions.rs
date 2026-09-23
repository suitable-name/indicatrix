//! The tier list's "Adopt" action, and the Deep Solve/Optimize start/cancel/apply
//! callbacks -- one `setup_*` function per Slint callback. See this group's `mod.rs`
//! doc comment for the "`History` is the only thing that mutates `Design`" rule every
//! callback here upholds via `EditorState::apply`.

use super::super::{
    auto_solve, deep_solve,
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
    optimize_solve::{self, OptimizeSolveOutcome},
    stall_guard::stall_guard,
    state::EditorState,
    view::{
        SolidLastSolved, build_optimize_preview_design, configured_optimize_max_evaluations,
        deep_solve_tier_rows, format_deep_solve_report, optimize_change_rows, optimize_result_rows,
        optimize_status_text, parse_optimize_weights, refresh_all_now, refresh_editor_panel_stale,
        submit_design_ghost_preview, submit_preview_replan,
    },
};
use crate::{
    EditorModel, MainWindow, OptimizeResultRow,
    bridge::render_thread::RenderContext,
    gui::{batch::tilt, show_toast, solid_preview::preview_state::SolidPreviewState},
};
use indicatrix::{
    geometry::{
        meet_solver::{MeetConstraint, SolvedTier},
        stone_metrics::ExternalProportions,
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design, DesignSolveError, Edit, MaterialSelection, ObjectiveWeights, OptimizeConfig,
    OptimizeOutcome, free_tier_indices, optimize::SearchStage,
};
use indicatrix_vault::{db::sqlite::Database, model::tilt_curves::TiltPerformanceCurves};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
    },
    thread,
};

thread_local! {
    /// The most recently completed Deep Solve's per-tier mast disagreement, read by
    /// the "Pin to verified mast" action ([`setup_deep_solve_pin_callback`]).
    /// A module-local `thread_local!` rather than a new `EditorState` field,
    /// matching `callbacks::retarget_actions::RETARGET_ASYNC`'s own precedent for
    /// exactly this situation. Sound here for the same reason that one is: Slint's event
    /// loop is single-threaded, so this never crosses a thread boundary. Cleared
    /// whenever a new Deep Solve run starts and whenever [`clear_analysis_results`]
    /// runs, so a pin action can never fire against a stale run's tier indices.
    static LAST_DEEP_SOLVE_DELTAS: RefCell<Vec<deep_solve::TierMastDelta>> =
        const { RefCell::new(Vec::new()) };

    /// The [`ActivityRegistry`] id of
    /// the currently running Deep Solve, if any -- shared between
    /// [`setup_deep_solve_callback`] (which registers it) and
    /// [`setup_deep_solve_cancel_callback`] (which must finish that SAME id
    /// immediately on cancel, rather than waiting even for the checkpoint-based
    /// cancellation's own fast `on_done` to arrive -- see `deep_solve`'s module doc
    /// comment). A module-local `thread_local!` for the
    /// identical single-threaded-event-loop reason [`LAST_DEEP_SOLVE_DELTAS`]
    /// documents just above.
    static DEEP_SOLVE_ACTIVITY_ID: RefCell<Option<u64>> = const { RefCell::new(None) };

    /// [`OPTIMIZE_ACTIVITY_ID`]'s Deep-Solve-shaped counterpart for
    /// [`setup_optimize_callback`]/[`setup_optimize_cancel_callback`].
    static OPTIMIZE_ACTIVITY_ID: RefCell<Option<u64>> = const { RefCell::new(None) };
}

/// The tier list's "Adopt" action: switches a pinned tier over to the meet
/// instruction the source file actually stated for it
/// (`ConstraintTier::imported_meet`), through [`Edit::SetConstraint`] like any other
/// edit. A silent no-op if the tier has nothing to adopt or the index is stale:
/// `EditorView` only shows the button when `imported_meet_text` is non-empty, so this
/// only guards a race with a concurrent edit.
///
/// Calls `refresh_editor_panel_stale` (with `dirty = {index}`), not `refresh_all`'s
/// real re-solve -- deliberately the one exception to "Solve on explicit action,
/// not on every edit" that it would otherwise be: adopting a suggested meet isn't
/// really a new edit whose result is unknown, since the value being adopted is
/// exactly what the tier already showed after the design's last real solve, so
/// paying the whole-schedule solve cost to re-confirm one already-known value
/// would be a bigger hammer than it needs. `refresh_editor_panel_stale` pushes the
/// immediate UI update from the CACHED last solve (which reads every OTHER row
/// from that cache -- see `push_stale_content`'s own doc comment) and lets the
/// background dirty-set replan confirm it via `resolve_dirty` instead, the same
/// approach `callbacks::tier_actions::setup_pin_to_mast_callback` uses for its own
/// per-row "Pin" action. "Solve, Adopt, and it says not solved again" cannot
/// happen: `refresh_editor_panel_stale` reads the actual cached solve rather than
/// a fixed "Not solved" banner for the untouched rows.
pub(in crate::gui::editor) fn setup_adopt_meet_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_adopt_meet(move |index: i32| {
        stall_guard("on_adopt_meet", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if index < 0 {
                return;
            }
            let index = index as usize;
            let mut st = state.borrow_mut();
            let Some(constraint) = st
                .design
                .tiers
                .get(index)
                .and_then(|t| t.imported_meet.clone())
            else {
                return;
            };
            match st.apply(Edit::SetConstraint { index, constraint }) {
                Ok(()) => {
                    let dirty: BTreeSet<usize> = std::iter::once(index).collect();
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &dirty);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        dirty,
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
    });
}

/// What a background Deep Solve/Optimize run knew about the design's identity when
/// it started, so its completion handler -- arriving minutes later, on a UI thread
/// that may be showing something else entirely -- can tell three situations apart.
///
/// The distinction matters because the two counters answer different questions.
/// [`Self::is_stale`] (the design was touched at all) means the result no longer
/// describes the current schedule exactly, so it must not be applied; but after an
/// ordinary edit it is still a verdict about THIS design a few edits ago, worth
/// showing with a caveat given the run cost minutes. [`Self::design_replaced`] is
/// strictly stronger: New / Load Selected / Open Native swapped in a different
/// stone, so the result describes nothing on screen and the panel is deliberately
/// blank -- there the handler has to stay quiet. Collapsing the two into one
/// `stale` flag forces a choice between throwing away a useful report and painting
/// a stale one over a fresh design's empty panel.
struct RunProvenance {
    generation: Arc<AtomicU64>,
    started_generation: u64,
    design_epoch: Arc<AtomicU64>,
    started_epoch: u64,
}

impl RunProvenance {
    /// Snapshots both of `state`'s counters and keeps a clone of each `Arc`, which
    /// is what lets a `Send` completion closure observe later changes without
    /// capturing the non-`Send` `Rc<RefCell<EditorState>>` itself.
    fn capture(state: &EditorState) -> Self {
        Self {
            generation: Arc::clone(&state.generation),
            started_generation: state.generation.load(AtomicOrdering::Relaxed),
            design_epoch: Arc::clone(&state.design_epoch),
            started_epoch: state.design_epoch.load(AtomicOrdering::Relaxed),
        }
    }

    /// The design changed somehow -- edited, undone/redone, or replaced.
    fn is_stale(&self) -> bool {
        self.generation.load(AtomicOrdering::Relaxed) != self.started_generation
    }

    /// The design was REPLACED outright, not merely edited. Implies
    /// [`Self::is_stale`], since `EditorState::replace_wholesale` bumps both.
    fn design_replaced(&self) -> bool {
        self.design_epoch.load(AtomicOrdering::Relaxed) != self.started_epoch
    }
}

/// Clears every on-screen Deep Solve/Optimize result: both status lines and their
/// `_is_problem` flags, all three result tables (`deep_solve_tier_rows`,
/// `optimize_result_rows`, `optimize_change_rows`) and `optimize_can_apply`.
///
/// Called from two places, for the same reason -- a verdict must never outlive the
/// design it describes:
///
/// - right after New / Load Selected / Open Native / Open plain `.asc` replace the
///   live [`EditorState`] wholesale (`tier_actions::do_new_design_create`,
///   `tier_actions::apply_loaded_design`, and `native_io::finish_state_replace` for
///   both native paths), and
/// - from the Deep Solve and Optimize completion handlers themselves when
///   [`RunProvenance::design_replaced`] says the swap happened WHILE the run was in
///   flight. Neither cancellation actually stops the in-flight work (Deep Solve's is
///   UI-level abandonment outright -- see `deep_solve`'s module doc comment), so the
///   abandoned run still arrives, still holds a report about the replaced design,
///   and would otherwise repaint it over the fresh design's deliberately blank
///   panel. Calling this there rather than simply returning also covers the live
///   progress text both status fields carry during a run.
///
/// Also finishes any Deep Solve/
/// Optimize [`ActivityRegistry`](super::super::activity::ActivityRegistry) entry
/// still outstanding -- both callers above can fire while a run is genuinely still
/// in flight (a New/Load Selected while Deep Solve is abandoning in the
/// background), and without this that activity would otherwise sit in the status
/// strip's list, seemingly still running, until the orphaned worker eventually
/// reports back (which, per `deep_solve`'s own module doc comment, may not be for
/// minutes).
pub(in crate::gui::editor) fn clear_analysis_results(ui: &MainWindow) {
    if let Some(activity) = auto_solve::activity() {
        if let Some(id) = DEEP_SOLVE_ACTIVITY_ID.with(RefCell::take) {
            activity.finish(id);
        }
        if let Some(id) = OPTIMIZE_ACTIVITY_ID.with(RefCell::take) {
            activity.finish(id);
        }
    }
    ui.global::<EditorModel>().set_deep_solve_status("".into());
    ui.global::<EditorModel>()
        .set_deep_solve_status_is_problem(false);
    ui.global::<EditorModel>()
        .set_deep_solve_tier_rows(ModelRc::new(VecModel::from(
            Vec::<crate::DeepSolveTierRow>::new(),
        )));
    ui.global::<EditorModel>().set_optimize_status("".into());
    ui.global::<EditorModel>()
        .set_optimize_status_is_problem(false);
    ui.global::<EditorModel>()
        .set_optimize_result_rows(ModelRc::new(
            VecModel::from(Vec::<OptimizeResultRow>::new()),
        ));
    ui.global::<EditorModel>()
        .set_optimize_change_rows(ModelRc::new(VecModel::from(
            Vec::<crate::OptimizeChangeRow>::new(),
        )));
    ui.global::<EditorModel>().set_optimize_can_apply(false);
    // A replaced design has no
    // Deep Solve/Optimize result of its own yet (`EditorState::
    // deep_solve_result_generation`/`pending_optimize` both reset to `None` by
    // `EditorState::replace_wholesale`, see each field's own doc comment) --
    // reset here too so the badge cannot keep showing the OLD design's staleness
    // for the one frame before the new design's first `push_stale_content` runs.
    ui.global::<EditorModel>().set_deep_solve_stale(false);
    ui.global::<EditorModel>().set_optimize_stale(false);
    // A "Pin to verified mast" click against a run that described a
    // since-replaced design must find nothing to pin, not old tier indices
    // reinterpreted against the new design.
    LAST_DEEP_SOLVE_DELTAS.with(|cell| cell.borrow_mut().clear());
}

/// "Deep Solve": the explicit, off-thread, cancellable verified-solve action -- see
/// `deep_solve`'s module doc comment for why it needs its own thread and why it's a
/// read-only diagnostic that never mutates `design`. Guarded by the
/// `editor_deep_solve_running` property rather than `EditorState::deep_solve` being
/// `Some`, since the completion handler can't clear that field itself.
///
/// `run_epoch` is a counter local to this callback (captured once, shared by every
/// invocation and by each run's own completion handler) that tells a superseded
/// run's eventual completion from the current one: cancelling and immediately
/// starting a fresh run races the OLD run's own `on_done` -- even though
/// `deep_solve::DeepSolveHandle::cancel` stops the search at its next
/// checkpoint within single-digit milliseconds (see `deep_solve`'s module doc
/// comment), that is not instantaneous, so the old run's `on_done` could still
/// arrive after the new run has already started and clobber its live status with
/// a stale one. Bumped only when a new run actually starts -- cancel itself needs
/// no such guard, see [`setup_deep_solve_cancel_callback`].
pub(in crate::gui::editor) fn setup_deep_solve_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    // The completion closure below must be `Send` (it crosses the worker thread
    // as a value before `upgrade_in_event_loop` runs it on the UI thread), so it
    // cannot capture this `Rc` -- it fetches it back on the UI thread through
    // `auto_solve::editor_state()` instead, the same stash idiom as
    // `auto_solve::activity()`.
    auto_solve::stash_editor_state(&state);
    let ui_weak = ui.as_weak();
    let run_epoch = Arc::new(AtomicU64::new(0));
    ui.global::<EditorModel>().on_deep_solve(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        stall_guard("on_deep_solve", || {
            begin_deep_solve_run(&ui, &state, &run_epoch);
        });
    });
}

/// Everything [`begin_deep_solve_run`] needs to capture from `state` BEFORE
/// dispatching the worker -- pulled into its own struct/function purely to keep
/// that function under clippy's function-length lint. See each field's own
/// former inline comment (now on this struct) for why it is captured up front
/// rather than re-read from `state` once the run completes.
struct DeepSolveRunPrep {
    /// The design this run solves -- `deep_solve::spawn_deep_solve` derives
    /// `gear_teeth_abs`/`meet_tier_inputs` from it internally.
    design_for_run: Design,
    /// The design's own plain solve, read from the already-rendered
    /// `solid_last_solved` cache (via `auto_solve::solid_last_solved`, the escape
    /// hatch that avoids threading a new parameter through `gui::editor::mod`'s
    /// fixed call site -- same pattern `retarget_actions::setup_snapshot_callbacks`
    /// uses) when that cache is already aligned with this design's tier count. So
    /// the completion handler can show which tiers Deep Solve's verified repair
    /// actually disagreed with -- see `deep_solve::tier_mast_deltas`.
    ///
    /// `None` when the cache is missing or misaligned (a tier was just
    /// added/removed and the background solve has not caught up yet) -- NEVER
    /// filled in by a fresh `Design::solve` here, since that is a full synchronous
    /// meet solve and this runs on the UI thread. [`needs_background_baseline`]
    /// is `true` exactly when this is `None`, and drives
    /// `SolveKind::Verified::compute_baseline` so the worker computes the same
    /// baseline off-thread instead -- see
    /// [`deep_solve::DeepSolveOutcome::Completed::baseline`]'s own doc comment for
    /// how `apply_deep_solve_outcome` reconciles the two possible sources.
    current_masts: Option<Vec<SolvedTier>>,
    design_snapshot: Design,
    /// Whether at least one edit has landed since this
    /// design was loaded/created/replaced -- `targets` was captured once at
    /// catalogue-load time and never re-derived from edits since, so a
    /// verdict against it is honestly a verdict against the design AS
    /// PRINTED, not necessarily the one on screen right now. See
    /// `deep_solve::edited_since_load_caveat`'s own doc comment for why this
    /// reads `History::can_undo` rather than `EditorState::is_dirty`.
    edited_since_load: bool,
    /// The design's own file name, so a verdict against
    /// `targets` can say WHERE those printed figures were recorded rather
    /// than leaving the cutter to guess.
    source_label: Option<String>,
    provenance: RunProvenance,
    this_run: u64,
    run_epoch_done: Arc<AtomicU64>,
}

/// Whether [`capture_deep_solve_run_prep`]'s cache read left `current_masts`
/// usable as-is (`false`), or whether the worker must compute a baseline plain
/// solve of its own before the completion handler has anything to show a
/// per-tier delta table against (`true`). Pulled out into its own pure function
/// -- the one genuinely testable decision in a prelude that otherwise needs a
/// real `Design`/`EditorState` -- so a test can call it directly without
/// spinning up either. `cache` is the same `Option<&[SolvedTier]>` the caller
/// already has; `tier_count` is `Design::tiers.len()` for the design this run
/// solves.
#[must_use]
fn needs_background_baseline(cache: Option<&[SolvedTier]>, tier_count: usize) -> bool {
    cache.is_none_or(|solved| solved.len() != tier_count)
}

/// [`begin_deep_solve_run`]'s prelude -- see [`DeepSolveRunPrep`]'s own doc
/// comment. `st` is the already-borrowed `EditorState` (read-only: nothing here
/// mutates it); `run_epoch` is bumped here, the one side effect this otherwise
/// pure capture has.
fn capture_deep_solve_run_prep(st: &EditorState, run_epoch: &Arc<AtomicU64>) -> DeepSolveRunPrep {
    let cached = auto_solve::solid_last_solved().and_then(|cache| {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    });
    let current_masts = cached.filter(|solved| {
        !needs_background_baseline(Some(solved.as_slice()), st.design.tiers.len())
    });
    let this_run = run_epoch.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    DeepSolveRunPrep {
        design_for_run: st.design.clone(),
        current_masts,
        design_snapshot: st.design.clone(),
        edited_since_load: st.history.can_undo(),
        source_label: st.asc_filename.clone(),
        provenance: RunProvenance::capture(st),
        this_run,
        run_epoch_done: Arc::clone(run_epoch),
    }
}

/// The `on_deep_solve` handler's actual body, pulled out of
/// [`setup_deep_solve_callback`] purely to keep that function under clippy's
/// function-length lint -- see that function's own doc comment for `run_epoch`'s
/// meaning and every other design note below.
fn begin_deep_solve_run(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    run_epoch: &Arc<AtomicU64>,
) {
    // Also guards against Solve or Optimize already
    // running, not only a re-entrant Deep Solve click -- see
    // `tier_actions::setup_solve_callback`'s own doc comment for why all
    // three sites read the three `*_running` flags directly rather than
    // the derived `EditorModel.busy_action`.
    let model = ui.global::<EditorModel>();
    if model.get_deep_solve_running() || model.get_solve_running() || model.get_optimize_running() {
        return;
    }
    let mut st = state.borrow_mut();
    let Some(targets) = st.printed_proportions else {
        show_toast(
            ui,
            "Deep Solve needs this design's printed proportions; none are available.",
            "error",
        );
        return;
    };
    let DeepSolveRunPrep {
        design_for_run,
        current_masts,
        design_snapshot,
        edited_since_load,
        source_label,
        provenance,
        this_run,
        run_epoch_done,
    } = capture_deep_solve_run_prep(&st, run_epoch);
    // `current_masts` is only `None` when the cache was missing/misaligned --
    // see `DeepSolveRunPrep::current_masts`'s own doc comment. Never re-solved
    // here: that would be the very UI-thread solve this run prep exists to
    // avoid. The worker computes it instead, off-thread, alongside the
    // verified search (`SolveKind::Verified::compute_baseline`).
    let need_baseline = current_masts.is_none();
    // A fresh run's eventual result must never be offered for pinning against
    // whatever the PREVIOUS run's deltas named -- see
    // `LAST_DEEP_SOLVE_DELTAS`'s own doc comment.
    LAST_DEEP_SOLVE_DELTAS.with(|cell| cell.borrow_mut().clear());

    ui.global::<EditorModel>().set_deep_solve_running(true);
    ui.global::<EditorModel>()
        .set_deep_solve_status("Deep solving... 0.0s elapsed".into());
    ui.global::<EditorModel>()
        .set_deep_solve_status_is_problem(false);

    // Activity registered before the
    // worker even spawns, so the status strip's activity list shows it from
    // the very first frame. `auto_solve::activity()` is the same
    // stashed-handle escape hatch `auto_solve::solid_last_solved`/
    // `preview_state` already use to reach a handle `gui::editor::mod`'s
    // fixed `setup_editor_callbacks` call site hands to `auto_solve::init`,
    // without adding a new parameter to this (or `mod.rs`'s) signature --
    // `None` only if this callback somehow fired before `init` ran, which
    // cannot happen in practice (see that function's own doc comment), so
    // this degrades to "no activity tracking for this one run" rather than
    // panicking. The cancel closure reaches back through `state` (not the
    // `handle` below directly, which does not exist yet at this point) so it
    // always calls the CURRENT `EditorState::deep_solve`, even if this
    // closure were somehow invoked after `st.deep_solve` is reassigned.
    let activity = auto_solve::activity();
    let activity_id = activity.as_ref().map(|a| {
        a.start(
            "deep_solve",
            "Deep Solve",
            Some({
                let state = Rc::clone(state);
                Box::new(move || {
                    if let Some(handle) = state.borrow().deep_solve.as_ref() {
                        handle.cancel();
                    }
                })
            }),
        )
    });
    if activity_id.is_some() {
        DEEP_SOLVE_ACTIVITY_ID.with(|cell| *cell.borrow_mut() = activity_id);
    }

    let ui_weak = ui.as_weak();
    let handle = deep_solve::spawn_deep_solve(
        ui_weak,
        design_for_run,
        targets,
        need_baseline,
        |ui: &MainWindow, progress: deep_solve::DeepSolveProgress| {
            ui.global::<EditorModel>().set_deep_solve_status(
                format!(
                    "Deep solving... {:.1}s elapsed",
                    progress.elapsed.as_secs_f32()
                )
                .into(),
            );
        },
        move |ui: &MainWindow, outcome: deep_solve::DeepSolveOutcome| {
            // This run is finishing on its own (as opposed to having already
            // been finished early by a Cancel click, see
            // `setup_deep_solve_cancel_callback`) -- `ActivityRegistry::finish`
            // is a harmless no-op if that already happened, per its own doc
            // comment.
            // Fetched here, on the UI thread, rather than captured: this
            // closure must be `Send`, and `ActivityRegistry` is an `Rc`.
            if let (Some(activity), Some(id)) = (auto_solve::activity(), activity_id) {
                activity.finish(id);
            }
            DEEP_SOLVE_ACTIVITY_ID.with(|cell| {
                if *cell.borrow() == activity_id {
                    *cell.borrow_mut() = None;
                }
            });
            // A superseded run (cancelled, then a fresh Deep Solve already
            // started before this one's background thread finished) -- ignore
            // it entirely rather than clobbering the current run's live status.
            if run_epoch_done.load(AtomicOrdering::Relaxed) != this_run {
                return;
            }
            // Always cleared, on every path below: `deep_solve_status` doubles
            // as the live "Deep solving... X.Xs elapsed" readout, and
            // `deep_solve_running` gates the command bar's own Deep Solve
            // button, so a handler that returns without touching either leaves
            // the strip frozen on a stale elapsed time with no way to start a
            // new run.
            ui.global::<EditorModel>().set_deep_solve_running(false);
            // Same `Send` reasoning as the activity handle above: the editor
            // state `Rc` is fetched back on the UI thread, not captured. It
            // is stashed by this very `setup_*` call, so `None` is unreachable
            // from a real callback; it is still handled rather than unwrapped.
            let Some(state_for_done) = auto_solve::editor_state() else {
                tracing::warn!("deep solve finished before the editor state was stashed");
                return;
            };
            apply_deep_solve_outcome(
                ui,
                outcome,
                &DeepSolveRunContext {
                    provenance: &provenance,
                    targets,
                    source_label: source_label.as_deref(),
                    edited_since_load,
                    current_masts: current_masts.as_deref(),
                    design_snapshot: &design_snapshot,
                    state: &state_for_done,
                },
            );
        },
    );
    st.deep_solve = Some(handle);
}

/// What [`apply_deep_solve_outcome`] needs beyond `ui` and the `outcome` itself --
/// bundled into one struct (clippy's `too_many_arguments`) rather than as separate
/// parameters. Every field is exactly what `setup_deep_solve_callback`'s closure
/// captured at dispatch time; see that call site's own comments for what each one
/// means.
#[derive(Clone, Copy)]
struct DeepSolveRunContext<'a> {
    provenance: &'a RunProvenance,
    targets: ExternalProportions,
    source_label: Option<&'a str>,
    edited_since_load: bool,
    current_masts: Option<&'a [SolvedTier]>,
    design_snapshot: &'a Design,
    /// Lets the `Completed` arm
    /// stamp [`EditorState::deep_solve_result_generation`] -- a SEPARATE
    /// `Rc::clone` from the one `setup_deep_solve_callback`'s own dispatch body
    /// still holds borrowed at the moment this context is built, see that call
    /// site's own comment.
    state: &'a Rc<RefCell<EditorState>>,
}

/// The Deep Solve run's completion handler -- split out of the closure
/// `setup_deep_solve_callback` passes to [`deep_solve::spawn_deep_solve`] purely to
/// keep that function under clippy's function-length lint. `outcome` is the
/// low-level result; `ctx` is everything else the closure captured at dispatch
/// time -- see [`DeepSolveRunContext`]'s own doc comment.
fn apply_deep_solve_outcome(
    ui: &MainWindow,
    outcome: deep_solve::DeepSolveOutcome,
    ctx: &DeepSolveRunContext<'_>,
) {
    let DeepSolveRunContext {
        provenance,
        targets,
        source_label,
        edited_since_load,
        current_masts,
        design_snapshot,
        state,
    } = *ctx;
    // The design was REPLACED (New / Load Selected / Open Native), not
    // merely edited: this verdict describes a different stone, and
    // `clear_analysis_results` has already blanked the panel on purpose.
    // Repainting a report over it -- even a caveated one -- would read
    // as a verdict on the design now on screen. A replacement does not cancel
    // a run already in flight, so its result still arrives here regardless.
    if provenance.design_replaced() {
        clear_analysis_results(ui);
        return;
    }
    match outcome {
        deep_solve::DeepSolveOutcome::Cancelled => {
            ui.global::<EditorModel>()
                .set_deep_solve_status("Deep Solve cancelled.".into());
            ui.global::<EditorModel>()
                .set_deep_solve_status_is_problem(false);
        }
        deep_solve::DeepSolveOutcome::Completed {
            solved,
            report,
            baseline,
        } => {
            // Reachable only for an EDIT, never a replacement (that
            // returned above) -- so the caveat this adds is the honest
            // one: same design, a few edits on.
            let mut status = format_deep_solve_report(&report, provenance.is_stale());
            // Names WHICH printed figures this
            // verdict verified against and WHERE they came from, and
            // warns when the design has its own edits since load that
            // those figures do not reflect -- see
            // `deep_solve::format_verification_targets`/
            // `edited_since_load_caveat`'s own doc comments.
            status.push_str(&deep_solve::format_verification_targets(
                &targets,
                source_label,
            ));
            status.push_str(deep_solve::edited_since_load_caveat(edited_since_load));
            // The per-tier breakdown behind the aggregate verdict above -- see
            // `deep_solve::tier_mast_deltas`'s own doc comment for why this status
            // text is the minimal consumer for that data; the Log popup's row
            // model just below is the fuller one. `current_masts` (the UI
            // thread's own cache read at dispatch time) and `baseline` (the
            // worker's own off-thread solve, only ever computed when that cache
            // was unusable -- see `DeepSolveRunPrep::current_masts`'s own doc
            // comment) are mutually exclusive: exactly one is `Some` for any run
            // that reaches here with `report.accepted` meaningful. Neither being
            // available (an unsolvable design) simply omits the table, same as
            // today -- never blocks waiting for anything.
            let current_masts = current_masts.map(<[SolvedTier]>::to_vec).or(baseline);
            if let Some(current) = current_masts.as_deref() {
                let deltas = deep_solve::tier_mast_deltas(current, &solved);
                status.push_str(&deep_solve::format_tier_mast_deltas(&deltas));
                // The same disagreement as a real per-tier table, for the
                // Log popup -- the status line only has room for a summary.
                ui.global::<EditorModel>()
                    .set_deep_solve_tier_rows(ModelRc::new(VecModel::from(deep_solve_tier_rows(
                        &deltas,
                        design_snapshot,
                    ))));
                // The deltas are
                // remembered here so `setup_deep_solve_pin_callback`
                // (below) can look a row's mast back up by tier index
                // without this crate's Deep Solve report growing a
                // reference to `EditorState` of its own.
                LAST_DEEP_SOLVE_DELTAS.with(|cell| cell.borrow_mut().clone_from(&deltas));
            }
            ui.global::<EditorModel>()
                .set_deep_solve_status(status.into());
            ui.global::<EditorModel>()
                .set_deep_solve_status_is_problem(!report.accepted);
            // Stamps the generation
            // this verdict describes (`started_generation`, not whatever the live
            // counter reads NOW) -- `view::push_stale_content` compares this
            // against the live generation on every later edit
            // (`state::result_is_stale`) to keep `EditorModel.deep_solve_stale`
            // current. Set immediately here too (`provenance.is_stale()`, the
            // identical comparison at THIS instant) so a run that was already
            // stale by the time it completed (edits landed mid-search) badges
            // itself right away, instead of waiting for the NEXT edit's own
            // `push_stale_content` to notice.
            state.borrow_mut().deep_solve_result_generation = Some(provenance.started_generation);
            ui.global::<EditorModel>()
                .set_deep_solve_stale(provenance.is_stale());
        }
    }
}

/// "Cancel" (shown only while a Deep Solve is running): see
/// [`deep_solve::DeepSolveHandle::cancel`] for exactly what the low-level
/// cancellation call does and doesn't stop.
///
/// Unlike that call alone, THIS is what actually gives the user their editor back:
/// it flips `editor_deep_solve_running`/`deep_solve_status` immediately, on the same
/// click, rather than waiting even for the checkpoint-based cancellation's own fast
/// (typically single-digit-millisecond, see `deep_solve`'s module doc comment)
/// `on_done` to arrive -- the completion handler is otherwise the only place
/// either gets reset, so without this the strip would stay busy on "Deep
/// solving... X.Xs elapsed" for that stretch, with no way to start a new Deep
/// Solve until it did.
pub(in crate::gui::editor) fn setup_deep_solve_cancel_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_deep_solve_cancel(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if let Some(handle) = state.borrow().deep_solve.as_ref() {
            handle.cancel();
        }
        // The activity is finished and
        // removed from the activity
        // list on THIS click, not whenever the abandoned worker eventually reports
        // back -- see `DEEP_SOLVE_ACTIVITY_ID`'s own doc comment.
        if let Some(id) = DEEP_SOLVE_ACTIVITY_ID.with(RefCell::take)
            && let Some(activity) = auto_solve::activity()
        {
            activity.finish(id);
        }
        ui.global::<EditorModel>().set_deep_solve_running(false);
        ui.global::<EditorModel>()
            .set_deep_solve_status("Deep Solve cancelled.".into());
        ui.global::<EditorModel>()
            .set_deep_solve_status_is_problem(false);
    });
}

/// The tier index encoded in a `"#N"` (1-based) row label -- the inverse of
/// `view::tier_name_for_row`'s callers, which all build that exact string via
/// `format!("#{}", tier_index + 1)` (see [`crate::DeepSolveTierRow::tier_number`]/
/// [`crate::OptimizeChangeRow::tier_number`]). `None` for anything that isn't
/// `#` followed by a positive integer -- in particular for `"#0"`, which this
/// format never produces (row numbers start at `#1`).
#[must_use]
fn tier_index_from_row_number(text: &str) -> Option<usize> {
    text.strip_prefix('#')?
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
}

/// [`LAST_DEEP_SOLVE_DELTAS`]'s lookup half: the verified mast Deep Solve proposed
/// for `tier_index`, if that tier is still named in `deltas`. Pulled out of
/// [`setup_deep_solve_pin_callback`] so the lookup itself -- the one genuinely
/// testable piece of that callback -- has something a test can call directly.
#[must_use]
fn verified_mast_for(deltas: &[deep_solve::TierMastDelta], tier_index: usize) -> Option<f64> {
    deltas
        .iter()
        .find(|d| d.tier_index == tier_index)
        .map(|d| d.after_mast)
}

/// Registers an opt-in, per-tier "Pin to verified mast"
/// action alongside Deep Solve's per-tier disagreement table
/// (`EditorModel.deep_solve_tier_rows`). Takes the SAME `"#N"` tier-number text that
/// table already prints (see [`tier_index_from_row_number`]) rather than a raw
/// index, so the button this still needs (not yet wired in
/// `editor_status_strip.slint`) needs no new field on `DeepSolveTierRow` to add.
///
/// Applies through [`EditorState::apply`] like every other edit in this crate --
/// `Edit::SetConstraint(ScaleReference(verified_mast))`, never a wholesale "apply
/// the verified configuration": `deep_solve`'s module doc comment explains why that
/// larger action isn't representable (the repair search's OTHER wins are
/// vertex-level picks with no `MeetConstraint` of their own). A no-op (with an
/// explanatory toast) when [`LAST_DEEP_SOLVE_DELTAS`] no longer names `tier_index`
/// -- stale after a fresh Deep Solve run, a design replacement, or an edit that
/// changed tier count out from under an old run's row.
pub(in crate::gui::editor) fn setup_deep_solve_pin_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_pin_verified_mast(move |tier_number: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(index) = tier_index_from_row_number(&tier_number) else {
                return;
            };
            let mast = LAST_DEEP_SOLVE_DELTAS.with(|cell| verified_mast_for(&cell.borrow(), index));
            let Some(mast) = mast else {
                show_toast(
                    &ui,
                    "No verified mast recorded for this tier any more -- re-run Deep \
                     Solve.",
                    "error",
                );
                return;
            };
            let mut st = state.borrow_mut();
            match st.apply(Edit::SetConstraint {
                index,
                constraint: MeetConstraint::ScaleReference(mast),
            }) {
                Ok(()) => {
                    // `refresh_all`
                    // now takes `Rc<RefCell<EditorState>>` (see `view::
                    // refresh_all_now`'s own doc comment) -- this call site's own
                    // logic is otherwise untouched.
                    drop(st);
                    refresh_all_now(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &state,
                        false,
                    );
                    show_toast(
                        &ui,
                        &format!("Pinned tier #{} to Deep Solve's verified mast.", index + 1),
                        "success",
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// "Optimize": the explicit, off-thread, cancellable coordinate search over the
/// design's free tier angles -- see `optimize_solve`'s module doc comment for the
/// threading/cancellation/progress machinery. Guarded by `editor_optimize_running`
/// (not `EditorState::optimize` being `Some`), the same non-`Send` completion-handler
/// reasoning [`setup_deep_solve_callback`] documents.
///
/// Unlike Deep Solve, a completed/cancelled run's result is not merely displayed: it
/// is stashed in `EditorState::pending_optimize` (paired with the design generation
/// it ran against) so [`setup_optimize_apply_callback`] can commit it later -- this
/// callback itself never touches `design`/`history`.
///
/// `run_epoch` guards against the same superseded-run race
/// [`setup_deep_solve_callback`] documents -- narrower a window here (Optimize's own
/// cancellation is a real mid-search checkpoint, typically resolving within
/// [`optimize_solve`]'s own documented "milliseconds to a few seconds"), but a
/// cancel-then-immediately-restart click is still possible, and its stale `on_done`
/// would otherwise be free to overwrite a genuinely running new search's live status
/// or stash the WRONG result into `pending_optimize`.
///
/// When `design.material` names no preset and carries no RI override (a brand-new
/// design, or an untouched `.asc` import), this resolves a `refractive_index_override`
/// from [`indicatrix_cut_core::design::Design::effective_refractive_index`] before
/// handing the selection to the worker -- otherwise
/// `MaterialSelection::resolve`/`resolved_gem_material` silently fall back to
/// diamond (`n_D` 2.42), scoring the search against the wrong RI with nothing on
/// screen saying so (see the "silently scores against Diamond" finding).
pub(in crate::gui::editor) fn setup_optimize_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    let run_epoch = Arc::new(AtomicU64::new(0));
    ui.global::<EditorModel>().on_optimize(
        move |windowing: SharedString,
              extinction: SharedString,
              tilt_brilliance: SharedString,
              yield_weight: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            stall_guard("on_optimize", || {
                begin_tier_optimize_run(
                    &ui,
                    &state,
                    &render_ctx,
                    &run_epoch,
                    OptimizeWeightForm {
                        windowing: &windowing,
                        extinction: &extinction,
                        tilt_brilliance: &tilt_brilliance,
                        yield_weight,
                    },
                );
            });
        },
    );
}

/// The `on_optimize` handler's actual body, pulled out of
/// [`setup_optimize_callback`] purely to keep that function under clippy's
/// function-length lint -- see its own doc comment for the material-RI-defaulting
/// and "only selected tiers" behaviour implemented here.
///
/// Named distinctly from `retarget_actions::start_optimize_run` (a different,
/// retarget-specific Optimize entry point in a sibling module) purely to avoid two
/// same-named private functions reading as one shared thing when they aren't --
/// Rust itself has no conflict either way, since each is module-scoped.
/// Everything [`begin_tier_optimize_run`] needs to build before dispatching the
/// worker, beyond the target `design` itself -- pulled into its own
/// struct/function purely to keep that function under clippy's function-length
/// lint. See each field's own former inline comment (now on this struct).
struct OptimizeRunPrep {
    material_selection: MaterialSelection,
    custom_materials: Vec<GemMaterial>,
    defaulted_ri: Option<f64>,
    config: OptimizeConfig,
    provenance: RunProvenance,
    pending_optimize: Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    this_run: u64,
    run_epoch_done: Arc<AtomicU64>,
}

/// [`begin_tier_optimize_run`]'s prelude -- see [`OptimizeRunPrep`]'s own doc
/// comment. `st` is the already-borrowed `EditorState` (read-only: nothing here
/// mutates it); `run_epoch` is bumped here, the one side effect this otherwise
/// pure capture has.
fn prepare_optimize_run(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    st: &EditorState,
    weights: ObjectiveWeights,
    run_epoch: &Arc<AtomicU64>,
) -> OptimizeRunPrep {
    let mut material_selection = st.design.material.clone();
    // `OptimizeJob::custom_materials` (see `optimize_solve::spawn_optimize_solve`)
    // is a plain `Vec` -- a one-shot worker's own owned copy, not
    // `RenderContext`'s hot-path per-frame snapshot -- so this is the one actual
    // deep copy on this path, same as before `RenderContext::custom_materials`
    // became `Arc`-backed. Fetched before `default_optimize_material_ri` below
    // (moved up from its own original spot) so that default can resolve a CUSTOM
    // catalogue material's own RI too, not just a built-in's.
    let custom_materials = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let defaulted_ri =
        default_optimize_material_ri(&st.design, &mut material_selection, &custom_materials);
    // Budget/seed/polish are all real
    // `OptimizeConfig` fields already (`crates/indicatrix-cut-core/src/
    // optimize/search.rs`) that nothing on the GUI side ever set to
    // anything but their defaults -- read here from `EditorModel`
    // properties `editor_inspector.slint` still needs a form for.
    let seed = ui
        .global::<EditorModel>()
        .get_optimize_seed_text()
        .trim()
        .parse::<u64>()
        .unwrap_or(0);
    let max_evaluations = configured_optimize_max_evaluations(ui);
    let mut config = OptimizeConfig {
        weights,
        seed,
        max_evaluations,
        ..OptimizeConfig::default()
    };
    if !ui.global::<EditorModel>().get_optimize_polish_enabled() {
        config.polish_start_step_deg = None;
    }
    let this_run = run_epoch.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    OptimizeRunPrep {
        material_selection,
        custom_materials,
        defaulted_ri,
        config,
        provenance: RunProvenance::capture(st),
        pending_optimize: Arc::clone(&st.pending_optimize),
        this_run,
        run_epoch_done: Arc::clone(run_epoch),
    }
}

/// The Optimize tab's four weight-form inputs, bundled purely to keep
/// [`begin_tier_optimize_run`] under clippy's `too_many_arguments` lint. `Copy`:
/// every field already is (a `&str`/an `f32`), so passing this by value is a plain
/// copy, never a move clippy's `needless_pass_by_value` would rather see taken
/// by reference.
#[derive(Clone, Copy)]
struct OptimizeWeightForm<'a> {
    windowing: &'a str,
    extinction: &'a str,
    tilt_brilliance: &'a str,
    yield_weight: f32,
}

/// [`begin_tier_optimize_run`]'s own weight-form parsing, split out purely to
/// keep that function under clippy's function-length lint -- toasts and returns
/// `None` on a malformed field, exactly like the inline version this replaces.
fn parse_weights_or_toast(
    ui: &MainWindow,
    weight_form: OptimizeWeightForm<'_>,
) -> Option<ObjectiveWeights> {
    match parse_optimize_weights(
        weight_form.windowing,
        weight_form.extinction,
        weight_form.tilt_brilliance,
        weight_form.yield_weight,
    ) {
        Ok(weights) => Some(weights),
        Err(e) => {
            show_toast(ui, &e, "error");
            None
        }
    }
}

fn begin_tier_optimize_run(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    run_epoch: &Arc<AtomicU64>,
    weight_form: OptimizeWeightForm<'_>,
) {
    // Also guards against Solve or Deep Solve
    // already running -- see `setup_deep_solve_callback`'s own doc
    // comment above for why.
    let model = ui.global::<EditorModel>();
    if model.get_optimize_running() || model.get_solve_running() || model.get_deep_solve_running() {
        return;
    }
    let Some(weights) = parse_weights_or_toast(ui, weight_form) else {
        return;
    };
    let mut st = state.borrow_mut();
    if free_tier_indices(&st.design).is_empty() {
        // `EditorView` only shows this button enabled when `optimize_available`
        // is true -- this only guards a race with a concurrent edit disabling
        // it out from under a stale click, not the common path.
        show_toast(
            ui,
            "Optimize has nothing free to move on this design right now.",
            "error",
        );
        return;
    }
    // A second snapshot for the completion closure: the per-tier change
    // table needs the ORIGINAL design's tiers/names to label its rows,
    // regardless of whichever design the search actually ran against below.
    let design_snapshot = st.design.clone();
    // When `EditorModel.optimize_only_selected` is on and at
    // least one tier is multi-selected, every OTHER free tier is pinned to
    // its own current mast before the search ever sees it, so
    // `free_tier_indices` inside `optimize_design` only ever finds the
    // tiers the cutter actually asked it to touch. `AngleChange::index`
    // stays valid against `design_snapshot`/the real `st.design` either
    // way -- see `pin_non_selected_free_tiers`'s own doc comment.
    let only_selected = ui.global::<EditorModel>().get_optimize_only_selected();
    let Some(design) = optimize_target_design(ui, &st, only_selected) else {
        return;
    };
    let OptimizeRunPrep {
        material_selection,
        custom_materials,
        defaulted_ri,
        config,
        provenance,
        pending_optimize,
        this_run,
        run_epoch_done,
    } = prepare_optimize_run(ui, render_ctx, &st, weights, run_epoch);

    ui.global::<EditorModel>().set_optimize_running(true);
    ui.global::<EditorModel>()
        .set_optimize_status(optimize_start_status(defaulted_ri).into());
    ui.global::<EditorModel>()
        .set_optimize_status_is_problem(false);
    ui.global::<EditorModel>().set_optimize_can_apply(false);
    ui.global::<EditorModel>()
        .set_optimize_result_rows(ModelRc::new(
            VecModel::from(Vec::<OptimizeResultRow>::new()),
        ));
    ui.global::<EditorModel>()
        .set_optimize_change_rows(ModelRc::new(VecModel::from(
            Vec::<crate::OptimizeChangeRow>::new(),
        )));

    // See
    // `setup_deep_solve_callback`'s matching comment for why this reaches the
    // shared registry through `auto_solve::activity()` rather than a new
    // parameter, and why `cancel` reaches back through `state` rather than
    // `handle` (which does not exist yet at this point).
    let activity = auto_solve::activity();
    let activity_id = activity.as_ref().map(|a| {
        a.start(
            "optimize",
            "Optimize",
            Some({
                let state = Rc::clone(state);
                Box::new(move || {
                    if let Some(handle) = state.borrow().optimize.as_ref() {
                        handle.cancel();
                    }
                })
            }),
        )
    });
    if activity_id.is_some() {
        OPTIMIZE_ACTIVITY_ID.with(|cell| *cell.borrow_mut() = activity_id);
    }

    let ui_weak = ui.as_weak();
    let handle = optimize_solve::spawn_optimize_solve(
        ui_weak,
        design,
        material_selection,
        custom_materials,
        config,
        |ui: &MainWindow, progress: optimize_solve::OptimizeSolveProgress| {
            ui.global::<EditorModel>()
                .set_optimize_status(optimize_progress_status(&progress).into());
        },
        move |ui: &MainWindow, outcome: OptimizeSolveOutcome| {
            // This run is finishing on its own -- see `setup_deep_solve_callback`'s
            // matching comment for why an already-finished id is a harmless no-op.
            // Fetched on the UI thread, not captured -- this closure must be `Send`.
            if let (Some(activity), Some(id)) = (auto_solve::activity(), activity_id) {
                activity.finish(id);
            }
            OPTIMIZE_ACTIVITY_ID.with(|cell| {
                if *cell.borrow() == activity_id {
                    *cell.borrow_mut() = None;
                }
            });
            // A superseded run -- see this function's own doc comment.
            if run_epoch_done.load(AtomicOrdering::Relaxed) != this_run {
                return;
            }
            handle_optimize_outcome(
                ui,
                outcome,
                &provenance,
                &pending_optimize,
                &design_snapshot,
            );
        },
    );
    st.optimize = Some(handle);
}

/// The material-RI-defaulting half of [`setup_optimize_callback`]'s doc comment,
/// pulled out to keep that callback under clippy's function-length lint. Mutates
/// `selection` in place (its own field, no cross-tier state) and returns the
/// defaulted RI only when a default was actually applied, so the caller can fold
/// it into the initial status line via [`optimize_start_status`].
///
/// This reads `design.effective_refractive_index_with(custom)`
/// -- catalogue-aware, so a design on a CUSTOM material defaults to that material's
/// own real `n_D` -- rather than the built-ins-only
/// `Design::effective_refractive_index()`, which silently falls through to the
/// legacy schedule RI for any design naming a custom catalogue material and would
/// score the optimizer's objective against a material the design was never
/// actually set to.
fn default_optimize_material_ri(
    design: &indicatrix_cut_core::Design,
    selection: &mut indicatrix_cut_core::MaterialSelection,
    custom: &[GemMaterial],
) -> Option<f64> {
    if selection.name.is_some() || selection.refractive_index_override.is_some() {
        return None;
    }
    let n_d = design.effective_refractive_index_with(custom);
    selection.refractive_index_override = Some(n_d);
    Some(n_d)
}

/// The design [`begin_tier_optimize_run`] actually hands to `optimize_design` --
/// `st.design` unchanged, or (when multi-selection is active)
/// [`pin_non_selected_free_tiers`]'s restricted clone. Pulled out purely to keep
/// `begin_tier_optimize_run` under clippy's function-length lint.
///
/// Reuses the cached last solve, so the UI thread never solves synchronously here,
/// instead of calling [`Design::solve`] inline. Returns `None` (having already
/// toasted) rather than silently falling back to the unrestricted design, when no
/// cached solve matches this design's current tier count.
fn optimize_target_design(
    ui: &MainWindow,
    st: &EditorState,
    only_selected: bool,
) -> Option<Design> {
    if !only_selected || st.multi_selected.is_empty() {
        return Some(st.design.clone());
    }
    let Some(solved) = auto_solve::solid_last_solved()
        .and_then(|cache| {
            cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .filter(|solved| solved.len() == st.design.tiers.len())
    else {
        show_toast(
            ui,
            "Solve first, then Optimize with \"Only selected tiers\".",
            "error",
        );
        return None;
    };
    Some(pin_non_selected_free_tiers(
        &st.design,
        &solved,
        &st.multi_selected,
    ))
}

/// A clone of `design` where every FREE tier
/// (`indicatrix_cut_core::free_tier_indices`) NOT in `keep_free` is pinned to a
/// [`MeetConstraint::ScaleReference`] at its own CURRENT solved mast -- so
/// `optimize_design`, which only ever moves a tier `free_tier_indices` names,
/// cannot touch it. Every tier already in `keep_free`, and every tier that was
/// already pinned, is left exactly as it was.
///
/// Never removes or reorders a tier, so the returned design's `AngleChange::index`
/// values from a search run against it stay valid against the CALLER's own
/// original design (and `EditorState::apply_optimize_outcome`'s tier lookup) with
/// no translation needed.
///
/// Takes `solved` from the caller (the cached last solve for this exact design)
/// rather than calling [`Design::solve`] itself, so [`setup_optimize_callback`]
/// never blocks the UI thread on a synchronous solve here. `solved` must have one
/// entry per `design.tiers` in the same order (a mismatch is treated the same as
/// "nothing to pin" for the tiers past the shorter length, via [`Vec::get`] --
/// never a panic).
fn pin_non_selected_free_tiers(
    design: &Design,
    solved: &[SolvedTier],
    keep_free: &BTreeSet<usize>,
) -> Design {
    let mut restricted = design.clone();
    for index in free_tier_indices(design) {
        if keep_free.contains(&index) {
            continue;
        }
        let Some(mast) = solved.get(index).map(|s| s.mast) else {
            continue;
        };
        if let Some(tier) = restricted.tiers.get_mut(index) {
            tier.constraint = MeetConstraint::ScaleReference(mast);
        }
    }
    restricted
}

/// The initial "Optimizing..." status line [`setup_optimize_callback`] sets before
/// the search's first progress tick arrives, naming the defaulted RI (see
/// [`default_optimize_material_ri`]) when one was applied so the assumption is
/// visible from the very first frame, not just in hindsight.
fn optimize_start_status(defaulted_ri: Option<f64>) -> String {
    defaulted_ri.map_or_else(
        || "Optimizing... 0 evaluations, 0.0s elapsed".to_string(),
        |n_d| {
            format!(
                "Optimizing (no material set -- scored for n_d={n_d:.4})... 0 \
                 evaluations, 0.0s elapsed"
            )
        },
    )
}

/// The running "Optimizing..." status line for each progress tick, naming
/// [`optimize_solve::OptimizeSolveProgress::stage`] explicitly instead of always
/// showing an evaluation fraction: the two full-fidelity scorings that bracket
/// every run report zero-progress ticks of their own, which would otherwise leave
/// the counter frozen for over a second each, reading as a hang rather than real
/// (if invisible-to-the-counter) work. The coordinate and polish stages instead
/// show `progress.max_evaluations` (a single combined figure, the coordinate cap
/// plus the polish stage's own) so the polish stage's evaluations climbing does
/// not read as sailing past the run's own stated budget.
fn optimize_progress_status(progress: &optimize_solve::OptimizeSolveProgress) -> String {
    let elapsed = progress.elapsed.as_secs_f32();
    match progress.stage {
        SearchStage::BaselineFull => {
            format!(
                "Optimizing... scoring the starting point at full fidelity, {elapsed:.1}s elapsed"
            )
        }
        SearchStage::Coordinate => format!(
            "Optimizing... {} of ~{} evaluations, {elapsed:.1}s elapsed",
            progress.evaluations, progress.max_evaluations
        ),
        SearchStage::Polish => format!(
            "Optimizing (polish)... {} of ~{} evaluations, {elapsed:.1}s elapsed",
            progress.evaluations, progress.max_evaluations
        ),
        SearchStage::FinalFull => {
            format!("Optimizing... scoring the result at full fidelity, {elapsed:.1}s elapsed")
        }
    }
}

/// The completion handler [`setup_optimize_callback`] hands to
/// [`optimize_solve::spawn_optimize_solve`], pulled out to keep that callback under
/// clippy's function-length lint. `pending_optimize` is stashed here with the real,
/// honest result -- `None` whenever nothing an Apply click could safely commit
/// (failed, no changes found, or already stale).
///
/// `OptimizeOutcome::before_score`/`after_score` are both measured at
/// [`indicatrix_cut_core::optimize::ObjectiveFidelity::Full`] (the search itself
/// only ever compares candidates at `Fast`, see that module's doc comment), so
/// `after_score >= before_score` here is a real, full-fidelity regression, not a
/// Fast-vs-Full discrepancy -- reported as a problem (ruby, not the usual emerald
/// "success" styling `editor_inspector.slint` already keys off
/// `optimize_status_is_problem` for) rather than the plain "changed N tier(s)"
/// summary, which alone would read as unconditional success even when the changes
/// the rows *did* show made the full-fidelity result worse. Apply stays available: a
/// worse full-fidelity score is a warning, not a proof the user cannot possibly
/// want it.
///
/// A non-stale `Completed`/`Cancelled`/`Failed` outcome also toasts, so completion
/// is visible even when the Optimize tab is not the one currently open, and, when
/// a real result is waiting for Apply, switches the inspector to the Optimize tab
/// and uncollapses it -- a stale result gets its own dedicated toast instead (see
/// the `stale` branch below), never both.
fn handle_optimize_outcome(
    ui: &MainWindow,
    outcome: OptimizeSolveOutcome,
    provenance: &RunProvenance,
    pending_optimize: &Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    // The design this run started from, for naming the tiers in the per-tier change
    // table. A snapshot, not the live state: this runs from a `Send` closure.
    design: &indicatrix_cut_core::Design,
) {
    // Always cleared first, on every path below, including the replaced-design
    // early return: `optimize_status` doubles as the live progress readout and
    // `optimize_running` gates the Optimize button, so a handler that returns
    // without touching either leaves the panel stuck mid-run.
    ui.global::<EditorModel>().set_optimize_running(false);
    // New / Load Selected / Open Native swapped the design out from under this run:
    // the result describes a different stone, `clear_analysis_results` has already
    // blanked the panel on purpose, and `EditorState::pending_optimize` is a fresh
    // `Arc` the replacement installed -- so there is nothing here worth showing, and
    // nothing a cutter could act on. Silent, not even a toast: they asked for a new
    // design, not for a postmortem on the old one.
    if provenance.design_replaced() {
        clear_analysis_results(ui);
        return;
    }
    match outcome {
        OptimizeSolveOutcome::Failed { error } => {
            // `MissingAnchor`'s own `Display` names the block(s) but not the
            // remedy itself -- the same text the plain Solve banner already shows
            // for the identical failure  -- so this appends
            // the "add a tier" instruction only for that variant. Every other
            // `DesignSolveError` variant (a `TierTarget` that could not be
            // resolved, most commonly) already renders as a complete, actionable
            // sentence on its own -- see that type's own `Display` impl -- so
            // appending the anchor-specific remedy to it would be wrong.
            let status = if matches!(error, DesignSolveError::MissingAnchor(_)) {
                format!(
                    "Optimize failed: {error} -- add an Exact scale value tier there \
                     and Solve first."
                )
            } else {
                format!("Optimize failed: {error}.")
            };
            ui.global::<EditorModel>()
                .set_optimize_status(status.clone().into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(true);
            ui.global::<EditorModel>().set_optimize_can_apply(false);
            show_toast(ui, &status, "error");
        }
        OptimizeSolveOutcome::Completed { outcome }
        | OptimizeSolveOutcome::Cancelled { outcome } => {
            // An EDIT only -- a replacement returned above. The result still
            // describes this design a few edits ago, so it is shown for reference
            // with its own toast, but Apply stays disabled.
            let stale = provenance.is_stale();
            let no_net_improvement =
                !outcome.changes.is_empty() && outcome.after_score >= outcome.before_score;
            let status = if no_net_improvement {
                format!(
                    "No net improvement at full tilt fidelity -- not recommended \
                     ({})",
                    optimize_status_text(&outcome)
                )
            } else {
                optimize_status_text(&outcome)
            };
            ui.global::<EditorModel>()
                .set_optimize_status(status.clone().into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(no_net_improvement);
            ui.global::<EditorModel>()
                .set_optimize_result_rows(ModelRc::new(VecModel::from(optimize_result_rows(
                    &outcome,
                ))));
            // Which tiers Optimize actually wants to move, so a cutter can read the
            // proposal before deciding whether to apply it.
            ui.global::<EditorModel>()
                .set_optimize_change_rows(ModelRc::new(VecModel::from(optimize_change_rows(
                    &outcome, design,
                ))));
            let can_apply = !outcome.changes.is_empty() && !stale;
            ui.global::<EditorModel>().set_optimize_can_apply(can_apply);
            // An immediate push of the
            // SAME comparison `view::push_stale_content` keeps live on every later
            // edit (`state::result_is_stale` against `pending_optimize`'s own
            // stored generation) -- see `apply_deep_solve_outcome`'s matching
            // comment for why this cannot simply wait for the next edit.
            ui.global::<EditorModel>().set_optimize_stale(stale);
            *pending_optimize
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                can_apply.then_some((outcome, provenance.started_generation));
            if stale {
                show_toast(
                    ui,
                    "The design changed while Optimize was running -- its result is \
                     shown for reference but can no longer be applied. Re-run \
                     Optimize on the current design.",
                    "info",
                );
            } else {
                // A toast plus bringing the Optimize tab into view (only when there
                // is something to act on) is what gets a cutter's attention if they
                // switched to the Tier tab or collapsed the inspector while the run
                // was going -- otherwise completion would go unnoticed unless the
                // Optimize tab already happened to be open.
                show_toast(
                    ui,
                    &format!("Optimize finished: {status}"),
                    if no_net_improvement {
                        "info"
                    } else {
                        "success"
                    },
                );
                if can_apply {
                    ui.global::<EditorModel>().set_inspector_tab(2);
                    ui.global::<EditorModel>().set_inspector_collapsed(false);
                }
            }
        }
    }
}

/// "Cancel" (shown only while an Optimize run is in progress): see
/// [`optimize_solve::OptimizeSolveHandle::cancel`] for what this does and doesn't
/// stop. `editor_optimize_running` is deliberately left alone here: the worker's
/// own `on_done`, arriving within about a search evaluation's latency (Optimize's
/// own real mid-search checkpoint, see `optimize_solve`'s module doc comment),
/// still carries the real, honest partial result and is what actually clears it.
///
/// Still gives immediate feedback on the same click -- matching Deep Solve's own
/// cancel button (see [`setup_deep_solve_cancel_callback`]) for "cannot tell that
/// Cancel did anything" -- without the risk flipping `optimize_running` off early
/// would create: the abandoned worker is still finishing, and a click on "Optimize"
/// in that window would start a second search racing the first one.
///
/// A second click while a cancel is already in flight
/// is harmless but pointless -- `handle.cancel()` just stores `true` again, and
/// this only re-sets the same status text -- so rather than adding a new
/// `EditorModel` property (this callback has no way to know the click even
/// happened without one), the button itself latches locally in
/// `ui/components/editor_command_bar.slint`'s `AnalysisGroup` --
/// `optimize_cancel_requested`, reset the moment `EditorModel.optimize_running`
/// goes false again -- and disables/relabels itself entirely in Slint, so a
/// second click never reaches this callback at all. No Rust
/// change was needed here beyond the status text this callback already sets.
pub(in crate::gui::editor) fn setup_optimize_cancel_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_optimize_cancel(move || {
        if let Some(handle) = state.borrow().optimize.as_ref() {
            handle.cancel();
        }
        if let Some(ui) = ui_weak.upgrade() {
            // The search polls `cancel` once per tier decision (see the module doc
            // comment's "Cancellation is a REAL mid-search checkpoint" section), so
            // "finishing the current evaluation" is literally true, not just a
            // reassuring guess.
            ui.global::<EditorModel>()
                .set_optimize_status("Cancelling -- finishing the current evaluation...".into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(false);
        }
    });
}

/// "Apply" (shown only while `EditorState::pending_optimize` holds a real,
/// not-yet-stale result): commits the held [`OptimizeOutcome`] through
/// `EditorState::apply_optimize_outcome` (one [`Edit::ModifyTier`] per changed tier,
/// via `History`), the only place this crate turns an Optimize result into a real edit.
///
/// Re-checks the design generation against what the search ran on even though
/// `editor_optimize_can_apply` should already be `false` by the time a stale result
/// could reach here -- a second, authoritative guard against reinterpreting a stale
/// tier index against a design it no longer describes.
///
/// Peeks (clones) `pending_optimize` rather than `take`ing it up front, and only
/// actually consumes it once the apply has gone through: `take`ing the result
/// unconditionally before checking staleness would mean a cutter who clicked Apply
/// one edit too late loses the held Optimize result permanently -- with
/// `optimize_result_rows`/`optimize_status` still on screen (nothing here ever
/// clears those two) while the one thing that could still commit them,
/// `pending_optimize`, is already gone, leaving a full re-run as the only
/// recovery. Left in place on a failed apply
/// ([`indicatrix_cut_core::edit::EditError`]) for the same reason -- `apply_optimize_outcome`
/// re-applies each `Edit::ModifyTier` by absolute angle, so a retry after whatever
/// the error named gets fixed is harmless to attempt again, never a double-delta.
pub(in crate::gui::editor) fn setup_optimize_apply_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_optimize_apply(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        let snapshot = st
            .pending_optimize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some((outcome, started_generation)) = snapshot else {
            return;
        };
        if st.generation.load(AtomicOrdering::Relaxed) != started_generation {
            ui.global::<EditorModel>().set_optimize_can_apply(false);
            // The one consistent
            // wording every generation-guard discard uses --
            // see `too_many_planes_message`'s sibling fix (item 5) for the same
            // "tell the cutter what happened" rule applied to a different silent
            // discard. Amber ("warning"), not "error": nothing failed -- the
            // design simply moved on before this result could be trusted.
            show_toast(
                &ui,
                "Result discarded: the design changed while Optimize was running. Re-run \
                 Optimize.",
                "warning",
            );
            return;
        }
        match st.apply_optimize_outcome(&outcome) {
            Ok(applied) => {
                // Only consumed now that the apply actually went through.
                *st.pending_optimize
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                ui.global::<EditorModel>().set_optimize_can_apply(false);
                // A plain `refresh_editor_panel_stale` +
                // `submit_preview_replan` would leave the panel reading "Not solved" with
                // "-" masts right after a successful Apply, as if the commit were
                // still pending -- `refresh_all` is the same call `New`/`Load
                // Selected`/`Solve` use, a REAL solve (synchronous under
                // `auto_solve::should_solve_synchronously`, backgrounded above it),
                // so the table and viewport read solved immediately instead of
                // waiting on the next unrelated edit's auto-solve to catch up.
                // routed through
                // `refresh_all_now` (`Rc<RefCell<EditorState>>`-based, paints
                // "Solving" before a synchronous solve runs) -- see that
                // function's own doc comment.
                drop(st);
                refresh_all_now(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &state,
                    false,
                );
                // The result table/status above still describe the PROPOSAL that
                // was just committed -- prefixing makes clear it is no longer
                // pending, rather than lingering as if Apply were still waiting.
                let applied_status = format!(
                    "Applied: {}",
                    ui.global::<EditorModel>().get_optimize_status()
                );
                ui.global::<EditorModel>()
                    .set_optimize_status(applied_status.into());
                show_toast(
                    &ui,
                    &format!("Applied {applied} tier change(s) from Optimize."),
                    "success",
                );
            }
            Err(e) => {
                ui.global::<EditorModel>().set_optimize_can_apply(false);
                show_toast(&ui, &e.to_string(), "error");
            }
        }
    });
}

/// A "Preview" toggle that shows the pending
/// Optimize result's candidate geometry in the shared solid viewport BEFORE the
/// cutter commits to Apply, and puts the real design straight back the moment it is
/// switched off (or the pending result stops being appliable at all). Wired to
/// `EditorModel.optimize_preview_toggled(bool)`, invoked from the checkbox in
/// `editor_inspector.slint`.
///
/// `true`: builds the candidate via [`build_optimize_preview_design`] from
/// whichever design `EditorState::pending_optimize` actually ran against (NOT
/// necessarily `state`'s current design -- a toggle click after further edits
/// would otherwise silently preview against the wrong baseline) and shows it via
/// [`submit_design_ghost_preview`], which never touches `solid_last_solved`/the
/// generation stash (see that function's own doc comment) -- so leaving this
/// toggle on can never corrupt the next real edit's incremental resolve.
///
/// `false` (or nothing pending any more): resubmits the REAL, live design through
/// the ordinary [`submit_preview_replan`] path, exactly undoing the ghost.
pub(in crate::gui::editor) fn setup_optimize_preview_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_optimize_preview_toggled(move |enabled: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let pending = enabled
                .then(|| {
                    st.pending_optimize
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                })
                .flatten();
            // `started_generation` is read and then
            // ignored -- built straight from `st.design` regardless of whether
            // the search that produced `outcome` ran against an OLDER
            // generation. Optimize completes, the user removes a tier (bumping
            // `st.generation`), then toggles Preview on: the stale
            // `AngleChange` indices from the search were applied to the
            // shifted tier list and the wrong tiers moved, while Apply was
            // (correctly) already greyed out by the same staleness. Mirrors
            // `view::apply_pending_optimize_ghost`'s own generation check
            // (private to that file, so re-checked here rather than shared).
            let current_generation = st.generation.load(AtomicOrdering::Relaxed);
            if let Some((outcome, started_generation)) = pending
                && started_generation == current_generation
            {
                let candidate = build_optimize_preview_design(&st.design, &outcome);
                if submit_design_ghost_preview(&ui, &render_ctx, &preview_state, &candidate) {
                    return;
                }
                // The candidate itself does not close/solve any more (an edit
                // moved the design since the search ran) -- fall through and show
                // the real design instead of leaving whatever the viewport had.
            }
            submit_preview_replan(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &st,
                BTreeSet::new(),
                false,
            );
        });
}

/// Computes the full 4-axis tilt-performance sweep for the
/// design currently open in the editor -- not whatever a catalogue row currently
/// stores -- via `gui::batch::tilt::tilt_curves_for_planes`, entirely off the UI
/// thread (~1.36s, see that function's own doc comment). Closes the crate's last
/// 3 clippy warnings: `tilt_curves_for_planes`/`save_tilt_curves_for_entry` were
/// real and tested but had no production caller before this.
///
/// `run_epoch` (the same superseded-run guard [`setup_deep_solve_callback`]/
/// [`setup_optimize_callback`] use) is what keeps a superseded run from ever
/// overwriting a fresher result: a second click bumps it, and the FIRST run's
/// eventual completion becomes a no-op the moment [`apply_batch_tilt_result`]
/// checks back in. Body split into [`begin_batch_tilt_for_open_design`]/
/// [`run_batch_tilt_for_open_design`]/[`apply_batch_tilt_result`] purely to keep
/// each piece under clippy's function-length lint.
pub(in crate::gui::editor) fn setup_batch_tilt_for_open_design_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();
    let run_epoch = Arc::new(AtomicU64::new(0));
    ui.global::<EditorModel>()
        .on_compute_tilt_curves_for_open_design(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            begin_batch_tilt_for_open_design(&ui, &state, &render_ctx, &db, &run_epoch);
        });
}

/// Everything one "Compute Tilt Curves" run needs, snapshotted once on the UI
/// thread by [`begin_batch_tilt_for_open_design`] and carried unchanged through
/// [`run_batch_tilt_for_open_design`]/[`apply_batch_tilt_result`] -- bundled into
/// one struct purely to keep those two functions' signatures under clippy's
/// argument-count lint.
struct BatchTiltRun {
    /// A snapshot of `state.design` at dispatch time -- solved by
    /// [`run_batch_tilt_for_open_design`], off the UI thread.
    design: Design,
    /// A snapshot of `RenderContext::custom_materials` at dispatch time.
    custom_materials: Vec<GemMaterial>,
    /// `state.source_entry_id` at dispatch time -- `None` means this design has
    /// never been saved to the catalogue, so a finished sweep has nothing to
    /// save against.
    source_entry_id: Option<i64>,
    /// Whether `state.design` has changed since this run started -- see
    /// [`RunProvenance::is_stale`].
    provenance: RunProvenance,
    /// The shared superseded-run counter -- see
    /// [`setup_batch_tilt_for_open_design_callback`]'s own doc comment.
    run_epoch_done: Arc<AtomicU64>,
    /// This run's own epoch, captured at dispatch time.
    this_run: u64,
    /// The shared design database -- only ever reached for a save, and only once
    /// this run is confirmed current and non-stale.
    db: Arc<Mutex<Database>>,
    /// The window this run reports back to, once finished.
    ui_weak: slint::Weak<MainWindow>,
}

impl BatchTiltRun {
    /// Whether a newer "Compute Tilt Curves" click has started since this run
    /// was dispatched -- `false` once that happens, meaning this run's result
    /// must never touch `EditorModel`/the database at all.
    fn is_current(&self) -> bool {
        self.run_epoch_done.load(AtomicOrdering::Relaxed) == self.this_run
    }
}

/// `on_compute_tilt_curves_for_open_design`'s actual body -- snapshots
/// `state.design` (and everything else [`BatchTiltRun`] bundles) on the UI
/// thread, cheap enough to do inline, then hands the real work to
/// [`run_batch_tilt_for_open_design`] on a background thread -- matching
/// `gui::tilt::tilt_profile::spawn_tilt_profile_sweep`'s own `thread::spawn` +
/// `upgrade_in_event_loop` shape rather than inventing a second pattern.
fn begin_batch_tilt_for_open_design(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
    run_epoch: &Arc<AtomicU64>,
) {
    let st = state.borrow();
    let design = st.design.clone();
    let source_entry_id = st.source_entry_id;
    let provenance = RunProvenance::capture(&st);
    drop(st);

    let custom_materials = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();

    let this_run = run_epoch.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    let run = BatchTiltRun {
        design,
        custom_materials,
        source_entry_id,
        provenance,
        run_epoch_done: Arc::clone(run_epoch),
        this_run,
        db: Arc::clone(db),
        ui_weak: ui.as_weak(),
    };
    show_toast(
        ui,
        "Computing tilt-performance curves for this design...",
        "info",
    );
    thread::spawn(move || run_batch_tilt_for_open_design(run));
}

/// The background half of [`begin_batch_tilt_for_open_design`]: solves
/// `run.design` (the one real, otherwise-unavoidable ~1.36s+ solve cost of this
/// action), builds its planes/material through
/// [`EditorMaterialLookup`]/[`resolved_gem_material`] (the same precedence
/// `bridge::render_thread::context::resolve_material` uses), runs the
/// local-only sweep (`worker: None` -- this action has no worker-settings
/// parameter to dispatch remotely against), and hands the result to
/// [`apply_batch_tilt_result`] back on the UI thread. A `MissingAnchor` solve
/// failure is reported the same superseded-run-aware way a successful sweep is,
/// rather than silently dropped.
fn run_batch_tilt_for_open_design(run: BatchTiltRun) {
    let solved = match run.design.solve() {
        Ok(solved) => solved,
        Err(e) => {
            let ui_weak = run.ui_weak.clone();
            let run_epoch_done = Arc::clone(&run.run_epoch_done);
            let this_run = run.this_run;
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                if run_epoch_done.load(AtomicOrdering::Relaxed) == this_run {
                    show_toast(&ui, &format!("Cannot compute tilt curves: {e}"), "error");
                }
            });
            return;
        }
    };
    let planes = auto_solve::design_to_gpu_planes_from_solved(&run.design, &solved);
    let lookup = EditorMaterialLookup::new(&run.custom_materials);
    let material = resolved_gem_material(&run.design.material, &lookup);
    let cancel = AtomicBool::new(false);
    let curves = tilt::tilt_curves_for_planes(&planes, &material, None, &cancel);

    let ui_weak = run.ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        apply_batch_tilt_result(&ui, curves, &run);
    });
}

/// The UI-thread completion half of [`run_batch_tilt_for_open_design`]: drops a
/// superseded run's result outright ([`BatchTiltRun::is_current`] -- see
/// [`setup_batch_tilt_for_open_design_callback`]'s own doc comment), reports a
/// sweep failure, and -- for a real result from the CURRENT run -- either saves
/// it via `gui::batch::tilt::save_tilt_curves_for_entry` (when
/// `run.source_entry_id` names a real catalogue row and the design has not
/// changed since this run started) or explains why it wasn't saved. A design
/// that changed mid-run is deliberately shown but never persisted: the curves
/// are honest for the design AS IT WAS when the run started, and saving them
/// now would silently attach a stale sweep to whatever the catalogue row
/// currently is.
fn apply_batch_tilt_result(
    ui: &MainWindow,
    curves: Option<TiltPerformanceCurves>,
    run: &BatchTiltRun,
) {
    if !run.is_current() {
        return;
    }
    let Some(curves) = curves else {
        show_toast(
            ui,
            "Tilt-curve computation failed for this design.",
            "error",
        );
        return;
    };
    if run.provenance.is_stale() {
        // Wording consistent with every other generation guard in this file --
        // see `setup_optimize_apply_callback`'s matching fix.
        // "Result discarded" wording and amber class as every other generation
        // guard in this file -- see `setup_optimize_apply_callback`'s matching
        // fix. The curves themselves are still shown (never saved, per this
        // function's own doc comment), so "discarded" here means specifically
        // "not saved," which the sentence's own second half already says.
        show_toast(
            ui,
            "Result discarded: the design changed while computing tilt curves. Re-run to \
             save an up-to-date result.",
            "warning",
        );
        return;
    }
    match run.source_entry_id {
        Some(entry_id) => {
            if tilt::save_tilt_curves_for_entry(&run.db, entry_id, &curves) {
                show_toast(
                    ui,
                    "Saved tilt-performance curves for this design.",
                    "success",
                );
            } else {
                show_toast(
                    ui,
                    "Could not save tilt-performance curves for this design.",
                    "error",
                );
            }
        }
        None => show_toast(
            ui,
            "Computed tilt-performance curves -- save this design to the \
             catalogue to keep them.",
            "info",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deep_solve::TierMastDelta;

    // --- needs_background_baseline: the Deep Solve UI-thread-solve guard ---
    //
    // These are the pure decision `capture_deep_solve_run_prep` makes about
    // whether it has a usable cached plain solve -- the fix for Deep Solve
    // freezing the UI thread on a large design relies on this NEVER falling back
    // to a synchronous `Design::solve()` here; the worker computes a missing
    // baseline off-thread instead (`SolveKind::Verified::compute_baseline`).

    fn dummy_solved_tier(mast: f64) -> indicatrix::geometry::meet_solver::SolvedTier {
        indicatrix::geometry::meet_solver::SolvedTier {
            mast,
            strategy: indicatrix::geometry::meet_solver::SolveStrategy::DependencyOrder,
            detail: String::new(),
        }
    }

    #[test]
    fn needs_background_baseline_is_false_when_the_cache_matches_the_tier_count() {
        let cache = vec![dummy_solved_tier(1.0), dummy_solved_tier(2.0)];
        assert!(!needs_background_baseline(Some(&cache), 2));
    }

    #[test]
    fn needs_background_baseline_is_true_when_the_cache_is_missing() {
        assert!(needs_background_baseline(None, 2));
    }

    #[test]
    fn needs_background_baseline_is_true_when_the_cache_is_the_wrong_tier_count() {
        // A tier was just added/removed and the background auto-solve has not
        // caught up yet -- the exact case that used to fall back to a
        // synchronous `Design::solve()` on the UI thread.
        let cache = vec![dummy_solved_tier(1.0)];
        assert!(needs_background_baseline(Some(&cache), 2));
    }

    // --- tier_index_from_row_number ---

    #[test]
    fn tier_index_from_row_number_parses_the_1_based_label_back_to_a_0_based_index() {
        assert_eq!(tier_index_from_row_number("#1"), Some(0));
        assert_eq!(tier_index_from_row_number("#17"), Some(16));
    }

    #[test]
    fn tier_index_from_row_number_rejects_anything_that_is_not_hash_digits() {
        assert_eq!(tier_index_from_row_number("17"), None);
        assert_eq!(tier_index_from_row_number("#"), None);
        assert_eq!(tier_index_from_row_number("#abc"), None);
        assert_eq!(tier_index_from_row_number(""), None);
    }

    #[test]
    fn tier_index_from_row_number_rejects_hash_zero() {
        // This format never produces "#0" (rows are numbered from #1), so there is
        // no valid 0-based index to underflow to.
        assert_eq!(tier_index_from_row_number("#0"), None);
    }

    // --- verified_mast_for ---

    fn delta(tier_index: usize, before: f64, after: f64) -> TierMastDelta {
        TierMastDelta {
            tier_index,
            before_mast: before,
            after_mast: after,
        }
    }

    #[test]
    fn verified_mast_for_finds_the_matching_tiers_after_mast() {
        let deltas = vec![delta(0, 1.0, 1.1), delta(3, 2.0, 2.5)];
        assert_eq!(verified_mast_for(&deltas, 3), Some(2.5));
    }

    #[test]
    fn verified_mast_for_is_none_when_the_tier_is_not_in_the_list() {
        let deltas = vec![delta(0, 1.0, 1.1)];
        assert_eq!(verified_mast_for(&deltas, 5), None);
    }

    // --- default_optimize_material_ri (must resolve a CUSTOM catalogue
    // material's own RI, not just a built-in's) ---

    fn fixture_design() -> Design {
        let preform = indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5);
        Design::fresh(preform, 96, 8, 1.54)
    }

    #[test]
    fn default_optimize_material_ri_leaves_an_already_named_selection_alone() {
        let design = fixture_design();
        let mut selection = indicatrix_cut_core::MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        assert_eq!(
            default_optimize_material_ri(&design, &mut selection, &[]),
            None
        );
        assert_eq!(selection.refractive_index_override, None);
    }

    #[test]
    fn default_optimize_material_ri_defaults_to_the_designs_effective_ri() {
        let mut design = fixture_design();
        design.meta.refractive_index = 1.62;
        let mut selection = indicatrix_cut_core::MaterialSelection::none();
        let defaulted = default_optimize_material_ri(&design, &mut selection, &[]);
        assert_eq!(defaulted, Some(1.62));
        assert_eq!(selection.refractive_index_override, Some(1.62));
    }

    /// The actual bug this finding closes: a design naming a CUSTOM catalogue
    /// material (not one of the built-ins `Design::effective_refractive_index`
    /// alone can resolve) must default to THAT material's own RI, not fall through
    /// to the legacy schedule value -- the optimizer would otherwise score against
    /// a material the design was never actually set to.
    #[test]
    fn default_optimize_material_ri_resolves_a_custom_materials_own_ri() {
        let mut design = fixture_design();
        design.material.name = Some("My Custom Garnet".to_string());
        // A legacy schedule RI that must NOT be the value picked, proving this
        // reads the custom material rather than falling through past it.
        design.meta.refractive_index = 1.54;
        let mut custom = GemMaterial::diamond();
        custom.name = "My Custom Garnet".to_string();
        custom.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
            a: 1.74,
            b: 0.0,
            c: 0.0,
        };
        let mut selection = indicatrix_cut_core::MaterialSelection::none();
        let defaulted = default_optimize_material_ri(&design, &mut selection, &[custom]);
        assert!(
            (defaulted.unwrap() - 1.74).abs() < 1e-6,
            "expected the custom material's own RI (1.74), got {defaulted:?}"
        );
        assert!((selection.refractive_index_override.unwrap() - 1.74).abs() < 1e-6);
    }

    // --- pin_non_selected_free_tiers ---

    fn tier(name: &str, constraint: MeetConstraint) -> indicatrix_cut_core::ConstraintTier {
        indicatrix_cut_core::ConstraintTier {
            angle_deg: -40.0,
            name: name.to_string(),
            indices: vec![0.0, 24.0],
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    fn two_free_tier_design() -> Design {
        Design::new(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
            indicatrix_cut_core::ScheduleMeta::default(),
            vec![
                tier("Anchor", MeetConstraint::ScaleReference(0.5)),
                tier("Free1", MeetConstraint::MeetExisting),
                tier("Free2", MeetConstraint::MeetExisting),
            ],
        )
    }

    #[test]
    fn pin_non_selected_free_tiers_pins_every_free_tier_not_kept() {
        let design = two_free_tier_design();
        let solved = design.solve().expect("fixture must solve");
        let keep = BTreeSet::from([1]);
        let restricted = pin_non_selected_free_tiers(&design, &solved, &keep);
        // Tier 1 (kept) is untouched: still free.
        assert!(matches!(
            restricted.tiers[1].constraint,
            MeetConstraint::MeetExisting
        ));
        // Tier 2 (not kept) is now pinned to its own solved mast.
        assert!(matches!(
            restricted.tiers[2].constraint,
            MeetConstraint::ScaleReference(_)
        ));
        // The already-pinned anchor tier is unaffected either way.
        assert!(matches!(
            restricted.tiers[0].constraint,
            MeetConstraint::ScaleReference(_)
        ));
    }

    #[test]
    fn pin_non_selected_free_tiers_keeps_every_free_tier_when_keep_set_is_full() {
        let design = two_free_tier_design();
        let solved = design.solve().expect("fixture must solve");
        let keep = BTreeSet::from([1, 2]);
        let restricted = pin_non_selected_free_tiers(&design, &solved, &keep);
        assert!(matches!(
            restricted.tiers[1].constraint,
            MeetConstraint::MeetExisting
        ));
        assert!(matches!(
            restricted.tiers[2].constraint,
            MeetConstraint::MeetExisting
        ));
    }

    #[test]
    fn pin_non_selected_free_tiers_never_reorders_or_removes_a_tier() {
        let design = two_free_tier_design();
        let solved = design.solve().expect("fixture must solve");
        let keep = BTreeSet::from([1]);
        let restricted = pin_non_selected_free_tiers(&design, &solved, &keep);
        assert_eq!(restricted.tiers.len(), design.tiers.len());
        for (a, b) in restricted.tiers.iter().zip(&design.tiers) {
            assert_eq!(a.name, b.name);
        }
    }
}
