//! The angle sheet: step buttons, the slider, "Typical" presets and the keypad.
//!
//! The steppers, the slider and the presets change the design at once; consecutive changes
//! within the history's coalescing window are one undo step (the keypad's draft is applied
//! by "OK"). A refusal (the angle follows a relation, a number over 90 degrees) shows on
//! the sheet instead of as a toast.

use std::rc::Rc;

use indicatrix_editor::slider_ranges::{ANGLE_STEP_DEG, angle_presets_for, angle_spec_data};
use indicatrix_mobile_core::{
    app_state::AppState,
    design_state::{
        EditOutcome, EditRefusal, Key, KeypadState, MobileTierRow, set_angle_from_text,
        set_angle_value,
    },
};
use slint::ComponentHandle;

use super::{
    edit_only, finish, rows,
    ui::{self, narrow, round_hundredths, sync_model},
};
use crate::{
    AnglePresetData, AppWindow, TiersModel,
    design::{Shared, solve::clock},
};

/// The steepest angle a facet can have, in degrees.
const MAX_ANGLE_DEG: f64 = 90.0;

/// Connects the sheet's callbacks.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let model = window.global::<TiersModel>();

    model.on_angle_requested({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move |row| {
            if let Some(window) = weak.upgrade() {
                open(&window, &state, row);
            }
        }
    });
    model.on_angle_dismissed({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move || {
            if let Some(window) = weak.upgrade() {
                dismissed(&window, &state);
            }
        }
    });
    model.on_angle_step({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move |delta| {
            if let Some(window) = weak.upgrade() {
                step(&window, &state, f64::from(delta));
            }
        }
    });
    model.on_angle_slider_moved({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move |value| {
            if let Some(window) = weak.upgrade() {
                slider_moved(&window, &state, f64::from(value));
            }
        }
    });
    model.on_angle_slider_released({
        let state = Rc::clone(state);
        move || state.borrow_mut().session.history.end_coalesce_run()
    });
    model.on_angle_preset({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move |index| {
            if let Some(window) = weak.upgrade() {
                preset(&window, &state, index);
            }
        }
    });
    model.on_keypad_key({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move |id| {
            if let Some(window) = weak.upgrade() {
                keypad_key(&window, &state, &id);
            }
        }
    });
    model.on_keypad_ok({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move || {
            if let Some(window) = weak.upgrade() {
                keypad_ok(&window, &state);
            }
        }
    });
}

/// The angle sheet was opened for model row `row`.
fn open(window: &AppWindow, state: &Shared, row: i32) {
    let Some(tier) = ui::flat_tier(row) else {
        window.global::<TiersModel>().invoke_close_angle();
        return;
    };
    {
        let mut app = state.borrow_mut();
        let select_mode = window.global::<TiersModel>().get_select_mode();
        indicatrix_mobile_core::design_state::select_tier(&mut app, Some(tier), select_mode);
    }
    ui::with(|ui| {
        ui.angle = Some(tier);
        ui.angle_keypad = KeypadState::new();
        ui.angle_error.clear();
    });
    crate::design::refresh(window, state);
}

/// The sheet was closed: the list is rebuilt, and the drag's coalescing run ends.
fn dismissed(window: &AppWindow, state: &Shared) {
    ui::with(|ui| ui.angle = None);
    state.borrow_mut().session.history.end_coalesce_run();
    rows::invalidate();
    crate::design::refresh(window, state);
}

/// The tier the sheet is open for.
fn open_tier() -> Option<usize> {
    ui::with(|ui| ui.angle)
}

/// Remembers what an edit said, to show on the sheet.
fn record(outcome: &EditOutcome) {
    ui::with(|ui| {
        ui.angle_error = match outcome {
            Err(EditRefusal::Failed(text) | EditRefusal::NeedsConfirmation(text)) => text.clone(),
            Ok(_) => String::new(),
        };
    });
}

/// Sets the open tier's angle to the magnitude `magnitude` (discarding a keypad draft).
fn set_value(window: &AppWindow, state: &Shared, tier: usize, magnitude: f64) {
    ui::with(|ui| ui.angle_keypad = KeypadState::new());
    let (outcome, moved) = edit_only(state, |app| set_angle_value(app, tier, magnitude, clock()));
    record(&outcome);
    finish(window, state, moved);
}

/// A stepper button: the shown angle moves by `delta` degrees.
fn step(window: &AppWindow, state: &Shared, delta: f64) {
    let Some(tier) = open_tier() else {
        return;
    };
    let magnitude = state
        .borrow()
        .session
        .design
        .tiers
        .get(tier)
        .map(|found| found.angle_deg.abs());
    if let Some(magnitude) = magnitude {
        let target = round_hundredths(magnitude + delta).clamp(0.0, MAX_ANGLE_DEG);
        set_value(window, state, tier, target);
    }
}

/// The slider was dragged to `value`: snap it to the slider's step.
fn slider_moved(window: &AppWindow, state: &Shared, value: f64) {
    let Some(tier) = open_tier() else {
        return;
    };
    let snapped = round_hundredths((value / ANGLE_STEP_DEG).round() * ANGLE_STEP_DEG);
    set_value(window, state, tier, snapped.clamp(0.0, MAX_ANGLE_DEG));
}

/// A "Typical" preset was tapped.
fn preset(window: &AppWindow, state: &Shared, index: i32) {
    let Some(tier) = open_tier() else {
        return;
    };
    let value = usize::try_from(index)
        .ok()
        .and_then(|index| ui::with(|ui| ui.preset_values.get(index).copied()));
    if let Some(value) = value {
        set_value(window, state, tier, value);
    }
}

/// A keypad key: updates the draft and repaints the readout.
fn keypad_key(window: &AppWindow, state: &Shared, id: &str) {
    let Some(key) = Key::from_id(id) else {
        return;
    };
    ui::with(|ui| ui.angle_keypad.press(key));
    push(window, &state.borrow());
}

/// "OK": applies the draft as typed angle text, or closes the sheet when there is none.
fn keypad_ok(window: &AppWindow, state: &Shared) {
    let Some(tier) = open_tier() else {
        return;
    };
    let Some(text) = ui::with(|ui| ui.angle_keypad.entry_text()) else {
        window.global::<TiersModel>().invoke_close_angle();
        return;
    };
    let (outcome, moved) = edit_only(state, |app| set_angle_from_text(app, tier, &text));
    if outcome.is_ok() {
        ui::with(|ui| ui.angle_keypad = KeypadState::new());
    }
    record(&outcome);
    finish(window, state, moved);
}

/// How the sheet's slider and presets see the tier: `"Driven"` for an angle that follows a
/// relation, `"Horizontal"` for a flat facet, else the row's block.
fn slider_kind(row: Option<&MobileTierRow>, magnitude: f64, negative: bool) -> &'static str {
    if row.is_some_and(|row| !row.relation_text.is_empty()) {
        return "Driven";
    }
    if magnitude < 1e-9 {
        return "Horizontal";
    }
    match row.map(|row| row.block.as_str()) {
        Some("Pavilion") => "Pavilion",
        Some("Crown") => "Crown",
        Some("Girdle") => "Girdle",
        _ if negative => "Pavilion",
        _ => "Crown",
    }
}

/// Pushes the sheet's properties from the state (nothing when it is closed).
pub(super) fn push(window: &AppWindow, state: &AppState) {
    let Some(tier) = open_tier() else {
        return;
    };
    let model = window.global::<TiersModel>();
    let Some(current) = state.session.design.tiers.get(tier) else {
        // The tier is gone (an undo removed it): close the sheet.
        ui::with(|ui| ui.angle = None);
        model.set_angle_open(false);
        model.invoke_lower_sheet();
        return;
    };
    let magnitude = current.angle_deg.abs();
    let negative = current.angle_deg.is_sign_negative();
    let angle_text = format!("{magnitude:.2}");
    let n_d = state.session.design.effective_refractive_index();
    let ri_text = format!("{n_d:.4}");

    let (title, block, kind) = ui::with(|ui| {
        let row = ui
            .rows
            .get(tier)
            .filter(|row| !row.is_concave && row.index == tier);
        let kind = slider_kind(row, magnitude, negative);
        let title = row.map_or_else(
            || format!("Tier {}", tier + 1),
            |row| {
                if row.name == row.code || row.name.is_empty() {
                    row.code.clone()
                } else {
                    format!("{} ({})", row.code, row.name)
                }
            },
        );
        let block = match (kind, row) {
            ("Driven", _) => "Angle follows a relation".to_owned(),
            ("Horizontal", _) => "Flat facet".to_owned(),
            (_, Some(row)) => row.block.clone(),
            (kind, None) => kind.to_owned(),
        };
        (title, block, kind)
    });

    let spec = angle_spec_data(kind, &angle_text, &current.name, &ri_text);
    let material = state.session.design.material.name.as_deref().unwrap_or("");
    let presets = angle_presets_for(&spec.side, &ri_text, material);
    let (readout, typing, error) = ui::with(|ui| {
        let typing = !ui.angle_keypad.is_pristine();
        let readout = if typing {
            ui.angle_keypad.display()
        } else {
            angle_text.clone()
        };
        ui.preset_values = presets.iter().map(|preset| preset.value_deg).collect();
        let items: Vec<AnglePresetData> = presets
            .iter()
            .map(|preset| AnglePresetData {
                label: preset.label.as_str().into(),
                value: preset.value_text.as_str().into(),
                reason: preset.reason.as_str().into(),
            })
            .collect();
        sync_model(&ui.presets_model, items);
        (readout, typing, ui.angle_error.clone())
    });

    model.set_angle_title(title.into());
    model.set_angle_block(block.into());
    model.set_angle_readout(readout.into());
    model.set_angle_typing(typing);
    model.set_angle_error(error.into());
    model.set_angle_slider_visible(spec.visible && spec.usable);
    model.set_angle_slider_value(narrow(spec.value));
    model.set_angle_slider_min(narrow(spec.range.min));
    model.set_angle_slider_max(narrow(spec.range.max));
    let (band_from, band_to) = spec.band.unwrap_or((0.0, 0.0));
    model.set_angle_band_from(narrow(band_from));
    model.set_angle_band_to(narrow(band_to));
    model.set_angle_note(spec.note.into());
}
