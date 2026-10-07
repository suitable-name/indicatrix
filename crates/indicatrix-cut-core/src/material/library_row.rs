//! A custom material of the design library as plain values ([`LibraryMaterial`]), and the two
//! things every front end does with it: build the [`GemMaterial`] it describes
//! ([`LibraryMaterial::gem_material`]) and the [`CustomMaterialSnapshot`] a saved design file
//! carries for it ([`LibraryMaterial::snapshot`]).
//!
//! `indicatrix-vault`'s `CustomMaterialRow` holds the same data, but that crate must not depend
//! on `indicatrix` and this crate must not depend on a database. So the caller copies the row's
//! fields one for one into a [`LibraryMaterial`] and everything past that point is shared: the
//! desktop editor and the command line build the same stone from the same row.
//!
//! The crystal system and optical character are stored as the variant's `Debug` name (for
//! example `"Trigonal"`); [`crystal_system_name`] and [`crystal_system_from_name`] (and the two
//! for the optical character) are the one place that spelling is written down.

use super::ColorMode;
use crate::native::{
    CustomMaterialSnapshot, color_recipe_dto, dispersion_dto_from_json, dispersion_model_from_json,
};
use indicatrix::optics::materials::{CrystalSystem, GemMaterial, OpticalCharacter};

/// Every crystal system, for reading a stored name back.
const CRYSTAL_SYSTEMS: [CrystalSystem; 7] = [
    CrystalSystem::Cubic,
    CrystalSystem::Tetragonal,
    CrystalSystem::Hexagonal,
    CrystalSystem::Trigonal,
    CrystalSystem::Orthorhombic,
    CrystalSystem::Monoclinic,
    CrystalSystem::Triclinic,
];

/// Every optical character, for reading a stored name back.
const OPTICAL_CHARACTERS: [OpticalCharacter; 5] = [
    OpticalCharacter::Isotropic,
    OpticalCharacter::UniaxialPositive,
    OpticalCharacter::UniaxialNegative,
    OpticalCharacter::BiaxialPositive,
    OpticalCharacter::BiaxialNegative,
];

/// A crystal system's `Debug` name, e.g. `"Trigonal"`: what a library row stores.
#[must_use]
pub const fn crystal_system_name(system: CrystalSystem) -> &'static str {
    match system {
        CrystalSystem::Cubic => "Cubic",
        CrystalSystem::Tetragonal => "Tetragonal",
        CrystalSystem::Hexagonal => "Hexagonal",
        CrystalSystem::Trigonal => "Trigonal",
        CrystalSystem::Orthorhombic => "Orthorhombic",
        CrystalSystem::Monoclinic => "Monoclinic",
        CrystalSystem::Triclinic => "Triclinic",
    }
}

/// Inverse of [`crystal_system_name`]. `None` for anything unrecognised.
///
/// A hand-edited database, or a variant a later build adds, gives such a name: the caller
/// treats that like an absent field and keeps what `GemMaterial::new_custom` infers.
#[must_use]
pub fn crystal_system_from_name(name: &str) -> Option<CrystalSystem> {
    CRYSTAL_SYSTEMS
        .iter()
        .copied()
        .find(|system| crystal_system_name(*system) == name)
}

/// An optical character's `Debug` name, e.g. `"UniaxialNegative"`: what a library row stores.
#[must_use]
pub const fn optical_character_name(character: OpticalCharacter) -> &'static str {
    match character {
        OpticalCharacter::Isotropic => "Isotropic",
        OpticalCharacter::UniaxialPositive => "UniaxialPositive",
        OpticalCharacter::UniaxialNegative => "UniaxialNegative",
        OpticalCharacter::BiaxialPositive => "BiaxialPositive",
        OpticalCharacter::BiaxialNegative => "BiaxialNegative",
    }
}

/// Inverse of [`optical_character_name`], with the same "unrecognised is `None`" contract as
/// [`crystal_system_from_name`].
#[must_use]
pub fn optical_character_from_name(name: &str) -> Option<OpticalCharacter> {
    OPTICAL_CHARACTERS
        .iter()
        .copied()
        .find(|character| optical_character_name(*character) == name)
}

/// One custom material of the design library, as plain values: the fields of
/// `indicatrix_vault::model::material::CustomMaterialRow` that describe the material, copied
/// one for one by the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LibraryMaterial<'a> {
    /// The material's name.
    pub name: &'a str,
    /// The refractive index (the `n_d` of the model when `dispersion_model_json` is set).
    pub refractive_index: f32,
    /// The `n_F - n_C` figure (the model's own when `dispersion_model_json` is set).
    pub dispersion: f32,
    /// The birefringence.
    pub birefringence: f32,
    /// The body colour as an absorption triple.
    pub absorption_rgb: [f32; 3],
    /// The stored [`crystal_system_name`], or `None` for "infer it from the birefringence".
    pub crystal_system: Option<&'a str>,
    /// The stored [`optical_character_name`], or `None` for "infer it from the birefringence".
    pub optical_character: Option<&'a str>,
    /// `n_beta - n_alpha` of a biaxial material, or `None`.
    pub biaxial_delta_beta_alpha: Option<f32>,
    /// The specific gravity, or `None` when none was recorded.
    pub specific_gravity: Option<f32>,
    /// The JSON of the material's [`ColorMode`] (fantasy payload and physics recipe), or
    /// `None`.
    pub color_recipe_json: Option<&'a str>,
    /// The JSON of the dispersion model (`dispersion_model_to_json`), or `None` for the plain
    /// refractive-index-and-dispersion path.
    pub dispersion_model_json: Option<&'a str>,
    /// The JSON of the seven-band body colour ([`absorption_bands_to_json`]), or `None` for "no
    /// bands": the material is then coloured from [`Self::absorption_rgb`] alone.
    pub absorption_bands_json: Option<&'a str>,
}

/// Stone width (mm) assumed for a banded custom material when nothing sets the real one: the
/// physics colour's default. The bands are per millimetre, the stone is about two model units
/// wide, so the path scale is half of this. A design with a girdle diameter overrides it.
const BANDED_DEFAULT_STONE_WIDTH_MM: f32 = 7.0;

/// `material` coloured by the body-colour band rows of a library row (the one place the path
/// scale of a banded custom material is chosen, shared by the editor's save and by loading).
#[must_use]
pub fn with_library_bands(material: GemMaterial, rows: &[[f32; 3]]) -> GemMaterial {
    material.with_body_color_bands(rows, BANDED_DEFAULT_STONE_WIDTH_MM / 2.0)
}

/// The JSON text a library row stores for body-colour band rows
/// (`[centre_nm, width_nm, amplitude_per_mm]` each), or `None` for no rows.
#[must_use]
pub fn absorption_bands_to_json(rows: &[[f32; 3]]) -> Option<String> {
    (!rows.is_empty())
        .then(|| serde_json::to_string(rows).ok())
        .flatten()
}

/// Inverse of [`absorption_bands_to_json`].
///
/// `None` for blank, malformed or empty text and for rows with a non-finite number or a non-positive width or amplitude: the caller then colours
/// the material from its legacy triple, exactly as before the column existed.
#[must_use]
pub fn absorption_bands_from_json(text: &str) -> Option<Vec<[f32; 3]>> {
    let rows: Vec<[f32; 3]> = serde_json::from_str(text.trim()).ok()?;
    let usable = !rows.is_empty()
        && rows
            .iter()
            .all(|r| r.iter().all(|v| v.is_finite()) && r[1] > 0.0 && r[2] > 0.0);
    usable.then_some(rows)
}

impl LibraryMaterial<'_> {
    /// The [`GemMaterial`] this row describes.
    ///
    /// `GemMaterial::new_custom`'s own two-variant inference (cubic or trigonal, isotropic or
    /// uniaxial from the sign of the birefringence) is applied first; the crystal system,
    /// optical character and biaxial delta the row stores then replace what it inferred. A row
    /// saved before those fields existed has none of them, so it renders exactly as it always
    /// did.
    ///
    /// A row whose `dispersion_model_json` holds a usable Sellmeier or Cauchy model builds its
    /// curve with `GemMaterial::new_custom_with_dispersion`; a row without one (or with one that
    /// fails `DispersionModel::validate`) takes the `new_custom` path from `refractive_index`
    /// and `dispersion`. A stored physics recipe renders from its `resolved_bands` while physics
    /// is the active colour mode.
    #[must_use]
    pub fn gem_material(&self) -> GemMaterial {
        let mut material = self
            .dispersion_model_json
            .and_then(dispersion_model_from_json)
            .map_or_else(
                || {
                    GemMaterial::new_custom(
                        self.name,
                        self.refractive_index,
                        self.dispersion,
                        self.birefringence,
                        self.absorption_rgb,
                    )
                },
                |model| {
                    GemMaterial::new_custom_with_dispersion(
                        self.name,
                        model,
                        self.birefringence,
                        self.absorption_rgb,
                    )
                },
            );
        if let Some(system) = self.crystal_system.and_then(crystal_system_from_name) {
            material.crystal_system = system;
        }
        if let Some(character) = self.optical_character.and_then(optical_character_from_name) {
            material.optical_character = character;
        }
        if let Some(delta) = self.biaxial_delta_beta_alpha {
            material.biaxial_delta_beta_alpha = Some(delta);
        }
        let physics = self
            .color_recipe_json
            .and_then(ColorMode::from_json)
            .filter(ColorMode::is_physics);
        if let Some(mode) = physics {
            material.absorption = mode.resolve_tensor();
        } else if let Some(rows) = self.absorption_bands() {
            // The seven-band colour replaces the triple's three bands (the triple stays what an
            // older build shows). The scale gives the bands per millimetre a physical stone;
            // a design with a girdle diameter overrides it at render time.
            material = with_library_bands(material, &rows);
        }
        material
    }

    /// The usable body-colour band rows, if the row stores any.
    #[must_use]
    pub fn absorption_bands(&self) -> Option<Vec<[f32; 3]>> {
        self.absorption_bands_json
            .and_then(absorption_bands_from_json)
    }

    /// The [`CustomMaterialSnapshot`] a design file carries for this material, so a design
    /// saved under a custom material does not reload as Diamond on a machine without it.
    ///
    /// The snapshot holds the numbers the cutter typed (refractive index, dispersion,
    /// birefringence, specific gravity) rather than anything re-derived from the resolved
    /// material, and the dispersion model when one is stored and valid. The crystal system and
    /// optical character go in as plain text for a person reading the file; reading the
    /// snapshot back re-derives both from the birefringence.
    ///
    /// Both colour payloads travel: the top-level body colour, and, when the row holds a
    /// recipe, the typed [`ColorMode`] with its fallback colour recorded exactly as the
    /// top-level colour was written. An older build that edits the top-level colour later makes
    /// the two differ, which the open path detects.
    #[must_use]
    pub fn snapshot(&self) -> CustomMaterialSnapshot {
        let mut snapshot = CustomMaterialSnapshot::new(
            f64::from(self.refractive_index),
            f64::from(self.dispersion),
            f64::from(self.birefringence),
            self.specific_gravity.map(f64::from),
            self.crystal_system.unwrap_or_default(),
            self.optical_character.unwrap_or_default(),
        )
        .with_body_color(Some(self.absorption_rgb))
        .with_dispersion_model(
            self.dispersion_model_json
                .and_then(dispersion_dto_from_json),
        );
        if let Some(mode) = self
            .color_recipe_json
            .and_then(ColorMode::from_json)
            .filter(|mode| mode.last_recipe.is_some())
        {
            let mut dto = color_recipe_dto(&mode);
            if let Some(top_level) = snapshot.absorption_rgb {
                dto.fallback_rgb = top_level;
            }
            snapshot = snapshot.with_color_recipe(Some(dto));
        }
        if let Some(rows) = self.absorption_bands() {
            snapshot = snapshot.with_absorption_bands(&rows);
        }
        snapshot
    }
}

#[cfg(test)]
mod tests;
