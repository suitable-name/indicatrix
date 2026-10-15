//! The tier sheet: name, index chips, orbit tools, what the tier meets, and Duplicate /
//! Delete / Move / Detach.

use indicatrix_mobile_core::{
    app_state::AppState,
    design_state::{
        EditOutcome, EditRefusal, Key, KeypadState, add_index, adopt, complete_orbit, duplicate,
        mirror_indices, move_tier, remove, remove_index, rotate_indices, select_tier, set_name,
        tier_detail, toggle_detach,
    },
};
use slint::{ComponentHandle, SharedString};

use super::{
    actions::{ask, handler, handler0},
    announce, edit_only, finish, rows, run_edit,
    ui::{self, Pending, flat_tier, sync_model},
};
use crate::{
    AppWindow, TierChipData, TiersModel,
    design::{Shared, solve::clock},
};

/// Connects the sheet's callbacks.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let model = window.global::<TiersModel>();

    model.on_tier_requested(handler(window, state, open));
    model.on_tier_dismissed(handler0(window, state, dismissed));
    model.on_tier_rename(handler(
        window,
        state,
        |window, state, text: SharedString| {
            rename(window, state, &text);
        },
    ));
    model.on_tier_remove_index(handler(window, state, |window, state, position: i32| {
        if let (Some(tier), Ok(position)) = (open_tier(), usize::try_from(position)) {
            run_edit(window, state, |app| remove_index(app, tier, position));
        }
    }));
    model.on_tier_rotate(handler(window, state, |window, state, teeth: i32| {
        if let Some(tier) = open_tier() {
            run_edit(window, state, |app| {
                rotate_indices(app, tier, i64::from(teeth), clock())
            });
        }
    }));
    model.on_tier_mirror(handler0(window, state, |window, state| {
        with_tier(window, state, mirror_indices);
    }));
    model.on_tier_complete_orbit(handler0(window, state, |window, state| {
        with_tier(window, state, complete_orbit);
    }));
    model.on_tier_adopt(handler0(window, state, |window, state| {
        with_tier(window, state, adopt);
    }));
    model.on_tier_detach(handler0(window, state, |window, state| {
        with_tier(window, state, toggle_detach);
    }));
    model.on_tier_duplicate(handler0(window, state, duplicate_tier));
    model.on_tier_delete(handler0(window, state, delete_tier));
    model.on_tier_move(handler(window, state, move_open_tier));
    model.on_index_key(handler(window, state, |window, state, id: SharedString| {
        if let Some(key) = Key::from_id(&id) {
            ui::with(|ui| ui.index_keypad.press(key));
            push(window, &state.borrow());
        }
    }));
    model.on_index_ok(handler0(window, state, index_ok));
}

/// The tier the sheet is open for.
fn open_tier() -> Option<usize> {
    ui::with(|ui| ui.sheet)
}

/// Runs a tier edit on the sheet's tier.
fn with_tier(
    window: &AppWindow,
    state: &Shared,
    edit: impl FnOnce(&mut AppState, usize) -> EditOutcome,
) {
    if let Some(tier) = open_tier() {
        run_edit(window, state, |app| edit(app, tier));
    }
}

/// The tier sheet was opened for model row `row`: select the tier and seed the name field.
fn open(window: &AppWindow, state: &Shared, row: i32) {
    let model = window.global::<TiersModel>();
    let Some(tier) = flat_tier(row) else {
        model.invoke_close_tier();
        return;
    };
    {
        let mut app = state.borrow_mut();
        select_tier(&mut app, Some(tier), model.get_select_mode());
        model.set_tier_name(
            app.session
                .design
                .tiers
                .get(tier)
                .map_or("", |found| found.name.as_str())
                .into(),
        );
    }
    ui::with(|ui| {
        ui.sheet = Some(tier);
        ui.index_keypad = KeypadState::new();
    });
    model.set_index_entry_open(false);
    crate::design::refresh(window, state);
}

/// The sheet was closed.
fn dismissed(window: &AppWindow, state: &Shared) {
    ui::with(|ui| {
        ui.sheet = None;
        ui.index_keypad = KeypadState::new();
    });
    rows::invalidate();
    crate::design::refresh(window, state);
}

/// The name field was committed.
fn rename(window: &AppWindow, state: &Shared, text: &str) {
    let Some(tier) = open_tier() else {
        return;
    };
    let current = state
        .borrow()
        .session
        .design
        .tiers
        .get(tier)
        .map(|found| found.name.clone());
    let Some(current) = current else {
        return;
    };
    if current == text.trim() {
        return;
    }
    let (outcome, moved) = edit_only(state, |app| set_name(app, tier, text));
    announce(window, &outcome);
    if outcome.is_err() {
        // Show the name the design really has.
        window.global::<TiersModel>().set_tier_name(current.into());
    }
    finish(window, state, moved);
}

/// "Duplicate": the copy is selected and the sheet moves to it.
fn duplicate_tier(window: &AppWindow, state: &Shared) {
    let Some(tier) = open_tier() else {
        return;
    };
    if run_edit(window, state, |app| duplicate(app, tier)) {
        follow_selection(window, state);
    }
}

/// "Move Up" / "Move Down": the sheet follows the tier.
fn move_open_tier(window: &AppWindow, state: &Shared, direction: i32) {
    let Some(tier) = open_tier() else {
        return;
    };
    if run_edit(window, state, |app| move_tier(app, tier, direction)) {
        follow_selection(window, state);
    }
}

/// Points the sheet at the selected tier (after an edit that selected another one).
fn follow_selection(window: &AppWindow, state: &Shared) {
    let (selected, name) = {
        let app = state.borrow();
        let selected = app.selected_tier;
        let name = selected
            .and_then(|tier| app.session.design.tiers.get(tier))
            .map(|found| found.name.clone())
            .unwrap_or_default();
        (selected, name)
    };
    ui::with(|ui| ui.sheet = selected);
    window.global::<TiersModel>().set_tier_name(name.into());
    crate::design::refresh(window, state);
}

/// "Delete Tier": asks first when other tiers still meet it by name.
fn delete_tier(window: &AppWindow, state: &Shared) {
    let Some(tier) = open_tier() else {
        return;
    };
    let (outcome, moved) = edit_only(state, |app| remove(app, tier, false));
    match &outcome {
        Err(EditRefusal::NeedsConfirmation(question)) => {
            ask(window, "Remove Tier?", question, Pending::RemoveTier(tier));
        }
        Ok(_) => {
            announce(window, &outcome);
            window.global::<TiersModel>().invoke_close_tier();
        }
        Err(EditRefusal::Failed(_)) => announce(window, &outcome),
    }
    finish(window, state, moved);
}

/// The index keypad's "OK": adds the typed position, or closes the keypad when nothing
/// was typed.
fn index_ok(window: &AppWindow, state: &Shared) {
    let Some(tier) = open_tier() else {
        return;
    };
    let model = window.global::<TiersModel>();
    let Some(value) = ui::with(|ui| ui.index_keypad.value()) else {
        model.set_index_entry_open(false);
        return;
    };
    let (outcome, moved) = edit_only(state, |app| add_index(app, tier, value));
    announce(window, &outcome);
    if outcome.is_ok() {
        ui::with(|ui| ui.index_keypad = KeypadState::new());
        model.set_index_entry_open(false);
    }
    finish(window, state, moved);
}

/// Pushes the sheet's properties from the state (nothing when it is closed).
pub(super) fn push(window: &AppWindow, state: &AppState) {
    let Some(tier) = open_tier() else {
        return;
    };
    let model = window.global::<TiersModel>();
    let detail = ui::with(|ui| ui.rows.get(tier).and_then(|row| tier_detail(state, row)));
    let Some(detail) = detail else {
        // The tier is gone (an undo removed it): close the sheet without touching the state.
        ui::with(|ui| ui.sheet = None);
        model.set_tier_open(false);
        model.set_index_entry_open(false);
        model.invoke_lower_sheet();
        return;
    };
    model.set_tier_row(i32::try_from(detail.index).unwrap_or(-1));
    model.set_tier_title(detail.title.into());
    model.set_tier_angle(format!("{}\u{b0}", detail.angle_text).into());
    model.set_tier_block(detail.block.into());
    model.set_tier_meet(detail.meet_summary.into());
    model.set_tier_solve(detail.solve_summary.into());
    model.set_tier_relation(detail.relation_text.into());
    model.set_tier_orbit(detail.orbit_text.into());
    model.set_tier_orbit_incomplete(detail.orbit_incomplete);
    model.set_tier_detached(detail.is_detached);
    model.set_tier_imported(detail.imported_meet_text.into());
    model.set_tier_warning(detail.warning_text.into());
    model.set_tier_can_up(detail.can_move_up);
    model.set_tier_can_down(detail.can_move_down);
    let chips: Vec<TierChipData> = detail
        .indices
        .iter()
        .map(|chip| TierChipData {
            label: chip.label.as_str().into(),
            detached: chip.detached,
        })
        .collect();
    let (readout, typing) = ui::with(|ui| {
        sync_model(&ui.chips_model, chips);
        (ui.index_keypad.display(), !ui.index_keypad.is_pristine())
    });
    model.set_index_readout(readout.into());
    model.set_index_typing(typing);
}
