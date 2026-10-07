//! Tests for [`LibraryMaterial`]: the material and the file snapshot built from a library row.

use super::*;
use crate::native::{dispersion_model_to_json, gem_material_from_custom_snapshot};
use indicatrix::optics::{
    chromophore::{ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve},
    dispersion::DispersionModel,
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
