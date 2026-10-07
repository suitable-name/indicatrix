//! The save/delete custom gemstone material callbacks.
//!
//! Split out of `gui::mod` purely to keep that module (already sizeable) from growing
//! further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`.

mod save;

use super::{
    dialog_color::{self, DialogColor},
    dispersion_editor::{prefill_for, push_prefill, setup_dispersion_callbacks},
};
use crate::{
    DispersionEditorModel, MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        optics::crystal_optics::{
            crystal_system_to_index, gem_material_from_row, optical_character_to_index,
        },
        refresh_material_options, show_toast,
        tutorial_events::raise,
    },
};
use indicatrix::optics::chromophore::ChromophoreCatalogue;
use indicatrix_cut_core::{
    material::{ColorMode, absorption_bands_from_json},
    native::dispersion_model_from_json,
};
use indicatrix_editor::guide::viewing_events as events;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};

// The material editor's color helpers; `physics_ui` reaches them through this module.
pub(super) use save::{
    CUSTOM_KEEP_COLOR_INDEX, absorption_rgb_for_color_index, color_index_for_absorption_rgb,
};
use save::{
    CustomMaterialForm, SavedCustomMaterial, apply_custom_material_delete,
    apply_custom_material_save, built_in_name_collision,
};

/// Wires up the save/delete custom gemstone material callbacks. Split out of
/// `run_gui` purely to keep that function under clippy's function-length lint.
/// Answers the material editor's two name-collision questions as the cutter types,
/// against the same sources the authoritative save-time
/// checks use: [`built_in_name_collision`] and the live `custom_materials` list.
///
/// Lives here rather than in Slint because Slint can neither loop over a list inside
/// an expression nor test a string for a substring -- and because a second
/// implementation of "does this name already exist" would be free to drift from the
/// real one. Comparison is case-insensitive, matching `built_in_name_collision`'s
/// own `eq_ignore_ascii_case`, so "quartz" is caught as readily as "Quartz".
fn setup_material_name_probe(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let render_ctx_probe = render_ctx.clone();
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_material_name_edited(move |name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let trimmed = name.trim();
            let collides_builtin = built_in_name_collision(trimmed).is_some();
            let is_existing_custom = {
                let ctx = render_ctx_probe
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ctx.custom_materials
                    .iter()
                    .any(|m| m.name.eq_ignore_ascii_case(trimmed))
            };
            let model = ui.global::<ViewportModel>();
            model.set_material_name_collides_with_builtin(collides_builtin);
            model.set_material_name_is_existing_custom(is_existing_custom);
        });
}

/// Pushes `selected_name`'s own saved values into `ViewportModel.selected_custom_
/// material_*`, the material editor's pre-fill source.
///
/// Reads the ORIGINAL `CustomMaterialRow`, not the converted `GemMaterial` the
/// render context's `custom_materials` list holds: `GemMaterial::new_custom`'s
/// RI/dispersion/absorption-RGB scalar inputs cannot be recovered from the material
/// they expand into (see `crystal_optics::save_gem_material`'s own doc comment) --
/// only the original row still has them. Crystal system/optical character/biaxial
/// delta are read off [`gem_material_from_row`]'s OWN reconstruction rather than the
/// row's raw (possibly-`None`) fields directly, so a legacy row with no explicit
/// crystal-optics columns still pre-fills with `new_custom`'s real inferred values
/// instead of a wrong "Cubic/Isotropic" default.
///
/// Sets `selected_custom_material_valid` false -- leaving every other property at
/// whatever it last was -- for a built-in selection, or a name no longer present
/// (e.g. deleted out from under an open combo): there is nothing honest to pre-fill
/// from either case, and `material_editor_dialog.slint`
/// must gate its own one-time pre-fill assignment on this flag, never assign from a
/// stale previous value while it reads `false`.
fn push_selected_custom_material_fields(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    selected_name: &str,
) {
    let row = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_custom_materials()
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(selected_name));

    let model = ui.global::<ViewportModel>();
    let Some(row) = row else {
        dialog_color::set(DialogColor::default());
        model.set_selected_custom_material_valid(false);
        model.set_selected_material_color_outdated(false);
        // The Dispersion section reads its starting state unconditionally (unlike the
        // optics fields above, which the dialog gates on `selected_custom_material_valid`),
        // so a built-in or vanished selection must not leave the previous material's model.
        push_prefill(ui, &prefill_for(None));
        return;
    };
    let material = gem_material_from_row(&row);
    model.set_selected_custom_material_valid(true);
    model.set_selected_custom_material_ri(row.refractive_index);
    model.set_selected_custom_material_dispersion(row.dispersion);
    model.set_selected_custom_material_birefringence(row.birefringence);
    model.set_selected_custom_material_specific_gravity(row.specific_gravity.unwrap_or(0.0));
    model.set_selected_custom_material_crystal_system_idx(crystal_system_to_index(
        material.crystal_system,
    ));
    model.set_selected_custom_material_optical_character_idx(optical_character_to_index(
        material.optical_character,
    ));
    model.set_selected_custom_material_biaxial_delta_beta_alpha(
        material.biaxial_delta_beta_alpha.unwrap_or(0.0),
    );
    // Reverse lookup against the SAME preset table `apply_custom_material_save`
    // resolves an index into -- `CUSTOM_KEEP_COLOR_INDEX` when the row's color
    // matches none of the nine fixed presets, so `material_editor_dialog.slint`'s
    // `init` can select "Custom (keep)" instead of silently defaulting to index 0
    // ("Clear"), which is what used to flatten a re-saved colored custom material
    // to colorless the moment its RI was merely tweaked (this module's own doc
    // comment).
    // The dialog's swatch row mirrors the FANTASY payload: a physics row's top-level color is
    // only the older-build fallback, so the stored `ColorMode`'s own fantasy color is used.
    let mode = row
        .color_recipe_json
        .as_deref()
        .and_then(ColorMode::from_json);
    let fantasy_rgb = mode.as_ref().map_or(row.absorption_rgb, |m| m.fantasy_rgb);
    model.set_selected_custom_material_color_idx(
        color_index_for_absorption_rgb(fantasy_rgb).unwrap_or(CUSTOM_KEEP_COLOR_INDEX),
    );
    model.set_selected_custom_material_rgb(ModelRc::new(VecModel::from(fantasy_rgb.to_vec())));
    // The L*C*h editor opens on the row's seven-band colour, and a plain re-save keeps it.
    dialog_color::set(DialogColor {
        bands: row
            .absorption_bands_json
            .as_deref()
            .and_then(absorption_bands_from_json),
        triple: Some(fantasy_rgb),
    });
    // The stored text itself, not a re-serialisation of it: the dialog hands it back untouched
    // when nothing about the color changed (see `PhysicsState::mode_json`).
    model.set_selected_custom_material_color_recipe_json(
        row.color_recipe_json
            .as_deref()
            .map(str::trim)
            .filter(|_| mode.is_some())
            .unwrap_or_default()
            .into(),
    );
    // The "color data updated" badge: the recipe predates the catalogue's data version.
    model.set_selected_material_color_outdated(
        mode.as_ref()
            .is_some_and(|m| m.data_outdated(ChromophoreCatalogue::global())),
    );
    // The Dispersion section opens in Coefficients mode on the stored model, or in Simple mode
    // on the row's refractive index and dispersion. An unusable stored model reads as none.
    let stored_model = row
        .dispersion_model_json
        .as_deref()
        .and_then(dispersion_model_from_json);
    push_prefill(ui, &prefill_for(stored_model.as_ref()));
}

/// Wires `ViewportModel::selected_material_changed_for_editor_prefill` (fired by that
/// global's own `changed selected_material_index` handler -- see `viewport.slint`'s
/// doc comment) to [`push_selected_custom_material_fields`], keeping the material
/// editor's pre-fill source fresh as the viewport's material selection changes.
/// `material_editor_dialog.slint` may also invoke the same callback directly right
/// before opening, to force a refresh against whatever is currently selected without
/// requiring the selection to have actually changed since the last one.
fn setup_selected_material_prefill_callback(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_selected_material_changed_for_editor_prefill(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let idx = usize::try_from(ui.global::<ViewportModel>().get_selected_material_index())
                .unwrap_or(0);
            let name = ui
                .global::<ViewportModel>()
                .get_material_options()
                .row_data(idx)
                .unwrap_or_default();
            push_selected_custom_material_fields(&ui, &db, &name);
        });
}

/// The success half of `on_save_custom_material`'s result handling: refreshing the
/// material combo/crystal-axis availability, syncing the Render Material dropdown when
/// the save also applied, and the resulting toast. Split out of
/// [`setup_custom_material_callbacks`] purely to keep that function under clippy's
/// function-length lint.
fn handle_saved_custom_material(ui: &MainWindow, outcome: &SavedCustomMaterial) {
    refresh_material_options(ui, &outcome.custom_list);
    // A plain "Save" leaves the live render's material untouched, so the
    // crystal-axis control (which tracks that selection) is only refreshed when the
    // save also applied.
    if let Some(c_axis_available) = outcome.c_axis_available {
        ui.global::<SettingsModel>()
            .set_c_axis_override_available(c_axis_available);
    }
    if outcome.applied_to_live_render {
        // Same two follow-ups `on_material_changed`
        // (`gui::render::material_quality::setup_material_changed_callback`) does for
        // a hand-picked material: unlink "linked to design" (an "apply" IS the
        // independent choice that property exists to suppress -- left on, the next
        // editor refresh would put the design's own material straight back), and keep
        // the Render Material dropdown's own displayed selection in sync with what
        // `apply_custom_material_save` just wrote into `RenderContext::material_name`
        // -- without this the dropdown kept showing whatever was selected before
        // "Save & Apply", even though the trace itself had already moved on.
        let viewport = ui.global::<ViewportModel>();
        if viewport.get_viewport_material_linked() {
            viewport.set_viewport_material_linked(false);
        }
        if let Some(index) = crate::gui::startup_settings::find_option_index(
            &viewport.get_material_options(),
            &outcome.trimmed_name,
        ) {
            viewport.set_selected_material_index(index);
        }
    }
    // This toast carries no biaxial-vs-GPU caveat: `gpu_supported()` is
    // unconditionally `true` since the `BiaxialIndicatrix` WGSL port (see its own doc
    // comment), so a biaxial custom material renders on the GPU like any other.
    // Re-read the selected material's stored values (and its "color data updated" badge): a
    // save that kept the same selection does not fire the selection-changed handler itself.
    ui.global::<ViewportModel>()
        .invoke_selected_material_changed_for_editor_prefill();
    let verb = if outcome.overwrote_existing {
        "Overwrote"
    } else {
        "Saved"
    };
    let suffix = if outcome.applied_to_live_render {
        " and applied"
    } else {
        ""
    };
    // Confirms the SG the cutter typed actually made it all the way through to the
    // carat-weight estimate's own side channel -- see `outcome.specific_gravity`'s own
    // doc comment for why this is read back rather than echoed from the form.
    let sg_suffix = outcome
        .specific_gravity
        .map(|sg| format!(" (SG {sg:.2})"))
        .unwrap_or_default();
    show_toast(
        ui,
        &format!(
            "{verb}{suffix} custom material '{}'{sg_suffix}",
            outcome.trimmed_name
        ),
        "success",
    );
}

pub(in crate::gui) fn setup_custom_material_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
) {
    setup_material_name_probe(ui, render_ctx);
    setup_selected_material_prefill_callback(ui, db);
    setup_dispersion_callbacks(ui);
    let render_ctx_save = render_ctx.clone();
    let db_save = db.clone();
    let ui_weak_save = ui.as_weak();
    ui.global::<ViewportModel>().on_save_custom_material(
        move |name: SharedString,
              ri: f32,
              disp: f32,
              biref: f32,
              color_idx: i32,
              crystal_system_idx: i32,
              optical_character_idx: i32,
              biaxial_delta_beta_alpha: f32,
              specific_gravity: f32,
              apply_to_live_render: bool,
              color_recipe_json: SharedString| {
            // The Dispersion section's model travels beside this callback's arguments: the
            // dialog sets `DispersionEditorModel.save_json` right before invoking it (a
            // twelfth argument would change the shared `ViewportModel` signature). Read and
            // cleared here, so a stale model can never ride along with a later save.
            let dispersion_model_json = ui_weak_save
                .upgrade()
                .map(|ui| {
                    let section = ui.global::<DispersionEditorModel>();
                    let json = section.get_save_json();
                    section.set_save_json(SharedString::default());
                    json
                })
                .unwrap_or_default();
            // Blank text is the plain two-slider path; anything else is a typed model.
            let typed_coefficients = !dispersion_model_json.trim().is_empty();
            // The actual save (or its validation failure) always runs, whether or
            // not the window handle below still upgrades -- only the toast/UI
            // refresh afterward is conditional on that, matching every other
            // callback in this module.
            let result = apply_custom_material_save(
                &db_save,
                &render_ctx_save,
                CustomMaterialForm {
                    name,
                    ri,
                    disp,
                    biref,
                    color_idx,
                    crystal_system_idx,
                    optical_character_idx,
                    biaxial_delta_beta_alpha,
                    specific_gravity,
                    apply_to_live_render,
                    color_recipe_json,
                    dispersion_model_json,
                    body_bands: dialog_color::current().bands,
                },
            );
            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            match result {
                Ok(outcome) => {
                    handle_saved_custom_material(&ui, &outcome);
                    // A tutorial step may wait for a material to be saved, and one for a
                    // material saved from typed coefficients.
                    raise(&ui, events::CUSTOM_MATERIAL_SAVED);
                    if typed_coefficients {
                        raise(&ui, events::COEFFICIENT_MATERIAL_SAVED);
                    }
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );

    let render_ctx_del = render_ctx.clone();
    let db_del = db.clone();
    let ui_weak_del = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_delete_custom_material(move |name: SharedString| {
            let trimmed = name.trim();
            // See the save callback above for why this always runs regardless of
            // whether the window handle below still upgrades.
            let result = apply_custom_material_delete(&db_del, &render_ctx_del, trimmed);
            let Some(ui) = ui_weak_del.upgrade() else {
                return;
            };
            match result {
                Ok(outcome) => {
                    refresh_material_options(&ui, &outcome.custom_list);
                    ui.global::<SettingsModel>()
                        .set_c_axis_override_available(outcome.c_axis_available);
                    if outcome.existed {
                        show_toast(&ui, &format!("Deleted material '{trimmed}'"), "info");
                    } else {
                        show_toast(
                            &ui,
                            &format!(
                                "'{trimmed}' was not a saved custom material -- nothing to delete."
                            ),
                            "error",
                        );
                    }
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        });

    super::physics_ui::setup_physics_callbacks(ui, render_ctx);
}

#[cfg(test)]
mod tests;
