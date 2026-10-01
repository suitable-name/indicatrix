//! Automatic advance: evaluating the CURRENT guide step's goal against the design
//! and reporting it through `GuideModel.notify`.
//!
//! [`check_progress`] is the one entry point, called wherever the design or its
//! solve state can have changed: the end of `view::refresh_all` (New, Load, Open,
//! an explicit Solve), `view::refresh_editor_panel_stale` (every ordinary edit --
//! the tier form, quick add, inline edits, undo/redo, material and yield applies),
//! the tier-save/material/yield success callbacks themselves, and
//! `GuideModel.step_entered`. The background auto-solve's completion
//! (`auto_solve::apply`) and a matching solid-preview frame
//! (`view::push_solved_preview`) hold only a design snapshot, not the
//! `EditorState`, so they call [`check_design_progress`] -- the same evaluation,
//! one layer down. Every caller runs it AFTER `EditorModel.solve_state`/
//! `status_is_problem` have been updated for the result it is applying, since the
//! `solved_closed` goal reads them.
//!
//! Goals are judged from STATE, never from which button was clicked, so every
//! route to a goal counts (quick add, inline edit, undo/redo, auto-solve), and a
//! failed validation -- which never changes the design -- can never advance.

use crate::{EditorModel, GuideModel, MainWindow, gui::editor::state::EditorState};
use indicatrix_cut_core::Design;
use indicatrix_editor::guide::reached_completion;
use slint::ComponentHandle;

/// Reports `key` to `GuideModel.notify` directly -- for a goal that is an EVENT
/// rather than a state `goal_reached` could read back afterwards (only
/// `NEW_DESIGN_CREATED` today). The Slint side ignores a key that is not the
/// current step's own.
pub(in crate::gui::editor) fn notify(ui: &MainWindow, key: &str) {
    ui.global::<GuideModel>().invoke_notify(key.into());
}

/// Evaluates the current guide step's goal against `state` and reports it to
/// `GuideModel.notify` when reached. A no-op while the guide is closed, on a manual
/// step, or while the current step is already showing "Done".
pub(in crate::gui::editor) fn check_progress(ui: &MainWindow, state: &EditorState) {
    check_design_progress(ui, &state.design);
}

/// [`check_progress`] for a caller holding only a `Design` (the background
/// auto-solve's completion, whose snapshot is the design it just solved).
pub(in crate::gui::editor) fn check_design_progress(ui: &MainWindow, design: &Design) {
    let guide = ui.global::<GuideModel>();
    if !guide.get_open() || guide.get_step_done() {
        return;
    }
    let Ok(index) = usize::try_from(guide.get_step_index()) else {
        return;
    };
    let model = ui.global::<EditorModel>();
    let solved_closed = model.get_solve_state() == "solved" && !model.get_status_is_problem();
    if let Some(key) = reached_completion(index, design, solved_closed) {
        guide.invoke_notify(key.into());
    }
}
