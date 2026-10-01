//! The tier list's "Adopt" action, and the Deep Solve/Optimize/batch-tilt
//! start/cancel/apply callbacks -- one `setup_*` function per Slint callback. See
//! this group's own `mod.rs` (one level up, in `callbacks`) doc comment for the
//! "`History` is the only thing that mutates `Design`" rule every callback here
//! upholds via `EditorState::apply`.
//!
//! Split by concern, along the seams the original single file already drew in its
//! own section comments: [`adopt`] (the tier list's "Adopt" action),
//! [`deep_solve_run`]/[`deep_solve_pin`] (Deep Solve's dispatch+outcome and its
//! per-tier "Pin to verified mast" action), [`optimize_run`]/[`optimize_outcome`]
//! (Optimize's dispatch and its outcome+cancel+apply+preview), and [`batch_tilt`]
//! ("Compute Tilt Curves" for the open design). [`RunProvenance`] and the three
//! `thread_local!`s below are shared infrastructure every run-dispatching sibling
//! needs -- private items defined here are visible to every descendant module of
//! this one, so no further visibility is needed for that sharing.

mod adopt;
mod batch_tilt;
mod deep_solve_pin;
mod deep_solve_run;
mod optimize_outcome;
mod optimize_run;

pub(in crate::gui::editor) use adopt::setup_adopt_meet_callback;
pub(in crate::gui::editor) use batch_tilt::setup_batch_tilt_for_open_design_callback;
pub(in crate::gui::editor) use deep_solve_pin::{
    setup_deep_solve_cancel_callback, setup_deep_solve_pin_callback,
};
pub(in crate::gui::editor) use deep_solve_run::setup_deep_solve_callback;
pub(in crate::gui::editor) use optimize_outcome::{
    setup_optimize_apply_callback, setup_optimize_cancel_callback, setup_optimize_preview_callback,
};
pub(in crate::gui::editor) use optimize_run::setup_optimize_callback;

use super::super::{auto_solve, deep_solve, state::EditorState};
use crate::{EditorModel, MainWindow, OptimizeResultRow};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

thread_local! {
    /// The most recently completed Deep Solve's per-tier mast disagreement, read by
    /// the "Pin to verified mast" action ([`deep_solve_pin::setup_deep_solve_pin_callback`]).
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
    /// [`deep_solve_run::setup_deep_solve_callback`] (which registers it) and
    /// [`deep_solve_pin::setup_deep_solve_cancel_callback`] (which must finish that SAME id
    /// immediately on cancel, rather than waiting even for the checkpoint-based
    /// cancellation's own fast `on_done` to arrive -- see `deep_solve`'s module doc
    /// comment). A module-local `thread_local!` for the
    /// identical single-threaded-event-loop reason [`LAST_DEEP_SOLVE_DELTAS`]
    /// documents just above.
    static DEEP_SOLVE_ACTIVITY_ID: RefCell<Option<u64>> = const { RefCell::new(None) };

    /// [`DEEP_SOLVE_ACTIVITY_ID`]'s Deep-Solve-shaped counterpart for
    /// [`optimize_run::setup_optimize_callback`]/[`optimize_outcome::setup_optimize_cancel_callback`].
    static OPTIMIZE_ACTIVITY_ID: RefCell<Option<u64>> = const { RefCell::new(None) };
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
///   live [`EditorState`] wholesale
///   (`tier_actions::do_new_design_create`, `tier_actions::apply_loaded_design`, and
///   `native_io::finish_state_replace` for both native paths), and
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
/// Optimize `ActivityRegistry` entry
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
    // a Deep Solve/Optimize run abandoned by a New/Load Selected/Open
    // Native replacement is cancelled by `EditorState::replace_wholesale`
    // (real, checkpoint-based cancellation -- see that method's own doc
    // comment) right before every one of this function's own call sites, but
    // that method has no `MainWindow` handle to reset the busy flags with.
    // Without this, either flag stayed stuck at `true` -- and its own Deep
    // Solve/Optimize button stayed disabled -- until the abandoned run's own
    // completion handler eventually reset it, which could be minutes away.
    ui.global::<EditorModel>().set_deep_solve_running(false);
    ui.global::<EditorModel>().set_optimize_running(false);
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
