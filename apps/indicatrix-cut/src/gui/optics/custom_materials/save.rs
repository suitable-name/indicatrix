//! Saving and deleting a custom material: the checks, the database write and the update of the
//! render context's own copy behind the material editor's two buttons. No Slint in here.

use crate::{
    bridge::render_thread::{RenderContext, resolve_material},
    gui::{
        is_c_axis_override_available,
        optics::{
            crystal_optics::{
                MaterialSaveFields, crystal_system_from_index,
                custom_material_specific_gravity_from_rows, is_biaxial,
                optical_character_from_index, save_gem_material_fields,
            },
            dispersion_editor::model_from_save_json,
        },
    },
};
use indicatrix::optics::{
    chromophore::ChromophoreCatalogue,
    fluorescence::Fluorescence,
    materials::{
        GemMaterial,
        body_color::{BODY_COLOR_PRESETS, preset_index_for_rgb},
    },
};
use indicatrix_cut_core::{
    material::{ColorMode, absorption_bands_to_json, with_library_bands},
    native::dispersion_model_to_json,
};
use indicatrix_vault::db::sqlite::Database;
use slint::SharedString;
use std::sync::{Arc, Mutex};

/// `name`'s real built-in [`GemMaterial`] name (`Some`) iff it collides
/// case-insensitively with one -- saving a custom material under a built-in's own
/// name would make `EditorMaterialLookup`/`resolve_material` (both of which prefer a
/// same-named custom material over its built-in twin) silently return the custom
/// optics everywhere the built-in name is selected, so this is checked before ANY
/// save reaches the database.
pub(super) fn built_in_name_collision(name: &str) -> Option<String> {
    GemMaterial::all_materials()
        .into_iter()
        .map(|m| m.name)
        .find(|builtin| builtin.eq_ignore_ascii_case(name))
}

/// The material editor's "Custom (keep)" swatch index -- one past
/// [`BODY_COLOR_PRESETS`]' fixed table. Selected by
/// [`color_index_for_absorption_rgb`] when a saved material's `absorption_rgb`
/// matches none of the nine presets exactly (authored outside the dialog, e.g. by an
/// imported `.asc`/template, or by a preset a future build adds); handled by
/// [`apply_custom_material_save`] as "keep whatever this name's row already has",
/// never as Clear.
pub(in crate::gui::optics) const CUSTOM_KEEP_COLOR_INDEX: i32 = 9;

/// [`BODY_COLOR_PRESETS`]' index -> rgb direction, for [`apply_custom_material_save`].
///
/// `MaterialColorPresets`' (`ui/components/material_editor/color_presets.slint`) nine
/// fixed swatches, index 0 ("Clear") through 8 ("Amber Topaz"), are exactly that table in
/// its own order -- the single source of truth both this function (index -> rgb, at
/// save time) and [`color_index_for_absorption_rgb`] (rgb -> index, the dialog's
/// pre-fill) resolve against, shared with the design's own body-color override.
/// An out-of-range index (defensively -- the dialog's own combo can never actually
/// produce one) falls back to Clear, matching the old bare `match`'s `_` arm.
pub(in crate::gui::optics) fn absorption_rgb_for_color_index(color_idx: i32) -> [f32; 3] {
    usize::try_from(color_idx)
        .ok()
        .and_then(|i| BODY_COLOR_PRESETS.get(i))
        .map_or([0.0, 0.0, 0.0], |preset| preset.absorption_rgb)
}

/// [`BODY_COLOR_PRESETS`]' rgb -> index direction (exact match, via
/// [`preset_index_for_rgb`]), for `push_selected_custom_material_fields`'s pre-fill.
/// `None` when `rgb` matches no preset -- the caller's cue to pre-fill
/// [`CUSTOM_KEEP_COLOR_INDEX`] instead of defaulting to 0 ("Clear"), which is what
/// used to silently flatten a re-saved colored custom material to colorless (see
/// this module's own doc comment).
#[must_use]
pub(in crate::gui::optics) fn color_index_for_absorption_rgb(rgb: [f32; 3]) -> Option<i32> {
    preset_index_for_rgb(rgb).and_then(|i| i32::try_from(i).ok())
}

/// The "Save Custom Material" dialog's raw field values, exactly as the
/// `on_save_custom_material` Slint callback hands them over. Bundled into its
/// own struct purely to keep [`apply_custom_material_save`] under clippy's
/// argument-count lint.
pub(super) struct CustomMaterialForm {
    /// The material name field, not yet trimmed.
    pub(super) name: SharedString,
    /// The refractive index field.
    pub(super) ri: f32,
    /// The dispersion field.
    pub(super) disp: f32,
    /// The birefringence field.
    pub(super) biref: f32,
    /// The swatch-color combo's selected index.
    pub(super) color_idx: i32,
    /// The crystal-system combo's selected index.
    pub(super) crystal_system_idx: i32,
    /// The optical-character combo's selected index.
    pub(super) optical_character_idx: i32,
    /// The biaxial delta/beta/alpha field (meaningful only for the two biaxial
    /// optical characters -- see [`apply_custom_material_save`]'s own body).
    pub(super) biaxial_delta_beta_alpha: f32,
    /// The specific-gravity slider field, `0.0` meaning "not recorded" -- see
    /// [`apply_custom_material_save`]'s own body for how that sentinel becomes
    /// `None` rather than a literal zero-density material.
    pub(super) specific_gravity: f32,
    /// Whether this save should also switch the live render's selected
    /// material, rather than only writing the database/in-memory
    /// custom-material list. `false` lets a cutter create or correct a custom
    /// material without disturbing whatever the viewport is currently showing.
    pub(super) apply_to_live_render: bool,
    /// The serialized physics color recipe or color mode JSON string.
    pub(super) color_recipe_json: SharedString,
    /// The Dispersion section's model as JSON (`DispersionEditorModel.save_json`), blank
    /// for the plain refractive-index-and-dispersion path. A usable model replaces `ri` and
    /// `disp` with its own `n_d` and `n_F - n_C`.
    pub(super) dispersion_model_json: SharedString,
    /// The seven-band body colour of the path-aware L*C*h editor (`gui::optics::dialog_color`),
    /// `None` when the colour came from a preset or the material has none. Ignored while the
    /// physics colour is the active mode (the recipe colours the material then). The legacy
    /// triple is written either way.
    pub(super) body_bands: Option<Vec<[f32; 3]>>,
}

/// A successfully applied "Save Custom Material": everything the callback needs
/// to show the right toast and refresh the UI afterward.
pub(super) struct SavedCustomMaterial {
    /// The trimmed material name that was actually saved/selected.
    pub(super) trimmed_name: String,
    /// Whether this save overwrote an existing custom material of the same
    /// name, rather than adding a new one.
    pub(super) overwrote_existing: bool,
    /// The freshly saved material's crystal-axis override availability. `None`
    /// when the save did not apply to the live render, since availability is
    /// only meaningful for whatever material is currently selected there.
    pub(super) c_axis_available: Option<bool>,
    /// The refreshed custom-material list to push into the material combo.
    pub(super) custom_list: Arc<Vec<GemMaterial>>,
    /// Whether this save also switched the live render's material, for the
    /// toast wording below.
    pub(super) applied_to_live_render: bool,
    /// The specific gravity now on file for this material, read back from
    /// `RenderContext::custom_material_specific_gravity` -- `None` when the
    /// dialog's SG slider was left at its "not recorded" sentinel, in which
    /// case the toast says nothing about it.
    pub(super) specific_gravity: Option<f64>,
}

/// The absorption color a saved material gets: the swatch preset for `color_idx`, or --
/// for [`CUSTOM_KEEP_COLOR_INDEX`] -- whatever the same-named row already stores.
fn resolve_absorption_rgb(db: &Arc<Mutex<Database>>, name: &str, color_idx: i32) -> [f32; 3] {
    if color_idx == CUSTOM_KEEP_COLOR_INDEX {
        // The swatch row was never touched (or "Custom (keep)" was picked
        // deliberately) -- reuse whatever THIS name's previously saved
        // `absorption_rgb` already was, rather than falling through to Clear.
        // This is the fix for the sapphire-turns-colorless bug: the dialog's
        // `init` (`material_editor_dialog.slint`) now pre-fills this index
        // itself whenever a reopened custom material's color matches no
        // fixed preset (see `color_index_for_absorption_rgb`'s own doc
        // comment), so a plain re-save with nothing touched must actually
        // keep it. A name with no existing row (nothing to keep) falls back
        // to Clear -- there is no real "keep" case for a brand-new material.
        db.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_custom_materials()
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
            .map_or([0.0f32, 0.0, 0.0], |r| r.absorption_rgb)
    } else {
        absorption_rgb_for_color_index(color_idx)
    }
}

/// Validates `form` (blank name, or one colliding with a built-in material),
/// saves it to the database, and updates the shared render context's in-memory
/// custom-material list -- everything [`setup_custom_material_callbacks`]'s save
/// callback does except showing the resulting toast, split out purely to keep
/// that closure under clippy's function-length lint. `Err` carries the exact
/// message to toast on a validation or database failure.
#[expect(
    clippy::too_many_lines,
    reason = "one linear validate -> color -> persist -> publish sequence; the steps share the locals"
)]
pub(super) fn apply_custom_material_save(
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
        specific_gravity,
        apply_to_live_render,
        color_recipe_json,
        dispersion_model_json,
        body_bands,
    } = form;
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

    // The Dispersion section's coefficient model, re-checked here whatever the dialog
    // believed: a blank text is the plain refractive-index path, anything else must pass
    // `DispersionModel::validate` or nothing is written. A usable model is the curve the
    // stone is traced with, so the row's `ri` and `disp` become its own `n_d` and
    // `n_F - n_C` (what an older build, or the Simple mode, would show for it) and the
    // stored text is the canonical re-serialisation, not whatever the dialog sent.
    let dispersion_model = model_from_save_json(&dispersion_model_json)?;
    let (ri, disp) = dispersion_model
        .as_ref()
        .map_or((ri, disp), |m| (m.n_d(), m.delta_f_c()));
    let dispersion_model_text = dispersion_model.as_ref().map(dispersion_model_to_json);

    // The dialog hands over the full `ColorMode` JSON whenever the material has (or had) a
    // recipe: both payloads are persisted, and saving while fantasy is active keeps the recipe.
    let input_json = color_recipe_json.trim();
    let parsed = ColorMode::from_json(input_json);
    let mut mode = parsed.clone();
    if let Some(mode) = mode.as_mut() {
        // The dialog's preset row is the fantasy payload, unless it is "Custom (keep)".
        if color_idx != CUSTOM_KEEP_COLOR_INDEX {
            mode.fantasy_rgb = absorption_rgb_for_color_index(color_idx);
        }
    }
    // What an older build reads as `absorption_rgb`: the fantasy triple, or -- while physics is
    // active -- the nearest legacy color of the recipe (spec 6).
    let abs_rgb = mode.as_ref().map_or_else(
        || resolve_absorption_rgb(db, &trimmed, color_idx),
        ColorMode::fallback_rgb,
    );
    // A payload the swatch row left alone is stored exactly as it came: re-serialising it could
    // change the last digit of an amount, and saving without a change must not rewrite the
    // material (the physics editor may be hidden, so nobody asked for a change).
    let color_json = mode.as_ref().map(|m| {
        if parsed.as_ref() == Some(m) {
            input_json.to_string()
        } else {
            m.to_json()
        }
    });
    let color_recipe_opt = color_json.as_deref();
    let is_physics = mode.as_ref().is_some_and(ColorMode::is_physics);
    // The glow of the recipe (empty for fantasy or a recipe without emitters), resolved from the
    // catalogue's elements and concentrations.
    let glow = mode.as_ref().map_or_else(
        || Fluorescence::new(Vec::new()),
        |m| m.fluorescence(ChromophoreCatalogue::global()),
    );

    let mut new_mat = dispersion_model.map_or_else(
        || GemMaterial::new_custom(&trimmed, ri, disp, biref, abs_rgb),
        |model| GemMaterial::new_custom_with_dispersion(&trimmed, model, biref, abs_rgb),
    );
    if let Some(mode) = mode.as_ref().filter(|m| m.is_physics()) {
        // Rendered from the stored resolved bands, never a re-resolve. Per millimetre
        // (`with_chromophore_absorption` tags them so).
        new_mat = new_mat.with_chromophore_absorption(mode.resolve_tensor());
    }
    // The L*C*h editor's bands colour a fantasy material; a physics recipe wins over them.
    let band_rows = body_bands.filter(|rows| !is_physics && !rows.is_empty());
    let bands_json = band_rows.as_deref().and_then(absorption_bands_to_json);
    if let Some(rows) = band_rows.as_deref() {
        new_mat = with_library_bands(new_mat, rows);
    }

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
    // `MutexGuard` `db.lock()` produces is released as soon as this
    // save call returns, not held for the rest of this function -- a
    // significant-`Drop` temporary in an `if let` scrutinee stays alive for
    // the whole arm, which has already caused one real panic in this codebase
    // from a lock held longer than intended.
    // `0.0` (the slider's own "not recorded" sentinel, matching
    // `material_editor_dialog.slint`'s `sg_val` doc comment) is stored as `None`
    // rather than a literal zero-density material.
    let sg = (specific_gravity > 0.0001).then_some(specific_gravity);
    let save_result = save_gem_material_fields(
        &db.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        &new_mat,
        &MaterialSaveFields {
            ri,
            dispersion: disp,
            birefringence: biref,
            absorption_rgb: abs_rgb,
            specific_gravity: sg,
            color_recipe_json: color_recipe_opt,
            dispersion_model_json: dispersion_model_text.as_deref(),
            absorption_bands_json: bands_json.as_deref(),
        },
    );
    if let Err(e) = save_result {
        return Err(format!("Could not save '{trimmed}': {e}"));
    }
    // Only an "apply" save selects the material just saved (see
    // `ctx.material_name` below) -- a plain "Save" writes the database/
    // in-memory list without touching whatever the live render currently
    // shows, so the crystal-axis control's availability (which tracks the
    // live render's selection) is only ever recomputed in that case. Read
    // before `new_mat` moves into `custom_materials` below.
    let c_axis_available = apply_to_live_render.then(|| is_c_axis_override_available(&new_mat));

    // Re-reads every custom material's SG straight from the
    // database (rather than hand-patching the in-memory side channel) so it can
    // never drift from what was actually just written -- the database, not this
    // struct, is the single source of truth for it. Read before locking
    // `render_ctx` below so the `db`/`render_ctx` locks are never held nested.
    let sg_rows = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_custom_materials()
        .unwrap_or_default();

    let mut ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    // Cloned before the move below -- both the `apply_to_live_render` and the
    // "refresh a matching active override" branches further down need a copy of the
    // freshly saved material, and `new_mat` itself is moved into `materials` here.
    let saved_mat = new_mat.clone();
    if let Some(pos) = materials
        .iter()
        .position(|m| m.name.eq_ignore_ascii_case(&trimmed))
    {
        materials[pos] = new_mat;
    } else {
        materials.push(new_mat);
    }
    ctx.custom_material_specific_gravity =
        Arc::new(custom_material_specific_gravity_from_rows(&sg_rows));
    ctx.set_custom_material_physics(&trimmed, is_physics);
    ctx.set_custom_material_fluorescence(&trimmed, glow);
    // Reads the just-saved SG back OUT of the side channel
    // (rather than trusting the locally-computed `sg` above) so the toast confirms
    // the FULL round trip -- dialog -> database -> `RenderContext`'s side channel --
    // actually worked, the same lookup [`crate::gui::editor::material_lookup::
    // EditorMaterialLookup::specific_gravity`] uses for the carat-weight estimate.
    let saved_specific_gravity = ctx.custom_specific_gravity(&trimmed);
    if apply_to_live_render {
        ctx.material_name.clone_from(&trimmed);
        // Mirrors `gui::render::material_quality::setup_material_changed_callback`'s
        // own clearing: `resolve_material_with_override` PREFERS `material_override`
        // over the name just written above, and "linked to design" may have left one
        // in place from the last editor refresh. Without this, "Save & Apply" moved
        // the label and did nothing to the actual trace whenever an override was
        // active -- exactly the bug this fix addresses. `material_unresolved` is
        // cleared for the same reason `on_material_changed` clears it: naming a real
        // material by hand is the way out of a refusal.
        ctx.material_override = None;
        ctx.material_unresolved = None;
        ctx.dirty = true;
    } else if ctx
        .material_override
        .as_ref()
        .is_some_and(|m| m.name.eq_ignore_ascii_case(&trimmed))
    {
        // A plain "Save" (not "Save & Apply") must still not leave the live render
        // tracing STALE optics when the material it is CURRENTLY overridden to is the
        // one just edited -- refresh the override in place rather than silently
        // requiring a second, separate "Save & Apply" to see the new numbers reflected.
        ctx.material_override = Some(saved_mat);
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
        specific_gravity: saved_specific_gravity,
    })
}

/// A successfully applied "Delete Custom Material": everything the callback
/// needs to show the right toast and refresh the UI afterward.
pub(super) struct DeletedCustomMaterial {
    /// Whether a custom material of that name actually existed to delete.
    pub(super) existed: bool,
    /// The crystal-axis override availability for whatever `material_name`
    /// ends up being (unchanged if a DIFFERENT material was deleted, or
    /// "Diamond" if the just-deleted one was the selected one).
    pub(super) c_axis_available: bool,
    /// The refreshed custom-material list to push into the material combo.
    pub(super) custom_list: Arc<Vec<GemMaterial>>,
}

/// Deletes the (already-trimmed) `name` from the database and the shared
/// render context's in-memory custom-material list -- everything
/// [`setup_custom_material_callbacks`]'s delete callback does except showing
/// the resulting toast, split out for the same reason
/// [`apply_custom_material_save`] is. `Err` carries the exact message to toast
/// on a blank name or database failure.
pub(super) fn apply_custom_material_delete(
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
    let delete_result = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .delete_custom_material(name);
    if let Err(e) = delete_result {
        return Err(format!("Could not delete '{name}': {e}"));
    }
    // Re-reads every remaining custom material's SG straight
    // from the database -- see `apply_custom_material_save`'s matching comment for
    // why this re-derives rather than hand-patches the in-memory side channel.
    // Read before locking `render_ctx` below so the `db`/`render_ctx` locks are
    // never held nested.
    let sg_rows = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_custom_materials()
        .unwrap_or_default();

    let mut ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    ctx.set_custom_material_physics(name, false);
    ctx.set_custom_material_fluorescence(name, Fluorescence::new(Vec::new()));
    ctx.custom_material_specific_gravity =
        Arc::new(custom_material_specific_gravity_from_rows(&sg_rows));
    // The deleted material can be the live selection either by
    // `material_name` OR by `material_override` (which `resolve_material_with_override`
    // prefers over the name -- see `render::material_quality::setup_material_changed_callback`'s
    // matching comment) -- checked here the same case-insensitive way, so a delete
    // never leaves `material_override` pointing at a `GemMaterial` the custom-material
    // list no longer has, which every downstream reader (viewport, HUD, tilt sweep,
    // hover preview, export) would otherwise keep rendering.
    let override_matches_deleted = ctx
        .material_override
        .as_ref()
        .is_some_and(|m| m.name.eq_ignore_ascii_case(name));
    if ctx.material_name.eq_ignore_ascii_case(name) || override_matches_deleted {
        ctx.material_name = "Diamond".to_string();
        ctx.material_override = None;
        ctx.material_unresolved = None;
    }
    ctx.dirty = true;
    // Deleting the currently selected custom material falls back to
    // "Diamond" above -- re-derive availability for whatever `material_name` ends
    // up being (unchanged if a DIFFERENT material was deleted). `resolve_material`
    // is `Option` (no `materials[0]` fallback, see its own doc comment); `ctx.
    // material_name` should always resolve here (either unchanged from a moment
    // ago, or just reset to the real "Diamond" above), but `None` still falls back
    // to `false` rather than panicking on a bare `.unwrap()`.
    let c_axis_available = resolve_material(
        &GemMaterial::all_materials(),
        &ctx.custom_materials,
        &ctx.material_name,
    )
    .as_ref()
    .is_some_and(is_c_axis_override_available);

    let custom_list = ctx.custom_materials.clone();
    drop(ctx);

    Ok(DeletedCustomMaterial {
        existed,
        c_axis_available,
        custom_list,
    })
}
