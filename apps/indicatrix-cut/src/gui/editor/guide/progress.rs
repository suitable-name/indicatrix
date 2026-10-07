//! Automatic advance: evaluating the CURRENT guide step's goal against the design and the
//! UI, and reporting it through `GuideModel.notify`.
//!
//! [`check_progress`] is the one entry point, called wherever the design or its solve
//! state can have changed: the end of `view::refresh_all` (New, Load, Open, an explicit
//! Solve), `view::refresh_editor_panel_stale` (every ordinary edit -- the tier form, quick
//! add, inline edits, undo/redo, material and yield applies), the tier-save/material/yield
//! success callbacks themselves, `GuideModel.step_entered` and `GuideModel.poll`. The
//! background auto-solve's completion (`auto_solve::apply`) and a matching solid-preview
//! frame (`view::push_solved_preview`) hold only a design snapshot, not the `EditorState`,
//! so they call [`check_design_progress`] -- the same evaluation, one layer down. Every
//! caller runs it AFTER `EditorModel.solve_state`/`status_is_problem` have been updated for
//! the result it is applying, since the solved goal reads them.
//!
//! A UI EVENT (a guide step that waits for `palette_opened`, say) arrives through
//! [`guide_event`] instead: it is remembered for the rest of the step and the step is
//! re-checked at once.
//!
//! Goals are judged from STATE, never from which button was clicked, so every route to a
//! goal counts (quick add, inline edit, undo/redo, auto-solve), and a failed validation --
//! which never changes the design -- can never advance.

use super::{
    launch,
    runtime::{self, UiFacts},
};
use crate::{
    EditorModel, GuideModel, MainWindow, SolidPreviewModel, gui::editor::state::EditorState,
};
use indicatrix_cut_core::Design;
use indicatrix_editor::guide::NEW_DESIGN_CREATED;
use slint::ComponentHandle;
use std::time::Duration;

/// Reports UI event `name` to the running guide (see `indicatrix_editor::guide::EVENTS`
/// for the names a step may wait for): the event counts for the rest of the current step,
/// and the step is re-checked at once.
///
/// Safe to call from anywhere on the UI thread, including while the editor state is
/// borrowed: the re-check then waits a moment and tries once more, and every later refresh
/// re-checks anyway.
///
/// This is the guide's own handler, the one `GuideModel.event` runs. Code outside the guide
/// reports an event with `gui::tutorial_events::raise`, which reaches it through that
/// callback while a tutorial is open; only an event that must also reach a launch still
/// waiting for its design, where no tutorial is open yet, calls it directly (see [`notify`]).
pub(super) fn guide_event(ui: &MainWindow, name: &str) {
    runtime::record_event(name);
    if name == NEW_DESIGN_CREATED {
        // The design a guide may have asked for is here: open it before the re-check.
        runtime::mark_new_design_arrived();
        launch::evaluate_launch(ui);
    }
    check_now(ui);
}

/// [`guide_event`] for the event the New Design action reports (the only one a caller used
/// to report by name). The key is an event name, not a step's completion key.
pub(in crate::gui::editor) fn notify(ui: &MainWindow, key: &str) {
    guide_event(ui, key);
}

/// Evaluates the current guide step's goal against `state` and reports it to
/// `GuideModel.notify` when reached. A no-op while the guide is closed, on a reading step,
/// or while the current step is already showing "Done".
pub(in crate::gui::editor) fn check_progress(ui: &MainWindow, state: &EditorState) {
    check_design_progress(ui, &state.design);
}

/// [`check_progress`] for a caller holding only a `Design` (the background auto-solve's
/// completion, whose snapshot is the design it just solved).
pub(in crate::gui::editor) fn check_design_progress(ui: &MainWindow, design: &Design) {
    let guide = ui.global::<GuideModel>();
    if !guide.get_open() || guide.get_step_done() {
        return;
    }
    let Ok(index) = usize::try_from(guide.get_step_index()) else {
        return;
    };
    let model = ui.global::<EditorModel>();
    let facts = UiFacts {
        solved_closed: model.get_solve_state() == "solved" && !model.get_status_is_problem(),
        view_mode: ui.global::<SolidPreviewModel>().get_view_mode(),
        inspector_tab: model.get_inspector_tab(),
    };
    if runtime::goal_is_met(guide.get_guide_id().as_str(), index, design, facts) {
        guide.invoke_notify(guide.get_current().completion);
    }
}

/// Re-checks the current step against the editor state as it is now.
pub(super) fn check_now(ui: &MainWindow) {
    if !try_check(ui) {
        // Someone is writing the state right now; their refresh ends in a check of its own,
        // and one more try a moment later covers a caller that does not refresh.
        let ui_weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(50), move || {
            if let Some(ui) = ui_weak.upgrade() {
                try_check(&ui);
            }
        });
    }
}

/// Runs the check unless the state is being written. Returns whether it ran.
fn try_check(ui: &MainWindow) -> bool {
    let Some(state) = runtime::state() else {
        return true;
    };
    let Ok(st) = state.try_borrow() else {
        return false;
    };
    check_progress(ui, &st);
    true
}
