//! Material resolution and the render-time override stack.
//!
//! By-name lookup and every opt-in override layered on top of it
//! (inclusion/subsurface scattering, crystal-axis orientation, facet edge rounding,
//! physical stone size). Moved out of the desktop viewer unchanged so the browser app
//! resolves a design's rendered material exactly the same way for the same settings --
//! see `crate::render_setup`'s own doc comment.

use super::stone_width::measure_model_width;
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::materials::{AbsorptionUnit, GemMaterial, OpticalCharacter},
};
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
    /// `GemMaterial::with_scattering_amount`. Per model unit, independent of the stone's
    /// size. `0.0` is off.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation: `Some(axis)` replaces the resolved material's
    /// `c_axis` (skipped for isotropic materials); `None` ("as cut") leaves it
    /// untouched.
    pub c_axis_override: Option<Vec3>,
    /// Facet edge (meet-point) rounding radius, via `GemMaterial::with_edge_rounding`.
    /// `0.0` is off (sharp edges).
    pub edge_rounding_radius: f32,
    /// Physical stone size: girdle width in millimetres for the absorption scale. `0.0` is
    /// "not set": a [`AbsorptionUnit::ModelUnit`] material then renders as it was tuned (7 mm,
    /// its swatch colour face-up, see [`MODEL_UNIT_FACE_UP_PATH`]) and a
    /// [`AbsorptionUnit::PerMm`] one assumes [`PHYSICS_DEFAULT_STONE_WIDTH_MM`].
    pub stone_width_mm: f32,
}

/// Applies every [`MaterialOverrides`] field on top of a resolved base material.
///
/// Each one is opt-in and skips its underlying `GemMaterial::with_*` call entirely at
/// its off position, so a material with nothing dialled in renders bit-identical to
/// before these controls existed.
///
/// # Absorption units
///
/// The stone size sets the material's `absorption_path_scale` through
/// [`absorption_path_scale_for`], by the material's own [`AbsorptionUnit`]: a
/// `ModelUnit` material (the built-in table apart from the zoned Ametrine, legacy colour
/// triples, fantasy customs) scales
/// by `stone_width_mm / 7 / MODEL_UNIT_FACE_UP_PATH` (the width factor is 1 while unset, so
/// the scale is never left at 1.0: the calibration always applies); a `PerMm` one (band colours,
/// physics recipes, the zoned Ametrine) by
/// `stone_width_mm / model_width`, with 7 mm for an unset width. See [`AbsorptionUnit`].
///
/// `model_width` is the active design's own girdle width in model units, already
/// resolved by the caller (see [`crate::render_setup::measure_model_width`]) -- this
/// function does no plane-arrangement measurement of its own, so a caller with a
/// persistent cache keyed on the design's geometry (the desktop's `StoneWidthCache`)
/// only pays for a remeasure when the design actually changes, and a one-shot caller
/// can just measure fresh and pass the result straight through. It is only read for a
/// `PerMm` material: ask [`needs_model_width`] before measuring.
#[must_use]
pub fn apply_material_overrides(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    model_width: Option<f64>,
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
    if let Some(scale) = absorption_path_scale_for(
        material.absorption_unit,
        overrides.stone_width_mm,
        model_width,
    ) {
        return material.with_absorption_path_scale(scale);
    }
    material
}

/// `material` sized for a stone of `stone_width_mm` on `planes` and nothing else dialled in.
///
/// The stone-size step of [`apply_material_overrides`] alone, measuring the plane arrangement
/// only when the material's unit needs it ([`needs_model_width`]).
///
/// For the callers that score or sweep a stone outside the render loop (tone/tilt metrics, the
/// optimizer, comparisons): they must see the absorption the render shows, so they take the
/// same size rule instead of tracing the bare resolved material. `stone_width_mm == 0.0` is the
/// render's "not set" (a `ModelUnit` material then gets the calibration scale
/// `1 / MODEL_UNIT_FACE_UP_PATH`; `planes` is only read for a `PerMm` material).
#[must_use]
pub fn material_for_stone(
    material: GemMaterial,
    stone_width_mm: f32,
    planes: &[GpuFacetPlane],
) -> GemMaterial {
    let model_width = if needs_model_width(&material) {
        measure_model_width(planes)
    } else {
        None
    };
    apply_material_overrides(
        material,
        &MaterialOverrides {
            inclusion_sigma_s: 0.0,
            c_axis_override: None,
            edge_rounding_radius: 0.0,
            stone_width_mm,
        },
        model_width,
    )
}

/// The stone width in millimetres the absorption was calibrated for, and the width assumed
/// when none is set: 7 mm.
///
/// A [`AbsorptionUnit::ModelUnit`] material renders exactly as it was tuned at this width; a
/// [`AbsorptionUnit::PerMm`] material uses it when no stone size is set.
pub const PHYSICS_DEFAULT_STONE_WIDTH_MM: f32 = 7.0;

/// The representative face-up light path, in model units, that a [`AbsorptionUnit::ModelUnit`]
/// absorption's "one unit" swatch stands for.
///
/// Built-in coloured materials, the toolbar colour presets, legacy `absorption_rgb` triples and
/// fantasy customs are tuned so their swatch looks right over ONE model unit of path
/// (`indicatrix_cut_core::material::color::FANTASY_PATH_UNITS`). Light that leaves a face-up stone
/// has travelled several chords (the tone metric's `mean_path_units`, about 2.5 for a round
/// brilliant), so rendering those numbers at scale 1.0 is several times too dark (the 2026-10-08
/// 10.87 mm green cubic zirconia came out black). The `ModelUnit` scale is therefore divided by
/// this constant: absorption is linear in the scale, so over `MODEL_UNIT_FACE_UP_PATH` units of
/// path the stone shows exactly the colour the swatch shows over one unit, at 7 mm or with no
/// size set, and a larger or smaller stone scales physically from there (`width / 7`).
///
/// Chosen from the tone metric (`FaceUpTone::mean_path_units`, light tent, colourless stone) for
/// the standard round brilliant and the emerald cut; the test
/// `model_unit_face_up_path_matches_the_measured_cuts` re-measures it and prints the values.
/// Measured 2026-10-08 by that test: round brilliant mean path 2.51 (mixture-equivalent 2.54),
/// emerald cut 2.52 (equivalent 2.49), median 2.52.
/// Stored numbers (vault `absorption_rgb`, `.toml` `body_color_override`, the built-in band peaks)
/// are NOT changed: the constant is applied once, here. The colour-to-triple solver
/// (`nearest_legacy_rgb`) keeps solving at one unit, so its outputs stay comparable with stored
/// triples and with the swatch.
pub const MODEL_UNIT_FACE_UP_PATH: f32 = 2.52;

/// Returns the effective stone width in mm: the requested one, or the 7 mm default
/// ([`PHYSICS_DEFAULT_STONE_WIDTH_MM`]) when `requested_mm` is 0.0 (unspecified).
#[must_use]
pub fn effective_stone_width_mm(requested_mm: f32) -> f32 {
    if requested_mm > 0.0 {
        requested_mm
    } else {
        PHYSICS_DEFAULT_STONE_WIDTH_MM
    }
}

/// Whether [`apply_material_overrides`] reads the design's model-unit girdle width for
/// `material` (only a [`AbsorptionUnit::PerMm`] material does), so a caller can skip the plane
/// measurement otherwise.
#[must_use]
pub fn needs_model_width(material: &GemMaterial) -> bool {
    material.absorption_unit == AbsorptionUnit::PerMm
}

/// The `absorption_path_scale` a stone of `stone_width_mm` gets for absorption numbers of
/// `unit`, or `None` when the material keeps the scale it has.
///
/// * [`AbsorptionUnit::ModelUnit`]: `(effective width / 7 mm) / MODEL_UNIT_FACE_UP_PATH`,
///   independent of the design's model width, so the tuned look never depends on how a file
///   happens to be normalised. While no width is set (`stone_width_mm <= 0`) the width factor is
///   1.0, so an unset size and 7 mm both give `1 / MODEL_UNIT_FACE_UP_PATH`: over the face-up path
///   the stone then shows the colour the swatch shows over one model unit.
/// * [`AbsorptionUnit::PerMm`]: `effective width / model_width` (millimetres per model unit),
///   the effective width being 7 mm while none is set. `None` when `model_width` is missing or
///   not positive.
///
/// `None` as well for a non-finite or non-positive result.
#[must_use]
pub fn absorption_path_scale_for(
    unit: AbsorptionUnit,
    stone_width_mm: f32,
    model_width: Option<f64>,
) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the render precision; a width ratio is in the normal float range"
    )]
    let scale = match unit {
        AbsorptionUnit::ModelUnit => {
            // `effective_stone_width_mm` maps an unset (0.0) width to 7 mm, i.e. factor 1.0; a
            // NaN or negative width is treated as unset too.
            let width_mm = if stone_width_mm.is_nan() || stone_width_mm <= 0.0 {
                PHYSICS_DEFAULT_STONE_WIDTH_MM
            } else {
                stone_width_mm
            };
            width_mm / PHYSICS_DEFAULT_STONE_WIDTH_MM / MODEL_UNIT_FACE_UP_PATH
        }
        AbsorptionUnit::PerMm => {
            let model_width = model_width.filter(|w| *w > 1e-9)?;
            let width_mm = f64::from(effective_stone_width_mm(stone_width_mm));
            (width_mm / model_width) as f32
        }
    };
    (scale.is_finite() && scale > 0.0).then_some(scale)
}

#[cfg(test)]
mod tests;
