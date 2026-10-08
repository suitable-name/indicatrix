//! Material resolution and per-frame quality derivation: by-name lookup, the
//! opt-in render-time overrides layered on top of it, and the samples-per-frame the
//! render loop derives from the user's target sample count.
//!
//! By-name resolution ([`resolve_material`]/[`resolve_material_with_override`]) and
//! the override stack ([`MaterialOverrides`]) now live in
//! `indicatrix::render_setup::materials` (re-exported here so nothing else in this
//! crate changes) so the browser app resolves a design's rendered material exactly
//! the same way for the same settings -- see that module's own doc comment.
//! [`apply_material_overrides`] stays a thin adapter here: the indicatrix version is
//! pure (it takes an already-resolved model width), while this one still owns the
//! `StoneWidthCache` lookup, matching this crate's live render loop, which wants to
//! reuse one persistent cache across frames.

use crate::bridge::frame_cache::stone_width::StoneWidthCache;
pub use indicatrix::render_setup::{
    MaterialOverrides, resolve_material, resolve_material_with_override,
};
use indicatrix::{geometry::plane::GpuFacetPlane, optics::materials::GemMaterial, render_setup};

/// Applies every [`MaterialOverrides`] field on top of a resolved base material -- see
/// `indicatrix::render_setup::apply_material_overrides`'s own doc comment for what
/// each field does and why a material with nothing dialled in renders bit-identical to
/// before these controls existed.
///
/// `active_planes`/`width_cache` are only consulted when the material's absorption unit needs
/// the design's model width (`render_setup::needs_model_width`: a per-millimetre band colour or
/// physics recipe) -- passed in rather than looked up internally so the live render loop can
/// reuse one persistent `StoneWidthCache` across frames while a one-shot caller can hand in a
/// fresh one. A per-model-unit material (the built-ins, legacy colour triples) is sized by
/// `stone_width_mm / 7` alone.
#[must_use]
pub fn apply_material_overrides(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    active_planes: &[GpuFacetPlane],
    width_cache: &mut StoneWidthCache,
) -> GemMaterial {
    let model_width = if render_setup::needs_model_width(&material) {
        width_cache.ensure(active_planes)
    } else {
        None
    };
    render_setup::apply_material_overrides(material, overrides, model_width)
}

/// Everything needed to name the material for a frame: the two tables to look a
/// name up in, the name itself, and the editor's already-resolved override that
/// beats both when it is set.
pub struct MaterialSources<'a> {
    /// Built-in gem materials.
    pub materials: &'a [GemMaterial],
    /// User-defined gem materials.
    pub custom_materials: &'a [GemMaterial],
    /// Explicit material that takes precedence over `material_name`.
    pub material_override: Option<&'a GemMaterial>,
    /// Name of the selected material.
    pub material_name: &'a str,
}

/// Resolves the current gem material (see [`resolve_material_with_override`], which
/// prefers `material_override` over `material_name` when present), applies every user
/// material override on top of it (see [`MaterialOverrides`]/[`apply_material_overrides`]),
/// and derives this frame's samples-per-frame from the user's target sample count.
///
/// Bounce count is not resolved here -- the settings dialog's "Max Ray Bounces"
/// selector is the only thing controlling it; callers use `RenderContext::max_bounces`
/// directly.
///
/// # Panics (in practice, never)
///
/// Falls back to `GemMaterial::diamond()` if neither the override nor the name
/// resolves. Both of this function's call sites (`render_thread::mod`'s frame loop)
/// only reach it while `SuspensionFlags::material_unresolved` is `false`, which
/// `RenderContext::material_unresolved` guarantees means `material_override` is set
/// or `material_name` names a real material -- so the fallback is unreachable, not a
/// silent substitution of the kind [`resolve_material`] itself now refuses to make.
pub(in crate::bridge::render_thread) fn resolve_material_and_quality(
    sources: &MaterialSources<'_>,
    target_samples: u32,
    overrides: &MaterialOverrides,
    active_planes: &[GpuFacetPlane],
    width_cache: &mut StoneWidthCache,
) -> (GemMaterial, u32) {
    let current_mat = resolve_material_with_override(
        sources.materials,
        sources.custom_materials,
        sources.material_override,
        sources.material_name,
    )
    .unwrap_or_else(GemMaterial::diamond);
    let current_mat = apply_material_overrides(current_mat, overrides, active_planes, width_cache);

    // Samples-per-frame is derived from the target, not chosen directly: every loop
    // iteration pays a fixed cost (dispatch set-up, readback, display hand-off) that does
    // not depend on spp, and the loop additionally pads iterations that show a picture
    // or follow a moving camera up to ~16ms (`render_thread::FRAME_PACE`). A large target
    // traced at a fixed low spp would spend most of its wall-clock time on that fixed
    // cost; scaling spp with the target keeps it roughly proportional instead.
    let spp = (target_samples / 64).clamp(1, 8);

    (current_mat, spp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::cuts::StandardGemCuts;

    /// Review fix (absorption units): the material the live loop traces and the HUD/metrics score
    /// (`resolve_material_and_quality`'s first value) is the very material the scoring consumers
    /// build with `render_setup::material_for_stone` for the same stone size -- for a built-in
    /// (`ModelUnit`) and a band colour (`PerMm`, which reads the plane arrangement).
    #[test]
    fn the_live_material_equals_the_scoring_material_for_a_sized_stone() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let builtins = GemMaterial::all_materials();
        let banded = GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0);
        for (name, override_material) in [("Emerald", None), ("Sapphire", Some(&banded))] {
            for stone_width_mm in [0.0_f32, 10.87] {
                let sources = MaterialSources {
                    materials: &builtins,
                    custom_materials: &[],
                    material_override: override_material,
                    material_name: name,
                };
                let overrides = MaterialOverrides {
                    inclusion_sigma_s: 0.0,
                    c_axis_override: None,
                    edge_rounding_radius: 0.0,
                    stone_width_mm,
                };
                let (live, _) = resolve_material_and_quality(
                    &sources,
                    512,
                    &overrides,
                    &planes,
                    &mut StoneWidthCache::new(),
                );
                let bare = resolve_material_with_override(&builtins, &[], override_material, name)
                    .expect("resolves");
                assert_eq!(
                    live,
                    render_setup::material_for_stone(bare, stone_width_mm, &planes),
                    "{name} at {stone_width_mm} mm"
                );
            }
        }
    }
}
