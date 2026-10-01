//! The New Design dialog (the desktop's `new_design_dialog.slint` and
//! `tier_actions::new_design`): every `FreshDesignSpec` field -- preform shape and size,
//! index gear, symmetry, mirror, starting material -- or a template from the gallery,
//! which locks gear, symmetry and mirror to its own tier table (the dialog does that in
//! Slint; the Create here parses whatever the fields hold).
//!
//! The dialog is open while `GalleryModel.open` is true. Creating over unsaved changes
//! goes through the Save / Discard / Cancel guard (`editor::unsaved`); the dialog closes
//! once the new design is installed.

use crate::{
    AppWindow, GalleryModel, NewDesignModel,
    app::{
        Ctx,
        persist::schedule_save,
        push::{MessageKind, push_all, show_message},
        solve::auto_solve,
        state::{DesignSource, DesignState},
    },
    editor::unsaved::confirm_discard,
};
use indicatrix_cut_core::FreshDesignSpec;
use indicatrix_editor::{
    EditorSession,
    loading::{parse_new_design_form, parse_preform_form},
    material::{builtin_preset_names, gear_choice_to_teeth},
    templates::template_cards,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// A cylinder preform's side count from the dialog: fixed, independent of the gear (the
/// desktop's `FIXED_CYLINDER_PREFORM_SIDES`).
const FIXED_CYLINDER_PREFORM_SIDES: usize = 96;

/// The spec the dialog's fields describe, or the message naming the field that is wrong.
fn spec_from_fields(model: &NewDesignModel<'_>) -> Result<FreshDesignSpec, String> {
    let gear_teeth = gear_choice_to_teeth(model.get_gear_index(), &model.get_gear_custom_text())?;
    let preform = parse_preform_form(
        model.get_preform_shape_index(),
        &model.get_preform_half_width(),
        &model.get_preform_length_over_width(),
        &model.get_preform_depth(),
        FIXED_CYLINDER_PREFORM_SIDES,
    )?;
    parse_new_design_form(
        gear_teeth,
        preform,
        &model.get_symmetry_order_text(),
        model.get_mirror(),
        model.get_material_index(),
    )
}

/// Create: parses the fields, then replaces the design (after the unsaved-changes guard).
fn create(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let template_index = ui.global::<GalleryModel>().get_selected_index();
    let spec = match spec_from_fields(&ui.global::<NewDesignModel>()) {
        Ok(spec) => spec,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    let c = ctx.clone();
    confirm_discard(ctx, move || install(&c, spec, template_index));
}

/// Installs the new design, once nothing unsaved is at stake.
fn install(ctx: &Ctx, spec: FreshDesignSpec, template_index: i32) {
    let name = template_cards()
        .into_iter()
        .nth(usize::try_from(template_index).unwrap_or(0))
        .map_or_else(|| "Empty".to_string(), |card| card.name);
    // Seeded directly on the fresh design, not through the history: a template is the
    // design's starting state, not an edit.
    let session = EditorSession::from_template(spec, template_index);
    ctx.state.borrow_mut().replace_design(DesignState::new(
        session,
        DesignSource::Template(name.clone()),
    ));
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<GalleryModel>().set_open(false);
        push_all(&ui, &ctx.state.borrow());
    }
    auto_solve(ctx);
    schedule_save(ctx);
    show_message(ctx, MessageKind::Success, &format!("New design: {name}."));
    // The guided walkthrough's first step waits for exactly this event.
    crate::editor::guide::notify(ctx, indicatrix_editor::guide::NEW_DESIGN_CREATED);
}

/// Fills the starting-material list and registers Create.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<NewDesignModel>();
    model.set_material_options(ModelRc::new(VecModel::from(
        builtin_preset_names()
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    let c = ctx.clone();
    model.on_create(move || create(&c));
}
