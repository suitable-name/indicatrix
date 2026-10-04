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
/// `active_planes`/`width_cache` are only consulted for `stone_width_mm`, and only
/// when it's actually on (matching the pure function's own opt-in skip) -- passed in
/// rather than looked up internally so the live render loop can reuse one persistent
/// `StoneWidthCache` across frames while a one-shot caller can hand in a fresh one.
#[must_use]
pub fn apply_material_overrides(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    active_planes: &[GpuFacetPlane],
    width_cache: &mut StoneWidthCache,
    physics_color: bool,
) -> GemMaterial {
    let eff_width =
        indicatrix::render_setup::effective_stone_width_mm(overrides.stone_width_mm, physics_color);
    let model_width = if eff_width > 0.0 {
        width_cache.ensure(active_planes)
    } else {
        None
    };
    render_setup::apply_material_overrides_for_mode(material, overrides, model_width, physics_color)
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
    /// Whether the selected material's color is physics mode (see `RenderContext::physics_color`).
    pub physics_color: bool,
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
    let current_mat = apply_material_overrides(
        current_mat,
        overrides,
        active_planes,
        width_cache,
        sources.physics_color,
    );

    // Samples-per-frame is derived from the target, not chosen directly: every loop
    // iteration pays a fixed cost (dispatch set-up, readback, display hand-off) that does
    // not depend on spp, and the loop additionally pads iterations that show a picture
    // or follow a moving camera up to ~16ms (`render_thread::FRAME_PACE`). A large target
    // traced at a fixed low spp would spend most of its wall-clock time on that fixed
    // cost; scaling spp with the target keeps it roughly proportional instead.
    let spp = (target_samples / 64).clamp(1, 8);

    (current_mat, spp)
}
