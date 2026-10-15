//! The tab's own commands: row taps, the toolbar chips, Select mode and its batch bar,
//! undo and redo, and the "Remove Anyway?" question.

use std::rc::Rc;

use indicatrix_mobile_core::{
    app_state::AppState,
    design_state::{
        EditOutcome, EditRefusal, QuickAdd, add_tier, adopt_all, adopt_selected, clear_multi,
        complete_orbit, mirror_indices, offset_multi, quick_add, redo, remove, remove_multi,
        select_all, select_tier, toggle_multi, undo,
    },
};
use slint::ComponentHandle;

use super::{
    announce, edit_only, finish, run_edit,
    ui::{self, Pending, flat_tier},
};
use crate::{
    AppWindow, TiersModel,
    design::{Shared, solve::clock},
    push::toast,
};

/// How far the batch bar's Offset chips move the angles, in degrees.
const OFFSET_STEP: f64 = 0.1;

/// Connects the toolbar, the rows, Select mode and the question.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let model = window.global::<TiersModel>();

    model.on_row_tapped(handler(window, state, row_tapped));
    model.on_row_long_pressed(handler(window, state, row_long_pressed));
    model.on_select_mode_changed(handler(window, state, select_mode_changed));
    model.on_select_all(handler0(window, state, |window, state| {
        select_all(&mut state.borrow_mut());
        crate::design::refresh(window, state);
    }));
    model.on_select_clear(handler0(window, state, |window, state| {
        clear_multi(&mut state.borrow_mut());
        crate::design::refresh(window, state);
    }));

    model.on_undo(handler0(window, state, |window, state| {
        run_edit(window, state, undo);
    }));
    model.on_redo(handler0(window, state, |window, state| {
        run_edit(window, state, redo);
    }));
    model.on_add_tier(handler0(window, state, |window, state| {
        run_edit(window, state, add_tier);
    }));
    model.on_quick_add(handler(window, state, |window, state, index: i32| {
        if let Some(preset) = QuickAdd::from_index(index) {
            run_edit(window, state, |app| quick_add(app, preset));
        }
    }));
    model.on_complete_orbit_selected(handler0(window, state, |window, state| {
        with_selected(window, state, complete_orbit);
    }));
    model.on_mirror_selected(handler0(window, state, |window, state| {
        with_selected(window, state, mirror_indices);
    }));
    model.on_adopt_all(handler0(window, state, |window, state| {
        run_edit(window, state, adopt_all);
    }));

    model.on_batch_delete(handler0(window, state, |window, state| {
        remove_group(window, state, false);
    }));
    model.on_batch_adopt(handler0(window, state, |window, state| {
        run_edit(window, state, adopt_selected);
    }));
    model.on_batch_offset(handler(window, state, |window, state, sign: f32| {
        let delta = f64::from(sign.signum()) * OFFSET_STEP;
        run_edit(window, state, |app| offset_multi(app, delta, clock()));
    }));

    model.on_confirm_result(handler(window, state, confirm_result));
}

/// A callback with one argument that gets the window and the state.
pub(super) fn handler<A: 'static>(
    window: &AppWindow,
    state: &Shared,
    run: impl Fn(&AppWindow, &Shared, A) + 'static,
) -> impl Fn(A) + 'static {
    let (weak, state) = (window.as_weak(), Rc::clone(state));
    move |argument| {
        if let Some(window) = weak.upgrade() {
            run(&window, &state, argument);
        }
    }
}

/// [`handler`] for a callback without arguments.
pub(super) fn handler0(
    window: &AppWindow,
    state: &Shared,
    run: impl Fn(&AppWindow, &Shared) + 'static,
) -> impl Fn() + 'static {
    let (weak, state) = (window.as_weak(), Rc::clone(state));
    move || {
        if let Some(window) = weak.upgrade() {
            run(&window, &state);
        }
    }
}

/// Runs a tier edit on the selected tier, or says nothing is selected.
fn with_selected(
    window: &AppWindow,
    state: &Shared,
    edit: impl FnOnce(&mut AppState, usize) -> EditOutcome,
) {
    let selected = state.borrow().selected_tier;
    match selected {
        Some(tier) => {
            run_edit(window, state, |app| edit(app, tier));
        }
        None => toast::show(window, "Select a tier first."),
    }
}

/// A tap on a row: select it; a tap on the selected row opens its tier sheet; in Select
/// mode it toggles the row instead.
fn row_tapped(window: &AppWindow, state: &Shared, row: i32) {
    let Some(tier) = flat_tier(row) else {
        toast::show(window, "Tools-cut tiers are edited on the desktop.");
        return;
    };
    let model = window.global::<TiersModel>();
    if model.get_select_mode() {
        toggle_multi(&mut state.borrow_mut(), tier);
    } else if state.borrow().selected_tier == Some(tier) {
        model.invoke_open_tier(row);
        return;
    } else {
        select_tier(&mut state.borrow_mut(), Some(tier), false);
    }
    crate::design::refresh(window, state);
}

/// A long press on a row (Select mode is already on): puts the row in the group.
fn row_long_pressed(window: &AppWindow, state: &Shared, row: i32) {
    let Some(tier) = flat_tier(row) else {
        return;
    };
    {
        let mut app = state.borrow_mut();
        if !app.session.multi_selected.contains(&tier) {
            toggle_multi(&mut app, tier);
        }
    }
    crate::design::refresh(window, state);
}

/// Select mode was switched on or off; leaving it empties the group.
fn select_mode_changed(window: &AppWindow, state: &Shared, on: bool) {
    if !on {
        clear_multi(&mut state.borrow_mut());
    }
    crate::design::refresh(window, state);
}

/// Removes the multi-selection; asks first when other tiers still meet some of it.
fn remove_group(window: &AppWindow, state: &Shared, cascade: bool) {
    let (outcome, moved) = edit_only(state, |app| remove_multi(app, cascade));
    match &outcome {
        Err(EditRefusal::NeedsConfirmation(question)) => {
            ask(window, "Remove Tiers?", question, Pending::RemoveMulti);
        }
        _ => announce(window, &outcome),
    }
    finish(window, state, moved);
}

/// Shows the "Remove Anyway?" card for `pending`.
pub(super) fn ask(window: &AppWindow, title: &str, question: &str, pending: Pending) {
    ui::with(|ui| ui.pending = Some(pending));
    let model = window.global::<TiersModel>();
    model.set_confirm_title(title.into());
    model.set_confirm_body(question.into());
    model.set_confirm_open(true);
}

/// The answer to the question: a "yes" repeats the removal with the references cleared.
fn confirm_result(window: &AppWindow, state: &Shared, confirmed: bool) {
    let Some(pending) = ui::with(|ui| ui.pending.take()) else {
        return;
    };
    if !confirmed {
        return;
    }
    match pending {
        Pending::RemoveMulti => remove_group(window, state, true),
        Pending::RemoveTier(tier) => {
            if run_edit(window, state, |app| remove(app, tier, true)) {
                // The tier sheet showed the tier that is gone.
                window.global::<TiersModel>().invoke_close_tier();
            }
        }
    }
}
