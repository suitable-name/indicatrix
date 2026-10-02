//! Optimize's completion handler, its "Cancel"/"Apply" buttons, and the
//! before/after "Preview" toggle for a pending (not yet applied) result.

use super::{RunProvenance, clear_analysis_results};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            optimize_solve::OptimizeSolveOutcome,
            state::EditorState,
            view::{
                SolidLastSolved, build_optimize_preview_design, optimize_change_rows,
                optimize_result_rows, optimize_status_text, refresh_all_now,
                submit_design_ghost_preview, submit_preview_replan,
            },
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_cut_core::{Design, DesignSolveError, OptimizeOutcome};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

/// The completion handler [`super::optimize_run::setup_optimize_callback`] hands to
/// [`crate::gui::editor::optimize_solve::spawn_optimize_solve`], pulled out to keep that callback under
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
pub(super) fn handle_optimize_outcome(
    ui: &MainWindow,
    outcome: OptimizeSolveOutcome,
    provenance: &RunProvenance,
    pending_optimize: &Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    // The design this run started from, for naming the tiers in the per-tier change
    // table. A snapshot, not the live state: this runs from a `Send` closure.
    design: &Design,
) {
    // Always cleared first, on every path below, including the replaced-design
    // early return: `optimize_status` doubles as the live progress readout and
    // `optimize_running` gates the Optimize button, so a handler that returns
    // without touching either leaves the panel stuck mid-run.
    ui.global::<EditorModel>().set_optimize_running(false);
    // New / Load Selected / Open swapped the design out from under this run:
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
/// [`crate::gui::editor::optimize_solve::OptimizeSolveHandle::cancel`] for what this does and doesn't
/// stop. `editor_optimize_running` is deliberately left alone here: the worker's
/// own `on_done`, arriving within about a search evaluation's latency (Optimize's
/// own real mid-search checkpoint, see `optimize_solve`'s module doc comment),
/// still carries the real, honest partial result and is what actually clears it.
///
/// Still gives immediate feedback on the same click -- matching Deep Solve's own
/// cancel button (see [`super::deep_solve_pin::setup_deep_solve_cancel_callback`]) for "cannot tell that
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
/// `EditorState::apply_optimize_outcome` (one [`indicatrix_cut_core::Edit::ModifyTier`] per changed tier,
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
                // Selected`/`Solve` use, a REAL solve (inline only for a cheap
                // design per `view::refresh_all`'s policy, backgrounded otherwise),
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
/// toggle on can never corrupt the next real edit's incremental resolve. The
/// candidate is solved on a background worker, so the toggle never blocks; the
/// ghost appears when that solve lands.
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
                // The ghost could not be queued -- fall through and show the real
                // design instead of leaving whatever the viewport had.
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
