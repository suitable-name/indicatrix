//! Crystal-classification <-> persistence conversions for the custom-material editor
//! (letting a user set `crystal_system`/`optical_character`/
//! `biaxial_delta_beta_alpha` explicitly instead of only ever getting
//! `GemMaterial::new_custom`'s two-variant inference).
//!
//! `indicatrix-vault`'s `CustomMaterialRow` deliberately does not depend on `indicatrix`
//! (see that struct's own doc comment), so it stores `crystal_system`/
//! `optical_character` as plain `Option<String>` -- a `indicatrix` enum variant's `Debug`
//! name, e.g. `"Trigonal"`. This module is the one place that knows both types, and
//! is where the string <-> enum and Slint-combo-index <-> enum conversions live. No
//! dependency on Slint itself -- pure data, exercised directly by the unit tests,
//! matching this app's `gui::c_axis`/`gui::sample_scale` precedent for where such a
//! helper lives.

use indicatrix::optics::materials::{CrystalSystem, GemMaterial, OpticalCharacter};
use indicatrix_vault::{
    db::sqlite::{CustomMaterialParams, Database},
    model::material::CustomMaterialRow,
};

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

/// `CrystalSystem`'s `Debug` name, e.g. `"Trigonal"` -- what gets persisted in
/// `CustomMaterialRow::crystal_system`/the `custom_gem_materials.crystal_system`
/// column.
#[must_use]
pub const fn crystal_system_to_str(cs: CrystalSystem) -> &'static str {
    match cs {
        CrystalSystem::Cubic => "Cubic",
        CrystalSystem::Tetragonal => "Tetragonal",
        CrystalSystem::Hexagonal => "Hexagonal",
        CrystalSystem::Trigonal => "Trigonal",
        CrystalSystem::Orthorhombic => "Orthorhombic",
        CrystalSystem::Monoclinic => "Monoclinic",
        CrystalSystem::Triclinic => "Triclinic",
    }
}

/// Inverse of [`crystal_system_to_str`]. `None` for anything unrecognized (a
/// hand-edited database, or a future variant this build predates) -- the caller must
/// treat that exactly like an absent (`None`) row field: fall back to
/// `GemMaterial::new_custom`'s own inference rather than guessing.
#[must_use]
pub fn crystal_system_from_str(s: &str) -> Option<CrystalSystem> {
    CRYSTAL_SYSTEMS
        .iter()
        .copied()
        .find(|cs| crystal_system_to_str(*cs) == s)
}

/// `OpticalCharacter`'s `Debug` name, e.g. `"UniaxialNegative"` -- what gets
/// persisted in `CustomMaterialRow::optical_character`/the
/// `custom_gem_materials.optical_character` column.
#[must_use]
pub const fn optical_character_to_str(oc: OpticalCharacter) -> &'static str {
    match oc {
        OpticalCharacter::Isotropic => "Isotropic",
        OpticalCharacter::UniaxialPositive => "UniaxialPositive",
        OpticalCharacter::UniaxialNegative => "UniaxialNegative",
        OpticalCharacter::BiaxialPositive => "BiaxialPositive",
        OpticalCharacter::BiaxialNegative => "BiaxialNegative",
    }
}

/// Inverse of [`optical_character_to_str`]. Same "unrecognized falls back to `None`,
/// treated as inference" contract as [`crystal_system_from_str`].
#[must_use]
pub fn optical_character_from_str(s: &str) -> Option<OpticalCharacter> {
    OPTICAL_CHARACTERS
        .iter()
        .copied()
        .find(|oc| optical_character_to_str(*oc) == s)
}

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
#[must_use]
pub fn gem_material_from_row(row: &CustomMaterialRow) -> GemMaterial {
    let mut material = GemMaterial::new_custom(
        &row.name,
        row.refractive_index,
        row.dispersion,
        row.birefringence,
        row.absorption_rgb,
    );
    if let Some(cs) = row
        .crystal_system
        .as_deref()
        .and_then(crystal_system_from_str)
    {
        material.crystal_system = cs;
    }
    if let Some(oc) = row
        .optical_character
        .as_deref()
        .and_then(optical_character_from_str)
    {
        material.optical_character = oc;
    }
    if let Some(delta) = row.biaxial_delta_beta_alpha {
        material.biaxial_delta_beta_alpha = Some(delta);
    }
    material
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
pub fn save_gem_material(
    db: &Database,
    material: &GemMaterial,
    ri: f32,
    dispersion: f32,
    birefringence: f32,
    absorption_rgb: [f32; 3],
    // CAD audit item 169: the Material Editor dialog already collects this as
    // `sg_val` (`material_editor_dialog.slint`), but `ViewportModel.save_custom_material`
    // does not yet carry it as a 10th argument -- that Slint global signature change
    // is the one remaining link, owned outside `gui/optics`. Until it lands, every
    // caller passes `None` here; this parameter exists so wiring the real value
    // through is a one-line change at the (single) call site rather than a new
    // parameter threaded through this function too.
    specific_gravity: Option<f32>,
) -> anyhow::Result<()> {
    db.save_custom_material(&CustomMaterialParams {
        name: &material.name,
        refractive_index: ri,
        dispersion,
        birefringence,
        absorption_rgb,
        crystal_system: Some(crystal_system_to_str(material.crystal_system)),
        optical_character: Some(optical_character_to_str(material.optical_character)),
        biaxial_delta_beta_alpha: material.biaxial_delta_beta_alpha,
        // This UI does not yet author per-axis dispersion data (that would need its
        // own editor surface, out of scope here) -- `None`, the same "not stored"
        // state every row saved before this column existed already has.
        per_axis_dispersion_json: None,
        specific_gravity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crystal_system_str_round_trips_all_seven_variants() {
        for cs in CRYSTAL_SYSTEMS {
            let s = crystal_system_to_str(cs);
            assert_eq!(crystal_system_from_str(s), Some(cs), "variant={cs:?}");
        }
    }

    #[test]
    fn optical_character_str_round_trips_all_five_variants() {
        for oc in OPTICAL_CHARACTERS {
            let s = optical_character_to_str(oc);
            assert_eq!(optical_character_from_str(s), Some(oc), "variant={oc:?}");
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
    fn unrecognized_strings_and_out_of_range_indices_return_none() {
        assert_eq!(crystal_system_from_str("Amorphous"), None);
        assert_eq!(optical_character_from_str(""), None);
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
    /// The final assertion reflects that `gpu_supported()` no longer excludes biaxial
    /// materials (see `indicatrix::optics::materials::GemMaterial::gpu_supported`'s own
    /// doc comment, and `docs/history/indicatrix-core.md`, for the eigenvector-
    /// conditioning fix that made this safe), so a row with
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
}
