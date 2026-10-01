//! Tests for the builder-style opt-in constructors: the per-species scattering
//! arm coverage and [`GemMaterial::new_custom`]'s dispersion-delta convention.

use crate::optics::materials::GemMaterial;

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

/// A body-colour variant changes ONLY the absorption: a yellow sapphire keeps
/// sapphire's own name, dispersion, birefringence and c-axis bit for bit, and its
/// absorption becomes exactly the isotropic band set the yellow preset expands to.
#[test]
fn with_body_colour_changes_only_the_absorption() {
    let yellow = crate::optics::materials::body_colour::BODY_COLOUR_PRESETS[5];
    assert_eq!(
        yellow.key, "yellow",
        "test premise: index 5 is the yellow preset"
    );
    let base = GemMaterial::sapphire();
    let coloured = base.clone().with_body_colour(yellow.absorption_rgb);

    assert_eq!(coloured.name, base.name);
    assert_eq!(coloured.dispersion, base.dispersion);
    assert_eq!(
        coloured.birefringence_delta.to_bits(),
        base.birefringence_delta.to_bits()
    );
    assert_eq!(
        coloured.c_axis.to_array().map(f32::to_bits),
        base.c_axis.to_array().map(f32::to_bits)
    );
    assert_ne!(
        coloured.absorption, base.absorption,
        "the colour variant must actually change the absorption"
    );
    assert_eq!(
        coloured.absorption,
        crate::optics::absorption::AbsorptionTensor::isotropic(
            crate::optics::absorption::legacy_rgb_bands(yellow.absorption_rgb)
        )
    );
    // Every other field too: restoring the base absorption must give back the base
    // material exactly.
    let mut restored = coloured;
    restored.absorption = base.absorption.clone();
    assert_eq!(restored, base);
}
