//! Material resolution and the render-time override stack.
//!
//! By-name lookup and every opt-in override layered on top of it
//! (inclusion/subsurface scattering, crystal-axis orientation, facet edge rounding,
//! physical stone size). Moved out of the desktop viewer unchanged so the browser app
//! resolves a design's rendered material exactly the same way for the same settings --
//! see `crate::render_setup`'s own doc comment.

use crate::optics::materials::{GemMaterial, OpticalCharacter};
use glam::Vec3;

/// Resolves the current gem material by name: custom materials take priority over the
/// built-in presets.
///
/// Returns `None` when `material_name` matches neither table -- deliberately no
/// `materials[0]` (Diamond) fallback, so a caller cannot silently
/// trace/export/tilt-sweep an unrecognized or unset name as a different stone.
#[must_use]
pub fn resolve_material(
    materials: &[GemMaterial],
    custom_materials: &[GemMaterial],
    material_name: &str,
) -> Option<GemMaterial> {
    custom_materials
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(material_name))
        .or_else(|| {
            materials
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(material_name))
        })
        .cloned()
}

/// Prefers `material_override` over the plain by-name lookup [`resolve_material`]
/// already does.
///
/// Like `resolve_material` -- returns `None` rather than substituting a different
/// stone when neither the override nor the name resolves.
#[must_use]
pub fn resolve_material_with_override(
    materials: &[GemMaterial],
    custom_materials: &[GemMaterial],
    material_override: Option<&GemMaterial>,
    material_name: &str,
) -> Option<GemMaterial> {
    material_override
        .cloned()
        .or_else(|| resolve_material(materials, custom_materials, material_name))
}

/// Every opt-in render-time material override bundled into one struct, to keep call
/// sites' argument lists short.
#[derive(Clone, Copy)]
pub struct MaterialOverrides {
    /// Inclusion/subsurface scattering: the Henyey-Greenstein `sigma_s` applied via
    /// `GemMaterial::with_scattering_amount`. `0.0` is off.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation: `Some(axis)` replaces the resolved material's
    /// `c_axis` (skipped for isotropic materials); `None` ("as cut") leaves it
    /// untouched.
    pub c_axis_override: Option<Vec3>,
    /// Facet edge (meet-point) rounding radius, via `GemMaterial::with_edge_rounding`.
    /// `0.0` is off (sharp edges).
    pub edge_rounding_radius: f32,
    /// Physical stone size: girdle width in millimetres for absorption/scattering
    /// scale. `0.0` is off.
    pub stone_width_mm: f32,
}

/// Applies every [`MaterialOverrides`] field on top of a resolved base material.
///
/// Each one is opt-in and skips its underlying `GemMaterial::with_*` call entirely at
/// its off position, so a material with nothing dialled in renders bit-identical to
/// before these controls existed. Equivalent to [`apply_material_overrides_for_mode`] with
/// `physics_color = false` (no 7 mm default stone width).
///
/// `model_width` is the active design's own girdle width in model units, already
/// resolved by the caller (see [`crate::render_setup::measure_model_width`]) -- this
/// function does no plane-arrangement measurement of its own, so a caller with a
/// persistent cache keyed on the design's geometry (the desktop's `StoneWidthCache`)
/// only pays for a remeasure when the design actually changes, and a one-shot caller
/// can just measure fresh and pass the result straight through.
#[must_use]
pub fn apply_material_overrides(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    model_width: Option<f64>,
) -> GemMaterial {
    apply_material_overrides_for_mode(material, overrides, model_width, false)
}

/// [`apply_material_overrides`] with an explicit color mode.
///
/// `physics_color` is passed by the caller, which knows whether the material's color comes
/// from a chromophore recipe (physics mode); it selects the 7 mm default stone width of
/// [`effective_stone_width_mm`] when no width was requested. It is deliberately not inferred from
/// the material's bands: a pure-host recipe has none, and `GemMaterial` carries no such field
/// (the net format is unchanged).
#[must_use]
pub fn apply_material_overrides_for_mode(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    model_width: Option<f64>,
    physics_color: bool,
) -> GemMaterial {
    // Opt-in only: skipped entirely (not called with 0.0) at the off position.
    let material = if overrides.inclusion_sigma_s > 0.0 {
        material.with_scattering_amount(overrides.inclusion_sigma_s)
    } else {
        material
    };

    // An isotropic material's optic axis is physically meaningless (no birefringence
    // to orient). This guard is what stops a leftover override from a previously
    // selected anisotropic material reaching an isotropic one's `c_axis`.
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

    // A degenerate/unmeasurable plane arrangement (`model_width` is `None`) or a
    // non-finite/non-positive scale leaves the material untouched, rather than risking
    // a NaN/negative path-length multiplier reaching the tracer.
    let eff_stone_width = effective_stone_width_mm(overrides.stone_width_mm, physics_color);
    if eff_stone_width > 0.0
        && let Some(model_width) = model_width
        && model_width > 1e-9
    {
        let scale = (f64::from(eff_stone_width) / model_width) as f32;
        if scale.is_finite() && scale > 0.0 {
            return material.with_absorption_path_scale(scale);
        }
    }
    material
}

/// Default stone width in millimeters used for physically-based chromophore materials
/// when no design girdle diameter has been specified.
pub const PHYSICS_DEFAULT_STONE_WIDTH_MM: f32 = 7.0;

/// Returns the effective stone width in mm, applying the 7.0 mm default for physics materials
/// when the requested width is 0.0 (unspecified).
#[must_use]
pub fn effective_stone_width_mm(requested_mm: f32, physics_color: bool) -> f32 {
    if requested_mm > 0.0 || !physics_color {
        requested_mm
    } else {
        PHYSICS_DEFAULT_STONE_WIDTH_MM
    }
}
