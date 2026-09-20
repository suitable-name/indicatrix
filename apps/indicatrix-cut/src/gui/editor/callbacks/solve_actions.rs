//! The tier list's "Adopt" action, and the Deep Solve/Optimize start/cancel/apply
//! callbacks -- one `setup_*` function per Slint callback. See this group's `mod.rs`
//! doc comment for the "`History` is the only thing that mutates `Design`" rule every
//! callback here upholds via `EditorState::apply`.

use super::super::{
    deep_solve,
    optimize_solve::{self, OptimizeSolveOutcome},
    state::EditorState,
    view::{
        SolidLastSolved, deep_solve_tier_rows, format_deep_solve_report, optimize_change_rows,
        optimize_result_rows, optimize_status_text, parse_optimize_weights, refresh_all,
    },
};
use crate::{
    EditorModel, MainWindow, OptimizeResultRow,
    bridge::render_thread::RenderContext,
    gui::{show_toast, solid_preview::preview_state::SolidPreviewState},
};
use indicatrix_cut_core::{
    Edit, OptimizeConfig, OptimizeOutcome, free_tier_indices, optimize::SearchStage,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

/// The tier list's "Adopt" action: switches a pinned tier over to the meet
/// instruction the source file actually stated for it
/// (`ConstraintTier::imported_meet`), through [`Edit::SetConstraint`] like any other
/// edit. A silent no-op if the tier has nothing to adopt or the index is stale:
/// `EditorView` only shows the button when `imported_meet_text` is non-empty, so this
/// only guards a race with a concurrent edit.
///
/// Calls [`refresh_all`] (a real re-solve), NOT `refresh_editor_panel_stale` like
/// every other tier edit -- deliberately the one exception to "Solve on explicit
/// action, not on every edit". Adopting a suggested meet isn't really a new edit
/// whose result is unknown: the value being adopted is exactly what the tier already
/// showed after the design's last real solve, so re-solving here pays the SAME
/// whole-schedule cost `refresh_all` always pays, not a new one. Calling
/// `refresh_editor_panel_stale` here instead was the actual bug behind "Solve, Adopt,
/// and it says not solved again": the mutated design had already solved (Adopt's
/// `SetConstraint` alone can't remove a block's last scale reference), but the panel
/// kept showing the fixed "Not solved" banner since that function never asks `Design`
/// its current status.
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
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if index < 0 {
            return;
        }
        let mut st = state.borrow_mut();
        let Some(constraint) = st
            .design
            .tiers
            .get(index as usize)
            .and_then(|t| t.imported_meet.clone())
        else {
            return;
        };
        match st.apply(Edit::SetConstraint {
            index: index as usize,
            constraint,
        }) {
            Ok(()) => refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st),
            Err(e) => show_toast(&ui, &e.to_string(), "error"),
        }
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
pub(in crate::gui::editor) fn clear_analysis_results(ui: &MainWindow) {
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
}

/// "Deep Solve": the explicit, off-thread, cancellable verified-solve action -- see
/// `deep_solve`'s module doc comment for why it needs its own thread and why it's a
/// read-only diagnostic that never mutates `design`. Guarded by the
/// `editor_deep_solve_running` property rather than `EditorState::deep_solve` being
/// `Some`, since the completion handler can't clear that field itself.
///
/// `run_epoch` is a counter local to this callback (captured once, shared by every
/// invocation and by each run's own completion handler) that tells a superseded
/// run's eventual, long-delayed completion from the current one: Deep Solve's own
/// cancellation cannot actually stop the in-flight call (see `deep_solve`'s module
/// doc comment), so cancelling and immediately starting a fresh run leaves the OLD
/// worker thread running to completion in the background, and its `on_done` would
/// otherwise arrive minutes later and clobber the NEW run's live status with a stale
/// one. Bumped only when a new run actually starts -- cancel itself needs no such
/// guard, see [`setup_deep_solve_cancel_callback`].
pub(in crate::gui::editor) fn setup_deep_solve_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    let run_epoch = Arc::new(AtomicU64::new(0));
    ui.global::<EditorModel>().on_deep_solve(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // CAD audit item 216: also guards against Solve or Optimize already
        // running, not only a re-entrant Deep Solve click -- see
        // `tier_actions::setup_solve_callback`'s own doc comment for why all
        // three sites read the three `*_running` flags directly rather than
        // the derived `EditorModel.busy_action`.
        let model = ui.global::<EditorModel>();
        if model.get_deep_solve_running()
            || model.get_solve_running()
            || model.get_optimize_running()
        {
            return;
        }
        let mut st = state.borrow_mut();
        let Some(targets) = st.printed_proportions else {
            show_toast(
                &ui,
                "Deep Solve needs this design's printed proportions; none are available.",
                "error",
            );
            return;
        };
        let tiers = st.design.meet_tier_inputs();
        let gear_teeth_abs = st.design.meta.gear_teeth_abs();
        // The design's own plain solve, captured now (same design this run's `tiers`
        // snapshot describes) so the completion handler can show which tiers Deep
        // Solve's verified repair actually disagreed with -- see
        // `deep_solve::tier_mast_deltas`. `None` when the design does not even
        // plain-solve (e.g. a missing scale-reference anchor); the per-tier
        // breakdown is simply omitted then, same as it would be with nothing to
        // compare against.
        let current_masts = st.design.solve().ok();
        let design_snapshot = st.design.clone();
        // CAD audit item 162: whether at least one edit has landed since this
        // design was loaded/created/replaced -- `targets` below was captured once
        // at catalogue-load time and never re-derived from edits since, so a
        // verdict against it is honestly a verdict against the design AS PRINTED,
        // not necessarily the one on screen right now. See
        // `deep_solve::edited_since_load_caveat`'s own doc comment for why this
        // reads `History::can_undo` rather than `EditorState::is_dirty`.
        let edited_since_load = st.history.can_undo();
        let provenance = RunProvenance::capture(&st);
        let this_run = run_epoch.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        let run_epoch_done = Arc::clone(&run_epoch);

        ui.global::<EditorModel>().set_deep_solve_running(true);
        ui.global::<EditorModel>()
            .set_deep_solve_status("Deep solving... 0.0s elapsed".into());
        ui.global::<EditorModel>()
            .set_deep_solve_status_is_problem(false);

        let handle = deep_solve::spawn_deep_solve(
            ui_weak.clone(),
            gear_teeth_abs,
            tiers,
            targets,
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
                // The design was REPLACED (New / Load Selected / Open Native), not
                // merely edited: this verdict describes a different stone, and
                // `clear_analysis_results` has already blanked the panel on purpose.
                // Repainting a report over it -- even a caveated one -- would read
                // as a verdict on the design now on screen. Cancellation here is
                // UI-level abandonment only (see `deep_solve`'s module doc comment),
                // so the abandoned thread really does still arrive and really does
                // still reach this closure.
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
                    deep_solve::DeepSolveOutcome::Completed { solved, report } => {
                        // Reachable only for an EDIT, never a replacement (that
                        // returned above) -- so the caveat this adds is the honest
                        // one: same design, a few edits on.
                        let mut status = format_deep_solve_report(&report, provenance.is_stale());
                        // CAD audit item 162: names WHICH printed figures this
                        // verdict verified against, and warns when the design has
                        // its own edits since load that those figures do not
                        // reflect -- see `deep_solve::format_verification_targets`/
                        // `edited_since_load_caveat`'s own doc comments.
                        status.push_str(&deep_solve::format_verification_targets(&targets));
                        status.push_str(deep_solve::edited_since_load_caveat(edited_since_load));
                        // The per-tier breakdown behind the aggregate verdict above --
                        // see `deep_solve::tier_mast_deltas`'s own doc comment for why
                        // this is the minimal consumer for that data (a dedicated
                        // table belongs to another lane).
                        if let Some(current) = &current_masts {
                            let deltas = deep_solve::tier_mast_deltas(current, &solved);
                            status.push_str(&deep_solve::format_tier_mast_deltas(&deltas));
                            // The same disagreement as a real per-tier table, for the
                            // Log popup -- the status line only has room for a summary.
                            ui.global::<EditorModel>()
                                .set_deep_solve_tier_rows(ModelRc::new(VecModel::from(
                                    deep_solve_tier_rows(&deltas, &design_snapshot),
                                )));
                        }
                        ui.global::<EditorModel>()
                            .set_deep_solve_status(status.into());
                        ui.global::<EditorModel>()
                            .set_deep_solve_status_is_problem(!report.accepted);
                    }
                }
            },
        );
        st.deep_solve = Some(handle);
    });
}

/// "Cancel" (shown only while a Deep Solve is running): see
/// [`deep_solve::DeepSolveHandle::cancel`] for exactly what the low-level
/// cancellation call does and doesn't stop.
///
/// Unlike that call alone, THIS is what actually gives the user their editor back:
/// it flips `editor_deep_solve_running`/`deep_solve_status` immediately, on the same
/// click, rather than waiting for the abandoned worker thread to eventually finish
/// and report back (potentially minutes later, per the `deep_solve` module doc
/// comment) -- previously the only place either was ever reset, which is why Cancel
/// used to leave the strip frozen on "Deep solving... X.Xs elapsed" with no visible
/// effect and no way to start a new Deep Solve until the abandoned one finally
/// returned.
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
        ui.global::<EditorModel>().set_deep_solve_running(false);
        ui.global::<EditorModel>()
            .set_deep_solve_status("Deep Solve cancelled.".into());
        ui.global::<EditorModel>()
            .set_deep_solve_status_is_problem(false);
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
        move |windowing: SharedString, extinction: SharedString, tilt_brilliance: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // CAD audit item 216: also guards against Solve or Deep Solve
            // already running -- see `setup_deep_solve_callback`'s own doc
            // comment above for why.
            let model = ui.global::<EditorModel>();
            if model.get_optimize_running()
                || model.get_solve_running()
                || model.get_deep_solve_running()
            {
                return;
            }
            let weights = match parse_optimize_weights(&windowing, &extinction, &tilt_brilliance) {
                Ok(weights) => weights,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let mut st = state.borrow_mut();
            if free_tier_indices(&st.design).is_empty() {
                // `EditorView` only shows this button enabled when `optimize_available`
                // is true -- this only guards a race with a concurrent edit disabling
                // it out from under a stale click, not the common path.
                show_toast(
                    &ui,
                    "Optimize has nothing free to move on this design right now.",
                    "error",
                );
                return;
            }
            let design = st.design.clone();
            // A second snapshot for the completion closure: `design` above is moved
            // into the worker, and the per-tier change table needs the tiers' names
            // to label its rows.
            let design_snapshot = st.design.clone();
            let mut material_selection = st.design.material.clone();
            let defaulted_ri = default_optimize_material_ri(&st.design, &mut material_selection);
            // `OptimizeJob::custom_materials` (see `optimize_solve::spawn_optimize_solve`)
            // is a plain `Vec` -- a one-shot worker's own owned copy, not
            // `RenderContext`'s hot-path per-frame snapshot -- so this is the one actual
            // deep copy on this path, same as before `RenderContext::custom_materials`
            // became `Arc`-backed.
            let custom_materials = render_ctx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .custom_materials
                .as_ref()
                .clone();
            let config = OptimizeConfig {
                weights,
                ..OptimizeConfig::default()
            };
            let provenance = RunProvenance::capture(&st);
            let pending_optimize = Arc::clone(&st.pending_optimize);
            let this_run = run_epoch.fetch_add(1, AtomicOrdering::Relaxed) + 1;
            let run_epoch_done = Arc::clone(&run_epoch);

            ui.global::<EditorModel>().set_optimize_running(true);
            ui.global::<EditorModel>()
                .set_optimize_status(optimize_start_status(defaulted_ri).into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(false);
            ui.global::<EditorModel>().set_optimize_can_apply(false);
            ui.global::<EditorModel>()
                .set_optimize_result_rows(ModelRc::new(VecModel::from(
                    Vec::<OptimizeResultRow>::new(),
                )));
            ui.global::<EditorModel>()
                .set_optimize_change_rows(ModelRc::new(VecModel::from(Vec::<
                    crate::OptimizeChangeRow,
                >::new())));

            let handle = optimize_solve::spawn_optimize_solve(
                ui_weak.clone(),
                design,
                material_selection,
                custom_materials,
                config,
                |ui: &MainWindow, progress: optimize_solve::OptimizeSolveProgress| {
                    ui.global::<EditorModel>()
                        .set_optimize_status(optimize_progress_status(&progress).into());
                },
                move |ui: &MainWindow, outcome: OptimizeSolveOutcome| {
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
        },
    );
}

/// The material-RI-defaulting half of [`setup_optimize_callback`]'s doc comment,
/// pulled out to keep that callback under clippy's function-length lint. Mutates
/// `selection` in place (its own field, no cross-tier state) and returns the
/// defaulted RI only when a default was actually applied, so the caller can fold
/// it into the initial status line via [`optimize_start_status`].
fn default_optimize_material_ri(
    design: &indicatrix_cut_core::Design,
    selection: &mut indicatrix_cut_core::MaterialSelection,
) -> Option<f64> {
    if selection.name.is_some() || selection.refractive_index_override.is_some() {
        return None;
    }
    let n_d = design.effective_refractive_index();
    selection.refractive_index_override = Some(n_d);
    Some(n_d)
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
/// [`optimize_solve::OptimizeSolveProgress::stage`] explicitly (CAD audit item
/// 161) instead of always showing an evaluation fraction: the two full-fidelity
/// scorings that bracket every run report zero-progress ticks of their own that
/// used to leave the counter frozen for over a second each, reading as a hang
/// rather than real (if invisible-to-the-counter) work. The coordinate and polish
/// stages instead show `progress.max_evaluations` (CAD audit item 153's inclusive
/// figure, coordinate cap plus the polish stage's own) so the polish stage's
/// evaluations climbing no longer reads as sailing past the run's own stated
/// budget.
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
/// summary that used to read as unconditional success even when the changes the
/// rows *did* show made the full-fidelity result worse. Apply stays available: a
/// worse full-fidelity score is a warning, not a proof the user cannot possibly
/// want it (see the "enables Apply for a result whose own full-fidelity score got
/// worse" finding).
///
/// A non-stale `Completed`/`Cancelled`/`Failed` outcome also toasts (CAD audit
/// item 155: completion used to be invisible unless the Optimize tab already
/// happened to be open) and, when a real result is waiting for Apply, switches
/// the inspector to the Optimize tab and uncollapses it -- a stale result gets
/// its own dedicated toast instead (see the `stale` branch below), never both.
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
            // Names the block(s) and the exact remedy via `MissingAnchor`'s own
            // `Display` impl -- the same text the plain Solve banner already shows
            // for the identical failure (CAD audit item 156), rather than a bare
            // block count that sends the cutter hunting for which block it was.
            ui.global::<EditorModel>().set_optimize_status(
                format!(
                    "Optimize failed: {error} -- add an Exact scale value tier there \
                     and Solve first."
                )
                .into(),
            );
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(true);
            ui.global::<EditorModel>().set_optimize_can_apply(false);
            show_toast(ui, &format!("Optimize failed: {error}"), "error");
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
                // CAD audit item 155: Optimize used to finish silently unless the
                // Optimize tab already happened to be open -- a toast plus bringing
                // that tab into view (only when there is something to act on) is
                // what actually gets a cutter's attention who switched to the Tier
                // tab or collapsed the inspector while the run was going.
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
/// stop -- unlike Deep Solve's UI-level abandonment, this is a REAL mid-search
/// checkpoint, so `editor_optimize_running` is deliberately left alone here (the
/// worker's own `on_done`, arriving within about a search evaluation's latency,
/// still carries the real, honest partial result and is what actually clears it).
///
/// Still gives immediate feedback on the same click -- matching Deep Solve's own
/// cancel button (see [`setup_deep_solve_cancel_callback`]) for "cannot tell that
/// Cancel did anything" -- without the risk flipping `optimize_running` off early
/// would create: the abandoned worker is still finishing, and a click on "Optimize"
/// in that window would start a second search racing the first one.
///
/// CAD audit item 154: a second click while a cancel is already in flight used to
/// be harmless but pointless -- `handle.cancel()` just stores `true` again, and
/// this only re-sets the same status text -- with nothing stopping the click from
/// reaching this callback at all. Rather than adding a new `EditorModel` property
/// (this callback has no way to know the click even happened without one, and this
/// group does not own `ui/models/editor.slint`), the button itself now latches
/// locally in `ui/components/editor_command_bar.slint`'s `AnalysisGroup` --
/// `optimize_cancel_requested`, reset the moment `EditorModel.optimize_running`
/// goes false again -- and disables/relabels itself entirely in Slint. No Rust
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
            // reassuring guess -- CAD audit item 154's wording fix.
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
/// actually consumes it once the apply has gone through: a refused (stale
/// generation) click used to `take()` the result unconditionally and then check
/// staleness afterward, so a cutter who clicked Apply one edit too late lost the
/// held Optimize result permanently -- `optimize_result_rows`/`optimize_status`
/// stayed on screen (nothing here ever clears those two) while the one thing that
/// could still commit them, `pending_optimize`, was already gone, and the only
/// recovery was a full re-run. Left in place on a failed apply
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
            show_toast(
                &ui,
                "The design changed since this Optimize result was computed; re-run \
                 Optimize.",
                "error",
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
                // CAD audit item 160: a plain `refresh_editor_panel_stale` +
                // `submit_preview_replan` left the panel reading "Not solved" with
                // "-" masts right after a successful Apply, as if the commit were
                // still pending -- `refresh_all` is the same call `New`/`Load
                // Selected`/`Solve` use, a REAL solve (synchronous under
                // `auto_solve::should_solve_synchronously`, backgrounded above it),
                // so the table and viewport read solved immediately instead of
                // waiting on the next unrelated edit's auto-solve to catch up.
                refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
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
