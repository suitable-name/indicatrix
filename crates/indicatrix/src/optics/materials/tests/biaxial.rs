//! Biaxial-indicatrix tests: which materials expose one, internal consistency
//! against the material's own fields, the optic-sign convention, and a
//! pleochroic band-set regression for Alexandrite.

use crate::optics::{
    absorption::AbsorptionBand,
    birefringence::AbsorptionTensor3,
    materials::{GemMaterial, OpticalCharacter},
};
use glam::Vec3;

/// Only the six orthorhombic (biaxial) built-ins -- Alexandrite, Topaz,
/// Tanzanite, and Chrysoberyl (Yellow), Peridot, Andalusite
/// -- should carry biaxial principal-index data; every other material (isotropic
/// or uniaxial) must resolve to `None`, keeping them on the existing uniaxial code
/// path unchanged.
#[test]
fn only_biaxial_materials_expose_a_biaxial_indicatrix() {
    let biaxial_names = [
        "Alexandrite",
        "Topaz",
        "Tanzanite",
        "Chrysoberyl (Yellow)",
        "Peridot",
        "Andalusite",
    ];
    for material in GemMaterial::all_materials() {
        let indicatrix = material.biaxial_indicatrix(589.3);
        if biaxial_names.contains(&material.name.as_str()) {
            assert!(
                indicatrix.is_some(),
                "{} should expose a biaxial indicatrix",
                material.name
            );
        } else {
            assert!(
                indicatrix.is_none(),
                "{} should NOT expose a biaxial indicatrix (uniaxial/isotropic)",
                material.name
            );
        }
    }
}
/// Each biaxial built-in's indicatrix must order its three principal indices
/// `n_alpha` <= `n_beta` <= `n_gamma`; must place `n_beta` exactly at the
/// material's own base dispersion curve (the documented convention); and must
/// place `n_gamma` minus `n_alpha` exactly at the material's own
/// `birefringence_delta` (its pre-existing "or max-min" meaning, unchanged) --
/// pinning the `biaxial_indicatrix` wiring itself, independent of whether the
/// underlying `biaxial_delta_beta_alpha` numbers are later refined.
#[test]
fn biaxial_indicatrix_is_internally_consistent_with_existing_fields() {
    for name in ["Alexandrite", "Topaz", "Tanzanite"] {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let n_d = material.dispersion.evaluate(589.3);
        let indicatrix = material
            .biaxial_indicatrix(589.3)
            .unwrap_or_else(|| panic!("{name} must expose a biaxial indicatrix"));

        assert!(
            indicatrix.n_alpha <= indicatrix.n_beta && indicatrix.n_beta <= indicatrix.n_gamma,
            "{name}: principal indices must be ordered n_alpha<=n_beta<=n_gamma, got ({}, {}, {})",
            indicatrix.n_alpha,
            indicatrix.n_beta,
            indicatrix.n_gamma
        );
        assert!(
            (indicatrix.n_beta - n_d).abs() < 1e-5,
            "{name}: n_beta ({}) must equal the base dispersion curve's n_d ({n_d})",
            indicatrix.n_beta
        );
        assert!(
            (indicatrix.n_gamma - indicatrix.n_alpha - material.birefringence_delta).abs() < 1e-5,
            "{name}: n_gamma - n_alpha ({}) must equal birefringence_delta ({})",
            indicatrix.n_gamma - indicatrix.n_alpha,
            material.birefringence_delta
        );
    }
}
/// `optical_character`'s `BiaxialPositive`/`BiaxialNegative`
/// sign must actually match the three principal indices `biaxial_indicatrix`
/// builds from `dispersion` (equal to `n_beta`), `biaxial_delta_beta_alpha` (equal
/// to `n_beta` minus `n_alpha`) and `birefringence_delta` (equal to `n_gamma`
/// minus `n_alpha`) -- optic sign is determined by where `n_beta` sits between
/// `n_alpha` and `n_gamma`: positive when beta sits closer to alpha (`n_beta`
/// minus `n_alpha` is less than `n_gamma` minus `n_beta`), negative when it sits
/// closer to gamma (`n_beta` minus `n_alpha` is greater than `n_gamma` minus
/// `n_beta`). This caught the Tanzanite entry shipping
/// `biaxial_delta_beta_alpha: Some(0.0070)` against `birefringence_delta: 0.0130`
/// under a declared `BiaxialPositive`: 0.0070 is greater than 0.0130 minus 0.0070
/// equals 0.0060, which is actually NEGATIVE. Fixed by re-deriving
/// `biaxial_delta_beta_alpha` from Hurlbut's per-specimen tanzanite indices
/// (American Mineralogist 54, 702 (1969): `n_alpha` = 1.6915, `n_beta` = 1.6935,
/// `n_gamma` = 1.7020) rather than the independent-range midpoints the old comment
/// used -- see the field's own comment on the Tanzanite entry above. Runs over
/// every built-in with `biaxial_delta_beta_alpha: Some(_)` (not just Tanzanite) so
/// this stays a live guard against the same mistake recurring on Alexandrite,
/// Topaz, Chrysoberyl (Yellow), Peridot or Andalusite, or on any future biaxial
/// built-in.
#[test]
fn biaxial_sign_matches_optical_character() {
    for material in GemMaterial::all_materials() {
        let Some(delta_beta_alpha) = material.biaxial_delta_beta_alpha else {
            continue;
        };
        let n_beta = material.dispersion.evaluate(589.3);
        let n_alpha = n_beta - delta_beta_alpha;
        let n_gamma = n_alpha + material.birefringence_delta;

        let beta_minus_alpha = n_beta - n_alpha;
        let gamma_minus_beta = n_gamma - n_beta;

        match material.optical_character {
            OpticalCharacter::BiaxialPositive => assert!(
                beta_minus_alpha < gamma_minus_beta,
                "{}: declared BiaxialPositive requires (n_beta - n_alpha) < \
                     (n_gamma - n_beta), got {beta_minus_alpha} >= {gamma_minus_beta} \
                     (n_alpha={n_alpha}, n_beta={n_beta}, n_gamma={n_gamma})",
                material.name
            ),
            OpticalCharacter::BiaxialNegative => assert!(
                beta_minus_alpha > gamma_minus_beta,
                "{}: declared BiaxialNegative requires (n_beta - n_alpha) > \
                     (n_gamma - n_beta), got {beta_minus_alpha} <= {gamma_minus_beta} \
                     (n_alpha={n_alpha}, n_beta={n_beta}, n_gamma={n_gamma})",
                material.name
            ),
            other => panic!(
                "{}: carries biaxial_delta_beta_alpha but optical_character is {other:?}, \
                     not BiaxialPositive/BiaxialNegative",
                material.name
            ),
        }
    }
}
/// Trichroism convention pin for Alexandrite's own shipped data (the same
/// discipline `birefringence::biaxial_reduction_tests` applies with synthetic
/// coefficients, applied here to the real Farrell & Newnham-derived entry):
/// each principal direction's band set must carry the qualitative amplitude
/// pattern the cited pleochroic colours dictate, and feeding the shipped band
/// sums through the REAL `AbsorptionTensor3::biaxial` constructor must land each
/// one on its own world axis (alpha -> +X, beta -> Z, gamma -> +Y for
/// `c_axis = Vec3::Y`, per `stable_orthonormal_basis`'s pinned construction) --
/// so a future swap of the alpha/beta/gamma argument order, or of the axis
/// convention underneath, fails loudly instead of silently recolouring the stone.
#[test]
fn alexandrite_trichroic_band_sets_follow_the_cited_pleochroic_pattern() {
    let alex =
        GemMaterial::by_name("Alexandrite").expect("Alexandrite must be a built-in material");
    let bands_alpha = &alex.absorption.o_ray;
    let bands_gamma = &alex.absorption.e_ray;
    let bands_beta = alex
        .absorption
        .beta_ray
        .as_deref()
        .expect("Alexandrite must carry a third (beta) trichroic band set");
    assert!(
        alex.absorption.is_pleochroic,
        "Alexandrite's trichroic tensor must be flagged pleochroic"
    );

    let sum_at =
        |bands: &[AbsorptionBand], nm: f32| -> f32 { bands.iter().map(|b| b.evaluate(nm)).sum() };

    // 4T2 system (yellow-to-red region, evaluated at gamma's cited 595nm centre):
    // gamma (crystal b, GREEN -- absorbs "both red and yellow") strongest, alpha
    // (crystal c, RED) intermediate, beta (crystal a, YELLOW -- red/yellow left
    // open) weakest, per the figure-read Fig. 4 amplitudes in the entry's comment.
    let (t2_alpha, t2_beta, t2_gamma) = (
        sum_at(bands_alpha, 595.0),
        sum_at(bands_beta, 595.0),
        sum_at(bands_gamma, 595.0),
    );
    assert!(
        t2_gamma > t2_alpha && t2_alpha > t2_beta,
        "4T2 amplitude order must be gamma(green) > alpha(red) > beta(yellow), got \
             gamma={t2_gamma:.3}, alpha={t2_alpha:.3}, beta={t2_beta:.3}"
    );

    // 4T1 system (blue-violet, evaluated at beta's cited 422nm centre): beta
    // (crystal a) strongest -- F&N: the 0.42u absorption "dominates the a
    // spectrum" -- with alpha and gamma both clearly weaker.
    let (t1_alpha, t1_beta, t1_gamma) = (
        sum_at(bands_alpha, 422.0),
        sum_at(bands_beta, 422.0),
        sum_at(bands_gamma, 422.0),
    );
    assert!(
        t1_beta > t1_alpha && t1_beta > t1_gamma,
        "4T1 amplitude must peak on beta(yellow, crystal a), got alpha={t1_alpha:.3}, \
             beta={t1_beta:.3}, gamma={t1_gamma:.3}"
    );

    // Convention pin through the real constructor: with c_axis = +Y, the alpha
    // set's sum must appear along +X, the beta set's along Z, and the gamma set's
    // along +Y (the `c_axis`/n_gamma direction) -- see
    // `AbsorptionTensor3::biaxial`'s doc comment.
    assert_eq!(
        alex.c_axis,
        Vec3::Y,
        "test premise: Alexandrite c_axis is +Y"
    );
    let tensor = AbsorptionTensor3::biaxial(t2_alpha, t2_beta, t2_gamma, alex.c_axis);
    for (axis, expected, label) in [
        (Vec3::X, t2_alpha, "alpha on +X"),
        (Vec3::Z, t2_beta, "beta on Z"),
        (Vec3::Y, t2_gamma, "gamma on +Y (c_axis)"),
    ] {
        let measured = tensor.quadratic_form(axis);
        assert!(
            (measured - expected).abs() < 1e-5,
            "{label}: quadratic_form along {axis:?} must return that principal set's \
                 band sum ({expected:.4}), got {measured:.4}"
        );
    }
}
/// Every new built-in must have an explicit birefringence sign/magnitude
/// matching the target table below (a cheap, high-signal regression against
/// a mistyped `birefringence_delta` literal).
#[test]
fn m4_new_species_match_their_target_birefringence() {
    let expected: &[(&str, f32, f32)] = &[
        ("Aquamarine", -0.006, 1e-4),
        ("Morganite", -0.006, 1e-4),
        ("Chrysoberyl (Yellow)", 0.009, 1e-4),
        ("Amethyst", 0.0091, 1e-4),
        ("Citrine", 0.0091, 1e-4),
        ("Peridot", 0.036, 1e-4),
        ("Benitoite", 0.047, 1e-4),
        ("Andalusite", 0.010, 1e-4),
    ];
    for &(name, target, tol) in expected {
        let material =
            GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} must resolve via by_name"));
        assert!(
            (material.birefringence_delta - target).abs() <= tol,
            "{name}: birefringence_delta={} does not match target {target}",
            material.birefringence_delta
        );
    }
    // The five garnets, YAG, GGG, Opal and the two glasses are isotropic --
    // birefringence_delta must be exactly 0.0.
    for name in [
        "Pyrope Garnet",
        "Almandine Garnet",
        "Spessartine Garnet",
        "Grossular Garnet (Tsavorite)",
        "Andradite Garnet (Demantoid)",
        "YAG",
        "GGG",
        "Opal",
        "Glass (N-BK7)",
        "Glass (F2)",
    ] {
        let material =
            GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} must resolve via by_name"));
        assert_eq!(
            material.birefringence_delta, 0.0,
            "{name}: isotropic material must have birefringence_delta == 0.0"
        );
    }
}
