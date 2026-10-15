//! The Tiers tab: the tier list, the angle sheet, the tier sheet, the keypad and
//! undo/redo.
//!
//! The logic lives in `indicatrix_mobile_core::design_state`; this module moves values
//! between it and `TiersModel` (`ui/design/tiers/tiers_model.slint`):
//!
//! - `rows`: the list rows, rebuilt only when the design, the solve or the selection
//!   changed;
//! - `actions`: the toolbar, row taps, Select mode and its batch bar, undo and redo, and
//!   the "Remove Anyway?" question;
//! - `angle`: the angle sheet (steppers, slider, presets, keypad);
//! - `sheet`: the tier sheet (name, index chips, orbit tools, duplicate, delete, move);
//! - `ui`: the UI-thread state those share.
//!
//! Every edit goes through [`edit_only`] and [`finish`] (or [`run_edit`], which is both):
//! the core function runs on the state, the generation tells whether the design moved, and
//! `design::after_edit` (auto-solve and re-push) or `design::refresh` follows.

mod actions;
mod angle;
mod rows;
mod sheet;
mod ui;

use std::rc::Rc;

use indicatrix_mobile_core::{
    app_state::AppState,
    design_state::{EditOutcome, EditRefusal, Edited, can_redo, can_undo, redo_hint, undo_hint},
};
use slint::{ComponentHandle, ModelRc};

use super::Shared;
use crate::{AppWindow, TiersModel, push::toast};

/// Connects the tab's callbacks and gives the list its models.
pub fn wire(window: &AppWindow, state: &Shared) {
    let model = window.global::<TiersModel>();
    ui::with(|ui| {
        model.set_rows(ModelRc::from(Rc::clone(&ui.rows_model)));
        model.set_tier_chips(ModelRc::from(Rc::clone(&ui.chips_model)));
        model.set_angle_presets(ModelRc::from(Rc::clone(&ui.presets_model)));
    });
    actions::wire(window, state);
    angle::wire(window, state);
    sheet::wire(window, state);
}

/// Pushes the tab from the state.
pub fn push(window: &AppWindow, state: &AppState) {
    let model = window.global::<TiersModel>();
    model.set_can_undo(can_undo(state));
    model.set_can_redo(can_redo(state));
    model.set_undo_hint(undo_hint(state).into());
    model.set_redo_hint(redo_hint(state).into());
    model.set_selected_tier(
        state
            .selected_tier
            .and_then(|tier| i32::try_from(tier).ok())
            .unwrap_or(-1),
    );
    model.set_multi_count(i32::try_from(state.session.multi_selected.len()).unwrap_or(0));
    // While the angle sheet is open the list is hidden behind it: a slider drag changes
    // the design many times a second, and the rows are rebuilt once when it closes.
    let angle_open = ui::with(|ui| ui.angle.is_some());
    if !angle_open {
        rows::push(state);
        sheet::push(window, state);
    }
    angle::push(window, state);
}

/// Runs the edit on the state. Returns what it said and whether the design moved.
pub(super) fn edit_only(
    state: &Shared,
    edit: impl FnOnce(&mut AppState) -> EditOutcome,
) -> (EditOutcome, bool) {
    let before = state.borrow().session.current_generation();
    let outcome = edit(&mut state.borrow_mut());
    let moved = state.borrow().session.current_generation() != before;
    (outcome, moved)
}

/// Re-pushes after an edit: `design::after_edit` if the design moved, else a plain refresh.
pub(super) fn finish(window: &AppWindow, state: &Shared, moved: bool) {
    if moved {
        super::after_edit(window, state);
    } else {
        super::refresh(window, state);
    }
}

/// Shows an outcome's notice, or the text of a plain failure, as a toast. A question
/// ([`EditRefusal::NeedsConfirmation`]) is left to the caller.
pub(super) fn announce(window: &AppWindow, outcome: &EditOutcome) {
    match outcome {
        Ok(Edited {
            notice: Some(text), ..
        })
        | Err(EditRefusal::Failed(text)) => toast::show(window, text),
        Ok(_) | Err(EditRefusal::NeedsConfirmation(_)) => {}
    }
}

/// [`edit_only`], a toast for what it said, then [`finish`]. Returns whether the edit went
/// through (no refusal).
pub(super) fn run_edit(
    window: &AppWindow,
    state: &Shared,
    edit: impl FnOnce(&mut AppState) -> EditOutcome,
) -> bool {
    let (outcome, moved) = edit_only(state, edit);
    announce(window, &outcome);
    finish(window, state, moved);
    outcome.is_ok()
}
