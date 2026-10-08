//! Tests for the builder-style opt-in constructors: the per-species scattering
//! arm coverage and [`GemMaterial::new_custom`]'s dispersion-delta convention.

use crate::optics::{
    dispersion::DispersionModel,
    materials::{CrystalSystem, GemMaterial, OpticalCharacter},
};

/// Review fix: scattering now divides by the path scale, so `with_absorption_path_scale` ignores a
/// non-finite or non-positive value (the previous scale stays), and takes a positive one.
#[test]
fn the_absorption_path_scale_builder_ignores_degenerate_values() {
    let base = GemMaterial::sapphire().with_absorption_path_scale(1.5);
    assert_eq!(base.absorption_path_scale, 1.5);
    for bad in [0.0, -2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let kept = base.clone().with_absorption_path_scale(bad);
        assert_eq!(kept.absorption_path_scale, 1.5, "{bad} must be ignored");
    }
    let fresh = GemMaterial::sapphire().with_absorption_path_scale(f32::NAN);
    assert_eq!(fresh.absorption_path_scale, 1.0);
    assert_eq!(
        base.with_absorption_path_scale(2.25).absorption_path_scale,
        2.25
    );
}

/// Every built-in material must have its OWN explicit `recommended_scattering` arm,
/// keyed by its exact `all_materials()` name -- falling through to the generic
/// default silently, as `"Moissanite"` (rather than the real name `"Synthetic
/// Moissanite"`) used to, is a bug this test now catches for every current AND
/// future built-in. See
/// `GemMaterial::recommended_scattering_arm`'s own doc comment for why this needed a
/// separate `Option`-returning helper to detect at all: a species whose intended
/// numbers happen to equal the default would look identical to a truly-forgotten one
/// if this only checked `recommended_scattering`'s return value.
#[test]
fn every_builtin_has_an_explicit_scattering_arm() {
    for material in GemMaterial::all_materials() {
        assert!(
            GemMaterial::recommended_scattering_arm(&material.name).is_some(),
            "{} has no explicit recommended_scattering arm -- it silently falls \
                 through to the generic default",
            material.name
        );
    }
}
/// `new_custom`'s `dispersion_delta` must be interpreted as the Fraunhofer
/// F-C interval -- i.e. `n(486.1nm) - n(656.3nm)` on the constructed material must
/// equal the requested `dispersion_delta`, not 66% or 113% of it (the old flat
/// `0.347` multiplier's actual behaviour). Checked across several representative
/// deltas, including one large enough
/// that a wrong conversion factor would be very obvious.
#[test]
fn new_custom_dispersion_delta_measures_exactly_at_f_and_c() {
    for dispersion_delta in [0.005f32, 0.010, 0.02564, 0.05] {
        let material =
            GemMaterial::new_custom("F-C probe", 1.6, dispersion_delta, 0.0, [0.0, 0.0, 0.0]);
        let n_f = material.dispersion.evaluate(486.1);
        let n_c = material.dispersion.evaluate(656.3);
        let measured = n_f - n_c;
        assert!(
            (measured - dispersion_delta).abs() < 1e-4,
            "requested F-C delta {dispersion_delta} but measured {measured} \
                 (n_F={n_f}, n_C={n_c})"
        );
    }
}
/// The mean refractive index (`mean_ri`) must be preserved exactly at the sodium D
/// line regardless of `dispersion_delta` -- the F-C conversion factor changes how
/// steeply `n(lambda)` varies away from D, not its value AT D (the Cauchy `a` term
/// is solved to compensate exactly, per `new_custom`'s own formula).
#[test]
fn new_custom_preserves_mean_ri_at_sodium_d_line_regardless_of_dispersion_delta() {
    for dispersion_delta in [0.0f32, 0.01, 0.03] {
        let material =
            GemMaterial::new_custom("D-line probe", 1.72, dispersion_delta, 0.0, [0.0, 0.0, 0.0]);
        let n_d = material.dispersion.evaluate(589.3);
        assert!(
            (n_d - 1.72).abs() < 1e-4,
            "dispersion_delta={dispersion_delta}: n_d should stay 1.72, got {n_d}"
        );
    }
}

/// A custom material built from a model keeps that model exactly (a Sellmeier curve is
/// not flattened to the Cauchy fit `new_custom` builds), reports the model's `n_d`, and
/// differs from `new_custom` in nothing else.
#[test]
fn new_custom_with_dispersion_keeps_the_model_and_its_n_d() {
    let model = GemMaterial::by_name("Glass (N-BK7)")
        .expect("N-BK7 is a built-in")
        .dispersion;
    assert!(
        matches!(model, DispersionModel::Sellmeier3 { .. }),
        "test premise: N-BK7 is a three-term Sellmeier"
    );
    let material = GemMaterial::new_custom_with_dispersion("BK7 copy", model, 0.0, [0.0; 3]);
    assert_eq!(material.dispersion, model);
    assert_eq!(
        material.dispersion.evaluate(589.3).to_bits(),
        model.n_d().to_bits()
    );
    assert!((model.n_d() - 1.5168).abs() < 0.002, "{}", model.n_d());

    let mut expected = GemMaterial::new_custom("BK7 copy", model.n_d(), 0.0, 0.0, [0.0; 3]);
    expected.dispersion = model;
    assert_eq!(material, expected);
}

/// The crystal system, optical character and colour come from the same rules as
/// `new_custom`; the Cauchy curve `new_custom` builds is reproduced by passing it back in.
#[test]
fn new_custom_with_dispersion_derives_the_rest_like_new_custom() {
    let model = DispersionModel::Cauchy {
        a: 1.7,
        b: 0.006,
        c: 0.0,
    };
    let uniaxial = GemMaterial::new_custom_with_dispersion("Probe", model, -0.008, [0.2, 0.4, 2.8]);
    assert_eq!(uniaxial.crystal_system, CrystalSystem::Trigonal);
    assert_eq!(
        uniaxial.optical_character,
        OpticalCharacter::UniaxialNegative
    );
    assert_eq!(
        uniaxial.birefringence_delta.to_bits(),
        (-0.008_f32).to_bits()
    );
    assert_ne!(
        uniaxial.absorption,
        GemMaterial::new_custom("Probe", 1.7, 0.0, 0.0, [0.0; 3]).absorption,
        "the colour reaches the absorption"
    );

    let simple = GemMaterial::new_custom("Same", 1.62, 0.02, 0.0, [0.0; 3]);
    let rebuilt = GemMaterial::new_custom_with_dispersion("Same", simple.dispersion, 0.0, [0.0; 3]);
    assert_eq!(
        rebuilt, simple,
        "a Cauchy fit passed back in changes nothing"
    );
}

/// A body-color variant changes ONLY the absorption: a yellow sapphire keeps
/// sapphire's own name, dispersion, birefringence and c-axis bit for bit, and its
/// absorption becomes exactly the isotropic band set the yellow preset expands to.
#[test]
fn with_body_color_changes_only_the_absorption() {
    let yellow = crate::optics::materials::body_color::BODY_COLOR_PRESETS[5];
    assert_eq!(
        yellow.key, "yellow",
        "test premise: index 5 is the yellow preset"
    );
    let base = GemMaterial::sapphire();
    let colored = base.clone().with_body_color(yellow.absorption_rgb);

    assert_eq!(colored.name, base.name);
    assert_eq!(colored.dispersion, base.dispersion);
    assert_eq!(
        colored.birefringence_delta.to_bits(),
        base.birefringence_delta.to_bits()
    );
    assert_eq!(
        colored.c_axis.to_array().map(f32::to_bits),
        base.c_axis.to_array().map(f32::to_bits)
    );
    assert_ne!(
        colored.absorption, base.absorption,
        "the color variant must actually change the absorption"
    );
    assert_eq!(
        colored.absorption,
        crate::optics::absorption::AbsorptionTensor::isotropic(
            crate::optics::absorption::legacy_rgb_bands(yellow.absorption_rgb)
        )
    );
    // Every other field too: restoring the base absorption must give back the base
    // material exactly.
    let mut restored = colored;
    restored.absorption = base.absorption.clone();
    assert_eq!(restored, base);
}
