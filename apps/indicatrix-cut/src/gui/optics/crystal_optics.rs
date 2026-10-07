//! Crystal-classification <-> persistence conversions for the custom-material editor
//! (letting a user set `crystal_system`/`optical_character`/
//! `biaxial_delta_beta_alpha` explicitly instead of only ever getting
//! `GemMaterial::new_custom`'s two-variant inference).
//!
//! `indicatrix-vault`'s `CustomMaterialRow` deliberately does not depend on `indicatrix`
//! (see that struct's own doc comment), so it stores `crystal_system`/
//! `optical_character` as plain `Option<String>` -- a `indicatrix` enum variant's `Debug`
//! name, e.g. `"Trigonal"`. This module is where the Slint-combo-index <-> enum
//! conversions live; the string <-> enum ones (and the library row -> `GemMaterial`
//! build, shared with the command line) are in `indicatrix_cut_core::material`. No
//! dependency on Slint itself -- pure data, exercised directly by the unit tests,
//! matching this app's `gui::optics::c_axis`/`gui::render::sample_scale` precedent for
//! where such a helper lives.

use indicatrix::optics::{
    chromophore::ChromophoreCatalogue,
    fluorescence::Fluorescence,
    materials::{CrystalSystem, GemMaterial, OpticalCharacter},
};
// The stored spelling of a crystal system and an optical character (the variant's `Debug`
// name, e.g. "Trigonal") lives in `indicatrix-cut-core`, next to the code the command line
// shares: `crystal_system_name` / `crystal_system_from_name` and the two for the optical
// character. A name nobody knows reads back as `None`, which a caller treats exactly like an
// absent row field.
use indicatrix_cut_core::material::{
    ColorMode, LibraryMaterial, crystal_system_name, optical_character_name,
};
use indicatrix_vault::{
    db::sqlite::{CustomMaterialParams, Database},
    model::material::CustomMaterialRow,
};
use std::sync::Arc;

/// `CrystalSystem`'s 7 variants, in the exact order `material_editor_dialog.slint`'s
/// crystal-system `ComboBox` lists them -- a combo index round-trips through this
/// order and [`crystal_system_from_index`]/[`crystal_system_to_index`] below.
const CRYSTAL_SYSTEMS: [CrystalSystem; 7] = [
    CrystalSystem::Cubic,
    CrystalSystem::Tetragonal,
    CrystalSystem::Hexagonal,
    CrystalSystem::Trigonal,
    CrystalSystem::Orthorhombic,
    CrystalSystem::Monoclinic,
    CrystalSystem::Triclinic,
];

/// `OpticalCharacter`'s 5 variants, in the exact order
/// `material_editor_dialog.slint`'s optical-character `ComboBox` lists them.
const OPTICAL_CHARACTERS: [OpticalCharacter; 5] = [
    OpticalCharacter::Isotropic,
    OpticalCharacter::UniaxialPositive,
    OpticalCharacter::UniaxialNegative,
    OpticalCharacter::BiaxialPositive,
    OpticalCharacter::BiaxialNegative,
];

/// `material_editor_dialog.slint`'s crystal-system `ComboBox` current-index -> enum.
/// `None` for an out-of-range index (defensive only -- the combo itself can never
/// produce one).
#[must_use]
pub fn crystal_system_from_index(idx: i32) -> Option<CrystalSystem> {
    usize::try_from(idx)
        .ok()
        .and_then(|idx| CRYSTAL_SYSTEMS.get(idx).copied())
}

/// `material_editor_dialog.slint`'s optical-character `ComboBox` current-index ->
/// enum. `None` for an out-of-range index (defensive only).
#[must_use]
pub fn optical_character_from_index(idx: i32) -> Option<OpticalCharacter> {
    usize::try_from(idx)
        .ok()
        .and_then(|idx| OPTICAL_CHARACTERS.get(idx).copied())
}

/// Inverse of [`crystal_system_from_index`] -- the material-editor pre-fill needs to
/// go the other way, from a saved row's real [`CrystalSystem`] back to the combo
/// index that selects it. `CRYSTAL_SYSTEMS` covers every variant, so this always
/// finds a match.
#[must_use]
pub fn crystal_system_to_index(cs: CrystalSystem) -> i32 {
    CRYSTAL_SYSTEMS
        .iter()
        .position(|&candidate| candidate == cs)
        .and_then(|idx| i32::try_from(idx).ok())
        .unwrap_or(0)
}

/// Inverse of [`optical_character_from_index`] -- see [`crystal_system_to_index`]'s
/// own doc comment for why the pre-fill needs this direction too.
#[must_use]
pub fn optical_character_to_index(oc: OpticalCharacter) -> i32 {
    OPTICAL_CHARACTERS
        .iter()
        .position(|&candidate| candidate == oc)
        .and_then(|idx| i32::try_from(idx).ok())
        .unwrap_or(0)
}

/// Whether `optical_character` is one of the two biaxial variants -- the only case
/// `biaxial_delta_beta_alpha` means anything (see that field's own doc comment on
/// `GemMaterial`) and the only case the dialog's delta-beta-alpha control is enabled
/// for.
#[must_use]
pub const fn is_biaxial(oc: OpticalCharacter) -> bool {
    matches!(
        oc,
        OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative
    )
}

/// Builds a `GemMaterial` from a persisted `CustomMaterialRow`, applying
/// `GemMaterial::new_custom`'s own two-variant inference (Cubic/Trigonal crystal
/// system, Isotropic/Uniaxial+-/- optical character from the sign of
/// `birefringence`) for any crystal-optics field the row doesn't have stored.
///
/// This is what keeps a custom material saved before these fields existed rendering
/// bit-identically: such a row's `crystal_system`/`optical_character`/
/// `biaxial_delta_beta_alpha` are all `None` (the column didn't exist yet, so the
/// migration left them `NULL` -- see `Database::migrate_crystal_optics_columns`), so
/// every one of those three fields on the returned `GemMaterial` stays exactly what
/// `new_custom` already infers, unchanged.
///
/// A row whose `dispersion_model_json` holds a usable Sellmeier or Cauchy model builds its
/// curve with `GemMaterial::new_custom_with_dispersion`; a row without one (every row saved
/// before the column existed, and every refractive-index-and-dispersion row) or with one
/// that fails `DispersionModel::validate` takes the `new_custom` path from `refractive_index`
/// and `dispersion`, exactly as before.
///
/// The work is `indicatrix_cut_core::material::LibraryMaterial::gem_material`, which the
/// command line shares; this function only copies the row's fields into it.
#[must_use]
pub fn gem_material_from_row(row: &CustomMaterialRow) -> GemMaterial {
    library_material(row).gem_material()
}

/// The fields of `row` as the plain values `indicatrix-cut-core` builds a material and a
/// file snapshot from (the vault crate cannot name that type, so the copy is made here).
#[must_use]
pub fn library_material(row: &CustomMaterialRow) -> LibraryMaterial<'_> {
    LibraryMaterial {
        name: &row.name,
        refractive_index: row.refractive_index,
        dispersion: row.dispersion,
        birefringence: row.birefringence,
        absorption_rgb: row.absorption_rgb,
        crystal_system: row.crystal_system.as_deref(),
        optical_character: row.optical_character.as_deref(),
        biaxial_delta_beta_alpha: row.biaxial_delta_beta_alpha,
        specific_gravity: row.specific_gravity,
        color_recipe_json: row.color_recipe_json.as_deref(),
        dispersion_model_json: row.dispersion_model_json.as_deref(),
        absorption_bands_json: row.absorption_bands_json.as_deref(),
    }
}

/// Saves `material`'s crystal-optics fields (name, RI, dispersion, birefringence,
/// absorption, plus the three crystal-optics fields) via [`Database::save_custom_material`],
/// converting `crystal_system`/`optical_character` to their persisted string form and
/// passing `biaxial_delta_beta_alpha` through as-is (already `None` for every
/// non-biaxial material).
///
/// `ri`/`dispersion`/`birefringence`/`absorption_rgb` are NOT redundant with `material`
/// itself, so this stays four loose scalars alongside `&GemMaterial` rather than reading
/// them back off it: the caller's dialog builds `material` FROM these exact scalars via
/// `GemMaterial::new_custom`, which expands each into a richer, non-invertible
/// representation (`dispersion` a full `DispersionModel` curve, `absorption_rgb` an
/// `AbsorptionTensor`) -- there is no `material.dispersion_scalar`/`material.absorption_rgb`
/// to read these back from, only the curve/tensor they were expanded into. Persisting the
/// original edit-buffer values (rather than, say, sampling the curve back down to one
/// number) is what lets a later load reconstruct the identical `GemMaterial` via the same
/// `new_custom` constructor.
///
/// # Errors
///
/// Returns whatever [`Database::save_custom_material`] returns.
///
/// The editor saves through [`save_gem_material_fields`] (which also carries the dispersion
/// model); this loose-scalar form is what the tests that predate the model still call.
#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the vault row's columns; a params struct would only move the list"
)]
pub fn save_gem_material(
    db: &Database,
    material: &GemMaterial,
    ri: f32,
    dispersion: f32,
    birefringence: f32,
    absorption_rgb: [f32; 3],
    specific_gravity: Option<f32>,
    color_recipe_json: Option<&str>,
) -> anyhow::Result<()> {
    save_gem_material_fields(
        db,
        material,
        &MaterialSaveFields {
            ri,
            dispersion,
            birefringence,
            absorption_rgb,
            specific_gravity,
            color_recipe_json,
            dispersion_model_json: None,
            absorption_bands_json: None,
        },
    )
}

/// The editor values [`save_gem_material_fields`] stores next to a material's crystal
/// optics: the loose scalars [`save_gem_material`] documents, plus the dispersion model.
#[derive(Debug, Clone, Copy)]
pub struct MaterialSaveFields<'a> {
    /// The refractive index (`n_d` of the model when `dispersion_model_json` is set).
    pub ri: f32,
    /// The Fraunhofer `n_F - n_C` figure (the model's own when `dispersion_model_json` is
    /// set).
    pub dispersion: f32,
    /// The birefringence.
    pub birefringence: f32,
    /// The absorption triple an older build reads as the color.
    pub absorption_rgb: [f32; 3],
    /// The specific gravity, `None` for "not recorded".
    pub specific_gravity: Option<f32>,
    /// The serialized `ColorMode`, stored exactly as given.
    pub color_recipe_json: Option<&'a str>,
    /// The dispersion model as the JSON `dispersion_model_to_json` writes, or `None` for
    /// the plain refractive-index-and-dispersion path.
    pub dispersion_model_json: Option<&'a str>,
    /// The seven-band body colour as the JSON `absorption_bands_to_json` writes, or `None` for "no
    /// bands" (the triple colours the material).
    pub absorption_bands_json: Option<&'a str>,
}

/// [`save_gem_material`] with the dispersion model: the same row, plus the
/// `dispersion_model_json` column.
///
/// # Errors
///
/// Returns whatever [`Database::save_custom_material`] returns.
pub fn save_gem_material_fields(
    db: &Database,
    material: &GemMaterial,
    fields: &MaterialSaveFields<'_>,
) -> anyhow::Result<()> {
    db.save_custom_material(&CustomMaterialParams {
        name: &material.name,
        refractive_index: fields.ri,
        dispersion: fields.dispersion,
        birefringence: fields.birefringence,
        absorption_rgb: fields.absorption_rgb,
        crystal_system: Some(crystal_system_name(material.crystal_system)),
        optical_character: Some(optical_character_name(material.optical_character)),
        biaxial_delta_beta_alpha: material.biaxial_delta_beta_alpha,
        // This UI does not yet author per-axis dispersion data (that would need its
        // own editor surface, out of scope here) -- `None`, the same "not stored"
        // state every row saved before this column existed already has.
        per_axis_dispersion_json: None,
        specific_gravity: fields.specific_gravity,
        color_recipe_json: fields.color_recipe_json,
        dispersion_model_json: fields.dispersion_model_json,
        absorption_bands_json: fields.absorption_bands_json,
    })
}

/// The names of the custom materials in `rows` whose stored color mode is Physics: the value
/// of `RenderContext::custom_material_physics`, so the render, export and remote scene paths
/// know which materials take the 7 mm default stone width.
#[must_use]
pub fn custom_material_physics_from_rows(rows: &[CustomMaterialRow]) -> Vec<String> {
    rows.iter()
        .filter(|row| {
            row.color_recipe_json
                .as_deref()
                .and_then(ColorMode::from_json)
                .is_some_and(|m| m.is_physics())
        })
        .map(|row| row.name.clone())
        .collect()
}

/// The fluorescence of the custom materials in `rows` whose stored color mode is Physics,
/// resolved from their recipes: the value of `RenderContext::custom_material_fluorescence`.
/// Materials without an emitter are left out.
#[must_use]
pub fn custom_material_fluorescence_from_rows(
    rows: &[CustomMaterialRow],
) -> Vec<(String, Arc<Fluorescence>)> {
    let catalogue = ChromophoreCatalogue::global();
    rows.iter()
        .filter_map(|row| {
            let mode = ColorMode::from_json(row.color_recipe_json.as_deref()?)?;
            let glow = mode.fluorescence(catalogue);
            (!glow.is_empty()).then(|| (row.name.clone(), Arc::new(glow)))
        })
        .collect()
}

/// Builds `RenderContext::custom_material_specific_gravity`'s value from a set of
/// custom-material database rows: every row that has a recorded
/// [`CustomMaterialRow::specific_gravity`], paired with its name.
///
/// The read-side half of getting a custom material's specific gravity from the
/// database into the carat-weight estimate -- see `RenderContext::
/// custom_material_specific_gravity`'s own doc comment for the full picture and why
/// this is a parallel side channel rather than a field on [`GemMaterial`] itself.
/// Called by `gui::optics::custom_materials`'s save/delete callbacks, so the side
/// channel never drifts out of sync with `custom_materials` itself, AND by
/// `gui::mod`'s startup load (`build_main_window`), from the exact same row read
/// that builds `initial_custom_mats` -- so `RenderContext::
/// custom_material_specific_gravity` is populated from the very first frame,
/// rather than starting empty until this session's own save/delete callback runs
/// at least once.
#[must_use]
pub fn custom_material_specific_gravity_from_rows(
    rows: &[CustomMaterialRow],
) -> Vec<(String, f64)> {
    rows.iter()
        .filter_map(|row| {
            row.specific_gravity
                .map(|sg| (row.name.clone(), f64::from(sg)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{
        material::{crystal_system_from_name, optical_character_from_name},
        native::dispersion_model_from_json,
    };

    #[test]
    fn crystal_system_str_round_trips_all_seven_variants() {
        for cs in CRYSTAL_SYSTEMS {
            let s = crystal_system_name(cs);
            assert_eq!(crystal_system_from_name(s), Some(cs), "variant={cs:?}");
        }
    }

    #[test]
    fn optical_character_str_round_trips_all_five_variants() {
        for oc in OPTICAL_CHARACTERS {
            let s = optical_character_name(oc);
            assert_eq!(optical_character_from_name(s), Some(oc), "variant={oc:?}");
        }
    }

    #[test]
    fn crystal_system_index_matches_the_declared_enum_order() {
        // `material_editor_dialog.slint`'s crystal-system `ComboBox` lists the seven
        // options in exactly this order -- pins the mapping so a reordering of either
        // side is caught here rather than silently mis-mapping a saved index.
        for (idx, cs) in CRYSTAL_SYSTEMS.iter().enumerate() {
            assert_eq!(
                crystal_system_from_index(i32::try_from(idx).unwrap()),
                Some(*cs),
                "idx={idx}"
            );
        }
    }

    #[test]
    fn optical_character_index_matches_the_declared_enum_order() {
        for (idx, oc) in OPTICAL_CHARACTERS.iter().enumerate() {
            assert_eq!(
                optical_character_from_index(i32::try_from(idx).unwrap()),
                Some(*oc),
                "idx={idx}"
            );
        }
    }

    #[test]
    fn crystal_system_to_index_round_trips_through_from_index_for_every_variant() {
        for cs in CRYSTAL_SYSTEMS {
            let idx = crystal_system_to_index(cs);
            assert_eq!(crystal_system_from_index(idx), Some(cs), "variant={cs:?}");
        }
    }

    #[test]
    fn optical_character_to_index_round_trips_through_from_index_for_every_variant() {
        for oc in OPTICAL_CHARACTERS {
            let idx = optical_character_to_index(oc);
            assert_eq!(
                optical_character_from_index(idx),
                Some(oc),
                "variant={oc:?}"
            );
        }
    }

    #[test]
    fn unrecognized_strings_and_out_of_range_indices_return_none() {
        assert_eq!(crystal_system_from_name("Amorphous"), None);
        assert_eq!(optical_character_from_name(""), None);
        assert_eq!(crystal_system_from_index(-1), None);
        assert_eq!(crystal_system_from_index(99), None);
        assert_eq!(optical_character_from_index(-1), None);
        assert_eq!(optical_character_from_index(99), None);
    }

    #[test]
    fn is_biaxial_is_true_only_for_the_two_biaxial_variants() {
        assert!(is_biaxial(OpticalCharacter::BiaxialPositive));
        assert!(is_biaxial(OpticalCharacter::BiaxialNegative));
        assert!(!is_biaxial(OpticalCharacter::Isotropic));
        assert!(!is_biaxial(OpticalCharacter::UniaxialPositive));
        assert!(!is_biaxial(OpticalCharacter::UniaxialNegative));
    }

    /// The load-time regression this whole module exists to guarantee: an existing
    /// custom material has no stored crystal system; when one is absent, fall back to
    /// exactly what `new_custom` infers today, so nothing a user already saved changes
    /// appearance. A row with all three crystal-optics fields `None` -- exactly what
    /// every such row looks like after the migration -- must produce a `GemMaterial`
    /// identical to calling `new_custom` directly.
    #[test]
    fn row_with_no_stored_crystal_optics_falls_back_to_new_custom_inference() {
        let row = CustomMaterialRow {
            name: "Legacy Custom Sapphire".to_string(),
            refractive_index: 1.768,
            dispersion: 0.018,
            birefringence: -0.008,
            absorption_rgb: [2.8, 1.2, 0.1],
            crystal_system: None,
            optical_character: None,
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity: None,
            color_recipe_json: None,
            dispersion_model_json: None,
            absorption_bands_json: None,
        };
        let from_row = gem_material_from_row(&row);
        let inferred = GemMaterial::new_custom(
            &row.name,
            row.refractive_index,
            row.dispersion,
            row.birefringence,
            row.absorption_rgb,
        );
        assert_eq!(from_row.crystal_system, inferred.crystal_system);
        assert_eq!(from_row.optical_character, inferred.optical_character);
        assert_eq!(
            from_row.biaxial_delta_beta_alpha,
            inferred.biaxial_delta_beta_alpha
        );
        // Sanity: this legacy row's negative birefringence must have inferred
        // Trigonal/UniaxialNegative, not silently fallen back to isotropic Cubic.
        assert_eq!(from_row.crystal_system, CrystalSystem::Trigonal);
        assert_eq!(
            from_row.optical_character,
            OpticalCharacter::UniaxialNegative
        );
    }

    /// A row WITH stored crystal-optics fields must override `new_custom`'s inference
    /// -- the whole point of this feature -- including reaching a `BiaxialPositive`/
    /// `Some(delta)` combination `new_custom` itself can never produce on its own.
    ///
    /// The final assertion reflects that `gpu_supported()` supports biaxial materials
    /// (see `indicatrix::optics::materials::GemMaterial::gpu_supported`'s own
    /// doc comment for the eigenvector-conditioning fix that makes this safe), so a
    /// row with
    /// `biaxial_delta_beta_alpha = Some(_)` is GPU-supported like every other material,
    /// not excluded from it.
    #[test]
    fn row_with_stored_crystal_optics_overrides_new_custom_inference() {
        let row = CustomMaterialRow {
            name: "Custom Tanzanite".to_string(),
            refractive_index: 1.691,
            dispersion: 0.030,
            birefringence: 0.0130,
            absorption_rgb: [1.8, 1.6, 0.2],
            crystal_system: Some("Orthorhombic".to_string()),
            optical_character: Some("BiaxialPositive".to_string()),
            biaxial_delta_beta_alpha: Some(0.0070),
            per_axis_dispersion_json: None,
            specific_gravity: None,
            color_recipe_json: None,
            dispersion_model_json: None,
            absorption_bands_json: None,
        };
        let material = gem_material_from_row(&row);
        assert_eq!(material.crystal_system, CrystalSystem::Orthorhombic);
        assert_eq!(
            material.optical_character,
            OpticalCharacter::BiaxialPositive
        );
        assert_eq!(material.biaxial_delta_beta_alpha, Some(0.0070));
        assert!(material.gpu_supported());
    }

    // --- custom_material_specific_gravity_from_rows ---

    fn row_named(name: &str, specific_gravity: Option<f32>) -> CustomMaterialRow {
        CustomMaterialRow {
            name: name.to_string(),
            refractive_index: 1.7,
            dispersion: 0.02,
            birefringence: 0.0,
            absorption_rgb: [0.0, 0.0, 0.0],
            crystal_system: None,
            optical_character: None,
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity,
            color_recipe_json: None,
            dispersion_model_json: None,
            absorption_bands_json: None,
        }
    }

    /// Only rows with a recorded SG contribute an entry -- a row with `None` (the
    /// state every save through this app produces today, per `save_gem_material`'s
    /// own doc comment) is skipped rather than showing up as a spurious `Some(0.0)`.
    #[test]
    fn only_rows_with_a_recorded_sg_produce_an_entry() {
        let rows = [
            row_named("My Garnet", Some(3.90)),
            row_named("Custom Diamond", None),
        ];
        let entries = custom_material_specific_gravity_from_rows(&rows);
        // Exact `f32` -> `f64` widening (no arithmetic in between), but still not
        // literal-equal to a directly-typed `f64` -- see the `f32`/`f64` binary
        // representations of 3.90 -- so this compares by name and by tolerance
        // rather than a brittle `assert_eq!` against a `vec![...]` literal.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "My Garnet");
        assert!((entries[0].1 - 3.90_f64).abs() < 1e-6);
    }

    #[test]
    fn an_empty_row_set_produces_no_entries() {
        assert_eq!(
            custom_material_specific_gravity_from_rows(&[]),
            Vec::<(String, f64)>::new()
        );
    }

    /// `f32` -> `f64` widening must not distort the stored value beyond ordinary
    /// float-widening precision.
    #[test]
    fn widens_the_stored_f32_without_meaningful_precision_loss() {
        let rows = [row_named("My Garnet", Some(3.9))];
        let entries = custom_material_specific_gravity_from_rows(&rows);
        assert!((entries[0].1 - 3.9_f64).abs() < 1e-6);
    }

    // --- dispersion model column ---

    const BK7_JSON: &str = r#"{"kind":"sellmeier3","b":[1.0396122,0.23179235,1.0104694],"c":[0.006000699,0.020017914,103.56065]}"#;

    fn row_with_model(json: Option<&str>) -> CustomMaterialRow {
        CustomMaterialRow {
            refractive_index: 1.5168,
            dispersion: 0.008,
            dispersion_model_json: json.map(str::to_string),
            ..row_named("My Glass", None)
        }
    }

    /// A row without a model is the `new_custom` Cauchy fit of its two scalars, curve
    /// included: nothing saved before the column existed changes.
    #[test]
    fn a_row_without_a_model_keeps_the_plain_cauchy_fit() {
        let row = row_with_model(None);
        let material = gem_material_from_row(&row);
        let plain = GemMaterial::new_custom(
            &row.name,
            row.refractive_index,
            row.dispersion,
            row.birefringence,
            row.absorption_rgb,
        );
        assert_eq!(material.dispersion, plain.dispersion);
        assert_eq!(material, plain);
    }

    /// A row with a model renders with that model, not the flat fit of its scalars.
    #[test]
    fn a_row_with_a_model_builds_that_curve() {
        let row = row_with_model(Some(BK7_JSON));
        let material = gem_material_from_row(&row);
        let model = dispersion_model_from_json(BK7_JSON).expect("the fixture is a valid model");
        assert_eq!(material.dispersion, model);
        assert!(matches!(
            material.dispersion,
            indicatrix::optics::dispersion::DispersionModel::Sellmeier3 { .. }
        ));
        assert!((material.dispersion.n_d() - 1.5168).abs() < 0.002);
        // Crystal optics still come from the same rules as the plain path.
        assert_eq!(material.crystal_system, CrystalSystem::Cubic);
        assert_eq!(material.optical_character, OpticalCharacter::Isotropic);
    }

    /// A stored model that cannot render (a resonance at 600 nm, text that is not a
    /// model) is ignored, and the row takes the plain path instead of drawing a clear block.
    #[test]
    fn an_unusable_stored_model_falls_back_to_the_plain_path() {
        let plain = gem_material_from_row(&row_with_model(None));
        for json in [
            r#"{"kind":"sellmeier1","b1":1.0,"c1":0.36}"#,
            r#"{"kind":"cauchy","a":0.5,"b":0.0,"c":0.0}"#,
            "not json",
            "",
        ] {
            let material = gem_material_from_row(&row_with_model(Some(json)));
            assert_eq!(material, plain, "{json}");
        }
    }

    /// The model written by the save path comes back from the vault as the same curve, and
    /// saving without one clears the column.
    #[test]
    fn the_vault_round_trips_the_dispersion_model() {
        let db = Database::new(Some(":memory:")).expect("in-memory database");
        let model = dispersion_model_from_json(BK7_JSON).expect("valid model");
        let material = GemMaterial::new_custom_with_dispersion("My Glass", model, 0.0, [0.0; 3]);
        let json = indicatrix_cut_core::native::dispersion_model_to_json(&model);
        let fields = MaterialSaveFields {
            ri: model.n_d(),
            dispersion: model.delta_f_c(),
            birefringence: 0.0,
            absorption_rgb: [0.0; 3],
            specific_gravity: None,
            color_recipe_json: None,
            dispersion_model_json: Some(&json),
            absorption_bands_json: None,
        };
        save_gem_material_fields(&db, &material, &fields).expect("save");
        let rows = db.get_custom_materials().expect("read");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].dispersion_model_json.as_deref(),
            Some(json.as_str())
        );
        assert_eq!(gem_material_from_row(&rows[0]).dispersion, model);

        // The stored scalars are the model's own n_d and n_F - n_C.
        assert!((rows[0].refractive_index - model.n_d()).abs() < 1e-6);
        assert!((rows[0].dispersion - model.delta_f_c()).abs() < 1e-6);

        // Back to the plain path: the legacy saver writes no model.
        save_gem_material(&db, &material, 1.5168, 0.008, 0.0, [0.0; 3], None, None)
            .expect("save plain");
        let rows = db.get_custom_materials().expect("read again");
        assert_eq!(rows[0].dispersion_model_json, None);
    }
}
