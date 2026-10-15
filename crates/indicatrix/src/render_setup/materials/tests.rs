//! Tests of the render-time material resolution and override stack (`render_setup::materials`).

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

/// Every `ModelUnit` built-in (all but the zoned Ametrine, which is
/// `PerMm`) and every legacy-triple colour: scale `1 / K` unset and at 7 mm,
/// `10.87 / 7 / K` at 10.87 mm (`K = MODEL_UNIT_FACE_UP_PATH`), whatever the design's model
/// width.
#[test]
fn model_unit_materials_scale_with_the_width_over_seven_mm() {
    let mut materials: Vec<GemMaterial> = GemMaterial::all_materials()
        .into_iter()
        .filter(|m| m.absorption_unit == AbsorptionUnit::ModelUnit)
        .collect();
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
                (big.absorption_path_scale - 10.87 / 7.0 / MODEL_UNIT_FACE_UP_PATH).abs() < 1e-6,
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
/// `every_built_in_has_a_deliberate_absorption_unit` until it is added here (or to
/// `ZONED_BUILT_IN_UNITS` when it is a zoned one): choose `ModelUnit` only for
/// absorption tuned per model unit for a ~7 mm stone, `PerMm` for a physical per-millimetre
/// one.
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
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        AbsorptionUnit::ModelUnit,
    ),
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

/// The zoned built-ins: the zoned Ametrine is a physical, per-millimetre material (its zones
/// are chromophore recipes).
const ZONED_BUILT_IN_UNITS: &[(&str, AbsorptionUnit)] = &[("Ametrine", AbsorptionUnit::PerMm)];

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
    let decisions = || BUILT_IN_UNITS.iter().chain(ZONED_BUILT_IN_UNITS);
    for material in &materials {
        let decided = decisions()
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
        decisions().count(),
        "BUILT_IN_UNITS / ZONED_BUILT_IN_UNITS lists a material that no longer exists"
    );
}

/// For every built-in: unset and 7 mm give the identical material and 14 mm is exactly twice
/// the optical density of 7 mm. A `ModelUnit` built-in does not depend on the design model
/// width at all.
#[test]
fn every_built_in_scales_with_the_size_only() {
    for material in GemMaterial::all_materials() {
        let reference = apply_material_overrides(material.clone(), &OFF, Some(2.0));
        // A `PerMm` built-in (the zoned Ametrine) has the scale `W / model_width`, so the
        // model width matters by design, and an unmeasurable width (`None`) leaves its scale
        // alone: neither the `unset == reference` equality nor the `None` case applies.
        let per_mm = material.absorption_unit == AbsorptionUnit::PerMm;
        for model_width in [None, Some(1.0), Some(2.0), Some(3.7)] {
            if per_mm && model_width.is_none() {
                continue;
            }
            let unset = apply_material_overrides(material.clone(), &OFF, model_width);
            let seven = apply_material_overrides(material.clone(), &sized(7.0), model_width);
            let fourteen = apply_material_overrides(material.clone(), &sized(14.0), model_width);
            assert_eq!(unset, seven, "{}: unset != 7 mm", material.name);
            if !per_mm {
                assert_eq!(unset, reference, "{}: model width mattered", material.name);
            }
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
/// 1-unit swatch transmittance, for every `ModelUnit` built-in (not the per-millimetre
/// Ametrine) and for every toolbar colour preset.
#[test]
fn unset_and_seven_mm_face_up_path_renders_the_one_unit_swatch() {
    use crate::optics::materials::body_color::BODY_COLOR_PRESETS;
    let mut materials: Vec<GemMaterial> = GemMaterial::all_materials()
        .into_iter()
        .filter(|m| m.absorption_unit == AbsorptionUnit::ModelUnit)
        .collect();
    for preset in &BODY_COLOR_PRESETS {
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
    for preset in &BODY_COLOR_PRESETS {
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
    for (width_mm, model_width) in [(0.0_f32, 2.0_f64), (7.0, 2.0), (10.87, 2.0), (10.87, 3.5)] {
        let scale = absorption_path_scale_for(AbsorptionUnit::PerMm, width_mm, Some(model_width))
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
            let mid = f64::midpoint(low, high);
            let l_star = body_colors(&green.absorption, mid, Illuminant::D65)
                .unpolarised
                .lab[0];
            if l_star > f64::from(tone.l_star) {
                low = mid;
            } else {
                high = mid;
            }
        }
        let equivalent = f64::midpoint(low, high);
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
        if values.len().is_multiple_of(2) {
            f64::midpoint(values[mid - 1], values[mid])
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
            let big = apply_material_overrides(material.clone(), &sized(10.87), Some(model_width));
            let expected = f64::from(10.87_f32) / model_width;
            assert!((f64::from(big.absorption_path_scale) - expected).abs() < 1e-5);
        }
    }
}

/// The zoned built-in is `PerMm`: the scale is `W / model_width` (7 mm while unset), and the
/// size step leaves the zones exactly as they were.
#[test]
fn the_zoned_built_in_scales_with_the_width_over_the_model_width() {
    let ametrine = GemMaterial::ametrine();
    assert!(needs_model_width(&ametrine));
    let unset = apply_material_overrides(ametrine.clone(), &OFF, Some(2.0));
    assert!(
        (unset.absorption_path_scale - 3.5).abs() < 1e-6,
        "7 mm / 2.0"
    );
    let ten = apply_material_overrides(ametrine, &sized(10.0), Some(2.0));
    assert!(
        (ten.absorption_path_scale - 5.0).abs() < 1e-6,
        "10 mm / 2.0"
    );
    let zoning = crate::optics::materials::ametrine_zoning();
    assert_eq!(unset.zoning.as_ref(), Some(&zoning));
    assert_eq!(ten.zoning.as_ref(), Some(&zoning));
}
