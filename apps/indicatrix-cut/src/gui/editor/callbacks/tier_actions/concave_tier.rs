//! Concave (tool-cut) tiers in the tier table and the inspector: the concave form's
//! Save/Add, the "+ Add Concave Tier" entry, the form's read-back at full precision, and
//! the row actions (duplicate, remove, move) a concave row shares with a flat one.
//!
//! A concave row's `EditorTierItem.index` is its table POSITION: the flat tiers come
//! first, so concave tier `c` sits at `design.tiers.len() + c` (see
//! `state::row_format::tier_items_from_rows`). Every callback here that takes an
//! `index` therefore takes a position and maps it back with [`concave_at`]; the flat
//! callbacks in [`super::tier_crud`] call the `*_now` functions below for a position
//! that names a concave tier and carry on as before for any other.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_editor::loading::{
    ConcaveTierFormFields, concave_tier_form_fields, parse_concave_tier_form, tier_form_error_field,
};
use slint::ComponentHandle;

use super::{
    misc::adjust_selection_after_remove,
    tier_form::{clear_tier_form_error, report_tier_form_error},
};
use crate::{
    ConcaveFormData, EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            guide,
            state::{EditorState, concave_tier_index},
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// The concave tier the table position `position` names in `st`'s design, or `None`
/// for a flat position (or one beyond the table).
#[must_use]
pub(super) fn concave_at(st: &EditorState, position: i32) -> Option<usize> {
    concave_tier_index(
        st.design.tiers.len(),
        st.design.concave_tiers.len(),
        position,
    )
}

/// The form's fields from the Slint struct the inspector passes.
fn fields_from_data(data: &ConcaveFormData) -> ConcaveTierFormFields {
    ConcaveTierFormFields {
        name: data.name.to_string(),
        angle_deg: data.angle_deg.to_string(),
        indices: data.indices.to_string(),
        instructions: data.instructions.to_string(),
        tool: data.tool.to_string(),
        tool_azimuth_deg: data.tool_azimuth_deg.to_string(),
        x: data.x.to_string(),
        y: data.y.to_string(),
        z: data.z.to_string(),
        diameter_ratio: data.diameter_ratio.to_string(),
        tool_angle_deg: data.tool_angle_deg.to_string(),
        reciprocating: data.reciprocating,
    }
}

/// The fields as the Slint struct the inspector seeds its draft from.
fn data_from_fields(fields: ConcaveTierFormFields) -> ConcaveFormData {
    ConcaveFormData {
        name: fields.name.into(),
        angle_deg: fields.angle_deg.into(),
        indices: fields.indices.into(),
        instructions: fields.instructions.into(),
        tool: fields.tool.into(),
        tool_azimuth_deg: fields.tool_azimuth_deg.into(),
        x: fields.x.into(),
        y: fields.y.into(),
        z: fields.z.into(),
        diameter_ratio: fields.diameter_ratio.into(),
        tool_angle_deg: fields.tool_angle_deg.into(),
        reciprocating: fields.reciprocating,
    }
}

/// Wires the three concave callbacks. Piggybacked onto
/// [`super::tier_form::setup_save_tier_callback`], the one existing call site that
/// already carries every argument they need (see
/// [`super::tier_crud::setup_toggle_detach_callback`]'s doc comment for the reason).
pub(super) fn setup_concave_tier_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // "+ Add Concave Tier": opens the Tier tab on a blank concave form. The mode flag is
    // set before the selection is cleared so the form that appears is the concave one
    // even when the selection was already -1 (a same-value write raises no `changed`).
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_add_concave_tier(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let model = ui.global::<EditorModel>();
        model.set_inspector_concave_mode(true);
        model.set_inspector_tab(0);
        model.set_inspector_collapsed(false);
        model.set_selected_tier_index(-1);
    });

    // A concave tier read back at full precision, for the inspector's draft. `try_borrow`
    // because this runs from a Slint function body, which a callback already holding the
    // state could in principle interleave with; an unavailable state reads as a blank form
    // rather than a panic.
    let state_data = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_concave_form_data(move |position: i32| {
            let Ok(st) = state_data.try_borrow() else {
                return ConcaveFormData::default();
            };
            concave_at(&st, position)
                .and_then(|index| st.design.concave_tiers.get(index))
                .map(concave_tier_form_fields)
                .map(data_from_fields)
                .unwrap_or_default()
        });

    let state_save = Rc::clone(state);
    let render_ctx_save = Arc::clone(render_ctx);
    let preview_state_save = Arc::clone(preview_state);
    let solid_last_solved_save = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_save_concave_tier(move |position: i32, data: ConcaveFormData| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            save_concave_tier(
                &ui,
                &Services {
                    state: &state_save,
                    render_ctx: &render_ctx_save,
                    preview_state: &preview_state_save,
                    solid_last_solved: &solid_last_solved_save,
                },
                position,
                &data,
            );
        });
}

/// The shared handles every concave edit refreshes the UI through, bundled so the
/// `*_now` functions below stay under clippy's argument-count lint.
pub(super) struct Services<'a> {
    pub(super) state: &'a Rc<RefCell<EditorState>>,
    pub(super) render_ctx: &'a Arc<Mutex<RenderContext>>,
    pub(super) preview_state: &'a Arc<SolidPreviewState>,
    pub(super) solid_last_solved: &'a SolidLastSolved,
}

/// The tail every concave edit shares: refresh the tier table and panel without
/// solving, and replan the solid preview in full (a concave edit changes the stone but
/// not the flat solve, so no tier is "dirty", and the planner must rebuild the tools).
fn refresh_after_concave_edit(ui: &MainWindow, services: &Services<'_>, st: &EditorState) {
    refresh_editor_panel_stale(ui, services.render_ctx, st, &BTreeSet::new());
    guide::check_progress(ui, st);
    submit_preview_replan(
        ui,
        services.render_ctx,
        services.preview_state,
        services.solid_last_solved,
        st,
        BTreeSet::new(),
        true,
    );
}

/// The concave form's Save/Add: parses the draft with [`parse_concave_tier_form`] (every
/// error is prefixed with its field, which [`tier_form_error_field`] turns into the red
/// border), then applies one `Edit::AddConcaveTier` (`position` -1) or
/// `Edit::ModifyConcaveTier` through the editor's own `concave_tier_save_edit`, so the
/// desktop and the web app save a concave tier identically.
fn save_concave_tier(
    ui: &MainWindow,
    services: &Services<'_>,
    position: i32,
    data: &ConcaveFormData,
) {
    let mut st = services.state.borrow_mut();
    let existing = if position < 0 {
        None
    } else if let Some(index) = concave_at(&st, position) {
        Some(index)
    } else {
        drop(st);
        report_tier_form_error(
            ui,
            "The selected row is not a concave tier -- use the flat tier form for it.",
            "",
        );
        return;
    };
    let tier = match parse_concave_tier_form(&fields_from_data(data), st.design.meta.gear_teeth) {
        Ok(tier) => tier,
        Err(message) => {
            drop(st);
            report_tier_form_error(ui, &message, tier_form_error_field(&message));
            return;
        }
    };
    if let Some(message) =
        indicatrix_editor::tier_save::concave_name_clash_message(&st.design, existing, &tier.name)
    {
        drop(st);
        report_tier_form_error(ui, &message, tier_form_error_field(&message));
        return;
    }
    let edit = indicatrix_editor::tier_save::concave_tier_save_edit(&st.design, existing, tier);
    match st.apply(edit) {
        Ok(()) => {
            clear_tier_form_error(ui);
            refresh_after_concave_edit(ui, services, &st);
            if existing.is_none() {
                // Appended at the end of the concave rows, so its position is the last one.
                let position = st.design.tiers.len() + st.design.concave_tiers.len() - 1;
                let name = st
                    .design
                    .concave_tiers
                    .last()
                    .map(|tier| tier.name.clone())
                    .unwrap_or_default();
                drop(st);
                ui.global::<EditorModel>()
                    .set_selected_tier_index(i32::try_from(position).unwrap_or(-1));
                let label = if name.is_empty() {
                    "concave tier".to_owned()
                } else {
                    format!("concave tier {name}")
                };
                show_toast(ui, &format!("Added {label}"), "info");
            }
        }
        Err(error) => {
            drop(st);
            let message = error.to_string();
            report_tier_form_error(ui, &message, tier_form_error_field(&message));
        }
    }
}

/// Duplicate for concave tier `index` (a position in `design.concave_tiers`): a copy right
/// after it, selected.
pub(super) fn duplicate_concave_now(ui: &MainWindow, services: &Services<'_>, index: usize) {
    let mut st = services.state.borrow_mut();
    match st.duplicate_concave_tier(index) {
        Ok(None) => {}
        Ok(Some(duplicated)) => {
            refresh_after_concave_edit(ui, services, &st);
            let position = st.design.tiers.len() + duplicated.new_index;
            drop(st);
            ui.global::<EditorModel>()
                .set_selected_tier_index(i32::try_from(position).unwrap_or(-1));
            show_toast(
                ui,
                &format!(
                    "Duplicated {} as {}",
                    duplicated.source_label, duplicated.duplicate_label
                ),
                "info",
            );
        }
        Err(error) => {
            drop(st);
            show_toast(ui, &error.to_string(), "error");
        }
    }
}

/// Remove for concave tier `index`; `position` is the table position the selection is
/// shifted from (see [`adjust_selection_after_remove`]). Nothing can meet a concave tier
/// by name, so there is no "remove anyway" question.
pub(super) fn remove_concave_now(
    ui: &MainWindow,
    services: &Services<'_>,
    index: usize,
    position: i32,
) {
    let mut st = services.state.borrow_mut();
    match st.remove_concave_tier(index) {
        Ok(None) => {}
        Ok(Some(removed)) => {
            refresh_after_concave_edit(ui, services, &st);
            adjust_selection_after_remove(ui, position);
            drop(st);
            let plural = if removed.facet_count == 1 { "" } else { "s" };
            show_toast(
                ui,
                &format!(
                    "Removed {} ({} facet{plural}), Undo",
                    removed.name, removed.facet_count
                ),
                "info",
            );
        }
        Err(error) => {
            drop(st);
            show_toast(ui, &error.to_string(), "error");
        }
    }
}

/// Reorder concave tier `index` one place (`direction` < 0 up, else down) within the
/// concave rows -- the cutting order inside a section's concave group -- and select it
/// at its new position. A flat row never swaps with a concave one: they are two lists.
pub(super) fn move_concave_now(
    ui: &MainWindow,
    services: &Services<'_>,
    index: usize,
    direction: i32,
) {
    let mut st = services.state.borrow_mut();
    match st.move_concave_tier(index, direction) {
        Ok(None) => {}
        Ok(Some(moved)) => {
            refresh_after_concave_edit(ui, services, &st);
            let position = st.design.tiers.len() + moved.target;
            drop(st);
            ui.global::<EditorModel>()
                .set_selected_tier_index(i32::try_from(position).unwrap_or(-1));
        }
        Err(error) => {
            drop(st);
            show_toast(ui, &error.to_string(), "error");
        }
    }
}
