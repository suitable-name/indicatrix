//! The tier list's "Adopt" action, and the Deep Solve/Optimize start/cancel/apply
//! callbacks -- one `setup_*` function per Slint callback. See this group's `mod.rs`
//! doc comment for the "`History` is the only thing that mutates `Design`" rule every
//! callback here upholds via `EditorState::apply`.

use super::super::{
    deep_solve,
    optimize_solve::{self, OptimizeSolveOutcome},
    state::EditorState,
    view::{
        SolidLastSolved, format_deep_solve_report, optimize_result_rows, optimize_status_text,
        parse_optimize_weights, refresh_all, refresh_editor_panel_stale, submit_preview_replan,
    },
};
use crate::{
    EditorModel, MainWindow, OptimizeResultRow,
    bridge::render_thread::RenderContext,
    gui::{show_toast, solid_preview::preview_state::SolidPreviewState},
};
use indicatrix_cut_core::{Edit, OptimizeConfig, OptimizeOutcome, free_tier_indices};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
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
/// Calls [`refresh_all`] (a real re-solve), NOT [`refresh_editor_panel_stale`] like
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

/// "Deep Solve": the explicit, off-thread, cancellable verified-solve action -- see
/// `deep_solve`'s module doc comment for why it needs its own thread and why it's a
/// read-only diagnostic that never mutates `design`. Guarded by the
/// `editor_deep_solve_running` property rather than `EditorState::deep_solve` being
/// `Some`, since the completion handler can't clear that field itself.
pub(in crate::gui::editor) fn setup_deep_solve_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_deep_solve(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if ui.global::<EditorModel>().get_deep_solve_running() {
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
        let started_generation = st.generation.load(AtomicOrdering::Relaxed);
        let generation = Arc::clone(&st.generation);

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
                ui.global::<EditorModel>().set_deep_solve_running(false);
                match outcome {
                    deep_solve::DeepSolveOutcome::Cancelled => {
                        ui.global::<EditorModel>()
                            .set_deep_solve_status("Deep Solve cancelled.".into());
                        ui.global::<EditorModel>()
                            .set_deep_solve_status_is_problem(false);
                    }
                    deep_solve::DeepSolveOutcome::Completed { report } => {
                        let stale = generation.load(AtomicOrdering::Relaxed) != started_generation;
                        ui.global::<EditorModel>()
                            .set_deep_solve_status(format_deep_solve_report(&report, stale).into());
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
/// [`deep_solve::DeepSolveHandle::cancel`] for exactly what this does and doesn't stop.
pub(in crate::gui::editor) fn setup_deep_solve_cancel_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    ui.global::<EditorModel>().on_deep_solve_cancel(move || {
        if let Some(handle) = state.borrow().deep_solve.as_ref() {
            handle.cancel();
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
pub(in crate::gui::editor) fn setup_optimize_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_optimize(
        move |windowing: SharedString, extinction: SharedString, tilt_brilliance: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if ui.global::<EditorModel>().get_optimize_running() {
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
            let material_selection = st.design.material.clone();
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
            let started_generation = st.generation.load(AtomicOrdering::Relaxed);
            let generation = Arc::clone(&st.generation);
            let pending_optimize = Arc::clone(&st.pending_optimize);

            ui.global::<EditorModel>().set_optimize_running(true);
            ui.global::<EditorModel>()
                .set_optimize_status("Optimizing... 0 evaluations, 0.0s elapsed".into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(false);
            ui.global::<EditorModel>().set_optimize_can_apply(false);
            ui.global::<EditorModel>()
                .set_optimize_result_rows(ModelRc::new(VecModel::from(
                    Vec::<OptimizeResultRow>::new(),
                )));

            let handle = optimize_solve::spawn_optimize_solve(
                ui_weak.clone(),
                design,
                material_selection,
                custom_materials,
                config,
                |ui: &MainWindow, progress: optimize_solve::OptimizeSolveProgress| {
                    ui.global::<EditorModel>().set_optimize_status(
                        format!(
                            "Optimizing... {} of {} evaluations, {:.1}s elapsed",
                            progress.evaluations,
                            progress.max_evaluations,
                            progress.elapsed.as_secs_f32()
                        )
                        .into(),
                    );
                },
                move |ui: &MainWindow, outcome: OptimizeSolveOutcome| {
                    handle_optimize_outcome(
                        ui,
                        outcome,
                        &generation,
                        started_generation,
                        &pending_optimize,
                    );
                },
            );
            st.optimize = Some(handle);
        },
    );
}

/// The completion handler [`setup_optimize_callback`] hands to
/// [`optimize_solve::spawn_optimize_solve`], pulled out to keep that callback under
/// clippy's function-length lint. `pending_optimize` is stashed here with the real,
/// honest result -- `None` whenever nothing an Apply click could safely commit
/// (failed, no changes found, or already stale).
fn handle_optimize_outcome(
    ui: &MainWindow,
    outcome: OptimizeSolveOutcome,
    generation: &Arc<AtomicU64>,
    started_generation: u64,
    pending_optimize: &Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
) {
    ui.global::<EditorModel>().set_optimize_running(false);
    match outcome {
        OptimizeSolveOutcome::Failed { error } => {
            ui.global::<EditorModel>().set_optimize_status(
                format!(
                    "Optimize failed: this design has no scale-reference anchor for \
                     {} block(s), so it cannot even solve -- there is nothing to \
                     optimize until that is fixed.",
                    error.blocks.len()
                )
                .into(),
            );
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(true);
            ui.global::<EditorModel>().set_optimize_can_apply(false);
        }
        OptimizeSolveOutcome::Completed { outcome }
        | OptimizeSolveOutcome::Cancelled { outcome } => {
            let stale = generation.load(AtomicOrdering::Relaxed) != started_generation;
            ui.global::<EditorModel>()
                .set_optimize_status(optimize_status_text(&outcome).into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(false);
            ui.global::<EditorModel>()
                .set_optimize_result_rows(ModelRc::new(VecModel::from(optimize_result_rows(
                    &outcome,
                ))));
            let can_apply = !outcome.changes.is_empty() && !stale;
            ui.global::<EditorModel>().set_optimize_can_apply(can_apply);
            *pending_optimize
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                can_apply.then_some((outcome, started_generation));
            if stale {
                show_toast(
                    ui,
                    "The design changed while Optimize was running -- its result is \
                     shown for reference but can no longer be applied. Re-run \
                     Optimize on the current design.",
                    "info",
                );
            }
        }
    }
}

/// "Cancel" (shown only while an Optimize run is in progress): see
/// [`optimize_solve::OptimizeSolveHandle::cancel`] for what this does and doesn't
/// stop -- unlike Deep Solve's UI-level abandonment, this is a REAL mid-search
/// checkpoint.
pub(in crate::gui::editor) fn setup_optimize_cancel_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    ui.global::<EditorModel>().on_optimize_cancel(move || {
        if let Some(handle) = state.borrow().optimize.as_ref() {
            handle.cancel();
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
        let pending = st
            .pending_optimize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some((outcome, started_generation)) = pending else {
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
                ui.global::<EditorModel>().set_optimize_can_apply(false);
                refresh_editor_panel_stale(&ui, &render_ctx, &st);
                // Optimize can move several tiers at once -- force a full
                // (non-blocking) solve rather than guessing which ones.
                submit_preview_replan(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    true,
                );
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
