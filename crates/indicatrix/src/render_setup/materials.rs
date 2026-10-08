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
/// `ModelUnit` material (the built-in table, legacy colour triples, fantasy customs) scales
/// by `stone_width_mm / 7 / MODEL_UNIT_FACE_UP_PATH` (the width factor is 1 while unset, so
/// the scale is never left at 1.0: the calibration always applies); a `PerMm` one (band colours, physics recipes) by
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

/// `material` sized for a stone of `stone_width_mm` on `planes` and nothing else dialled in:
/// the stone-size step of [`apply_material_overrides`] alone, measuring the plane arrangement
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
/// 10.87 mm green cubic zirconia came out black). The ModelUnit scale is therefore divided by
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
mod tests {
    use super::*;

    const OFF: MaterialOverrides = MaterialOverrides {
        inclusion_sigma_s: 0.0,
        c_axis_override: None,
        edge_rounding_radius: 0.0,
        stone_width_mm: 0.0,
    };

    fn sized(width_mm: f32) -> MaterialOverrides {
        MaterialOverrides {
            stone_width_mm: width_mm,
            ..OFF
        }
    }

    /// Every built-in and every legacy-triple colour is `ModelUnit`: scale `1 / K` unset and at
    /// 7 mm, `10.87 / 7 / K` at 10.87 mm (`K = MODEL_UNIT_FACE_UP_PATH`), whatever the design's
    /// model width.
    #[test]
    fn model_unit_materials_scale_with_the_width_over_seven_mm() {
        let mut materials = GemMaterial::all_materials();
        materials.push(GemMaterial::sapphire().with_body_color([0.2, 0.9, 0.3]));
        materials.push(GemMaterial::new_custom(
            "Fantasy",
            1.7,
            0.02,
            0.0,
            [0.1, 0.4, 0.9],
        ));
        for material in materials {
            assert_eq!(material.absorption_unit, AbsorptionUnit::ModelUnit);
            for model_width in [None, Some(1.3), Some(2.0), Some(6.0)] {
                let unset = apply_material_overrides(material.clone(), &OFF, model_width);
                let calibrated = 1.0 / MODEL_UNIT_FACE_UP_PATH;
                assert!(
                    (unset.absorption_path_scale - calibrated).abs() < 1e-7,
                    "{}",
                    material.name
                );
                let seven = apply_material_overrides(material.clone(), &sized(7.0), model_width);
                assert_eq!(
                    seven.absorption_path_scale, unset.absorption_path_scale,
                    "{}",
                    material.name
                );
                let big = apply_material_overrides(material.clone(), &sized(10.87), model_width);
                assert!(
                    (big.absorption_path_scale - 10.87 / 7.0 / MODEL_UNIT_FACE_UP_PATH).abs()
                        < 1e-6,
                    "{}: {}",
                    material.name,
                    big.absorption_path_scale
                );
            }
        }
    }

    /// Band colours and physics recipes are `PerMm`: `W / model_width`, 7 mm when no size is set.
    #[test]
    fn per_mm_materials_scale_with_the_width_over_the_model_width() {
        let banded = GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0);
        assert_eq!(banded.absorption_unit, AbsorptionUnit::PerMm);
        let model_width = 2.1_f64;
        let unset = apply_material_overrides(banded.clone(), &OFF, Some(model_width));
        assert!((f64::from(unset.absorption_path_scale) - 7.0 / model_width).abs() < 1e-5);
        let sized_up = apply_material_overrides(banded.clone(), &sized(10.87), Some(model_width));
        assert!((f64::from(sized_up.absorption_path_scale) - 10.87 / model_width).abs() < 1e-5);
        // No measurable width: the material keeps the scale it has.
        let unmeasured = apply_material_overrides(banded.clone(), &sized(10.87), None);
        assert_eq!(
            unmeasured.absorption_path_scale,
            banded.absorption_path_scale
        );
        assert!(needs_model_width(&banded));
        assert!(!needs_model_width(&GemMaterial::sapphire()));
    }

    /// The inclusion coefficient is the caller's number untouched: the size never reaches it.
    #[test]
    fn the_stone_size_never_changes_the_scattering_coefficient() {
        let with = |width_mm: f32| {
            apply_material_overrides(
                GemMaterial::sapphire(),
                &MaterialOverrides {
                    inclusion_sigma_s: 0.2,
                    stone_width_mm: width_mm,
                    ..OFF
                },
                Some(2.0),
            )
        };
        assert_eq!(with(0.0).scattering_sigma_s, 0.2);
        assert_eq!(with(5.0).scattering_sigma_s, 0.2);
        assert_eq!(with(12.0).scattering_sigma_s, 0.2);
    }

    #[test]
    fn effective_width_defaults_to_seven_mm() {
        assert_eq!(effective_stone_width_mm(0.0), 7.0);
        assert_eq!(effective_stone_width_mm(5.0), 5.0);
    }

    // ---- regression locks (lane AU) ----

    /// The unit decision of every built-in, written out. A new material fails
    /// `every_built_in_has_a_deliberate_absorption_unit` until it is added here: choose
    /// `ModelUnit` only for absorption tuned per model unit for a ~7 mm stone, `PerMm` for a
    /// physical per-millimetre one.
    const BUILT_IN_UNITS: &[(&str, AbsorptionUnit)] = &[
        ("Diamond", AbsorptionUnit::ModelUnit),
        ("Sapphire", AbsorptionUnit::ModelUnit),
        ("Ruby", AbsorptionUnit::ModelUnit),
        ("Emerald", AbsorptionUnit::ModelUnit),
        ("Zircon", AbsorptionUnit::ModelUnit),
        ("Alexandrite", AbsorptionUnit::ModelUnit),
        ("Topaz", AbsorptionUnit::ModelUnit),
        ("Spinel", AbsorptionUnit::ModelUnit),
        ("Quartz", AbsorptionUnit::ModelUnit),
        ("Tourmaline", AbsorptionUnit::ModelUnit),
        ("Tanzanite", AbsorptionUnit::ModelUnit),
        ("Synthetic Moissanite", AbsorptionUnit::ModelUnit),
        ("Cubic Zirconia", AbsorptionUnit::ModelUnit),
        ("Aquamarine", AbsorptionUnit::ModelUnit),
        ("Morganite", AbsorptionUnit::ModelUnit),
        ("Chrysoberyl (Yellow)", AbsorptionUnit::ModelUnit),
        ("Amethyst", AbsorptionUnit::ModelUnit),
        ("Citrine", AbsorptionUnit::ModelUnit),
        ("Pyrope Garnet", AbsorptionUnit::ModelUnit),
        ("Almandine Garnet", AbsorptionUnit::ModelUnit),
        ("Spessartine Garnet", AbsorptionUnit::ModelUnit),
        ("Grossular Garnet (Tsavorite)", AbsorptionUnit::ModelUnit),
        ("Andradite Garnet (Demantoid)", AbsorptionUnit::ModelUnit),
        ("Peridot", AbsorptionUnit::ModelUnit),
        ("YAG", AbsorptionUnit::ModelUnit),
        ("GGG", AbsorptionUnit::ModelUnit),
        ("Benitoite", AbsorptionUnit::ModelUnit),
        ("Andalusite", AbsorptionUnit::ModelUnit),
        ("Opal", AbsorptionUnit::ModelUnit),
        ("Glass (N-BK7)", AbsorptionUnit::ModelUnit),
        ("Glass (F2)", AbsorptionUnit::ModelUnit),
        ("Rutile", AbsorptionUnit::ModelUnit),
    ];

    /// Optical density of `material` at `lambda_nm` over `path_units` model units, as the tracer
    /// computes it: summed band absorption (ordinary ray) times `absorption_path_scale` times the
    /// path.
    fn density(material: &GemMaterial, lambda_nm: f32, path_units: f32) -> f64 {
        let alpha: f32 = material
            .absorption
            .o_ray
            .iter()
            .map(|band| band.evaluate(lambda_nm))
            .sum();
        f64::from(alpha) * f64::from(material.absorption_path_scale) * f64::from(path_units)
    }

    fn transmittance(material: &GemMaterial, lambda_nm: f32, path_units: f32) -> f64 {
        (-density(material, lambda_nm, path_units)).exp()
    }

    #[test]
    fn every_built_in_has_a_deliberate_absorption_unit() {
        let materials = GemMaterial::all_materials();
        for material in &materials {
            let decided = BUILT_IN_UNITS
                .iter()
                .find(|(name, _)| *name == material.name)
                .unwrap_or_else(|| {
                    panic!(
                        "built-in {} has no entry in BUILT_IN_UNITS: decide its absorption unit",
                        material.name
                    )
                });
            assert_eq!(material.absorption_unit, decided.1, "{}", material.name);
        }
        assert_eq!(
            materials.len(),
            BUILT_IN_UNITS.len(),
            "BUILT_IN_UNITS lists a material that no longer exists"
        );
    }

    /// For every built-in: unset and 7 mm give the identical material, 14 mm is exactly twice
    /// the optical density of 7 mm, and none of it depends on the design model width.
    #[test]
    fn every_built_in_scales_with_the_size_only() {
        for material in GemMaterial::all_materials() {
            let reference = apply_material_overrides(material.clone(), &OFF, Some(2.0));
            for model_width in [None, Some(1.0), Some(2.0), Some(3.7)] {
                let unset = apply_material_overrides(material.clone(), &OFF, model_width);
                let seven = apply_material_overrides(material.clone(), &sized(7.0), model_width);
                let fourteen =
                    apply_material_overrides(material.clone(), &sized(14.0), model_width);
                assert_eq!(unset, seven, "{}: unset != 7 mm", material.name);
                assert_eq!(unset, reference, "{}: model width mattered", material.name);
                assert_eq!(
                    fourteen.absorption_path_scale,
                    2.0 * seven.absorption_path_scale,
                    "{}",
                    material.name
                );
                for lambda in [450.0_f32, 530.0, 650.0] {
                    assert_eq!(
                        density(&fourteen, lambda, 5.0),
                        2.0 * density(&seven, lambda, 5.0),
                        "{} at {lambda} nm",
                        material.name
                    );
                }
            }
        }
    }

    fn green_preset() -> [f32; 3] {
        crate::optics::materials::body_color::BODY_COLOR_PRESETS
            .iter()
            .find(|preset| preset.label.to_ascii_lowercase().contains("green"))
            .expect("a green body-colour preset")
            .absorption_rgb
    }

    /// `material` with the stored (uncalibrated) scale 1.0: the "per model unit" numbers the swatch
    /// is tuned on.
    fn bare(material: GemMaterial) -> GemMaterial {
        material.with_absorption_path_scale(1.0)
    }

    /// The transmittance the swatch shows over ONE model unit at `lambda_nm`.
    fn one_unit_swatch_transmittance(material: &GemMaterial, lambda_nm: f32) -> f64 {
        transmittance(&bare(material.clone()), lambda_nm, 1.0)
    }

    /// 2026-10-08 photo-vs-render bug: a real green cubic zirconia, 10.87 mm across the girdle,
    /// is vivid green in daylight, but the render was black because the per-model-unit absorption
    /// (which the swatch shows over ONE unit) was integrated over the several-unit face-up path.
    ///
    /// Arithmetic (green preset `[2.2, 0.2, 2.0]`, the three Gaussian bands overlap so the
    /// stone's alpha at 550 nm is about 1.34 per unit, not 0.2): the 1-unit swatch has
    /// `T1 = exp(-1.34) = 0.26`. The scale at 10.87 mm is `10.87 / 7 / K` (K = 2.52, but the
    /// result does not depend on K), so over K units the density is `1.34 * 10.87 / 7 = 2.08`
    /// and `T = T1^(10.87/7) = 0.125`. The old behaviour (scale `10.87 / 7 = 1.55` over the same
    /// K units) had density `1.34 * 1.55 * K`: 3.3e-5 at the original estimate K = 5 (density
    /// 10.4), 5e-3 at K = 2.52. The floor 0.05 is 2.5 times below the new value and 10 times
    /// above the old one at the measured K.
    #[test]
    fn regression_green_cubic_zirconia_at_10_87_mm_is_not_black() {
        let green = GemMaterial::by_name("Cubic Zirconia")
            .expect("built-in")
            .with_body_color(green_preset());
        let t1 = one_unit_swatch_transmittance(&green, 550.0);
        let expected = t1.powf(f64::from(10.87_f32 / 7.0));
        for model_width in [1.8, 2.0, 2.2] {
            let stone = apply_material_overrides(green.clone(), &sized(10.87), Some(model_width));
            let t_green = transmittance(&stone, 550.0, MODEL_UNIT_FACE_UP_PATH);
            assert!(
                (t_green / expected - 1.0).abs() < 1e-3,
                "model width {model_width}: green T {t_green} vs analytic {expected}"
            );
            assert!(
                t_green > 0.05,
                "model width {model_width}: green T {t_green} (old behaviour: 3e-5)"
            );
            assert!(t_green > transmittance(&stone, 450.0, MODEL_UNIT_FACE_UP_PATH));
            assert!(t_green > transmittance(&stone, 650.0, MODEL_UNIT_FACE_UP_PATH));
        }
    }

    /// Same lock for plain Emerald and Ruby at 10.87 mm: the face-up transmittance equals the
    /// 1-unit swatch raised to `10.87 / 7` and the own colour channel stays above the opposite one.
    ///
    /// Emerald (alpha at 530 nm about 0.57 per unit): new `T = exp(-0.57 * 1.553) = 0.41`, old
    /// (scale 1.553 over K units) `exp(-0.885 * K)`: 0.012 at the original estimate K = 5, 0.107
    /// at the measured K = 2.52; the floor 0.2 is half the new value and 1.9 times the old one at
    /// the measured K. Ruby has no separate absolute floor beyond 0.05: its swatch T is
    /// already high, and the analytic equality is the real check.
    #[test]
    fn regression_emerald_and_ruby_at_10_87_mm_stay_coloured() {
        let factor = f64::from(10.87_f32 / 7.0);
        let emerald = apply_material_overrides(GemMaterial::emerald(), &sized(10.87), Some(2.0));
        let green = transmittance(&emerald, 530.0, MODEL_UNIT_FACE_UP_PATH);
        let expected = one_unit_swatch_transmittance(&GemMaterial::emerald(), 530.0).powf(factor);
        assert!(
            (green / expected - 1.0).abs() < 1e-3,
            "emerald {green} vs {expected}"
        );
        assert!(
            green > 0.2,
            "emerald green T {green} (old behaviour: 0.012)"
        );
        assert!(green > 1.5 * transmittance(&emerald, 650.0, MODEL_UNIT_FACE_UP_PATH));
        let ruby = apply_material_overrides(GemMaterial::ruby(), &sized(10.87), Some(2.0));
        let red = transmittance(&ruby, 650.0, MODEL_UNIT_FACE_UP_PATH);
        let expected = one_unit_swatch_transmittance(&GemMaterial::ruby(), 650.0).powf(factor);
        assert!(
            (red / expected - 1.0).abs() < 1e-3,
            "ruby {red} vs {expected}"
        );
        assert!(red > 0.05, "ruby red T {red}");
        assert!(red > 1.5 * transmittance(&ruby, 530.0, MODEL_UNIT_FACE_UP_PATH));
    }

    /// The calibration itself: unset size and 7 mm render, over `K` units of path, exactly the
    /// 1-unit swatch transmittance, for every built-in and for every toolbar colour preset.
    #[test]
    fn unset_and_seven_mm_face_up_path_renders_the_one_unit_swatch() {
        use crate::optics::materials::body_color::BODY_COLOR_PRESETS;
        let mut materials = GemMaterial::all_materials();
        for preset in BODY_COLOR_PRESETS.iter() {
            materials.push(
                GemMaterial::by_name("Cubic Zirconia")
                    .expect("built-in")
                    .with_body_color(preset.absorption_rgb),
            );
        }
        for material in materials {
            assert_eq!(material.absorption_unit, AbsorptionUnit::ModelUnit);
            for overrides in [OFF, sized(7.0)] {
                let stone = apply_material_overrides(material.clone(), &overrides, Some(2.0));
                for lambda in [420.0_f32, 450.0, 500.0, 530.0, 580.0, 620.0, 650.0, 700.0] {
                    let face_up = density(&stone, lambda, MODEL_UNIT_FACE_UP_PATH);
                    let swatch = density(&bare(material.clone()), lambda, 1.0);
                    assert!(
                        (face_up - swatch).abs() <= 1e-5 * swatch.max(1.0),
                        "{} at {lambda} nm: face-up density {face_up} vs swatch {swatch}",
                        material.name
                    );
                }
            }
        }
    }

    /// Swatch == render at 5, 7 and 10.87 mm: the colour of the 1-unit absorption over the swatch
    /// path `width / 7` (what the swatch shows) is the colour of the sized material over
    /// `K` face-up units (what the render integrates), for every preset.
    #[test]
    fn the_swatch_equals_the_render_material_at_every_size() {
        use crate::{
            color::body_color::{Illuminant, body_colors},
            optics::materials::body_color::BODY_COLOR_PRESETS,
        };
        for preset in BODY_COLOR_PRESETS.iter() {
            let material = GemMaterial::by_name("Cubic Zirconia")
                .expect("built-in")
                .with_body_color(preset.absorption_rgb);
            for width_mm in [5.0_f32, 7.0, 10.87] {
                let stone = apply_material_overrides(material.clone(), &sized(width_mm), Some(2.0));
                let render = body_colors(
                    &stone.absorption,
                    f64::from(MODEL_UNIT_FACE_UP_PATH) * f64::from(stone.absorption_path_scale),
                    Illuminant::D65,
                )
                .unpolarised;
                let swatch = body_colors(
                    &bare(material.clone()).absorption,
                    f64::from(width_mm) / 7.0,
                    Illuminant::D65,
                )
                .unpolarised;
                for (r, s) in render.lab.iter().zip(swatch.lab) {
                    assert!(
                        (r - s).abs() < 1e-3,
                        "{} at {width_mm} mm: render {:?} vs swatch {:?}",
                        preset.label,
                        render.lab,
                        swatch.lab
                    );
                }
            }
        }
    }

    /// Per-mm materials are physically calibrated: the face-up constant never reaches them.
    #[test]
    fn the_calibration_constant_never_reaches_per_mm_materials() {
        for (width_mm, model_width) in [(0.0_f32, 2.0_f64), (7.0, 2.0), (10.87, 2.0), (10.87, 3.5)]
        {
            let scale =
                absorption_path_scale_for(AbsorptionUnit::PerMm, width_mm, Some(model_width))
                    .expect("a measurable width");
            let expected = f64::from(effective_stone_width_mm(width_mm)) / model_width;
            assert!((f64::from(scale) - expected).abs() < 1e-5, "{width_mm} mm");
        }
    }

    /// Calibration of [`MODEL_UNIT_FACE_UP_PATH`]: the measured face-up `mean_path_units` of the
    /// standard round brilliant and the emerald cut (colourless stone, light tent, table-up) must
    /// be within +-20 % of the constant at their median, and the single path whose colour matches
    /// the Beer-Lambert mixture of a green stone (the mixture is lighter than the mean path) must
    /// be of the same order. The failure message prints every measured value so the owner can
    /// re-pin the constant.
    #[test]
    fn model_unit_face_up_path_matches_the_measured_cuts() {
        use crate::{
            color::{
                body_color::{Illuminant, body_colors},
                metrics::evaluate_gem_optical_metrics_with_tone,
            },
            geometry::cuts::StandardGemCuts,
            optics::raytracer::LightingPreset,
        };
        use std::{f32::consts::FRAC_PI_2, fmt::Write as _};

        let environment = LightingPreset::LightTent.studio(1.0, 0.85, 0.95);
        let cuts = [
            (
                "round brilliant",
                StandardGemCuts::standard_round_brilliant(),
            ),
            ("emerald cut", StandardGemCuts::emerald_cut()),
        ];
        let green = bare(
            GemMaterial::by_name("Cubic Zirconia")
                .expect("built-in")
                .with_body_color(green_preset()),
        );
        let mut mean_paths = Vec::new();
        let mut equivalent_paths = Vec::new();
        let mut report = String::new();
        for (name, planes) in &cuts {
            // `mean_path_units` is independent of the absorption (the path histogram is collected
            // before any colour), so the green stone gives the colourless stone's path too.
            let (_, tone) =
                evaluate_gem_optical_metrics_with_tone(planes, &green, 0.0, FRAC_PI_2, environment);
            // The single path (model units) whose D65 lightness equals the mixture's, by bisection
            // (lightness falls monotonically with the path).
            let (mut low, mut high) = (0.01_f64, 80.0_f64);
            for _ in 0..60 {
                let mid = 0.5 * (low + high);
                let l_star = body_colors(&green.absorption, mid, Illuminant::D65)
                    .unpolarised
                    .lab[0];
                if l_star > f64::from(tone.l_star) {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            let equivalent = 0.5 * (low + high);
            let _ = write!(
                report,
                "{name}: mean_path_units {}, mixture-equivalent path {equivalent:.2}; ",
                tone.mean_path_units
            );
            mean_paths.push(f64::from(tone.mean_path_units));
            equivalent_paths.push(equivalent);
        }
        let median = |values: &mut Vec<f64>| {
            values.sort_by(f64::total_cmp);
            let mid = values.len() / 2;
            if values.len() % 2 == 0 {
                0.5 * (values[mid - 1] + values[mid])
            } else {
                values[mid]
            }
        };
        let mean_median = median(&mut mean_paths);
        let equivalent_median = median(&mut equivalent_paths);
        let constant = f64::from(MODEL_UNIT_FACE_UP_PATH);
        assert!(
            (constant / mean_median - 1.0).abs() <= 0.20,
            "MODEL_UNIT_FACE_UP_PATH {constant} is not within 20 % of the median mean path \
             {mean_median:.2} ({report}equivalent median {equivalent_median:.2}): re-pin it"
        );
        assert!(
            constant > 0.4 * equivalent_median && constant < 2.5 * equivalent_median,
            "MODEL_UNIT_FACE_UP_PATH {constant} is far from the mixture-equivalent path \
             {equivalent_median:.2} ({report}mean median {mean_median:.2})"
        );
    }

    /// B5: a stone sized for scoring outside the render loop (`material_for_stone`: tone,
    /// tilt, comparisons, the optimizer) is the very material the render uses, for a built-in,
    /// a legacy triple and a per-mm material.
    #[test]
    fn the_scoring_material_equals_the_render_material_for_a_sized_stone() {
        use crate::{geometry::cuts::StandardGemCuts, render_setup::measure_model_width};
        let planes = StandardGemCuts::standard_round_brilliant();
        let width = measure_model_width(&planes);
        let materials = [
            GemMaterial::emerald(),
            GemMaterial::sapphire().with_body_color([0.2, 0.9, 0.3]),
            GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0),
        ];
        for material in materials {
            for width_mm in [0.0_f32, 7.0, 10.87] {
                assert_eq!(
                    material_for_stone(material.clone(), width_mm, &planes),
                    apply_material_overrides(material.clone(), &sized(width_mm), width),
                    "{} at {width_mm} mm",
                    material.name
                );
            }
        }
    }

    /// Per-mm materials: scale is `W / model_width`, 7 mm while unset, whatever made the bands
    /// (L*C*h or library band rows, a physics recipe tensor).
    #[test]
    fn per_mm_materials_scale_with_width_over_model_width_whatever_made_them() {
        let bands = GemMaterial::sapphire().with_body_color_bands(&[[460.0, 45.0, 0.25]], 3.5);
        let physics = GemMaterial::sapphire().with_chromophore_absorption(bands.absorption.clone());
        for material in [bands, physics] {
            assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
            for model_width in [1.0_f64, 2.0, 3.7] {
                let unset = apply_material_overrides(material.clone(), &OFF, Some(model_width));
                assert!((f64::from(unset.absorption_path_scale) - 7.0 / model_width).abs() < 1e-5);
                let big =
                    apply_material_overrides(material.clone(), &sized(10.87), Some(model_width));
                let expected = f64::from(10.87_f32) / model_width;
                assert!((f64::from(big.absorption_path_scale) - expected).abs() < 1e-5);
            }
        }
    }
}
