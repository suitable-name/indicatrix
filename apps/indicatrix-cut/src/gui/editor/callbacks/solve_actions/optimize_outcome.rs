//! Optimize's completion handler, its "Cancel"/"Apply" buttons, and the
//! before/after "Preview" toggle for a pending (not yet applied) result.

use super::{RunProvenance, clear_analysis_results};
use crate::{
    EditorModel, MainWindow, OptimizeModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            optimize_panel::{self, FinishedRun},
            optimize_solve::OptimizeRunOutcome,
            state::EditorState,
            view::{
                SolidLastSolved, build_optimize_preview_design, refresh_all_now,
                submit_design_ghost_preview, submit_preview_replan,
            },
        },
        show_toast,
        solid_preview::{cut_slider, preview_state::SolidPreviewState},
        tutorial_events::raise,
    },
};
use indicatrix_cut_core::{Design, DesignSolveError, OptimizeOutcome};
use indicatrix_editor::{
    edit_intent::DRAIN_INTERVAL, guide::solving_events::OPTIMIZE_FINISHED,
    optimize_view::optimize_run_status,
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

/// The completion handler [`super::optimize_run::setup_optimize_callback`] hands to
/// [`crate::gui::editor::optimize_solve::spawn_optimize_run`], pulled out to keep that callback under
/// clippy's function-length lint. `pending_optimize` is stashed here (by
/// `optimize_panel::show_run`) with the best candidate -- `None` whenever nothing an Apply
/// click could safely commit (failed, no candidates found, or already stale).
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
    outcome: OptimizeRunOutcome,
    provenance: &RunProvenance,
    pending_optimize: &Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    // The design this run started from, for naming the tiers in the per-tier change
    // table. A snapshot, not the live state: this runs from a `Send` closure.
    design: &Design,
    // Wall time of the run, for the next run's time estimate.
    elapsed_secs: f32,
) {
    // Always cleared first, on every path below, including the replaced-design
    // early return: `optimize_status` doubles as the live progress readout and
    // `optimize_running` gates the Optimize button, so a handler that returns
    // without touching either leaves the panel stuck mid-run.
    ui.global::<EditorModel>().set_optimize_running(false);
    ui.global::<OptimizeModel>().set_progress(0.0);
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
        OptimizeRunOutcome::Failed { error } => {
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
        OptimizeRunOutcome::Completed { result } | OptimizeRunOutcome::Cancelled { result } => {
            // An EDIT only -- a replacement returned above. The result still
            // describes this design a few edits ago, so it is shown for reference
            // with its own toast, but Apply stays disabled.
            let stale = provenance.is_stale();
            // Candidates only qualify when their full-fidelity score is no worse than the
            // start's, so this is the edge case of a tie.
            let no_net_improvement = result
                .candidates
                .first()
                .is_some_and(|best| best.score >= result.outcome.before_score);
            let summary = optimize_run_status(&result, Some(elapsed_secs));
            let status = if no_net_improvement {
                format!(
                    "No net improvement at full tilt fidelity -- not recommended \
                     ({summary})"
                )
            } else {
                summary
            };
            ui.global::<EditorModel>()
                .set_optimize_status(status.clone().into());
            ui.global::<EditorModel>()
                .set_optimize_status_is_problem(no_net_improvement);
            // An immediate push of the
            // SAME comparison `view::push_stale_content` keeps live on every later
            // edit (`state::result_is_stale` against `pending_optimize`'s own
            // stored generation) -- see `apply_deep_solve_outcome`'s matching
            // comment for why this cannot simply wait for the next edit.
            ui.global::<EditorModel>().set_optimize_stale(stale);
            // The candidate rows, the best candidate picked, its result and change tables
            // (which tiers Optimize wants to move, so a cutter can read the proposal
            // before deciding whether to apply it) and `pending_optimize`.
            let picked = optimize_panel::show_run(
                ui,
                pending_optimize,
                FinishedRun {
                    result,
                    design: design.clone(),
                    generation: provenance.started_generation,
                    stale,
                    elapsed_secs,
                },
            );
            let can_apply = picked.is_some() && !stale;
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
                raise(ui, OPTIMIZE_FINISHED);
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
/// `optimize_panel::apply_pending` (the picked candidate's angles and masts, plus the tiers
/// that follow a relation, as one undo step), the only place this crate turns an Optimize
/// result into a real edit.
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
/// for the same reason -- the apply sets each tier to an absolute angle, so a retry after
/// whatever the error named gets fixed is harmless to attempt again, never a double-delta.
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
        // The picked candidate: angles and masts in one undo step, with the tiers that
        // follow a relation moved along.
        match optimize_panel::apply_pending(&mut st, &outcome) {
            Ok(applied) => {
                // Only consumed now that the apply actually went through.
                *st.pending_optimize
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                ui.global::<EditorModel>().set_optimize_can_apply(false);
                // The candidate list described the design before this apply.
                optimize_panel::clear_results(&ui);
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
                show_toast(&ui, &e, "error");
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
///
/// The checkbox raises this from a Slint `changed` handler, which runs on a later pass of
/// the event loop and so can meet a writer that still holds the editor state. The state is
/// therefore only `try_borrow`ed: a toggle that finds it held is retried a few ticks later
/// ([`cut_slider::retries_after_busy`]), the way the Cut slider's replan is, instead of
/// panicking on a plain `borrow()`.
pub(in crate::gui::editor) fn setup_optimize_preview_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let preview = Rc::new(OptimizePreview {
        ui_weak: ui.as_weak(),
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
        latest_toggle: LatestToggle::default(),
    });
    ui.global::<EditorModel>()
        .on_optimize_preview_toggled(move |enabled: bool| {
            preview.latest_toggle.set(enabled);
            run_optimize_preview(&preview, cut_slider::REPLAN_RETRIES);
        });
}

/// What one Preview toggle needs, shared by the callback and its retries.
struct OptimizePreview {
    ui_weak: slint::Weak<MainWindow>,
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
    /// What the checkbox said at its newest toggle. A retry reads it when it runs, never
    /// the value the click carried when it first met a held editor state: the cutter may
    /// have switched the box the other way since, and replaying the old value would show
    /// the ghost with the box unticked (or put the real design back over a ticked box).
    latest_toggle: LatestToggle,
}

/// The Preview checkbox's newest value, written by every toggle and read by whichever
/// attempt (the click itself or a later retry) is running.
#[derive(Default)]
struct LatestToggle(Cell<bool>);

impl LatestToggle {
    fn set(&self, enabled: bool) {
        self.0.set(enabled);
    }

    const fn get(&self) -> bool {
        self.0.get()
    }
}

/// The pending Optimize result the Preview may show, or `None` for the real design.
///
/// `pending` is the result and the design generation its search ran against
/// (`EditorState::pending_optimize`). Only a switched-on toggle whose result still matches
/// `current_generation` previews it: a result from an older generation holds tier indices
/// that no longer name the same tiers.
#[must_use]
fn preview_candidate<T>(
    enabled: bool,
    pending: Option<(T, u64)>,
    current_generation: u64,
) -> Option<T> {
    pending
        .filter(|_| enabled)
        .and_then(|(outcome, started_generation)| {
            (started_generation == current_generation).then_some(outcome)
        })
}

/// One Preview toggle: shows the pending result's ghost, or puts the real design back, as
/// the checkbox's newest value says ([`OptimizePreview::latest_toggle`]). When a writer
/// holds the editor state it tries again on the next tick, until `retries_left` runs out;
/// the retry re-reads the checkbox's value then.
fn run_optimize_preview(preview: &Rc<OptimizePreview>, retries_left: u8) {
    let Some(ui) = preview.ui_weak.upgrade() else {
        return;
    };
    let Ok(st) = preview.state.try_borrow() else {
        if let Some(next) = cut_slider::retries_after_busy(retries_left) {
            let again = Rc::clone(preview);
            slint::Timer::single_shot(DRAIN_INTERVAL, move || {
                run_optimize_preview(&again, next);
            });
        }
        return;
    };
    let enabled = preview.latest_toggle.get();
    let pending = enabled
        .then(|| {
            st.pending_optimize
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .flatten();
    // `started_generation` is compared and not ignored -- built straight from
    // `st.design` regardless of whether the search that produced `outcome` ran against
    // an OLDER generation. Optimize completes, the user removes a tier (bumping
    // `st.generation`), then toggles Preview on: the stale `AngleChange` indices from
    // the search were applied to the shifted tier list and the wrong tiers moved, while
    // Apply was (correctly) already greyed out by the same staleness. Mirrors
    // `view::apply_pending_optimize_ghost`'s own generation check (private to that
    // file, so re-checked here rather than shared).
    let current_generation = st.generation.load(AtomicOrdering::Relaxed);
    if let Some(outcome) = preview_candidate(enabled, pending, current_generation) {
        let candidate = build_optimize_preview_design(&st.design, &outcome);
        if submit_design_ghost_preview(&ui, &preview.render_ctx, &preview.preview_state, &candidate)
        {
            return;
        }
        // The ghost could not be queued -- fall through and show the real design
        // instead of leaving whatever the viewport had.
    }
    submit_preview_replan(
        &ui,
        &preview.render_ctx,
        &preview.preview_state,
        &preview.solid_last_solved,
        &st,
        BTreeSet::new(),
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_switched_on_preview_of_a_current_result_shows_the_ghost() {
        // On, and the search ran against the design as it is now.
        assert_eq!(
            preview_candidate(true, Some(("result", 7)), 7),
            Some("result")
        );
        // Off: the real design comes back whatever is pending.
        assert_eq!(preview_candidate(false, Some(("result", 7)), 7), None);
        // Nothing pending.
        assert_eq!(preview_candidate::<&str>(true, None, 7), None);
        // The design changed after the search (a tier removed, an angle nudged): its
        // tier indices no longer name the same tiers.
        assert_eq!(preview_candidate(true, Some(("result", 6)), 7), None);
        assert_eq!(preview_candidate(true, Some(("result", 8)), 7), None);
    }

    /// A toggle that met a held editor state is retried a tick later. The retry shows what
    /// the checkbox says THEN: ticked, unticked and ticked again before the retry runs
    /// must never replay the first click's value.
    #[test]
    fn a_retry_follows_the_checkbox_not_the_click_it_was_scheduled_for() {
        let toggle = LatestToggle::default();
        let pending = || Some(("ghost", 7));
        let attempt = |toggle: &LatestToggle| preview_candidate(toggle.get(), pending(), 7);

        toggle.set(true);
        assert_eq!(attempt(&toggle), Some("ghost"), "the click itself");
        // The click met a held state and scheduled a retry; the cutter unticks the box
        // before the retry runs. The retry puts the real design back.
        toggle.set(false);
        assert_eq!(
            attempt(&toggle),
            None,
            "the retry after the box was unticked"
        );
        toggle.set(true);
        assert_eq!(attempt(&toggle), Some("ghost"), "ticked again");
    }
}
