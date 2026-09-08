//! The save/delete custom gemstone material callbacks.
//!
//! Split out of `gui::mod` purely to keep that module (already sizeable) from growing
//! further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`.

use crate::{
    MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::{RenderContext, resolve_material},
    gui::{
        is_c_axis_override_available,
        optics::crystal_optics::{
            crystal_system_from_index, is_biaxial, optical_character_from_index, save_gem_material,
        },
        refresh_material_options, show_toast,
    },
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

/// Wires up the save/delete custom gemstone material callbacks. Split out of
/// `run_gui` purely to keep that function under clippy's function-length lint.
pub(in crate::gui) fn setup_custom_material_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
) {
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
              biaxial_delta_beta_alpha: f32| {
            let abs_rgb = match color_idx {
                1 => [2.8f32, 1.2, 0.1], // Sapphire Blue
                2 => [0.1f32, 2.5, 2.2], // Ruby Red
                3 => [2.2f32, 0.2, 2.0], // Emerald Green
                4 => [1.8f32, 1.6, 0.2], // Tanzanite Violet
                5 => [0.2f32, 0.4, 2.8], // Canary Yellow
                6 => [0.4f32, 2.2, 1.6], // Pink Spinel
                7 => [0.2f32, 0.6, 1.8], // Teal / Zircon
                8 => [1.2f32, 0.4, 0.1], // Amber Topaz
                _ => [0.0f32, 0.0, 0.0], // Clear
            };
            let mut new_mat = GemMaterial::new_custom(&name, ri, disp, biref, abs_rgb);
            // The dialog always sends a definite combo selection (its own
            // defaults already mirror what `new_custom` above would infer -- see
            // `material_editor_dialog.slint`'s initial property values), so an
            // in-range index always overrides here; only a defensively-out-of-range
            // index (which the combo itself can never actually produce) leaves
            // `new_custom`'s own inference in place.
            if let Some(cs) = crystal_system_from_index(crystal_system_idx) {
                new_mat.crystal_system = cs;
            }
            if let Some(oc) = optical_character_from_index(optical_character_idx) {
                new_mat.optical_character = oc;
            }
            // `biaxial_delta_beta_alpha` only means anything for the two biaxial
            // variants (see that field's own doc comment on `GemMaterial`) -- storing
            // it unconditionally would leave a stale nonzero value on a material the
            // user switched back to uniaxial/isotropic.
            new_mat.biaxial_delta_beta_alpha =
                is_biaxial(new_mat.optical_character).then_some(biaxial_delta_beta_alpha);

            let _ = save_gem_material(&db_save.lock().unwrap(), &new_mat, ri, disp, biref, abs_rgb);
            // Saving always selects the material just saved (see
            // `ctx.material_name` below), so its own freshly-built `new_mat` is exactly
            // what the crystal-axis control's availability must now reflect -- no need
            // to re-resolve. Read before `new_mat` moves into `custom_materials` below.
            let c_axis_available = is_c_axis_override_available(&new_mat);

            let mut ctx = render_ctx_save.lock().unwrap();
            // `Arc::make_mut`: clones the underlying `Vec` only if some other snapshot
            // (e.g. a render-thread `FrameInputs` snapshot mid-flight) still holds a
            // reference to it; mutates in place otherwise. Keeps the common case (no
            // outstanding snapshot) as cheap as the old bare `Vec`.
            let materials = Arc::make_mut(&mut ctx.custom_materials);
            if let Some(pos) = materials
                .iter()
                .position(|m| m.name.eq_ignore_ascii_case(&name))
            {
                materials[pos] = new_mat;
            } else {
                materials.push(new_mat);
            }
            ctx.material_name = name.to_string();
            ctx.dirty = true;

            let custom_list = ctx.custom_materials.clone();
            drop(ctx);

            if let Some(ui) = ui_weak_save.upgrade() {
                refresh_material_options(&ui, &custom_list);
                ui.global::<SettingsModel>()
                    .set_c_axis_override_available(c_axis_available);
                // This toast used to append a "(biaxial: CPU-only render, no GPU
                // acceleration)" variant, branching on `GemMaterial::gpu_supported()`.
                // That branch was both dead and wrong: `gpu_supported()` is
                // unconditionally `true` since the `BiaxialIndicatrix` WGSL port (see
                // its own doc comment), so a biaxial custom material renders on the GPU
                // like any other and the warning could only ever mislead.
                show_toast(
                    &ui,
                    &format!("Saved and applied custom material '{name}'"),
                    "success",
                );
            }
        },
    );

    let render_ctx_del = render_ctx.clone();
    let db_del = db.clone();
    let ui_weak_del = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_delete_custom_material(move |name: SharedString| {
            let _ = db_del.lock().unwrap().delete_custom_material(&name);

            let mut ctx = render_ctx_del.lock().unwrap();
            // See the save callback above for why `Arc::make_mut` rather than a bare
            // in-place mutation.
            Arc::make_mut(&mut ctx.custom_materials)
                .retain(|m| !m.name.eq_ignore_ascii_case(&name));
            if ctx.material_name.eq_ignore_ascii_case(&name) {
                ctx.material_name = "Diamond".to_string();
            }
            ctx.dirty = true;
            // Deleting the currently selected custom material falls back to
            // "Diamond" above -- re-derive availability for whatever `material_name` ends
            // up being (unchanged if a DIFFERENT material was deleted).
            let c_axis_available = is_c_axis_override_available(&resolve_material(
                &GemMaterial::all_materials(),
                &ctx.custom_materials,
                &ctx.material_name,
            ));

            let custom_list = ctx.custom_materials.clone();
            drop(ctx);

            if let Some(ui) = ui_weak_del.upgrade() {
                refresh_material_options(&ui, &custom_list);
                ui.global::<SettingsModel>()
                    .set_c_axis_override_available(c_axis_available);
                show_toast(&ui, &format!("Deleted material '{name}'"), "info");
            }
        });
}
