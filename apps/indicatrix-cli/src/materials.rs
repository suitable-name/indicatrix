//! Materials: the catalogue a command resolves names against, and the one material a command
//! scores a design in.
//!
//! The catalogue is the built-in presets plus, when `--db` names a design library, its custom
//! materials (opened read-only) and the one custom material a `.indicatrix` file may carry as
//! its own snapshot. A design names its material by name; `--material` and `--ri` replace it.
//!
//! # Library rows are read as the desktop reads them
//!
//! [`gem_from_row`] builds a library row's `GemMaterial` with `LibraryMaterial::gem_material`
//! of `indicatrix-cut-core`, the same code the desktop editor runs. That restores the
//! refractive index, dispersion (or its Sellmeier/Cauchy curve), birefringence, body colour,
//! the stored crystal system, optical character and biaxial delta, and the physics colour
//! recipe. The one thing neither front end restores is per-axis dispersion: no editor writes
//! it, so no row holds it.
//!
//! # Library materials are saved as the desktop saves them
//!
//! The catalogue also keeps each library row's [`CustomMaterialSnapshot`], built with
//! `LibraryMaterial::snapshot`, the code the desktop's Save runs. A `.indicatrix` file that names
//! a library material embeds it ([`Catalogue::library_snapshot`]), so the file opens on a machine
//! without the library, and without `--db`.

use crate::{args::MaterialArg, outcome::CliError};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{
    Design, MaterialSelection, ResolvedMaterial,
    material::{LibraryMaterial, MaterialLookup, built_in_material_by_exact_name},
    native::CustomMaterialSnapshot,
};
use indicatrix_editor::{
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
    optimize_view::default_optimize_material_ri,
    retarget::view::resolved_material_from_selection,
};
use indicatrix_vault::{db::sqlite::Database, model::material::CustomMaterialRow};
use std::path::Path;

/// The built-in materials plus the custom ones a command may name.
#[derive(Debug, Clone, Default)]
pub struct Catalogue {
    custom: Vec<GemMaterial>,
    /// The snapshot of each library row, by the row's name.
    snapshots: Vec<(String, CustomMaterialSnapshot)>,
}

impl Catalogue {
    /// The catalogue for a command: the custom materials of the library at `db` (read-only),
    /// and `from_file`, the custom material the design file itself carries, unless the library
    /// already has one of that name.
    ///
    /// # Errors
    ///
    /// [`CliError::io`] when the library cannot be opened or its material table cannot be read.
    pub fn open(db: Option<&Path>, from_file: Option<GemMaterial>) -> Result<Self, CliError> {
        let rows = match db {
            Some(path) => {
                let database =
                    Database::open_read_only(&path.to_string_lossy()).map_err(|error| {
                        CliError::io(format!(
                            "cannot open the design library {}: {error:#}",
                            path.display()
                        ))
                    })?;
                database.get_custom_materials().map_err(|error| {
                    CliError::io(format!(
                        "cannot read the custom materials of {}: {error:#}",
                        path.display()
                    ))
                })?
            }
            None => Vec::new(),
        };
        Ok(Self::from_rows(&rows, from_file))
    }

    /// The catalogue of the library rows `rows` and, unless a row has the same name, `from_file`.
    #[must_use]
    pub fn from_rows(rows: &[CustomMaterialRow], from_file: Option<GemMaterial>) -> Self {
        let mut custom: Vec<GemMaterial> = rows.iter().map(gem_from_row).collect();
        let snapshots = rows
            .iter()
            .map(|row| (row.name.clone(), library_material(row).snapshot()))
            .collect();
        if let Some(gem) = from_file
            && !custom
                .iter()
                .any(|known| known.name.eq_ignore_ascii_case(&gem.name))
        {
            custom.push(gem);
        }
        Self { custom, snapshots }
    }

    /// The custom materials, library first.
    #[must_use]
    pub fn custom(&self) -> &[GemMaterial] {
        &self.custom
    }

    /// The snapshot a saved design on the library material `name` carries, as the desktop's Save
    /// writes it. `None` for a built-in material name (any build knows those: the desktop
    /// embeds nothing for them either) and for a name the library does not have.
    #[must_use]
    pub fn library_snapshot(&self, name: &str) -> Option<&CustomMaterialSnapshot> {
        if built_in_material_by_exact_name(name).is_some() {
            return None;
        }
        self.snapshots
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, snapshot)| snapshot)
    }

    /// The material named exactly `name` (any letter case): a custom material first, then a
    /// built-in one. Unlike a design's own lookup this never matches part of a name.
    #[must_use]
    pub fn lookup_exact(&self, name: &str) -> Option<GemMaterial> {
        self.custom
            .iter()
            .find(|known| known.name.eq_ignore_ascii_case(name))
            .cloned()
            .or_else(|| built_in_material_by_exact_name(name))
    }

    /// Whether the design's own lookup (the one every desktop view uses) finds `name`.
    fn design_lookup_finds(&self, name: &str) -> bool {
        EditorMaterialLookup::new(&self.custom)
            .lookup(name)
            .is_some()
    }

    /// Every name a command may use, built-ins first.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        GemMaterial::all_materials()
            .into_iter()
            .map(|gem| gem.name)
            .chain(self.custom.iter().map(|gem| gem.name.clone()))
            .collect()
    }
}

/// The shared [`LibraryMaterial`] of a library row. The row's fields are copied into it, because
/// the vault crate that owns the row cannot name that type.
fn library_material(row: &CustomMaterialRow) -> LibraryMaterial<'_> {
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

/// The `GemMaterial` of a library row: the very material the desktop editor builds from the same
/// row (see the module documentation).
#[must_use]
pub fn gem_from_row(row: &CustomMaterialRow) -> GemMaterial {
    library_material(row).gem_material()
}

/// The material a command scores a design in.
#[derive(Debug, Clone)]
pub struct Scoring {
    /// The selection that produced it (what `retarget` saves as the design's material).
    pub selection: MaterialSelection,
    /// The material, its refractive index at the sodium D line and its critical angle.
    pub resolved: ResolvedMaterial,
    /// How to say it in a report: `Diamond`, `refractive index 1.7600`.
    pub label: String,
    /// An assumption worth saying aloud (the design names no material).
    pub note: Option<String>,
}

impl Scoring {
    /// The refractive index scoring is done at.
    #[must_use]
    pub const fn n_d(&self) -> f64 {
        self.resolved.n_d
    }
}

/// The selection naming `canonical`, keeping what a material change keeps: the specific
/// gravity override, and the body colour while the material stays the same.
#[must_use]
pub fn named_selection(base: &MaterialSelection, canonical: &str) -> MaterialSelection {
    let same = base
        .name
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case(canonical));
    MaterialSelection {
        name: Some(canonical.to_string()),
        specific_gravity_override: base.specific_gravity_override,
        refractive_index_override: None,
        body_color_override: if same { base.body_color_override } else { None },
        body_color_bands_override: if same {
            base.body_color_bands_override.clone()
        } else {
            None
        },
        absorption_path_scale_override: if same {
            base.absorption_path_scale_override
        } else {
            None
        },
    }
}

/// The selection of a bare refractive index `ri`, keeping the specific gravity override and,
/// when the design had no name either, the body colour.
#[must_use]
pub const fn ri_selection(base: &MaterialSelection, ri: f64) -> MaterialSelection {
    MaterialSelection {
        name: None,
        specific_gravity_override: base.specific_gravity_override,
        refractive_index_override: Some(ri),
        body_color_override: if base.name.is_none() {
            base.body_color_override
        } else {
            None
        },
        // The bands are dropped with a name change (a bare-RI selection recolours nothing).
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
}

/// How a selection reads in a report: `Diamond`, `Quartz, refractive index 1.7000`,
/// `refractive index 1.7600`.
#[must_use]
pub fn label_of(selection: &MaterialSelection, n_d: f64) -> String {
    match (&selection.name, selection.refractive_index_override) {
        (Some(name), Some(_)) => format!("{name}, refractive index {n_d:.4}"),
        (Some(name), None) => name.clone(),
        (None, Some(_)) => format!("refractive index {n_d:.4}"),
        (None, None) => "none".to_string(),
    }
}

/// The error for a material name nothing answers to, naming what is available.
fn unknown_material(name: &str, catalogue: &Catalogue) -> CliError {
    let hint = if catalogue.custom().is_empty() {
        " (custom materials need --db)"
    } else {
        ""
    };
    CliError::usage(format!(
        "unknown material {name:?}: it is not a built-in material{hint}. Available: {}",
        catalogue.names().join(", ")
    ))
}

/// The material to score `design` in: `arg` when given, else the design's own.
///
/// A design that names no material is scored with its refractive index as a flat material (the
/// Optimize tab's rule), and the result's `note` says so. A design that names a material the
/// catalogue does not have, with no refractive index override, is refused rather than scored as
/// diamond.
///
/// # Errors
///
/// [`CliError::usage`] for an unknown `--material`; [`CliError::design`] for a design whose own
/// material cannot be found.
pub fn resolve_scoring(
    design: &Design,
    catalogue: &Catalogue,
    arg: Option<&MaterialArg>,
) -> Result<Scoring, CliError> {
    let mut note = None;
    let selection = match arg {
        Some(MaterialArg::Named(name)) => {
            let gem = catalogue
                .lookup_exact(name)
                .ok_or_else(|| unknown_material(name, catalogue))?;
            named_selection(&design.material, &gem.name)
        }
        Some(MaterialArg::Ri(ri)) => ri_selection(&design.material, *ri),
        None => {
            let mut selection = design.material.clone();
            if let Some(name) = &selection.name
                && selection.refractive_index_override.is_none()
                && !catalogue.design_lookup_finds(name)
            {
                return Err(CliError::design(format!(
                    "the design's material {name:?} is not a built-in material or a custom \
                     material of the library: give --db FILE with that library, or choose one \
                     with --material NAME or --ri N"
                )));
            }
            if let Some(n_d) =
                default_optimize_material_ri(design, &mut selection, catalogue.custom())
            {
                note = Some(format!(
                    "the design names no material, so its refractive index {n_d:.4} is used \
                     as a flat material"
                ));
            }
            selection
        }
    };
    let resolved = resolved_material_from_selection(&selection, catalogue.custom());
    let label = label_of(&selection, resolved.n_d);
    Ok(Scoring {
        selection,
        resolved,
        label,
        note,
    })
}

/// The design's own material as the tracer sees it, when it names one (or an index override):
/// the "current material" column of the retarget metrics.
#[must_use]
pub fn current_gem(design: &Design, catalogue: &Catalogue) -> Option<GemMaterial> {
    let material = &design.material;
    (material.name.is_some() || material.refractive_index_override.is_some())
        .then(|| resolved_gem_material(material, &EditorMaterialLookup::new(catalogue.custom())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load::{Loaded, from_bytes},
        testing,
    };
    use indicatrix::optics::materials::{CrystalSystem, OpticalCharacter};

    fn row(name: &str) -> CustomMaterialRow {
        CustomMaterialRow {
            name: name.to_string(),
            refractive_index: 1.75,
            dispersion: 0.02,
            birefringence: 0.0,
            absorption_rgb: [0.0, 0.0, 0.0],
            crystal_system: None,
            optical_character: None,
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity: Some(3.9),
            color_recipe_json: None,
            dispersion_model_json: None,
            absorption_bands_json: None,
        }
    }

    fn catalogue_with(name: &str) -> Catalogue {
        Catalogue::from_rows(&[row(name)], None)
    }

    /// The design `testing::template()` made to name the material `name`, as `retarget` does.
    fn design_on(name: &str) -> Design {
        let mut design = testing::template();
        design.material = named_selection(&design.material, name);
        design
    }

    #[test]
    fn a_library_material_is_embedded_so_the_file_opens_without_the_library() {
        let catalogue = catalogue_with("My Spinel");
        let design = design_on("My Spinel");
        let original = Loaded::in_memory(testing::template(), "round.indicatrix");
        let text = original
            .indicatrix_text(&design, &catalogue)
            .expect("writes");
        assert!(text.contains("[material.custom]"), "{text}");

        // Reopened with no library at all: the file's own snapshot answers.
        let again = from_bytes("spinel.indicatrix", text.as_bytes()).expect("opens");
        let bare = again.catalogue(None).expect("no library to fail");
        let without = resolve_scoring(&again.design, &bare, None).expect("resolves without --db");
        let with = resolve_scoring(&design, &catalogue, None).expect("resolves with the library");
        assert_eq!(without.label, "My Spinel");
        assert!(
            (without.n_d() - with.n_d()).abs() < 1e-9,
            "{} against {}",
            without.n_d(),
            with.n_d()
        );
        assert!((without.n_d() - 1.75).abs() < 0.01, "{}", without.n_d());

        // Saving the reopened file again, still without a library, keeps the snapshot.
        let resaved = again
            .indicatrix_text(&again.design, &bare)
            .expect("writes again");
        assert_eq!(resaved, text);
    }

    #[test]
    fn a_built_in_material_and_a_name_the_library_lacks_embed_nothing() {
        let catalogue = catalogue_with("My Spinel");
        assert!(catalogue.library_snapshot("My Spinel").is_some());
        assert!(catalogue.library_snapshot("my spinel").is_some());
        assert!(catalogue.library_snapshot("Sapphire").is_none());
        assert!(catalogue.library_snapshot("Not In The Library").is_none());
        // A library row that shares a built-in's name is not embedded, as on the desktop.
        assert!(
            catalogue_with("Diamond")
                .library_snapshot("Diamond")
                .is_none()
        );

        let loaded = Loaded::in_memory(testing::template(), "round.indicatrix");
        let text = loaded
            .indicatrix_text(&design_on("Sapphire"), &catalogue)
            .expect("writes");
        assert!(!text.contains("[material.custom]"), "{text}");
    }

    #[test]
    fn a_row_becomes_a_material_of_its_name_and_index() {
        let gem = gem_from_row(&row("My Spinel"));
        assert_eq!(gem.name, "My Spinel");
        let n_d = f64::from(gem.dispersion.evaluate(589.3));
        assert!((n_d - 1.75).abs() < 0.01, "{n_d}");
    }

    #[test]
    fn a_row_keeps_its_stored_crystal_optics_as_the_desktop_does() {
        let mut stored = row("Biaxial");
        stored.birefringence = 0.01;
        stored.crystal_system = Some("Orthorhombic".to_string());
        stored.optical_character = Some("BiaxialPositive".to_string());
        stored.biaxial_delta_beta_alpha = Some(0.004);
        let gem = gem_from_row(&stored);
        assert_eq!(gem.crystal_system, CrystalSystem::Orthorhombic);
        assert_eq!(gem.optical_character, OpticalCharacter::BiaxialPositive);
        assert_eq!(gem.biaxial_delta_beta_alpha, Some(0.004));
        // A row that stores none of them is inferred, exactly as before.
        assert_eq!(gem_from_row(&row("Plain")).biaxial_delta_beta_alpha, None);
    }

    #[test]
    fn lookup_is_exact_and_ignores_case() {
        let catalogue = catalogue_with("My Spinel");
        assert_eq!(
            catalogue.lookup_exact("sapphire").map(|g| g.name),
            Some("Sapphire".to_string())
        );
        assert_eq!(
            catalogue.lookup_exact("MY SPINEL").map(|g| g.name),
            Some("My Spinel".to_string())
        );
        // A name that merely contains a built-in's name is not that built-in.
        assert!(catalogue.lookup_exact("My Blue Sapphire").is_none());
        assert!(catalogue.lookup_exact("").is_none());
    }

    #[test]
    fn a_library_material_beats_a_built_in_of_the_same_name() {
        let catalogue = catalogue_with("Diamond");
        let gem = catalogue.lookup_exact("diamond").expect("found");
        let n_d = f64::from(gem.dispersion.evaluate(589.3));
        assert!((n_d - 1.75).abs() < 0.01, "{n_d}");
    }

    #[test]
    fn opening_without_a_library_adds_only_the_file_material() {
        let none = Catalogue::open(None, None).expect("no library to fail");
        assert_eq!(none.custom().len(), 0);
        let from_file = Catalogue::open(None, Some(gem_from_row(&row("Filed")))).expect("opens");
        assert_eq!(from_file.custom().len(), 1);
        assert_eq!(from_file.names().last().map(String::as_str), Some("Filed"));
    }

    #[test]
    fn a_missing_library_is_an_io_error() {
        let path = testing::temp_path("no-such-library.sqlite");
        let error = Catalogue::open(Some(&path), None).expect_err("there is no such file");
        assert_eq!(error.code, crate::outcome::EXIT_IO);
        assert!(
            error.message.contains("design library"),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_design_material_is_the_default() {
        let design = testing::template();
        let scoring = resolve_scoring(&design, &Catalogue::default(), None).expect("resolves");
        assert_eq!(scoring.label, "Diamond");
        assert!(scoring.note.is_none());
        assert!((scoring.n_d() - 2.417).abs() < 0.01, "{}", scoring.n_d());
    }

    #[test]
    fn a_named_material_replaces_the_design_material() {
        let design = testing::template();
        let arg = MaterialArg::Named("sapphire".to_string());
        let scoring =
            resolve_scoring(&design, &Catalogue::default(), Some(&arg)).expect("resolves");
        assert_eq!(scoring.selection.name.as_deref(), Some("Sapphire"));
        assert_eq!(scoring.selection.refractive_index_override, None);
        assert_eq!(scoring.label, "Sapphire");
        assert!((scoring.n_d() - 1.77).abs() < 0.02, "{}", scoring.n_d());
    }

    #[test]
    fn a_bare_index_is_a_nameless_override() {
        let design = testing::template();
        let scoring = resolve_scoring(&design, &Catalogue::default(), Some(&MaterialArg::Ri(1.76)))
            .expect("resolves");
        assert_eq!(scoring.selection.name, None);
        assert_eq!(scoring.selection.refractive_index_override, Some(1.76));
        assert_eq!(scoring.label, "refractive index 1.7600");
        assert_eq!(scoring.n_d(), 1.76);
    }

    #[test]
    fn an_unknown_name_is_a_usage_error_that_lists_the_choices() {
        let design = testing::template();
        let arg = MaterialArg::Named("Unobtainium".to_string());
        let error =
            resolve_scoring(&design, &Catalogue::default(), Some(&arg)).expect_err("unknown");
        assert_eq!(error.code, crate::outcome::EXIT_USAGE);
        assert!(
            error.message.contains("\"Unobtainium\""),
            "{}",
            error.message
        );
        assert!(error.message.contains("Diamond"), "{}", error.message);
        assert!(error.message.contains("--db"), "{}", error.message);
    }

    #[test]
    fn a_design_with_no_material_is_scored_at_its_own_index() {
        let mut design = testing::template();
        design.material = MaterialSelection::none();
        let scoring = resolve_scoring(&design, &Catalogue::default(), None).expect("resolves");
        assert!(
            scoring
                .note
                .as_deref()
                .is_some_and(|n| n.contains("names no material"))
        );
        assert_eq!(
            scoring.selection.refractive_index_override,
            Some(design.effective_refractive_index())
        );
    }

    #[test]
    fn a_design_naming_a_missing_custom_material_is_refused() {
        let mut design = testing::template();
        design.material = MaterialSelection {
            name: Some("Vanished Custom".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        };
        let error = resolve_scoring(&design, &Catalogue::default(), None).expect_err("refused");
        assert_eq!(error.code, crate::outcome::EXIT_DESIGN);
        assert!(
            error.message.contains("Vanished Custom"),
            "{}",
            error.message
        );
        // The library that has it resolves it.
        let found =
            resolve_scoring(&design, &catalogue_with("Vanished Custom"), None).expect("found");
        assert_eq!(found.label, "Vanished Custom");
    }

    #[test]
    fn a_material_change_keeps_the_specific_gravity_and_drops_a_stale_colour() {
        let base = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: Some(3.5),
            refractive_index_override: None,
            body_color_override: Some([0.1, 0.2, 0.3]),
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        };
        let same = named_selection(&base, "diamond");
        assert_eq!(same.body_color_override, Some([0.1, 0.2, 0.3]));
        let other = named_selection(&base, "Sapphire");
        assert_eq!(other.specific_gravity_override, Some(3.5));
        assert_eq!(other.body_color_override, None);
        let flat = ri_selection(&base, 1.7);
        assert_eq!(flat.specific_gravity_override, Some(3.5));
        assert_eq!(flat.body_color_override, None);
    }

    #[test]
    fn labels_read_naturally() {
        let mut selection = MaterialSelection::none();
        assert_eq!(label_of(&selection, 1.5), "none");
        selection.name = Some("Quartz".to_string());
        assert_eq!(label_of(&selection, 1.5442), "Quartz");
        selection.refractive_index_override = Some(1.7);
        assert_eq!(label_of(&selection, 1.7), "Quartz, refractive index 1.7000");
    }

    #[test]
    fn the_current_gem_needs_a_name_or_an_override() {
        let catalogue = Catalogue::default();
        let mut design = testing::template();
        assert!(current_gem(&design, &catalogue).is_some());
        design.material = MaterialSelection::none();
        assert!(current_gem(&design, &catalogue).is_none());
    }
}
