//! The Design settings dialog's compact custom-material editor (the desktop's material
//! editor dialog, cut down to the fields that decide the optics -- see
//! `indicatrix_web_core::custom_material` for what is and is not mirrored).
//!
//! "Save and use" builds the material from the typed fields, registers it in the page's
//! catalogue (so the material combos offer it), makes it the design's material as one
//! undoable edit, and keeps its snapshot with the design so the next native save carries it
//! (`[material.custom]`) and a reload of the tab restores it.

use crate::{
    AppWindow, DesignSettingsModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    editor::inspector::finish_no_tier,
};
use indicatrix_cut_core::Edit;
use indicatrix_web_core::custom_material::{CustomMaterialForm, build_custom_material};
use slint::ComponentHandle;

/// Save and use: builds the material from the editor's fields and applies it to the design.
fn save_and_use(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let name = model.get_custom_name();
    let ri = model.get_custom_ri();
    let dispersion = model.get_custom_dispersion();
    let birefringence = model.get_custom_birefringence();
    let specific_gravity = model.get_custom_specific_gravity();
    let built = match build_custom_material(&CustomMaterialForm {
        name: &name,
        ri: &ri,
        dispersion: &dispersion,
        birefringence: &birefringence,
        specific_gravity: &specific_gravity,
        colour_index: model.get_custom_colour_index(),
    }) {
        Ok(built) => built,
        Err(message) => {
            model.set_custom_error(message.as_str().into());
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    model.set_custom_error(slint::SharedString::new());
    let material_name = built.material.name.clone();
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        // Only the NAME (and no RI override, which would shadow the new material's own
        // index) changes; the design's colour override stays as the cutter set it.
        let mut selection = design_state.session.design.material.clone();
        selection.name = Some(material_name.clone());
        selection.refractive_index_override = None;
        let applied = design_state
            .session
            .apply(Edit::SetMaterial {
                material: selection,
            })
            .map(|_| ())
            .map_err(|e| e.to_string());
        if applied.is_ok() {
            design_state.custom_material = Some(built.snapshot);
            app.register_custom_material(built.material)
        } else {
            applied
        }
    };
    match result {
        Ok(()) => {
            finish_no_tier(ctx);
            show_message(
                ctx,
                MessageKind::Success,
                &format!(
                    "'{material_name}' saved and set as this design's material. It is kept \
                     with the design's native file, colour included."
                ),
            );
        }
        Err(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// Registers the editor's callback.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<DesignSettingsModel>();
    let c = ctx.clone();
    model.on_save_custom_material(move || save_and_use(&c));
}
