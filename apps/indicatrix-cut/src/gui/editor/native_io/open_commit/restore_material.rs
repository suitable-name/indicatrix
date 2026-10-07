//! Restoring the custom material a design file carries a snapshot of.

use crate::{
    MainWindow, PhysicscolorModel,
    bridge::render_thread::RenderContext,
    gui::optics::{physics_state::PHYSICS_COLOR_UI, physics_ui::keep_pending_recipe},
};
use indicatrix::optics::{
    chromophore::ChromophoreCatalogue, fluorescence::Fluorescence, materials::GemMaterial,
};
use indicatrix_cut_core::{
    material::ColorMode,
    native::{
        CustomMaterialSnapshot, MaterialResolution, SnapshotColor,
        gem_material_from_custom_snapshot, gem_material_from_custom_snapshot_keeping_recipe,
        snapshot_color,
    },
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// A custom material the opened file carried a snapshot for is registered in this
/// session's custom-material list (replacing a same-named entry), so the design does
/// not silently render as Diamond. Returns the note for the open toast, if any, and
/// whether the material is still unresolved (named, but neither built in nor
/// restorable).
pub(super) fn restore_custom_material(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    snapshot: Option<&CustomMaterialSnapshot>,
    material_name: Option<&str>,
    resolution: MaterialResolution,
) -> (Option<String>, bool) {
    if let (Some(snapshot), Some(name)) = (snapshot, material_name) {
        // A physics recipe renders from its stored `resolved_bands`. When an older build edited
        // the top-level color since (it differs from the fallback the file recorded) the file
        // counts as fantasy-edited: the edited color is restored now and the user is asked
        // whether to keep the recipe instead.
        let color = snapshot_color(snapshot);
        let gem: GemMaterial = gem_material_from_custom_snapshot(name, snapshot);
        let edited_elsewhere = matches!(color, SnapshotColor::EditedElsewhere(_));
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let materials = Arc::make_mut(&mut ctx.custom_materials);
        if let Some(pos) = materials
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
        {
            materials[pos] = gem;
        } else {
            materials.push(gem);
        }
        ctx.set_custom_material_physics(name, matches!(color, SnapshotColor::Physics(_)));
        let catalogue = ChromophoreCatalogue::global();
        let glow = |mode: &ColorMode| mode.fluorescence(catalogue);
        ctx.set_custom_material_fluorescence(
            name,
            match &color {
                SnapshotColor::Physics(mode) => glow(mode),
                _ => Fluorescence::new(Vec::new()),
            },
        );
        ctx.pending_color_choice = match &color {
            SnapshotColor::EditedElsewhere(mode) => Some((
                name.to_string(),
                gem_material_from_custom_snapshot_keeping_recipe(name, snapshot),
                glow(mode),
            )),
            _ => None,
        };
        // Without the physics color editor in this build there is nobody to ask: the recipe
        // the file was saved with stays in use, silently apart from the toast note below. The
        // file itself is not touched.
        let keeps_recipe_silently = edited_elsewhere && !PHYSICS_COLOR_UI;
        if keeps_recipe_silently {
            keep_pending_recipe(&mut ctx);
            tracing::info!(
                "Material '{name}': an older version edited its color after the physics recipe \
                 was saved; the saved recipe is kept (this build has no physics color editor)"
            );
        }
        drop(ctx);
        let mut note = format!(" '{name}' was restored from this file's own saved material data.");
        if keeps_recipe_silently {
            note.push_str(
                " Its color was edited by an older version after the original was saved; \
                 the original color is used.",
            );
        } else if edited_elsewhere {
            note.push_str(
                " Its color was changed by an older version since the physics recipe was saved.",
            );
            ui.global::<PhysicscolorModel>()
                .set_color_conflict_name(name.into());
        }
        return (Some(note), false);
    }
    // A material name this build can't resolve AND has no snapshot to restore from
    // still silently becomes Diamond once `MaterialSelection::resolve` runs -- see
    // `MaterialResolution`'s own doc comment. Surfaced rather than swallowed, so at
    // least the open toast says so.
    let unresolved = matches!(resolution, MaterialResolution::Unresolved);
    (unresolved.then(|| format!(" {resolution}")), unresolved)
}
