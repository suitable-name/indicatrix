//! "Deep Solve"'s dispatch and completion handler.

use super::{DEEP_SOLVE_ACTIVITY_ID, LAST_DEEP_SOLVE_DELTAS, RunProvenance};
use crate::{
    EditorModel, MainWindow,
    gui::{
        editor::{
            auto_solve, deep_solve,
            stall_guard::stall_guard,
            state::EditorState,
            view::{deep_solve_tier_rows, format_deep_solve_report},
        },
        show_toast,
    },
};
use indicatrix::geometry::{meet_solver::SolvedTier, stone_metrics::ExternalProportions};
use indicatrix_cut_core::Design;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

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
/// no such guard, see [`super::deep_solve_pin::setup_deep_solve_cancel_callback`].
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
    // the shared cache is now generation-tagged (`(u64, Vec<SolvedTier>)`)
    // -- only the masts themselves matter here.
    let current_masts = cached.and_then(|(_, solved)| {
        (!needs_background_baseline(Some(solved.as_slice()), st.design.tiers.len()))
            .then_some(solved)
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
            // The design was REPLACED (New / Load Selected / Open)
            // while this run was still in flight: checked here, before
            // touching EITHER the busy flag or the result panel, because by
            // the time this abandoned run's completion arrives the cutter may
            // already have started (and even finished) a FRESH Deep Solve
            // against the new design -- unconditionally resetting
            // `deep_solve_running`/repainting the panel below would then
            // clobber that fresh run's own live state with this stale one's,
            // exactly the "stale completion wipes the new design's results"
            // failure mode. `clear_analysis_results` already ran once, from
            // `EditorState::replace_wholesale`'s own call site, at the moment
            // of replacement -- see `apply_deep_solve_outcome`'s own doc
            // comment for why this must NOT call it again.
            if provenance.design_replaced() {
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
    // The design-replaced case (New / Load Selected / Open swapped in a
    // different stone while this run was still in flight) is already filtered
    // out by this function's one caller, in `setup_deep_solve_callback`'s own
    // completion closure -- see that closure's own comment for why it
    // must return before EITHER of this function's own effects, not merely
    // before `clear_analysis_results` a second time.
    debug_assert!(
        !provenance.design_replaced(),
        "the caller must filter out a replaced design before calling this"
    );
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
