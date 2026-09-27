//! Rebuilds a design's 3D facet planes and resolves the viewport's material, shared
//! by the local and remote detail-load paths so both can never drift apart.

use crate::{
    AngleItem, MainWindow, TiltModel, ViewportModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor::material_lookup::{MATERIAL_MATCH_TOLERANCE, material_for_refractive_index},
        show_toast,
    },
};
use indicatrix::{
    geometry::{
        cuts::{FacetSpec, StandardGemCuts},
        plane::GpuFacetPlane,
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::material::{BuiltinMaterials, MaterialLookup};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use tracing::info;

/// [`crate::gui::editor::resolve_catalogue_planes`], `None`-ified
/// on any failure -- a resolution failure (no attached `.asc` and no angle-settings
/// row at all) and "the design has no valid anchor" (`Ok(None)`) both mean the same
/// thing to this route: fall back to [`reconstruct_planes`].
///
/// `real_design` is the SAME `Design`'s already-converted
/// planes/gear-teeth/reference-angle the editor's own "Load Selected" would show for
/// `entry_id` (`resolve_catalogue_planes_for_entry`, `None` on the remote route --
/// see that function's own call site), preferred over this module's placeholder-only
/// `reconstruct_planes` guess whenever it resolved to something. See
/// [`planes_gear_and_reference_angle`] for exactly when each path is taken.
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
}

/// Falls back to [`reconstruct_planes`] (gear reference angle `0.0`, matching this
/// route's long-standing "library records carry no reference angle" note) whenever
/// `real_design` is `None` -- [`crate::gui::editor::resolve_catalogue_planes`] itself
/// returned `None`/`Err`: no attached `.asc`/angle-settings row to resolve a `Design`
/// from at all, a resolved design with no valid `ScaleReference` anchor for
/// `Design::planes()` to place its tiers against, or (`apply_design_record_to_ui`'s
/// own call site) the remote route, which has no attached `.asc` bytes to parse yet
/// and so never even calls it.
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
    let facet_specs: Vec<FacetSpec> = angle_items
        .iter()
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
    } = input;
    let (planes, gear_teeth, gear_reference_angle) =
        planes_gear_and_reference_angle(real_design, shape, index_gear, angle_items);

    info!(
        "Reconstructed {} 3D facet planes for diagram #{}",
        planes.len(),
        entry_id
    );

    let mut ctx = RenderContext::lock(render_ctx);
    // A catalogue click must not steal the viewport out from
    // under a design being edited. `claim_active_planes` refuses when the editor
    // owns the planes; say so rather than appearing to work and changing nothing.
    let claimed = ctx.claim_active_planes(
        std::sync::Arc::new(planes),
        Some((gear_teeth, gear_reference_angle)),
        PlanesOwner::Catalogue { entry_id },
    );
    let resolved_material = if claimed {
        let resolved = apply_catalogue_material(&mut ctx, refractive_index, preview_material);
        ctx.dirty = true;
        resolved
    } else {
        None
    };
    drop(ctx);
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
        show_toast(
            ui,
            "The 3D view is showing the design you are editing -- it was left alone. \
             Switch to the Library tab's own view to preview this row.",
            "info",
        );
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

/// Points the viewport's material at whichever built-in this catalogue row's own
/// refractive index names -- the material half of a catalogue selection. Without it the
/// preview would keep whatever the last editor refresh, the last Render Material pick or
/// the PREVIOUS catalogue row left behind, so clicking through the library would change
/// the shape on screen and never the stone.
///
/// Both halves are written, not just the name: `material_override` BEATS `material_name`
/// in `render_thread::context::resolve_material_with_override`, so leaving a stale
/// override in place would make the name below purely decorative.
///
/// A row whose refractive index is absent, unparsable, or within
/// [`MATERIAL_MATCH_TOLERANCE`] of no built-in at all sets `material_unresolved` instead:
/// the viewport then says why it will not trace rather than borrowing some other
/// design's optics.
///
/// `preview_material` -- this design's own persisted preview/tilt-curve
/// material, when one is on file -- is tried FIRST, via [`lookup_named_material`],
/// ahead of the refractive-index nearest-match below. It is a recorded FACT (the exact
/// material the cached thumbnail and tilt curves were actually rendered under), not a
/// ±[`MATERIAL_MATCH_TOLERANCE`] nearest-preset GUESS that can tie-break two different
/// ways in two different code paths (`gui::editor::material_lookup::
/// material_for_refractive_index` here vs. `indicatrix_vault::model::material_match`'s
/// own nearest-with-random-tie rule, which decided what the persisted preview material
/// actually IS) -- preferring the fact closes the "unresolved in the viewport while the
/// thumbnail was rendered as a real preset" seam that let those two rules disagree.
///
/// Returns the material's name on success, so the caller can point the Render Material
/// dropdown at it once the `RenderContext` lock is released; `None` on either refusal,
/// where there is no name to show.
fn apply_catalogue_material(
    ctx: &mut RenderContext,
    refractive_index: Option<&str>,
    preview_material: Option<&str>,
) -> Option<String> {
    if let Some(name) = preview_material.map(str::trim).filter(|s| !s.is_empty())
        && let Some(gem) = lookup_named_material(ctx, name)
    {
        ctx.material_unresolved = None;
        ctx.material_name = name.to_string();
        ctx.material_override = Some(gem);
        return Some(name.to_string());
    }

    let n_d = refractive_index.and_then(|text| text.trim().parse::<f64>().ok());
    let Some(n_d) = n_d.filter(|v| v.is_finite() && *v > 1.0) else {
        ctx.material_unresolved = Some(
            "This design records no usable refractive index, so there is no honest \
             material to render it in. Pick one in the Render Material dropdown above."
                .to_string(),
        );
        return None;
    };
    let Some((name, gem)) = material_for_refractive_index(n_d) else {
        ctx.material_unresolved = Some(format!(
            "This design's refractive index ({n_d:.4}) matches no built-in preset within \
             {MATERIAL_MATCH_TOLERANCE:.2}. Pick a material in the Render Material dropdown \
             above -- rendering it as something else would give you the optics of a \
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
