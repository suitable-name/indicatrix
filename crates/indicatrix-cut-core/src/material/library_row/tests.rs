//! Tests for [`LibraryMaterial`]: the material and the file snapshot built from a library row.

use super::*;
use crate::native::{dispersion_model_to_json, gem_material_from_custom_snapshot};
use indicatrix::optics::{
    chromophore::{ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve},
    dispersion::DispersionModel,
    materials::AbsorptionUnit,
};

fn plain(name: &str) -> LibraryMaterial<'_> {
    LibraryMaterial {
        name,
        refractive_index: 1.75,
        dispersion: 0.02,
        birefringence: 0.0,
        absorption_rgb: [0.0; 3],
        crystal_system: None,
        optical_character: None,
        biaxial_delta_beta_alpha: None,
        specific_gravity: Some(3.9),
        color_recipe_json: None,
        dispersion_model_json: None,
        absorption_bands_json: None,
    }
}

fn ruby() -> ColorRecipe {
    let catalogue = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("corundum", catalogue.data_version);
    recipe.set_amount("Cr", 0.3);
    let (tensor, _) = resolve(&recipe, catalogue).expect("ruby resolves");
    recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    recipe
}

#[test]
fn the_stored_names_read_back_and_an_unknown_name_is_none() {
    for system in CRYSTAL_SYSTEMS {
        assert_eq!(
            crystal_system_from_name(crystal_system_name(system)),
            Some(system)
        );
    }
    for character in OPTICAL_CHARACTERS {
        assert_eq!(
            optical_character_from_name(optical_character_name(character)),
            Some(character)
        );
    }
    assert_eq!(crystal_system_from_name("Amorphous"), None);
    assert_eq!(crystal_system_from_name(""), None);
    assert_eq!(optical_character_from_name("Isotropy"), None);
    // The spelling is the variant's `Debug` name, which is what older rows hold.
    assert_eq!(crystal_system_name(CrystalSystem::Trigonal), "Trigonal");
    assert_eq!(
        optical_character_name(OpticalCharacter::UniaxialNegative),
        "UniaxialNegative"
    );
}

#[test]
fn a_plain_row_is_exactly_what_new_custom_builds() {
    let row = plain("My Spinel");
    assert_eq!(
        row.gem_material(),
        GemMaterial::new_custom("My Spinel", 1.75, 0.02, 0.0, [0.0; 3])
    );
}

#[test]
fn stored_crystal_optics_replace_the_inference_and_unknown_names_are_ignored() {
    let inferred = GemMaterial::new_custom("Biaxial", 1.65, 0.02, 0.01, [0.0; 3]);
    let row = LibraryMaterial {
        refractive_index: 1.65,
        birefringence: 0.01,
        crystal_system: Some("Orthorhombic"),
        optical_character: Some("BiaxialPositive"),
        biaxial_delta_beta_alpha: Some(0.004),
        ..plain("Biaxial")
    };
    let material = row.gem_material();
    assert_eq!(material.crystal_system, CrystalSystem::Orthorhombic);
    assert_eq!(
        material.optical_character,
        OpticalCharacter::BiaxialPositive
    );
    assert_eq!(material.biaxial_delta_beta_alpha, Some(0.004));
    assert_ne!(material.crystal_system, inferred.crystal_system);

    // Names this build does not know leave the inference alone.
    let unknown = LibraryMaterial {
        refractive_index: 1.65,
        birefringence: 0.01,
        crystal_system: Some("Amorphous"),
        optical_character: Some("Mystery"),
        ..plain("Biaxial")
    };
    let material = unknown.gem_material();
    assert_eq!(material.crystal_system, inferred.crystal_system);
    assert_eq!(material.optical_character, inferred.optical_character);
}

#[test]
fn a_stored_dispersion_model_builds_the_curve_and_a_bad_one_takes_the_plain_path() {
    let model = DispersionModel::Cauchy {
        a: 1.7,
        b: 0.006,
        c: 0.0,
    };
    let json = dispersion_model_to_json(&model);
    let row = LibraryMaterial {
        dispersion_model_json: Some(&json),
        absorption_bands_json: None,
        ..plain("Glass")
    };
    assert_eq!(row.gem_material().dispersion, model);

    let plain_path = plain("Glass").gem_material();
    for bad in [
        r#"{"kind":"cauchy","a":0.5,"b":0.0,"c":0.0}"#,
        "not json",
        "",
    ] {
        let row = LibraryMaterial {
            dispersion_model_json: Some(bad),
            absorption_bands_json: None,
            ..plain("Glass")
        };
        assert_eq!(row.gem_material(), plain_path, "{bad:?}");
    }
}

#[test]
fn a_physics_recipe_renders_from_its_bands_and_a_fantasy_colour_does_not() {
    let mode = ColorMode::physics(ruby(), [0.4, 0.9, 1.8]);
    let json = mode.to_json();
    let physics = LibraryMaterial {
        absorption_rgb: [0.4, 0.9, 1.8],
        color_recipe_json: Some(&json),
        ..plain("Ruby")
    };
    assert_eq!(physics.gem_material().absorption, mode.resolve_tensor());

    let fantasy_json = ColorMode::fantasy([0.4, 0.9, 1.8]).to_json();
    let fantasy = LibraryMaterial {
        absorption_rgb: [0.4, 0.9, 1.8],
        color_recipe_json: Some(&fantasy_json),
        ..plain("Ruby")
    };
    assert_eq!(
        fantasy.gem_material(),
        GemMaterial::new_custom("Ruby", 1.75, 0.02, 0.0, [0.4, 0.9, 1.8])
    );
}

#[test]
fn the_snapshot_holds_the_typed_numbers_and_the_names_as_text() {
    let row = LibraryMaterial {
        crystal_system: Some("Trigonal"),
        optical_character: Some("UniaxialNegative"),
        birefringence: -0.02,
        absorption_rgb: [0.2, 0.4, 0.6],
        ..plain("Typed")
    };
    let snapshot = row.snapshot();
    assert_eq!(snapshot.mean_ri, f64::from(1.75_f32));
    assert_eq!(snapshot.dispersion_delta, f64::from(0.02_f32));
    assert_eq!(snapshot.birefringence_delta, f64::from(-0.02_f32));
    assert_eq!(snapshot.specific_gravity, Some(f64::from(3.9_f32)));
    assert_eq!(snapshot.crystal_system, "Trigonal");
    assert_eq!(snapshot.optical_character, "UniaxialNegative");
    assert_eq!(snapshot.body_color(), Some([0.2, 0.4, 0.6]));
    assert!(snapshot.dispersion_model.is_none());
    assert!(snapshot.color_recipe.is_none());

    // A row with no crystal optics writes empty text, as rows always did.
    let bare = plain("Bare").snapshot();
    assert_eq!(bare.crystal_system, "");
    assert_eq!(bare.optical_character, "");
}

#[test]
fn a_biaxial_rows_snapshot_restores_its_crystal_optics() {
    let row = LibraryMaterial {
        refractive_index: 1.65,
        birefringence: 0.009,
        crystal_system: Some("Orthorhombic"),
        optical_character: Some("BiaxialPositive"),
        biaxial_delta_beta_alpha: Some(0.004),
        ..plain("Biaxial")
    };
    let snapshot = row.snapshot();
    assert_eq!(
        snapshot.biaxial_delta_beta_alpha,
        Some(f64::from(0.004_f32))
    );

    let restored = gem_material_from_custom_snapshot("Biaxial", &snapshot);
    let direct = row.gem_material();
    assert_eq!(restored.crystal_system, CrystalSystem::Orthorhombic);
    assert_eq!(restored.crystal_system, direct.crystal_system);
    assert_eq!(restored.optical_character, direct.optical_character);
    assert_eq!(
        restored.biaxial_delta_beta_alpha,
        direct.biaxial_delta_beta_alpha
    );
    assert!(restored.biaxial_delta_beta_alpha.is_some());

    // A row with no biaxial delta writes no figure.
    assert!(plain("Bare").snapshot().biaxial_delta_beta_alpha.is_none());
}

/// A row that stores a biaxial delta but no readable names (a row written before the names were
/// stored) is biaxial by the optic-sign rule, directly and through the snapshot it writes.
#[test]
fn a_row_with_a_delta_and_no_names_is_biaxial_by_the_optic_sign_rule() {
    // Topaz-like (beta nearer alpha: positive) and andalusite-like (beta nearer gamma: negative).
    let cases = [
        (0.003_f32, OpticalCharacter::BiaxialPositive),
        (0.006, OpticalCharacter::BiaxialNegative),
    ];
    let unreadable = [
        (None, None),
        (Some(""), Some("")),
        (Some("Amorphous"), Some("Mystery")),
    ];
    for (delta, expected) in cases {
        for (crystal_system, optical_character) in unreadable {
            let row = LibraryMaterial {
                refractive_index: 1.609,
                birefringence: 0.011,
                crystal_system,
                optical_character,
                biaxial_delta_beta_alpha: Some(delta),
                ..plain("Biaxial")
            };
            let direct = row.gem_material();
            assert_eq!(
                direct.optical_character, expected,
                "{crystal_system:?} {optical_character:?} {delta}"
            );
            assert_eq!(direct.biaxial_delta_beta_alpha, Some(delta));

            // The snapshot writes empty names; the open path reaches the same character.
            let restored = gem_material_from_custom_snapshot("Biaxial", &row.snapshot());
            assert_eq!(restored.optical_character, expected, "{delta}");
            assert_eq!(restored.biaxial_delta_beta_alpha, Some(delta));
        }
    }

    // A readable name wins over the rule.
    let named = LibraryMaterial {
        refractive_index: 1.609,
        birefringence: 0.011,
        crystal_system: Some("Orthorhombic"),
        optical_character: Some("BiaxialNegative"),
        biaxial_delta_beta_alpha: Some(0.003),
        ..plain("Named")
    };
    assert_eq!(
        named.gem_material().optical_character,
        OpticalCharacter::BiaxialNegative
    );
}

/// The optic-sign rule on its own: `beta` nearer `alpha` is positive, the exact midpoint is
/// negative, and a delta that cannot describe a biaxial material gives no character.
#[test]
fn the_optic_sign_rule_picks_the_character_and_rejects_unusable_deltas() {
    use OpticalCharacter::{BiaxialNegative, BiaxialPositive};
    // Topaz (1.606 / 1.609 / 1.617) and andalusite (1.632 / 1.638 / 1.643).
    assert_eq!(
        biaxial_character_from_deltas(0.003, 0.011),
        Some(BiaxialPositive)
    );
    assert_eq!(
        biaxial_character_from_deltas(0.006, 0.011),
        Some(BiaxialNegative)
    );

    // The boundary 2 (beta - alpha) == gamma - alpha is negative; just under it is positive.
    // 0.25 and 0.5 are exact in binary, so the product is exactly the birefringence.
    assert_eq!(
        biaxial_character_from_deltas(0.25, 0.5),
        Some(BiaxialNegative)
    );
    assert_eq!(
        biaxial_character_from_deltas(0.249, 0.5),
        Some(BiaxialPositive)
    );
    // beta on gamma (delta == birefringence) is still negative, by the rule.
    assert_eq!(
        biaxial_character_from_deltas(0.5, 0.5),
        Some(BiaxialNegative)
    );
    // The sign of the birefringence is ignored.
    assert_eq!(
        biaxial_character_from_deltas(0.003, -0.011),
        Some(BiaxialPositive)
    );

    // Unusable pairs: a delta that is zero, negative or not finite, a birefringence that is not
    // finite or zero, and a delta above the birefringence.
    let unusable = [
        (0.0, 0.011),
        (-0.003, 0.011),
        (f64::NAN, 0.011),
        (f64::INFINITY, 0.011),
        (f64::NEG_INFINITY, 0.011),
        (0.003, f64::NAN),
        (0.003, f64::INFINITY),
        (0.003, 0.0),
        (0.012, 0.011),
    ];
    for (delta, birefringence) in unusable {
        assert_eq!(
            biaxial_character_from_deltas(delta, birefringence),
            None,
            "delta {delta}, birefringence {birefringence}"
        );
    }
}

#[test]
fn the_snapshot_carries_a_stored_model_and_a_recipe_with_its_fallback_colour() {
    let model = DispersionModel::Cauchy {
        a: 1.7,
        b: 0.006,
        c: 0.0,
    };
    let model_json = dispersion_model_to_json(&model);
    let mode = ColorMode::physics(ruby(), [0.4, 0.9, 1.8]);
    let recipe_json = mode.to_json();
    let row = LibraryMaterial {
        absorption_rgb: [0.1, 0.2, 0.3],
        color_recipe_json: Some(&recipe_json),
        dispersion_model_json: Some(&model_json),
        absorption_bands_json: None,
        ..plain("Ruby")
    };
    let snapshot = row.snapshot();
    assert_eq!(
        snapshot
            .dispersion_model
            .as_ref()
            .and_then(crate::native::dispersion_model_from_dto),
        Some(model)
    );
    let dto = snapshot.color_recipe.expect("a recipe is carried");
    // The fallback colour is recorded exactly as the top-level colour was written.
    assert_eq!(Some(dto.fallback_rgb), snapshot.absorption_rgb);
    assert_eq!(ColorMode::from_json(&dto.recipe_json), Some(mode));

    // A fantasy colour (a payload with no recipe) writes no recipe at all.
    let fantasy_json = ColorMode::fantasy([0.4, 0.9, 1.8]).to_json();
    let fantasy = LibraryMaterial {
        color_recipe_json: Some(&fantasy_json),
        ..plain("Ruby")
    };
    assert!(fantasy.snapshot().color_recipe.is_none());
}

#[test]
fn a_snapshot_restores_the_material_the_row_describes() {
    let row = LibraryMaterial {
        absorption_rgb: [0.2, 0.4, 0.6],
        ..plain("Round trip")
    };
    let restored = gem_material_from_custom_snapshot("Round trip", &row.snapshot());
    let direct = row.gem_material();
    assert_eq!(restored.dispersion, direct.dispersion);
    assert_eq!(restored.absorption, direct.absorption);
}

const BANDS: [[f32; 3]; 2] = [[460.0, 45.0, 0.25], [540.0, 45.0, 0.1]];

#[test]
fn band_rows_round_trip_through_their_json_and_bad_text_is_none() {
    let json = absorption_bands_to_json(&BANDS).expect("rows serialise");
    assert_eq!(absorption_bands_from_json(&json), Some(BANDS.to_vec()));
    assert_eq!(absorption_bands_to_json(&[]), None);
    assert_eq!(absorption_bands_from_json(""), None);
    assert_eq!(absorption_bands_from_json("[]"), None);
    assert_eq!(absorption_bands_from_json("not json"), None);
    assert_eq!(absorption_bands_from_json("[[460.0,0.0,0.25]]"), None);
    assert_eq!(absorption_bands_from_json("[[460.0,45.0,-1.0]]"), None);
}

#[test]
fn a_row_with_bands_is_coloured_by_them_and_without_bands_by_the_triple() {
    let json = absorption_bands_to_json(&BANDS).expect("rows serialise");
    let banded = LibraryMaterial {
        absorption_rgb: [0.2, 0.4, 0.6],
        absorption_bands_json: Some(&json),
        ..plain("Banded")
    };
    let triple_only = LibraryMaterial {
        absorption_rgb: [0.2, 0.4, 0.6],
        ..plain("Banded")
    };
    assert_ne!(
        banded.gem_material().absorption,
        triple_only.gem_material().absorption
    );
    assert!(banded.gem_material().absorption_path_scale > 1.0);
    // Unusable text falls back to the triple, exactly as a row without the column.
    let broken = LibraryMaterial {
        absorption_bands_json: Some("[[460.0,0.0,0.25]]"),
        ..triple_only
    };
    assert_eq!(broken.gem_material(), triple_only.gem_material());
}

#[test]
fn a_banded_row_carries_its_bands_into_the_file_snapshot() {
    let json = absorption_bands_to_json(&BANDS).expect("rows serialise");
    let banded = LibraryMaterial {
        absorption_bands_json: Some(&json),
        ..plain("Banded")
    };
    assert!(banded.snapshot().absorption_bands().is_some());
    assert!(plain("Plain").snapshot().absorption_bands().is_none());
}

// ---- absorption units (lane AU) ----

fn sized(width_mm: f32) -> indicatrix::render_setup::MaterialOverrides {
    indicatrix::render_setup::MaterialOverrides {
        inclusion_sigma_s: 0.0,
        c_axis_override: None,
        edge_rounding_radius: 0.0,
        stone_width_mm: width_mm,
    }
}

/// B3: the vault row and the design file's custom-material snapshot of the same banded custom material
/// build the identical material (absorption, unit and scale), and the render setup gives both
/// `W / model_width` (7 mm while no size is set).
#[test]
fn a_banded_row_and_its_snapshot_build_the_same_per_mm_material() {
    let json = absorption_bands_to_json(&BANDS).expect("rows serialise");
    let banded = LibraryMaterial {
        absorption_bands_json: Some(&json),
        ..plain("Banded")
    };
    let from_row = banded.gem_material();
    let from_file = gem_material_from_custom_snapshot("Banded", &banded.snapshot());
    assert_eq!(from_row.absorption, from_file.absorption);
    assert_eq!(from_row.absorption_unit, AbsorptionUnit::PerMm);
    assert_eq!(from_file.absorption_unit, AbsorptionUnit::PerMm);
    assert_eq!(
        from_row.absorption_path_scale,
        from_file.absorption_path_scale
    );
    let model_width = 2.3_f64;
    for (width_mm, expected_mm) in [(0.0_f32, 7.0_f64), (10.87, f64::from(10.87_f32))] {
        let row = indicatrix::render_setup::apply_material_overrides(
            from_row.clone(),
            &sized(width_mm),
            Some(model_width),
        );
        let file = indicatrix::render_setup::apply_material_overrides(
            from_file.clone(),
            &sized(width_mm),
            Some(model_width),
        );
        assert_eq!(row, file);
        assert!((f64::from(row.absorption_path_scale) - expected_mm / model_width).abs() < 1e-5);
    }
}

/// A physics recipe row is per millimetre too (a pure-host one included).
#[test]
fn a_physics_recipe_row_is_per_mm_and_scales_with_the_model_width() {
    let mode = ColorMode::physics(ruby(), [0.0; 3]);
    let json = mode.to_json();
    let row = LibraryMaterial {
        color_recipe_json: Some(&json),
        ..plain("Physics Ruby")
    };
    let material = row.gem_material();
    assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
    let one = indicatrix::render_setup::apply_material_overrides(
        material.clone(),
        &sized(0.0),
        Some(2.0),
    );
    let two = indicatrix::render_setup::apply_material_overrides(material, &sized(0.0), Some(4.0));
    assert!((one.absorption_path_scale - 3.5).abs() < 1e-6);
    assert!((two.absorption_path_scale - 1.75).abs() < 1e-6);
}
