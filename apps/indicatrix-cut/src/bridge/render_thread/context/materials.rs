//! Material resolution and per-frame quality derivation: by-name lookup, the
//! opt-in render-time overrides layered on top of it, and the samples-per-frame the
//! render loop derives from the user's target sample count.

use crate::bridge::frame_cache::stone_width::StoneWidthCache;
use glam::Vec3;
use indicatrix::{
    geometry::plane::GpuFacetPlane,
    optics::materials::{GemMaterial, OpticalCharacter},
};

/// Resolves the current gem material by name: custom materials take priority over the
/// built-in presets, falling back to `materials[0]` if `material_name` matches
/// neither. Shared by the live render loop and `export_thread::SceneSnapshot::capture`
/// so both pick a material the same way.
pub fn resolve_material(
    materials: &[GemMaterial],
    custom_materials: &[GemMaterial],
    material_name: &str,
) -> GemMaterial {
    custom_materials
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(material_name))
        .or_else(|| {
            materials
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(material_name))
        })
        .cloned()
        .unwrap_or_else(|| materials[0].clone())
}

/// Prefers `material_override` (see [`RenderContext::material_override`]) over the
/// plain by-name lookup [`resolve_material`] already does. Purely additive: [`resolve_material`]'s own signature and every
/// existing call site are untouched, so a caller that has no override to offer
/// (or hasn't been updated to look one up yet) keeps by-name-only resolution by
/// passing `None`.
///
/// Callers outside `bridge::render_thread` that want a design's real effective
/// material honoured end to end (the tilt sweep, the tilt hover preview, a
/// high-resolution export) should switch their existing `resolve_material(...)`
/// call to `resolve_material_with_override(..., ctx.material_override.as_ref(),
/// ...)`.
#[must_use]
pub fn resolve_material_with_override(
    materials: &[GemMaterial],
    custom_materials: &[GemMaterial],
    material_override: Option<&GemMaterial>,
    material_name: &str,
) -> GemMaterial {
    material_override
        .cloned()
        .unwrap_or_else(|| resolve_material(materials, custom_materials, material_name))
}

/// Every opt-in render-time material override bundled into one struct, to keep call
/// sites' argument lists short. Shared by the live render loop and
/// `export_thread::SceneSnapshot::capture`, which is what keeps a high-resolution
/// export from silently differing from the viewport it was taken from.
#[derive(Clone, Copy)]
pub struct MaterialOverrides {
    /// Inclusion/subsurface scattering: see `RenderContext::inclusion_sigma_s`.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation: see `RenderContext::c_axis_override`.
    pub c_axis_override: Option<Vec3>,
    /// Facet edge rounding: see `RenderContext::edge_rounding_radius`.
    pub edge_rounding_radius: f32,
    /// Physical stone size: see `RenderContext::stone_width_mm`.
    pub stone_width_mm: f32,
}

/// Applies every [`MaterialOverrides`] field on top of a resolved base material. Each
/// one is opt-in and skips its underlying `GemMaterial::with_*` call entirely at its
/// off position, so a material with nothing dialled in renders bit-identical to before
/// these controls existed.
///
/// `active_planes`/`width_cache` are only consulted for `stone_width_mm` -- passed in
/// rather than looked up internally so the live render loop can reuse one persistent
/// `StoneWidthCache` across frames while a one-shot caller can hand in a fresh one.
#[must_use]
pub fn apply_material_overrides(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    active_planes: &[GpuFacetPlane],
    width_cache: &mut StoneWidthCache,
) -> GemMaterial {
    // Opt-in only: skipped entirely (not called with 0.0) at the off position.
    let material = if overrides.inclusion_sigma_s > 0.0 {
        material.with_scattering_amount(overrides.inclusion_sigma_s)
    } else {
        material
    };

    // An isotropic material's optic axis is physically meaningless (no birefringence
    // to orient). The settings-dialog control is disabled for one, but this guard is
    // what stops a leftover override from a previously selected anisotropic material
    // reaching an isotropic one's `c_axis`.
    let mut material = material;
    if let Some(axis) = overrides.c_axis_override
        && material.optical_character != OpticalCharacter::Isotropic
    {
        material.c_axis = axis;
    }

    let material = if overrides.edge_rounding_radius > 0.0 {
        material.with_edge_rounding(overrides.edge_rounding_radius)
    } else {
        material
    };

    // A degenerate/unmeasurable plane arrangement or a non-finite/non-positive scale
    // leaves the material untouched, rather than risking a NaN/negative path-length
    // multiplier reaching the tracer.
    if overrides.stone_width_mm > 0.0
        && let Some(model_width) = width_cache.ensure(active_planes)
        && model_width > 1e-9
    {
        let scale = (f64::from(overrides.stone_width_mm) / model_width) as f32;
        if scale.is_finite() && scale > 0.0 {
            return material.with_absorption_path_scale(scale);
        }
    }
    material
}

/// Everything needed to name the material for a frame: the two tables to look a
/// name up in, the name itself, and the editor's already-resolved override that
/// beats both when it is set.
pub struct MaterialSources<'a> {
    pub materials: &'a [GemMaterial],
    pub custom_materials: &'a [GemMaterial],
    pub material_override: Option<&'a GemMaterial>,
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
    );
    let current_mat = apply_material_overrides(current_mat, overrides, active_planes, width_cache);

    // Samples-per-frame is derived from the target, not chosen directly: the render
    // loop (`render_thread::mod`) sleeps ~16ms per frame regardless of spp, so a large
    // target rendered at a fixed low spp would spend most of its wall-clock time
    // sleeping rather than tracing. Scaling spp with the target keeps that sleep
    // overhead roughly proportional instead of dominating at high targets.
    let spp = (target_samples / 64).clamp(1, 8);

    (current_mat, spp)
}
