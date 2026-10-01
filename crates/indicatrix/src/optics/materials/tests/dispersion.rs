//! Dispersion regression tests: Abbe numbers, `n_d` at the sodium D line, physical
//! bounds at the sampled band edges, and the wavelength-dependent extraordinary
//! index.

use crate::optics::materials::{GemMaterial, OpticalCharacter};

/// Verifies every built-in material's computed Abbe number
/// `V_d = (n_d - 1) / (n_F - n_C)` against a trusted-source reference value, so a
/// future mistyped coefficient (Sellmeier or Cauchy) gets caught instead of
/// silently shipping -- see each material's own doc comment in `all_materials` for
/// the primary/reputable source and the derivation of its target Abbe number.
///
/// Source priority for these targets (re-verified against trusted online sources,
/// which now OUTRANK `GEMSTONE_RENDERING_BLUEPRINT.md` -- the document is used only
/// as a tiebreaker/corroboration, never as the primary authority, because it is a
/// research compilation whose own "Fire Delta n(F-C)" column demonstrably mixes
/// Fraunhofer B-G and F-C conventions row-to-row, e.g. Quartz lists 0.013, the
/// well-known B-G figure, in a column labelled F-C, contradicting that same row's
/// own `V_d`):
///   1. Primary optical measurements (Sellmeier/Cauchy fits from refractiveindex.info
///      and the papers it cites).
///   2. Reputable gemological references (GIA, Mindat, Handbook of Mineralogy,
///      International Gem Society), corroborated by 2+ independent sources where
///      possible, with their "dispersion" figures explicitly treated as Fraunhofer
///      B-G (never plugged directly into an F-C slot).
///   3. The document, as a tiebreaker only.
///
/// Two confidence tiers, reflected in the tolerance:
///   - TIGHT (0.5): Diamond, Sapphire, Ruby, Quartz, Spinel, Cubic Zirconia,
///     Alexandrite, Synthetic Moissanite -- dispersion coefficients transcribed
///     directly from a primary optical-constants paper (Peter 1923; Malitson &
///     Dodge 1972; Ghosh 1999; Tropf & Thomas 1991; Wood & Nassau 1982; Walling
///     1980; Wang 2013 corroborated by Singh 1971), target computed from that exact
///     published formula. Alexandrite and Moissanite CONTRADICT the document here
///     (68.0 -> 73.90 and 21.5 -> 25.94 respectively) -- per the source priority
///     above the primary measurement wins; see each entry's comment in
///     `all_materials` for the two-source corroboration.
///   - LOWER CONFIDENCE (2.5-4.0, roughly 5-12% relative): Zircon, Topaz,
///     Tourmaline, Tanzanite, Emerald -- no primary Sellmeier/Cauchy fit exists in
///     the optics literature for these species (confirmed absent from
///     refractiveindex.info's database), so `n_d` is taken from 2+ corroborating
///     reputable gemological sources and the dispersion shape is a 2-parameter
///     Cauchy fit solved from that `n_d` and a Delta n(F-C) obtained by converting
///     each species' well-corroborated gemological B-G "dispersion" figure via a
///     B-G->F-C ratio (0.579, range 0.569-0.587) computed directly from the 8
///     genuine primary Sellmeier curves above -- not from cross-comparing
///     gemological tables against each other, which is how the earlier 0.591
///     factor was derived. All five CONTRADICT the document (Zircon 28.0 -> 40.96, Topaz 64.0 ->
///     76.83, Tourmaline 55.0 -> 64.58, Tanzanite 45.0 -> 40.21, Emerald's estimate
///     shifts modestly from 69.99 to 70.93 with the refined ratio). Zircon's `n_d`
///     also carried an unrelated, unsourced error, corrected to 1.925 from
///     1.956878 (see its entry's comment) -- critical-angle-relevant since `n_d`
///     drives it directly.
#[test]
fn builtin_material_abbe_numbers_match_published_values() {
    // (name, sourced Abbe number V_d, tolerance)
    let expected: &[(&str, f32, f32)] = &[
        // -- tight tier: primary Sellmeier source for both n_d and V_d --
        ("Diamond", 55.27, 0.5),
        ("Sapphire", 72.27, 0.5),
        ("Ruby", 72.27, 0.5),
        ("Quartz", 69.65, 0.5),
        ("Spinel", 60.63, 0.5),
        ("Cubic Zirconia", 33.52, 0.5),
        ("Alexandrite", 73.90, 0.5),
        ("Synthetic Moissanite", 25.94, 0.5),
        // -- lower-confidence tier: no primary fit exists; n_d from corroborated
        // gemological sources, dispersion shape from a gemological B-G figure
        // converted via the physically-derived 0.579 ratio -- see each entry's
        // comment in `all_materials` --
        ("Zircon", 40.96, 3.0),
        ("Topaz", 76.83, 4.0),
        ("Tourmaline", 64.58, 3.5),
        ("Tanzanite", 40.21, 3.0),
        ("Emerald", 70.93, 3.5),
    ];

    for &(name, published_abbe, tol) in expected {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let n_f = material.dispersion.evaluate(486.1);
        let n_d = material.dispersion.evaluate(589.3);
        let n_c = material.dispersion.evaluate(656.3);
        let dn = n_f - n_c;
        assert!(
            dn > 0.0,
            "{name}: Delta n(F-C) must be positive (normal dispersion), got {dn}"
        );
        let computed_abbe = (n_d - 1.0) / dn;
        assert!(
            (computed_abbe - published_abbe).abs() <= tol,
            "{name}: computed Abbe V_d={computed_abbe:.2} (n_d={n_d:.5}, n_F={n_f:.5}, n_C={n_c:.5}) \
                 does not match published V_d={published_abbe:.2} within tolerance {tol} -- check the \
                 dispersion coefficients against the source cited in all_materials()"
        );
    }
}
/// Companion to `builtin_material_abbe_numbers_match_published_values`: pins down
/// `n_d` at the sodium D line (589.3nm) for every re-sourced material below, so a
/// future coefficient edit that silently shifts `n_d` gets caught. `n_d` drives
/// the critical angle and therefore brilliance/windowing/extinction, so any
/// intentional change here is significant downstream and must be deliberate, not
/// an accident of refitting the dispersion shape.
///
/// For five of the six re-sourced materials (Synthetic Moissanite, Topaz,
/// Tourmaline, Tanzanite, Alexandrite), `n_d` is unchanged from before -- only the
/// dispersion shape (`V_d`) moved, per
/// `builtin_material_abbe_numbers_match_published_values`.
///
/// Zircon is the exception, and the one material in this file whose `n_d`
/// DID change materially: from 1.956878 to 1.925, a -0.0319 shift, far above
/// the ~0.002 threshold at which critical-angle-driven behaviour (brilliance,
/// windowing, extinction) is expected to move. The old value was never actually
/// sourced from anywhere (not the document, which gives `n_o=1.9250`, nor any
/// external reference found); 1.925 is corroborated by the Handbook of Mineralogy,
/// International Gem Society, and the document itself. See Zircon's comment in
/// `all_materials` for the full derivation. Any test elsewhere in the workspace
/// that hardcodes zircon's brilliance/windowing/pose behaviour will need
/// re-stabilising against this new `n_d`.
#[test]
fn builtin_material_n_d_matches_sourced_values() {
    // (name, sourced n_d, tolerance)
    let expected_n_d: &[(&str, f32, f32)] = &[
        ("Zircon", 1.925, 1e-4),
        ("Synthetic Moissanite", 2.647_434, 1e-4),
        ("Topaz", 1.627_178, 1e-4),
        ("Tourmaline", 1.639_405, 1e-4),
        ("Tanzanite", 1.700_858, 1e-4),
        ("Alexandrite", 1.742_73, 1e-4),
    ];

    for &(name, expected, tol) in expected_n_d {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let n_d = material.dispersion.evaluate(589.3);
        assert!(
            (n_d - expected).abs() < tol,
            "{name}: n_d={n_d:.6} does not match the sourced value {expected:.6} -- check the \
                 dispersion coefficients against the source cited in all_materials(); if this is a \
                 deliberate re-sourcing, update this test's expected value and flag the n_d change \
                 in the task report if it moves by more than ~0.002 (critical-angle significance)"
        );
    }
}
/// Every built-in material's dispersion curve (Sellmeier or Cauchy) must
/// evaluate to a finite, physically-sane (`n >= 1.0`) index at BOTH ends of this
/// renderer's actual sampled visible band (380nm violet, 780nm red) -- not just
/// near the sodium D line every other dispersion test in this file checks. Most
/// Cauchy fits here are only solved/verified near D and the F/C Fraunhofer lines
/// (486-656nm); this pins the `.max(1.0)` floor added to
/// `DispersionModel::Cauchy::evaluate` (see that variant's own doc comment) so an
/// out-of-fit-range extrapolation can never silently produce `n < 1` or a NaN.
#[test]
fn every_builtin_dispersion_curve_stays_physical_at_the_sampled_band_edges() {
    for material in GemMaterial::all_materials() {
        for lambda_nm in [380.0f32, 780.0] {
            let n = material.dispersion.evaluate(lambda_nm);
            assert!(
                n.is_finite(),
                "{}: dispersion.evaluate({lambda_nm}) must be finite, got {n}",
                material.name
            );
            assert!(
                n >= 1.0,
                "{}: dispersion.evaluate({lambda_nm}) = {n} must be >= 1.0 \
                     (physically-impossible index)",
                material.name
            );
            // The extraordinary ray (when a genuine independent curve is
            // present, or the constant-offset fallback otherwise) must be
            // equally well-behaved.
            if material.optical_character == OpticalCharacter::UniaxialPositive
                || material.optical_character == OpticalCharacter::UniaxialNegative
            {
                let n_e = material.extraordinary_index_at(lambda_nm, n);
                assert!(
                    n_e.is_finite() && n_e >= 1.0,
                    "{}: extraordinary_index_at({lambda_nm}, {n}) = {n_e} must be \
                         finite and >= 1.0",
                    material.name
                );
            }
        }
    }
}
/// Every new built-in must resolve by its own exact name
/// (the same `by_name` round-trip `by_name_round_trips_every_builtin_material`
/// already covers exhaustively -- this test's real job is the tolerance check
/// below) and its `n(589.3nm)` must match the target table below to within
/// 0.002.
#[test]
fn m4_new_species_match_their_target_n_d_within_tolerance() {
    // (name, target n_D, tolerance)
    let expected: &[(&str, f32, f32)] = &[
        ("Aquamarine", 1.577, 0.002),
        ("Morganite", 1.577, 0.002),
        ("Chrysoberyl (Yellow)", 1.746, 0.002),
        ("Amethyst", 1.544, 0.001),
        ("Citrine", 1.544, 0.001),
        ("Pyrope Garnet", 1.714, 0.002),
        ("Almandine Garnet", 1.790, 0.002),
        ("Spessartine Garnet", 1.800, 0.002),
        ("Grossular Garnet (Tsavorite)", 1.734, 0.002),
        ("Andradite Garnet (Demantoid)", 1.887, 0.002),
        ("Peridot", 1.654, 0.002),
        ("YAG", 1.833, 0.002),
        ("Benitoite", 1.757, 0.002),
        ("Andalusite", 1.634, 0.002),
        ("Opal", 1.45, 0.002),
        ("Glass (N-BK7)", 1.5168, 0.002),
        ("Glass (F2)", 1.620, 0.002),
    ];

    for &(name, target_n_d, tol) in expected {
        let material =
            GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} must resolve via by_name"));
        assert_eq!(material.name, name, "{name} must resolve to itself exactly");
        let n_d = material.dispersion.evaluate(589.3);
        assert!(
            (n_d - target_n_d).abs() <= tol,
            "{name}: n_d={n_d:.5} does not match target {target_n_d} within {tol}"
        );
    }

    // GGG is checked separately: its target n_D (1.970) is only reproduced to
    // within ~0.002 by construction from this entry's own Cauchy fit (see that
    // entry's comment for why no primary Sellmeier fit was available), so this
    // uses the same tolerance but is called out on its own for clarity.
    let ggg = GemMaterial::by_name("GGG").expect("GGG must resolve via by_name");
    let ggg_n_d = ggg.dispersion.evaluate(589.3);
    assert!(
        (ggg_n_d - 1.970).abs() <= 0.002,
        "GGG: n_d={ggg_n_d:.5} does not match target 1.970 within 0.002"
    );
}

/// GGG's stored dispersion is the F-C convention: the gemological B-G figure 0.045 times
/// the file-wide 0.579 B-G->F-C ratio gives `Delta n(F-C) = 0.026055`, which the Cauchy
/// fit reproduces between the Fraunhofer F (486.1nm) and C (656.3nm) lines. Feeding 0.045
/// straight into the F-C slot would give 0.045 here, 1.7x too dispersive.
#[test]
fn ggg_dispersion_uses_the_converted_f_minus_c_value() {
    let ggg = GemMaterial::by_name("GGG").expect("GGG must resolve via by_name");
    let delta_f_c = ggg.dispersion.evaluate(486.1) - ggg.dispersion.evaluate(656.3);
    assert!(
        (delta_f_c - 0.026_055).abs() <= 1e-3,
        "GGG: n(F)-n(C)={delta_f_c:.5} should be 0.045 * 0.579 = 0.026055 within 1e-3"
    );
}
/// Quartz's genuine per-axis (Ghosh o/e) dispersion must make its
/// extraordinary-ray index actually VARY with wavelength in a way the old
/// constant-offset approximation could not (the offset `n_o(lambda) +
/// birefringence_delta` tracks `n_o`'s own curvature exactly, so `n_e(lambda) -
/// n_o(lambda)` is constant under the old model but need not be under a genuine
/// independent curve); a material with no `uniaxial_extraordinary_dispersion` set
/// (e.g. Sapphire) must keep the exact old constant-delta behaviour, bit-identical.
#[test]
fn quartz_extraordinary_index_has_wavelength_dependent_birefringence_while_legacy_entries_stay_constant()
 {
    let quartz = GemMaterial::by_name("Quartz").expect("Quartz must resolve");
    assert!(
        quartz.uniaxial_extraordinary_dispersion.is_some(),
        "test premise: Quartz must carry a genuine per-axis e-ray dispersion curve"
    );

    let delta_at = |lambda_nm: f32| {
        let n_o = quartz.dispersion.evaluate(lambda_nm);
        let n_e = quartz.extraordinary_index_at(lambda_nm, n_o);
        n_e - n_o
    };
    let delta_380 = delta_at(380.0);
    let delta_780 = delta_at(780.0);
    assert!(
        (delta_380 - delta_780).abs() > 1e-4,
        "Quartz's n_e - n_o must genuinely vary across the visible band: \
             delta(380nm)={delta_380:.6}, delta(780nm)={delta_780:.6}"
    );

    // A legacy entry (no per-axis curve) must keep the OLD constant-delta
    // behaviour exactly: n_e - n_o == birefringence_delta at every wavelength.
    let sapphire = GemMaterial::by_name("Sapphire").expect("Sapphire must resolve");
    assert!(
        sapphire.uniaxial_extraordinary_dispersion.is_none(),
        "test premise: Sapphire must NOT carry a per-axis e-ray dispersion curve"
    );
    for lambda_nm in [380.0f32, 589.3, 780.0] {
        let n_o = sapphire.dispersion.evaluate(lambda_nm);
        let n_e = sapphire.extraordinary_index_at(lambda_nm, n_o);
        assert!(
            (n_e - n_o - sapphire.birefringence_delta).abs() < 1e-6,
            "Sapphire (legacy entry) at {lambda_nm}nm: n_e - n_o must equal the \
                 constant birefringence_delta exactly"
        );
    }
}

/// Rutile's principal indices follow `DeVore` (1951): `n_o(D) = 2.6129`, `n_e(D) =
/// 2.9086`, with Fraunhofer `Delta n(F-C)` of 0.1636 (o) and 0.2072 (e) -- far below
/// the gemological B-G figure (~0.30) the earlier Cauchy fit mistook for F-C -- and the
/// stored `birefringence_delta` equals `n_e(D) - n_o(D)`.
#[test]
fn rutile_matches_devore_indices_and_dispersion() {
    let rutile = GemMaterial::by_name("Rutile").expect("Rutile must be a built-in material");
    let e_ray = rutile
        .uniaxial_extraordinary_dispersion
        .expect("Rutile must carry its own e-ray dispersion curve");

    let n_o = rutile.dispersion.evaluate(589.3);
    let n_e = e_ray.evaluate(589.3);
    assert!((n_o - 2.613).abs() <= 0.002, "n_o(D) = {n_o}");
    assert!((n_e - 2.909).abs() <= 0.002, "n_e(D) = {n_e}");

    let fc_o = rutile.dispersion.evaluate(486.1) - rutile.dispersion.evaluate(656.3);
    let fc_e = e_ray.evaluate(486.1) - e_ray.evaluate(656.3);
    assert!((fc_o - 0.164).abs() <= 0.003, "o-ray F-C = {fc_o}");
    assert!((fc_e - 0.207).abs() <= 0.003, "e-ray F-C = {fc_e}");

    assert!(
        (rutile.birefringence_delta - (n_e - n_o)).abs() <= 1e-3,
        "birefringence_delta {} must equal n_e(D) - n_o(D) = {}",
        rutile.birefringence_delta,
        n_e - n_o
    );
}
