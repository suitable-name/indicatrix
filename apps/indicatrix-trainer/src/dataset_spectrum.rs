//! The mineral optical spectrum the retarget task samples its target material from: thirteen
//! species from fluorite (n 1.43) to rutile (n 2.90), each resolved to a built-in preset or,
//! for a species the library has no preset for, to a neutral stone of its refractive index.

use indicatrix_ai::model::Prng;
use indicatrix_cut_core::{BuiltinMaterials, MaterialLookup, MaterialSelection, ResolvedMaterial};

use super::DatasetError;

/// Entry in the mineral optical spectrum table with nominal refractive index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MineralSpectrumEntry {
    /// Species name.
    pub name: &'static str,
    /// Name of the built-in material preset that stands for the species, or `None` when the
    /// library has no preset: the species is then a neutral stone of exactly `refractive_index`.
    pub preset: Option<&'static str>,
    /// Nominal refractive index (sodium D).
    pub refractive_index: f32,
    /// Critical angle in degrees, `asin(1 / n)`.
    pub critical_angle_deg: f32,
}

const fn entry(
    name: &'static str,
    preset: Option<&'static str>,
    refractive_index: f32,
    critical_angle_deg: f32,
) -> MineralSpectrumEntry {
    MineralSpectrumEntry {
        name,
        preset,
        refractive_index,
        critical_angle_deg,
    }
}

/// Gemstone species across the optical refraction spectrum (n from 1.43 to 2.90).
pub const MINERAL_SPECTRUM: &[MineralSpectrumEntry] = &[
    entry("Fluorite", None, 1.434, 44.22),
    entry("Quartz", Some("Quartz"), 1.544, 40.36),
    entry("Beryl", None, 1.577, 39.35),
    entry("Topaz", Some("Topaz"), 1.619, 38.14),
    entry("Tourmaline", Some("Tourmaline"), 1.624, 38.01),
    entry("Spinel", Some("Spinel"), 1.718, 35.60),
    entry("Sapphire", Some("Sapphire"), 1.768, 34.45),
    entry("Garnet", None, 1.830, 33.12),
    entry("Zircon", Some("Zircon"), 1.950, 30.85),
    entry("Cubic Zirconia", Some("Cubic Zirconia"), 2.150, 27.72),
    entry("Diamond", Some("Diamond"), 2.417, 24.44),
    entry(
        "Synthetic Moissanite",
        Some("Synthetic Moissanite"),
        2.650,
        22.17,
    ),
    entry("Rutile", Some("Rutile"), 2.903, 20.15),
];

/// Finds the spectrum entry for `name`. Accepts the UI labels (`Sapphire (n=1.77)`), is case
/// insensitive, and knows the aliases `Corundum`, `Moissanite` and `CZ`.
///
/// # Errors
///
/// [`DatasetError::UnknownMaterial`] when nothing matches; there is no fallback to diamond.
pub fn find_spectrum_entry(name: &str) -> Result<MineralSpectrumEntry, DatasetError> {
    let base = name.split(" (").next().unwrap_or(name).trim();
    let canonical = match base.to_ascii_lowercase().as_str() {
        "corundum" => "sapphire".to_owned(),
        "moissanite" => "synthetic moissanite".to_owned(),
        "cz" => "cubic zirconia".to_owned(),
        other => other.to_owned(),
    };
    MINERAL_SPECTRUM
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(&canonical))
        .copied()
        .ok_or_else(|| DatasetError::UnknownMaterial(name.to_owned()))
}

/// Resolves a spectrum entry into a material: its preset, or a neutral stone of the entry's
/// refractive index when the library has no preset for the species.
///
/// # Errors
///
/// [`DatasetError::UnknownMaterial`] when the entry names a preset the library lacks (the
/// resolver would otherwise silently substitute diamond).
pub fn resolve_entry(entry: &MineralSpectrumEntry) -> Result<ResolvedMaterial, DatasetError> {
    let selection = match entry.preset {
        Some(preset) => {
            if BuiltinMaterials.lookup(preset).is_none() {
                return Err(DatasetError::UnknownMaterial(preset.to_owned()));
            }
            MaterialSelection {
                name: Some(preset.to_owned()),
                ..MaterialSelection::none()
            }
        }
        None => MaterialSelection {
            refractive_index_override: Some(f64::from(entry.refractive_index)),
            ..MaterialSelection::none()
        },
    };
    Ok(selection.resolve(&BuiltinMaterials))
}

/// Resolves a material name (see [`find_spectrum_entry`]) into a material.
///
/// # Errors
///
/// As [`find_spectrum_entry`] and [`resolve_entry`].
pub fn resolve_material(name: &str) -> Result<ResolvedMaterial, DatasetError> {
    resolve_entry(&find_spectrum_entry(name)?)
}

/// Deterministically draws a target material from the spectrum.
#[must_use]
pub fn sample_target(seed: u64) -> MineralSpectrumEntry {
    let index = Prng::from_stream(seed, 0x7A12_0001).next_index(MINERAL_SPECTRUM.len());
    MINERAL_SPECTRUM[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_critical_angles_match_the_refractive_indices() {
        for e in MINERAL_SPECTRUM {
            let exact = indicatrix_cut_core::critical_angle_deg(f64::from(e.refractive_index));
            assert!(
                (exact - f64::from(e.critical_angle_deg)).abs() < 0.02,
                "{}: table {} vs exact {exact}",
                e.name,
                e.critical_angle_deg
            );
        }
    }

    #[test]
    fn every_entry_resolves_and_presetless_species_use_their_index() {
        for e in MINERAL_SPECTRUM {
            let material = resolve_entry(e).expect("resolves");
            if e.preset.is_none() {
                assert!((material.n_d - f64::from(e.refractive_index)).abs() < 1e-6);
            }
        }
        let fluorite = resolve_material("Fluorite (n=1.43)").unwrap();
        let diamond = resolve_material("Diamond").unwrap();
        assert!(fluorite.n_d < 1.5 && diamond.n_d > 2.3);
    }

    #[test]
    fn unknown_material_is_an_error_not_diamond() {
        assert!(matches!(
            resolve_material("Unobtainium"),
            Err(DatasetError::UnknownMaterial(_))
        ));
        assert_eq!(find_spectrum_entry("corundum").unwrap().name, "Sapphire");
    }

    #[test]
    fn sampling_a_target_is_deterministic() {
        assert_eq!(sample_target(5), sample_target(5));
    }
}
