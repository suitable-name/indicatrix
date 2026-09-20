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

/// `name`'s real built-in [`GemMaterial`] name (`Some`) iff it collides
/// case-insensitively with one -- saving a custom material under a built-in's own
/// name would make `EditorMaterialLookup`/`resolve_material` (both of which prefer a
/// same-named custom material over its built-in twin) silently return the custom
/// optics everywhere the built-in name is selected, so this is checked before ANY
/// save reaches the database.
fn built_in_name_collision(name: &str) -> Option<String> {
    GemMaterial::all_materials()
        .into_iter()
        .map(|m| m.name)
        .find(|builtin| builtin.eq_ignore_ascii_case(name))
}

/// The "Save Custom Material" dialog's raw field values, exactly as the
/// `on_save_custom_material` Slint callback hands them over. Bundled into its
/// own struct purely to keep [`apply_custom_material_save`] under clippy's
/// argument-count lint.
struct CustomMaterialForm {
    /// The material name field, not yet trimmed.
    name: SharedString,
    /// The refractive index field.
    ri: f32,
    /// The dispersion field.
    disp: f32,
    /// The birefringence field.
    biref: f32,
    /// The swatch-color combo's selected index.
    color_idx: i32,
    /// The crystal-system combo's selected index.
    crystal_system_idx: i32,
    /// The optical-character combo's selected index.
    optical_character_idx: i32,
    /// The biaxial delta/beta/alpha field (meaningful only for the two biaxial
    /// optical characters -- see [`apply_custom_material_save`]'s own body).
    biaxial_delta_beta_alpha: f32,
    /// Item 243: whether this save should also switch the live render's
    /// selected material, rather than only writing the database/in-memory
    /// custom-material list. `false` lets a cutter create or correct a custom
    /// material without disturbing whatever the viewport is currently showing.
    apply_to_live_render: bool,
}

/// A successfully applied "Save Custom Material": everything the callback needs
/// to show the right toast and refresh the UI afterward.
struct SavedCustomMaterial {
    /// The trimmed material name that was actually saved/selected.
    trimmed_name: String,
    /// Whether this save overwrote an existing custom material of the same
    /// name, rather than adding a new one.
    overwrote_existing: bool,
    /// The freshly saved material's crystal-axis override availability. `None`
    /// when the save did not apply to the live render, since availability is
    /// only meaningful for whatever material is currently selected there.
    c_axis_available: Option<bool>,
    /// The refreshed custom-material list to push into the material combo.
    custom_list: Arc<Vec<GemMaterial>>,
    /// Item 243: whether this save also switched the live render's material,
    /// for the toast wording below.
    applied_to_live_render: bool,
}

/// Validates `form` (blank name, or one colliding with a built-in material),
/// saves it to the database, and updates the shared render context's in-memory
/// custom-material list -- everything [`setup_custom_material_callbacks`]'s save
/// callback does except showing the resulting toast, split out purely to keep
/// that closure under clippy's function-length lint. `Err` carries the exact
/// message to toast on a validation or database failure.
fn apply_custom_material_save(
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    form: CustomMaterialForm,
) -> Result<SavedCustomMaterial, String> {
    let CustomMaterialForm {
        name,
        ri,
        disp,
        biref,
        color_idx,
        crystal_system_idx,
        optical_character_idx,
        biaxial_delta_beta_alpha,
        apply_to_live_render,
    } = form;
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
    // A blank/whitespace-only name, or one that collides with a built-in
    // material, is refused outright here -- the dialog's own `can_save`
    // property only catches the exact-empty case (see
    // `material_editor_dialog.slint`'s own doc comment), so this Rust-side
    // check is the authoritative one, and the only one that knows the full
    // built-in material list.
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("A material name is required.".to_string());
    }
    if let Some(builtin) = built_in_name_collision(&trimmed) {
        return Err(format!(
            "'{trimmed}' is a built-in material name -- saving a custom \
             material under it would silently shadow '{builtin}' everywhere \
             this design's material is used. Rename it first."
        ));
    }

    let mut new_mat = GemMaterial::new_custom(&trimmed, ri, disp, biref, abs_rgb);
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

    // Bound to a `let` first (rather than in the `if let` below) so the
    // `MutexGuard` `db.lock().unwrap()` produces is released as soon as this
    // save call returns, not held for the rest of this function -- a
    // significant-`Drop` temporary in an `if let` scrutinee stays alive for
    // the whole arm, which has already caused one real panic in this codebase
    // from a lock held longer than intended.
    // CAD audit item 169: no specific gravity reaches here yet -- `CustomMaterialForm`
    // has no `sg` field because `ViewportModel.save_custom_material` (Slint, owned
    // outside `gui/optics`) does not pass one. See `save_gem_material`'s own doc
    // comment for the exact one-line change this becomes once that lands.
    let save_result = save_gem_material(
        &db.lock().unwrap(),
        &new_mat,
        ri,
        disp,
        biref,
        abs_rgb,
        None,
    );
    if let Err(e) = save_result {
        return Err(format!("Could not save '{trimmed}': {e}"));
    }
    // Item 243: only an "apply" save selects the material just saved (see
    // `ctx.material_name` below) -- a plain "Save" writes the database/
    // in-memory list without touching whatever the live render currently
    // shows, so the crystal-axis control's availability (which tracks the
    // live render's selection) is only ever recomputed in that case. Read
    // before `new_mat` moves into `custom_materials` below.
    let c_axis_available = apply_to_live_render.then(|| is_c_axis_override_available(&new_mat));

    let mut ctx = render_ctx.lock().unwrap();
    // `Arc::make_mut`: clones the underlying `Vec` only if some other snapshot
    // (e.g. a render-thread `FrameInputs` snapshot mid-flight) still holds a
    // reference to it; mutates in place otherwise. Keeps the common case (no
    // outstanding snapshot) as cheap as the old bare `Vec`.
    let materials = Arc::make_mut(&mut ctx.custom_materials);
    // Whether this save overwrites an existing custom material of the same
    // name -- surfaced in the toast below rather than a single, silent
    // "Saved" wording for both cases (see this module's own doc comment).
    let overwrote_existing = materials
        .iter()
        .any(|m| m.name.eq_ignore_ascii_case(&trimmed));
    if let Some(pos) = materials
        .iter()
        .position(|m| m.name.eq_ignore_ascii_case(&trimmed))
    {
        materials[pos] = new_mat;
    } else {
        materials.push(new_mat);
    }
    if apply_to_live_render {
        ctx.material_name.clone_from(&trimmed);
        ctx.dirty = true;
    }

    let custom_list = ctx.custom_materials.clone();
    drop(ctx);

    Ok(SavedCustomMaterial {
        trimmed_name: trimmed,
        overwrote_existing,
        c_axis_available,
        custom_list,
        applied_to_live_render: apply_to_live_render,
    })
}

/// A successfully applied "Delete Custom Material": everything the callback
/// needs to show the right toast and refresh the UI afterward.
struct DeletedCustomMaterial {
    /// Whether a custom material of that name actually existed to delete.
    existed: bool,
    /// The crystal-axis override availability for whatever `material_name`
    /// ends up being (unchanged if a DIFFERENT material was deleted, or
    /// "Diamond" if the just-deleted one was the selected one).
    c_axis_available: bool,
    /// The refreshed custom-material list to push into the material combo.
    custom_list: Arc<Vec<GemMaterial>>,
}

/// Deletes the (already-trimmed) `name` from the database and the shared
/// render context's in-memory custom-material list -- everything
/// [`setup_custom_material_callbacks`]'s delete callback does except showing
/// the resulting toast, split out for the same reason
/// [`apply_custom_material_save`] is. `Err` carries the exact message to toast
/// on a blank name or database failure.
fn apply_custom_material_delete(
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    name: &str,
) -> Result<DeletedCustomMaterial, String> {
    if name.is_empty() {
        return Err("No material name given to delete.".to_string());
    }

    // Bound to a `let` first -- see [`apply_custom_material_save`]'s matching
    // comment for why a significant-`Drop` `MutexGuard` must never sit in an
    // `if let` scrutinee.
    let delete_result = db.lock().unwrap().delete_custom_material(name);
    if let Err(e) = delete_result {
        return Err(format!("Could not delete '{name}': {e}"));
    }

    let mut ctx = render_ctx.lock().unwrap();
    // See `apply_custom_material_save`'s matching comment for why `Arc::make_mut`
    // rather than a bare in-place mutation. `delete_custom_material` itself
    // reports no affected row count (a nonexistent name deletes zero rows
    // without erroring, per its own doc comment) -- checked here instead,
    // against the in-memory list this app already keeps in lock-step with the
    // database, so the toast below can say whether anything was actually
    // removed.
    let materials = Arc::make_mut(&mut ctx.custom_materials);
    let existed = materials.iter().any(|m| m.name.eq_ignore_ascii_case(name));
    materials.retain(|m| !m.name.eq_ignore_ascii_case(name));
    if ctx.material_name.eq_ignore_ascii_case(name) {
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

    Ok(DeletedCustomMaterial {
        existed,
        c_axis_available,
        custom_list,
    })
}

/// Wires up the save/delete custom gemstone material callbacks. Split out of
/// `run_gui` purely to keep that function under clippy's function-length lint.
/// Answers the material editor's two name-collision questions (CAD audit items
/// 87/88) as the cutter types, against the same sources the authoritative save-time
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

pub(in crate::gui) fn setup_custom_material_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
) {
    setup_material_name_probe(ui, render_ctx);
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
              apply_to_live_render: bool| {
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
                    apply_to_live_render,
                },
            );
            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            match result {
                Ok(outcome) => {
                    refresh_material_options(&ui, &outcome.custom_list);
                    // Item 243: a plain "Save" leaves the live render's material
                    // untouched, so the crystal-axis control (which tracks that
                    // selection) is only refreshed when the save also applied.
                    if let Some(c_axis_available) = outcome.c_axis_available {
                        ui.global::<SettingsModel>()
                            .set_c_axis_override_available(c_axis_available);
                    }
                    // This toast used to append a "(biaxial: CPU-only render, no GPU
                    // acceleration)" variant, branching on `GemMaterial::gpu_supported()`.
                    // That branch was both dead and wrong: `gpu_supported()` is
                    // unconditionally `true` since the `BiaxialIndicatrix` WGSL port (see
                    // its own doc comment), so a biaxial custom material renders on the GPU
                    // like any other and the warning could only ever mislead.
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
                    show_toast(
                        &ui,
                        &format!("{verb}{suffix} custom material '{}'", outcome.trimmed_name),
                        "success",
                    );
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
}
