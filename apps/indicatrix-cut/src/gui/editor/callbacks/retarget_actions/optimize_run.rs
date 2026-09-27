//! `RetargetMode::Optimize`'s off-thread search: the synchronous prelude
//! [`start_optimize_run`] runs on the UI thread before handing off to
//! [`optimize_solve::spawn_optimize_solve`], and [`finish_optimize_run`] is its
//! completion handler -- see this group's own `mod.rs` doc comment
//! ("`RetargetMode::Optimize` runs off the UI thread").

use super::{
    RETARGET_ASYNC, apply_ghost_preview_or_revert,
    material::{resolve_target_selection, resolved_material_from_selection},
    proposal::cancel_optimize_run,
    proposal_view::{
        RetargetRowView, RetargetView, push_retarget_view, push_target_error, push_target_readout,
        row_view,
    },
};
use crate::{
    EditorModel, MainWindow, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            optimize_solve::{self, OptimizeSolveOutcome},
            retarget::{self, CrownShift, RetargetMode, RetargetProposal},
            stale::{self, ResultKind},
            state::EditorState,
            view,
        },
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix::{geometry::meet_solver::Block, optics::materials::GemMaterial};
use indicatrix_cut_core::{Design, ObjectiveWeights, OptimizeConfig, ResolvedMaterial};
use slint::ComponentHandle;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

/// `RetargetMode::Optimize`'s config, reusing the standalone Optimize panel's own
/// weight fields when they parse, else [`OptimizeConfig::default`]'s weights.
/// Best-effort: a malformed weight field isn't this dialog's form to validate (the
/// Optimize panel's own button already does), so this never surfaces a second error.
fn optimize_config_from_ui(ui: &MainWindow) -> OptimizeConfig {
    let weights = view::parse_optimize_weights(
        &ui.global::<EditorModel>().get_optimize_weight_windowing(),
        &ui.global::<EditorModel>().get_optimize_weight_extinction(),
        &ui.global::<EditorModel>()
            .get_optimize_weight_tilt_brilliance(),
        ui.global::<EditorModel>().get_optimize_weight_yield(),
    )
    .unwrap_or_else(|_| ObjectiveWeights::default());
    OptimizeConfig {
        weights,
        ..OptimizeConfig::default()
    }
}

/// Pushes an `anchored_tier_errors` view (see `RetargetError::AnchoredTiers`'s own
/// doc comment) and clears any pending proposal -- [`start_optimize_run`]'s up-front
/// refusal path, pulled out to keep that function under clippy's function-length
/// lint.
fn push_anchored_refusal(ui: &MainWindow, st: &mut EditorState, anchored: &[(usize, String)]) {
    let anchored_errors = anchored
        .iter()
        .map(|(index, name)| format!("#{index} \"{name}\""))
        .collect();
    push_retarget_view(
        ui,
        RetargetView {
            rows: Vec::new(),
            notes: Vec::new(),
            anchored_errors,
            solve_error: String::new(),
        },
    );
    st.pending_retarget = None;
    ui.global::<RetargetModel>().set_is_busy(false);
}

/// Pushes `RetargetModel.optimize_evaluations`/`optimize_max_evaluations`, clamping
/// each to `i32`'s range (real evaluation counts never come close) -- shared by
/// [`start_optimize_run`]'s own initial `0`/`max_evaluations` push and the running
/// search's own progress ticks. Also feeds the SAME fraction into this run's own
/// `ActivityRegistry` entry, if one is registered (
/// section 3.3/8, BUILD item 1) -- `max_evaluations == 0` (not yet known) reports
/// indeterminate rather than a division by zero.
fn set_optimize_progress(ui: &MainWindow, evaluations: usize, max_evaluations: usize) {
    ui.global::<RetargetModel>()
        .set_optimize_evaluations(i32::try_from(evaluations).unwrap_or(i32::MAX));
    ui.global::<RetargetModel>()
        .set_optimize_max_evaluations(i32::try_from(max_evaluations).unwrap_or(i32::MAX));
    if let (Some(activity), Some(id)) = (
        auto_solve::activity(),
        RETARGET_ASYNC.with(|cell| cell.borrow().activity_id),
    ) {
        let fraction = if max_evaluations == 0 {
            crate::gui::editor::activity::INDETERMINATE
        } else {
            evaluations as f32 / max_evaluations as f32
        };
        activity.progress(id, fraction);
    }
}

/// [`super::proposal::setup_retarget_proposal_changed_callback`]'s
/// `RetargetMode::Optimize` branch: runs the cheap, synchronous part of
/// `retarget::build_proposal`'s Optimize path (target resolution, scope, the
/// up-front anchored-tier check, the shift seed) here on the UI thread, then hands
/// the actual `optimize_design` search off to
/// [`optimize_solve::spawn_optimize_solve`]. `st` is the already-borrowed
/// `EditorState` (borrowed by the caller so the anchored-tier/no-op cases can update
/// `pending_retarget` without a second borrow).
///
/// `preview_state`/`solid_last_solved`: reverts the viewport
/// to the real, live design the moment a fresh search starts (so a stale ghost from
/// a previous proposal never lingers through several seconds of search with no
/// candidate of its own yet) -- [`finish_optimize_run`] shows the new ghost once
/// the search actually has a result, via the same `Arc` clones stashed on
/// [`OptimizeRunContext`].
pub(super) fn start_optimize_run(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
    st: &mut EditorState,
    crown: CrownShift,
) {
    let custom = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let combo_index = ui.global::<RetargetModel>().get_target_material_index();
    let ri_text = ui.global::<RetargetModel>().get_target_ri_override_text();
    // Checked up front so an unparseable override refuses the run with an
    // explicit error, matching `rebuild_and_push`'s own guard, instead of silently
    // searching against `st.design.material` unchanged.
    let selection = match resolve_target_selection(&st.design, &custom, combo_index, &ri_text) {
        Ok(selection) => selection,
        Err(message) => {
            push_target_error(ui, &message);
            // Matches `push_anchored_refusal`'s own clearing: `RETARGET_ASYNC.pending`
            // is already `None` (the caller cancelled/superseded before calling this
            // function), but `st.pending_retarget` could still hold an earlier,
            // genuinely valid Optimize proposal -- Apply must not offer THAT one up
            // as if it answered this now-unparseable text.
            st.pending_retarget = None;
            ui.global::<RetargetModel>().set_is_busy(false);
            return;
        }
    };
    let target = resolved_material_from_selection(&selection, &custom);
    push_target_readout(ui, &selection, &target);

    let (scope, blocks) = retarget::retarget_scope(&st.design);
    let anchored = retarget::anchored_tiers_in(&st.design, &scope);
    if !anchored.is_empty() {
        push_anchored_refusal(ui, st, &anchored);
        return;
    }

    // `_with(&custom)`, not the bare (built-ins-and-override-only) accessor: `st.
    // design.material.name` may name a CUSTOM catalogue material this session
    // defined, which the bare accessor cannot see at all and would silently score
    // against the wrong index for -- see `Design::effective_refractive_index_with`'s
    // own doc comment.
    let n_from = st.design.effective_refractive_index_with(&custom);
    let n_to = target.n_d;
    let seeded = retarget::seed_shift_design(&st.design, &scope, &blocks, n_from, n_to, crown);
    let ctx = OptimizeRunContext {
        original: st.design.clone(),
        seeded: seeded.clone(),
        scope,
        blocks,
        n_to,
        target,
        custom: custom.clone(),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
    };
    let config = optimize_config_from_ui(ui);
    let max_evaluations = config.max_evaluations;

    ui.global::<RetargetModel>().set_is_busy(true);
    // Registered
    // before the worker spawns (and before the first `set_optimize_progress`
    // call just below, so that call's own activity-progress push actually has
    // an id to reach), so the status strip's activity list shows it from the
    // first frame -- same `auto_solve::activity()` stashed-handle pattern
    // `solve_actions::setup_optimize_callback` uses for the standalone Optimize
    // panel. Cancelling from the activity chip runs [`cancel_optimize_run`], the
    // exact same reset `on_cancel_optimize` (below) applies.
    let activity_id = auto_solve::activity().map(|a| {
        let ui_weak = ui.as_weak();
        a.start(
            "retarget_optimize",
            "Retarget Optimize",
            Some(Box::new(move || {
                // `cancel_optimize_run` reaches `ActivityRegistry::finish` (via
                // `RetargetAsyncRun::cancel_and_supersede`), and THIS closure
                // runs synchronously from inside `ActivityRegistry::cancels.
                // borrow().invoke(id)` (`ActivityModel.cancel`'s own handler,
                // `activity.rs::ActivityRegistry::new`) -- calling `finish` (its
                // own `self.cancels.borrow_mut()`) right here would panic on a
                // re-entrant borrow. Deferred one event-loop tick via a
                // single-shot `Timer` (the same "run after this handler returns"
                // idiom `auto_solve::schedule_idle_replan_if_stale` already
                // uses), by which point `invoke`'s own borrow has been dropped.
                let ui_weak = ui_weak.clone();
                let timer = slint::Timer::default();
                timer.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::ZERO,
                    move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            cancel_optimize_run(&ui);
                        }
                    },
                );
            })),
        )
    });
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().activity_id = activity_id);
    set_optimize_progress(ui, 0, max_evaluations);
    // A stale ghost from a previous proposal must not sit in
    // the viewport for the several seconds this search can take before it has any
    // candidate of its own to show -- `finish_optimize_run` shows the new one once
    // the search actually has a result.
    view::submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        st,
        BTreeSet::new(),
        false,
    );

    let run_id = RETARGET_ASYNC.with(|cell| {
        let mut run = cell.borrow_mut();
        run.run_id = run.run_id.wrapping_add(1);
        run.run_id
    });
    let started_generation = st.generation.load(AtomicOrdering::Relaxed);

    // `on_done` below must be `Send` (`spawn_optimize_solve`'s own bound, since it
    // crosses into the worker thread before hopping back to the UI thread) --
    // everything it captures is therefore plain owned data (bundled into `ctx`),
    // never `Rc<RefCell<EditorState>>`. See this group's own `mod.rs` doc comment.
    let handle = optimize_solve::spawn_optimize_solve(
        ui.as_weak(),
        seeded,
        selection,
        custom,
        config,
        |ui: &MainWindow, progress: optimize_solve::OptimizeSolveProgress| {
            set_optimize_progress(ui, progress.evaluations, progress.max_evaluations);
        },
        move |ui: &MainWindow, outcome: OptimizeSolveOutcome| {
            finish_optimize_run(ui, outcome, run_id, started_generation, &ctx);
        },
    );
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().handle = Some(handle));
    // `st` (the caller's already-borrowed `EditorState`) intentionally receives no
    // further update here: the async result reaches `EditorState::pending_retarget`
    // only through `super::apply::setup_retarget_apply_callback` reading
    // `RETARGET_ASYNC::pending` back out on the main thread once the worker
    // finishes, never from this function's own return.
}

/// Everything [`finish_optimize_run`] needs about the run it is completing, bundled
/// purely to keep that function's own signature short -- the same reasoning
/// `optimize_solve::OptimizeJob` documents on itself. Built once by
/// [`start_optimize_run`] and moved, whole, into the `Send`-bound completion closure.
struct OptimizeRunContext {
    /// The design as it stood before the shift seed/search ran -- every row's
    /// `old_angle` reads from this, per [`retarget::rows_from_outcome`].
    original: Design,
    /// `original` with every `scope` tier's angle already shifted -- the baseline
    /// [`optimize_solve::spawn_optimize_solve`]'s own worker searched from.
    seeded: Design,
    /// The pavilion/crown tier indices this run retargets (see
    /// [`retarget::retarget_scope`]).
    scope: Vec<usize>,
    /// `scope`'s own [`Block`] classification, same order.
    blocks: Vec<Block>,
    /// The target material's refractive index -- `retarget::build_notes`/
    /// `retarget::rows_from_outcome` both need it.
    n_to: f64,
    /// The resolved target material this run searched against.
    target: ResolvedMaterial,
    /// The custom catalogue materials in scope when this run started -- carried
    /// through so [`finish_optimize_run`] can also resolve `original`'s effective RI
    /// via `Design::effective_refractive_index_with` rather than the bare,
    /// custom-catalogue-blind accessor (see [`start_optimize_run`]'s matching
    /// comment on `n_from`).
    custom: Vec<GemMaterial>,
    /// The shared render context, carried through so [`finish_optimize_run`] can show the
    /// finished search's own candidate as a ghost preview -- both are plain `Arc`
    /// clones, `Send`-safe like every other field here.
    render_ctx: Arc<Mutex<RenderContext>>,
    /// See [`Self::render_ctx`].
    preview_state: Arc<SolidPreviewState>,
}

/// [`start_optimize_run`]'s completion handler. Runs on the UI thread (via
/// [`optimize_solve::spawn_optimize_solve`]'s own `upgrade_in_event_loop` hop), but
/// reached through a `Send`-bound closure (see this group's own `mod.rs` doc
/// comment) that cannot capture `Rc<RefCell<EditorState>>` -- so the finished
/// proposal goes into [`RETARGET_ASYNC`], not straight into `EditorState`.
///
/// Discards the result (leaving `RetargetModel`/[`RETARGET_ASYNC`] exactly as
/// whatever superseded this run already left them) when `run_id` no longer matches
/// [`RETARGET_ASYNC`]'s current one -- this run was cancelled or superseded before it
/// finished.
fn finish_optimize_run(
    ui: &MainWindow,
    outcome: OptimizeSolveOutcome,
    run_id: u64,
    started_generation: u64,
    ctx: &OptimizeRunContext,
) {
    let still_current = RETARGET_ASYNC.with(|cell| cell.borrow().run_id == run_id);
    if !still_current {
        return;
    }
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().handle = None);
    // This run is finishing on its
    // own (a cancelled/superseded run's own activity was already finished by
    // `RetargetAsyncRun::cancel_and_supersede`, which also cleared `activity_id`,
    // so `take()` here is a harmless no-op on that path).
    if let (Some(activity), Some(id)) = (
        auto_solve::activity(),
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().activity_id.take()),
    ) {
        activity.finish(id);
    }

    let (rows, notes, solve_error) = match outcome {
        OptimizeSolveOutcome::Completed { outcome }
        | OptimizeSolveOutcome::Cancelled { outcome } => {
            let rows = retarget::rows_from_outcome(
                &ctx.original,
                &ctx.seeded,
                &ctx.scope,
                &ctx.blocks,
                ctx.n_to,
                &outcome.changes,
            );
            let n_from = ctx.original.effective_refractive_index_with(&ctx.custom);
            let notes = retarget::build_notes(
                RetargetMode::Optimize(OptimizeConfig::default()),
                n_from,
                ctx.n_to,
            );
            (rows, notes, String::new())
        }
        OptimizeSolveOutcome::Failed { error } => (Vec::new(), Vec::new(), error.to_string()),
    };

    let row_views: Vec<RetargetRowView> = rows.iter().map(row_view).collect();
    let proposal = (!rows.is_empty()).then(|| RetargetProposal {
        rows,
        target: ctx.target.clone(),
        notes: notes.clone(),
    });
    // Shows the ghost preview for THIS finished search's own
    // candidate (when the toggle is on and a proposal actually built), against the
    // SAME `original` design `apply::apply_pending_retarget` would retarget from --
    // before `proposal` is moved into `RETARGET_ASYNC` below. No live
    // `EditorState` is reachable from this `Send`-bound closure (see this group's
    // own `mod.rs` doc comment) to revert through the ordinary
    // `submit_preview_replan` path if this shows nothing, but there is nothing
    // stale to revert either: `start_optimize_run` already put the real design
    // back the moment this search started.
    let _ = apply_ghost_preview_or_revert(
        ui,
        &ctx.render_ctx,
        &ctx.preview_state,
        &ctx.original,
        proposal.as_ref(),
    );
    let has_proposal = proposal.is_some();
    RETARGET_ASYNC.with(|cell| {
        cell.borrow_mut().pending = proposal.map(|p| (p, started_generation));
    });
    // Stamps this result's own
    // generation so `push_stale_content` (`view.rs`) can badge it the instant a
    // FURTHER edit lands -- see `stale::ResultKind::Retarget`'s own doc comment.
    if has_proposal {
        stale::stamp(ResultKind::Retarget, started_generation);
    } else {
        stale::clear(ResultKind::Retarget);
    }

    let view = RetargetView {
        rows: row_views,
        notes,
        anchored_errors: Vec::new(),
        solve_error,
    };
    push_retarget_view(ui, view);
    ui.global::<RetargetModel>().set_is_busy(false);
}
