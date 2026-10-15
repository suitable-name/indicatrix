//! Rebuilds a design's 3D facet planes and resolves the viewport's material, shared
//! by the local and remote detail-load paths so both can never drift apart.

use crate::{
    AngleItem, MainWindow, SettingsModel, TiltModel, ViewportModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        batch::catalogue_material::design_file_material,
        editor::{
            CataloguePlanes,
            material_lookup::{MATERIAL_MATCH_TOLERANCE, material_for_refractive_index},
        },
        is_c_axis_override_available,
        render::stone_effects,
    },
};
use indicatrix::{
    geometry::{
        cuts::{FacetSpec, StandardGemCuts},
        plane::GpuFacetPlane,
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    StoneRenderEffects,
    material::{BuiltinMaterials, MaterialLookup, MaterialSelection},
    native::CustomMaterialSnapshot,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use tracing::info;

/// What a catalogue design's own file says about the stone's material and size, carried
/// from the planes resolution to [`apply_catalogue_material`] so the library detail view
/// draws the stone as the editor's viewport and the exports do
/// (`gui::batch::catalogue_material` is the rule the preview and tilt batches apply to the
/// same three facts).
///
/// The remote route has no design file to read: it passes the [`Default`] -- no selection,
/// no snapshot, no size -- and so keeps the persisted-material and refractive-index rules.
#[derive(Debug, Clone, Default)]
pub(super) struct DesignFileMaterial {
    /// The design file's material selection: the species name, and the refractive-index and
    /// body-color overrides applied on top of it. `None` when there is no design file.
    pub(super) selection: Option<MaterialSelection>,
    /// The design file's own snapshot of a custom material the vault may lack.
    pub(super) custom_snapshot: Option<CustomMaterialSnapshot>,
    /// The design file's girdle diameter in millimetres; `None` is "not set".
    pub(super) girdle_mm: Option<f64>,
}

impl DesignFileMaterial {
    /// The stone width the viewport draws the design at, in millimetres: the file's girdle
    /// diameter, or `0.0` -- "not set", the 7 mm the built-in colors are tuned for.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a girdle diameter in millimetres is a small positive value; f32 is the viewport precision"
    )]
    pub(super) fn stone_width_mm(&self) -> f32 {
        self.girdle_mm
            .filter(|mm| mm.is_finite() && *mm > 0.0)
            .map_or(0.0, |mm| mm as f32)
    }
}

impl From<&CataloguePlanes> for DesignFileMaterial {
    /// The material facts of a resolved catalogue design; the angle-table path has none, so
    /// it converts to the default.
    fn from(resolved: &CataloguePlanes) -> Self {
        Self {
            selection: resolved.material.clone(),
            custom_snapshot: resolved.custom_material.clone(),
            girdle_mm: resolved.girdle_diameter_mm,
        }
    }
}

/// Everything [`apply_reconstructed_planes`] needs for one design.
///
/// `real_design` is [`crate::gui::editor::resolve_catalogue_planes`]'
/// planes/gear-teeth/reference-angle for `entry_id` (`None` on the remote route -- see the
/// call site in `local_load`): the design file's geometry when the record has a usable one,
/// the angle table's otherwise. See [`planes_gear_and_reference_angle`] for exactly when each
/// path is taken. `design_material` is what the same resolution found about the design's
/// material and size ([`DesignFileMaterial`]).
///
/// Bundled into [`ReconstructedPlanesInput`] purely to keep this function's argument
/// count under clippy's `too_many_arguments` limit -- the `real_design` field is what
/// pushed it over.
///
/// This claims the shared plane slot exactly
/// like `gui::editor::view::refresh_viewport` does for an in-editor edit -- if the
/// Tilt Performance dialog is open over a PREVIOUS design's curves when a cutter
/// picks a different catalogue entry, those curves and summary badges would
/// otherwise go on describing geometry that no longer exists. `ui` is only needed
/// for that re-sweep request (`TiltModel.dialog_open`/
/// `invoke_request_tilt_profile_axes`), not for anything else this function does.
pub(super) struct ReconstructedPlanesInput<'a> {
    pub(super) shape: Option<&'a str>,
    pub(super) index_gear: Option<&'a str>,
    pub(super) angle_items: &'a [AngleItem],
    pub(super) refractive_index: Option<&'a str>,
    /// This design's own persisted preview material (`Database::
    /// get_preview_images(entry_id).material` locally, `DesignRecord::preview_material`
    /// remotely) -- already read by both callers into `TiltModel.cached_curve_material`
    /// -- tried by [`apply_catalogue_material`] ahead of the refractive-index guess.
    pub(super) preview_material: Option<&'a str>,
    pub(super) real_design: Option<(Vec<GpuFacetPlane>, u32, f32)>,
    pub(super) design_material: DesignFileMaterial,
}

/// Falls back to [`reconstruct_planes`] (gear reference angle `0.0`, matching this
/// route's long-standing "library records carry no reference angle" note) whenever
/// `real_design` is `None` -- only the remote route (`apply_design_record_to_ui`'s own
/// call site), which has no attached design-file bytes to parse yet and so never
/// calls [`crate::gui::editor::resolve_catalogue_planes`]. The local route always
/// passes that function's result, which applies the same angle-table fallback itself
/// when the record has no usable design file.
pub(super) fn planes_gear_and_reference_angle(
    real_design: Option<(Vec<GpuFacetPlane>, u32, f32)>,
    shape: Option<&str>,
    index_gear: Option<&str>,
    angle_items: &[AngleItem],
) -> (Vec<GpuFacetPlane>, u32, f32) {
    if let Some(resolved) = real_design {
        return resolved;
    }

    // `indicatrix` must not depend on Slint, so convert the Slint-generated `AngleItem`
    // rows into plain `FacetSpec`s at this boundary.
    // A concave tier (a row with a tool line) is no flat facet: reconstructing it as one
    // would draw a facet the stone does not have, so it is left out of the placeholder.
    let facet_specs: Vec<FacetSpec> = angle_items
        .iter()
        .filter(|a| a.second_line.is_empty())
        .map(|a| FacetSpec {
            facet: a.facet.to_string(),
            angle: a.angle.to_string(),
            index: a.index_val.to_string(),
            notes: a.notes.to_string(),
        })
        .collect();
    let planes = reconstruct_planes(shape, index_gear, &facet_specs);
    // Library records carry no reference angle, so the diagram view's index wheel
    // uses the gear tooth count alone (96 when the record has none).
    let gear_teeth: u32 = index_gear
        .and_then(|g| g.trim().parse().ok())
        .filter(|&g| g > 0)
        .unwrap_or(96);
    (planes, gear_teeth, 0.0)
}

/// Shared by the local and remote detail-load paths: rebuilds the 3D viewport's facet
/// planes from a design's shape/gear/angle-settings -- exactly one reconstruction
/// implementation for both sources, so they can never drift apart.
pub(super) fn apply_reconstructed_planes(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    input: ReconstructedPlanesInput<'_>,
) {
    let ReconstructedPlanesInput {
        shape,
        index_gear,
        angle_items,
        refractive_index,
        preview_material,
        real_design,
        design_material,
    } = input;
    let (planes, gear_teeth, gear_reference_angle) =
        planes_gear_and_reference_angle(real_design, shape, index_gear, angle_items);

    info!(
        "Reconstructed {} 3D facet planes for diagram #{}",
        planes.len(),
        entry_id
    );

    let mut ctx = RenderContext::lock(render_ctx);
    // An explicit Library selection takes the viewport, also from a design loaded in
    // the editor: the editor's next load or edit claims it back.
    let claimed = ctx.claim_active_planes(
        std::sync::Arc::new(planes),
        Some((gear_teeth, gear_reference_angle)),
        PlanesOwner::Catalogue { entry_id },
    );
    let (resolved_material, c_axis_available, stone_width_mm) = if claimed {
        let resolved = apply_catalogue_material(
            &mut ctx,
            refractive_index,
            preview_material,
            &design_material,
        );
        // A library design is not editable and carries no stone effects of its own: the
        // Stone tab's effects are view-only session values that start from the defaults
        // for every library row (the editor design's values must not leak onto it). Its
        // stone width is the design file's girdle diameter when the file has one
        // (`apply_catalogue_material` set it, as the editor's viewport and the exports
        // draw the stone), otherwise the default 7 mm. The editor's next load or edit
        // claims the viewport back and its design's own values apply again.
        stone_effects::clear_live_edit_pending();
        stone_effects::apply_effects_to_context(&mut ctx, &StoneRenderEffects::default());
        ctx.dirty = true;
        // The crystal-axis control follows the material this row is traced as.
        let c_axis_available = ctx
            .material_override
            .as_ref()
            .is_some_and(is_c_axis_override_available);
        // The viewport now shows a Library row, drawn whole: Live Render drops its
        // "Cut: ..." pill (the Cut slider belongs to the editor's design).
        crate::gui::editor::push_viewport_owner(ui, ctx.planes_owner);
        (resolved, c_axis_available, ctx.stone_width_mm)
    } else {
        (None, false, 0.0)
    };
    drop(ctx);
    if claimed {
        // Relative-to-stone zones follow the width this row shows, not the
        // previous design's.
        crate::gui::rough_colour::material_zones::stone_width_applied(render_ctx, stone_width_mm);
        stone_effects::push_effects_to_controls(ui, &StoneRenderEffects::default());
        ui.global::<SettingsModel>()
            .set_stone_width_mm(stone_width_mm);
        ui.global::<SettingsModel>()
            .set_c_axis_override_available(c_axis_available);
        // The colour readouts show the colour at this size, and the colour control now
        // targets the view (a library design is never edited).
        ui.global::<ViewportModel>().invoke_render_color_refresh();
    }
    // The dropdown's own displayed selection, not just the material being traced: the
    // two are separate pieces of state, and writing only `RenderContext` left the
    // toolbar reading (say) "Quartz" while the stone on screen was already the newly
    // selected design's sapphire. Same treatment `editor::view::refresh_design_settings`
    // gives its own material sync, via the same helper. `find_option_index` returning
    // `None` leaves the selection alone rather than guessing -- it cannot happen for a
    // built-in (`refresh_material_options` lists every one of them), so that case only
    // covers an options model not pushed yet.
    if let Some(name) = resolved_material {
        let options = ui.global::<ViewportModel>().get_material_options();
        if let Some(index) = crate::gui::startup_settings::find_option_index(&options, &name) {
            ui.global::<ViewportModel>()
                .set_selected_material_index(index);
        }
    }
    if !claimed {
        return;
    }

    // See this function's own doc comment --
    // `AxesCacheKey` (`gui::tilt::tilt_profile`) already hashes the planes, so this
    // is a no-op resweep whenever nothing about them actually moved.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }
}

/// Resolves `name` against the render context's own loaded custom materials first,
/// falling back to the built-in table -- the same custom-over-built-in precedence
/// `gui::editor::material_lookup::EditorMaterialLookup::lookup` already applies (that
/// type isn't reachable from this module -- `pub(super)` to `gui::editor` -- so this is
/// a narrower, local equivalent), kept in step so a persisted preview-material name
/// never resolves differently depending on which of the two lookups happens to see it.
/// Used only by [`apply_catalogue_material`]'s preview-material fast path.
fn lookup_named_material(ctx: &RenderContext, name: &str) -> Option<GemMaterial> {
    ctx.custom_materials
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .cloned()
        .or_else(|| BuiltinMaterials.lookup(name))
}

/// The material `design`'s file names, resolved as the editor's viewport resolves it
/// (`gui::batch::catalogue_material::design_file_material`: the render context's custom
/// materials over the built-ins, the file's refractive-index and body-color overrides on top,
/// the file's own custom-material snapshot when nothing answers to the name), written to
/// both halves of the context's material -- `material_override` BEATS `material_name`.
///
/// `None`, with the context untouched, when the design has no file, the file names no
/// material, or the name resolves nowhere (the viewport would refuse such a design; this view
/// goes on to the persisted material and the refractive-index guess).
fn apply_design_file_material(
    ctx: &mut RenderContext,
    design: &DesignFileMaterial,
) -> Option<String> {
    let selection = design.selection.as_ref()?;
    let (name, material) = design_file_material(
        selection,
        design.custom_snapshot.as_ref(),
        &ctx.custom_materials,
    )?;
    ctx.material_unresolved = None;
    ctx.material_name.clone_from(&name);
    ctx.material_override = Some(material);
    Some(name)
}

/// Points the viewport's material at the stone this catalogue row is -- the material half
/// of a catalogue selection, and the size it is drawn at. Without it the
/// preview would keep whatever the last editor refresh, the last material pick or
/// the PREVIOUS catalogue row left behind, so clicking through the library would change
/// the shape on screen and never the stone.
///
/// The material is the first of these that answers, the order the library batches use
/// for the thumbnails and tilt curves of the same design
/// (`gui::batch::catalogue_material`):
///
/// 1. the design file's own material ([`apply_design_file_material`]) -- what the editor and
///    the exports show;
/// 2. the persisted `preview_material`;
/// 3. the built-in the row's refractive index names.
///
/// The stone width follows the design file as well: `ctx.stone_width_mm` becomes the file's
/// girdle diameter ([`DesignFileMaterial::stone_width_mm`]; `0.0`, the 7 mm the built-in
/// colors are tuned for, when the file has none or there is no file), set whichever of the
/// three answers, so a thumbnail, a tilt curve and this view agree on the stone.
///
/// Both halves of the material are written, not just the name: `material_override` BEATS
/// `material_name` in `render_thread::context::resolve_material_with_override`, so leaving a
/// stale override in place would make the name below purely decorative.
///
/// A row whose refractive index is absent, unparsable, or within
/// [`MATERIAL_MATCH_TOLERANCE`] of no built-in at all sets `material_unresolved` instead:
/// the viewport then says why it will not trace rather than borrowing some other
/// design's optics.
///
/// `preview_material` -- this design's own persisted preview/tilt-curve
/// material, when one is on file -- is tried before the refractive-index nearest-match below,
/// via [`lookup_named_material`]. It is a recorded FACT (the exact
/// material the cached thumbnail and tilt curves were actually rendered under), not a
/// ±[`MATERIAL_MATCH_TOLERANCE`] nearest-preset GUESS that can tie-break two different
/// ways in two different code paths (`gui::editor::material_lookup::
/// material_for_refractive_index` here vs. `indicatrix_vault::model::material_match`'s
/// own nearest-with-random-tie rule, which decided what the persisted preview material
/// actually IS) -- preferring the fact closes the "unresolved in the viewport while the
/// thumbnail was rendered as a real preset" seam that let those two rules disagree.
///
/// Returns the material's name on success, so the caller can point the viewport's material
/// list at it once the `RenderContext` lock is released; `None` on either refusal,
/// where there is no name to show.
pub(super) fn apply_catalogue_material(
    ctx: &mut RenderContext,
    refractive_index: Option<&str>,
    preview_material: Option<&str>,
    design: &DesignFileMaterial,
) -> Option<String> {
    ctx.stone_width_mm = design.stone_width_mm();
    if let Some(name) = apply_design_file_material(ctx, design) {
        return Some(name);
    }
    if let Some(name) = preview_material.map(str::trim).filter(|s| !s.is_empty()) {
        return if let Some(gem) = lookup_named_material(ctx, name) {
            ctx.material_unresolved = None;
            ctx.material_name = name.to_string();
            ctx.material_override = Some(gem);
            Some(name.to_string())
        } else {
            // The row names a SPECIFIC material that resolves to neither a
            // built-in preset nor a saved custom material -- refuse outright
            // (matching `gui::editor::view::inspector::traced_material_for`'s own
            // rule for the same situation) rather than falling through to the
            // RI-based guess below, which would silently trace/tilt-sweep/HUD-score
            // a DIFFERENT stone than the one this row actually names, with no hint
            // why. Clears `material_name`/`material_override` rather than leaving
            // them at whatever the PREVIOUSLY loaded catalogue row set them to --
            // without this, switching from a resolving row to this one kept
            // showing the previous row's material as if it were this row's own
            // honest resolution.
            ctx.material_name.clear();
            ctx.material_override = None;
            ctx.material_unresolved = Some(format!(
                "This design's material '{name}' is not a built-in preset or a saved \
                 custom material, so there is nothing to trace -- pick a material in \
                 the Material list in the Stone tab, or re-save the missing custom \
                 material."
            ));
            None
        };
    }

    let n_d = refractive_index.and_then(|text| text.trim().parse::<f64>().ok());
    let Some(n_d) = n_d.filter(|v| v.is_finite() && *v > 1.0) else {
        // See the comment above for why every refusal in this function clears
        // `material_name`/`material_override` rather than leaving the previous
        // row's.
        ctx.material_name.clear();
        ctx.material_override = None;
        ctx.material_unresolved = Some(
            "This design records no usable refractive index, so there is no honest \
             material to render it in. Pick one in the Material list of the Stone tab."
                .to_string(),
        );
        return None;
    };
    let Some((name, gem)) = material_for_refractive_index(n_d) else {
        ctx.material_name.clear();
        ctx.material_override = None;
        ctx.material_unresolved = Some(format!(
            "This design's refractive index ({n_d:.4}) matches no built-in preset within \
             {MATERIAL_MATCH_TOLERANCE:.2}. Pick a material in the Material list of the \
             Stone tab -- rendering it as something else would give you the optics of a \
             different stone."
        ));
        return None;
    };
    ctx.material_unresolved = None;
    ctx.material_name.clone_from(&name);
    ctx.material_override = Some(gem);
    Some(name)
}

/// Rebuilds a design's 3D facet planes from its shape/gear/angle-settings. Pulled out
/// of [`apply_reconstructed_planes`] (which still owns the actual `RenderContext`
/// write) so `gui::library`'s metadata-fill step for a freshly imported `.asc` --
/// `measure_solid` needs the SAME planes the viewport would show, not a second,
/// possibly-drifting re-parse -- can call exactly this and nothing more. `shape` is
/// `None` at import time (it's the very thing not parsed yet), which is fine: the
/// emerald-cut special case below just doesn't fire, same as it wouldn't for any other
/// design whose `shape` isn't one of those three substrings.
pub fn reconstruct_planes(
    shape: Option<&str>,
    index_gear: Option<&str>,
    facet_specs: &[FacetSpec],
) -> Vec<GpuFacetPlane> {
    let shape_str = shape.unwrap_or_default().to_lowercase();
    let gear_num: u32 = index_gear.unwrap_or_default().parse().unwrap_or(96);

    if shape_str.contains("emerald") || shape_str.contains("baguette") || shape_str.contains("rect")
    {
        StandardGemCuts::emerald_cut()
    } else if !facet_specs.is_empty() {
        StandardGemCuts::from_database_angles(facet_specs, gear_num)
    } else {
        StandardGemCuts::standard_round_brilliant()
    }
}
