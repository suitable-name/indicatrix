//! Zoned materials in the material editor (`zoning` feature only): the read-only "Zones..." list
//! of a zoned material, and writing an edited base colour back into the stored base zone.
//!
//! Window-free except [`push_zone_rows`], the one function that touches the dialog's global.
//!
//! A zoned custom material (an adopted "<rough> colour", or a zoned library material) is an
//! ordinary row whose seven-band colour is the BASE zone, plus a `material_zoning` row holding all
//! the zones. When the cutter edits the colour in the dialog, the dialog saves new band rows into
//! the ordinary row; without the write-back the stored base zone would silently override them the
//! next time the zones are attached (`with_stored_zoning` installs the stored base zone as the
//! material's absorption). [`write_back_base_colour`] puts the edited bands into the stored base
//! zone, so the edit sticks, and leaves every shaped zone as it was.

use super::{
    store::{StoredZoning, load_material_zoning, save_material_zoning},
    wizard::zone_rows::zone_rows,
};
use crate::{MainWindow, MaterialZoneRow, SettingsModel, Zoning};
use anyhow::{Result, bail};
use indicatrix::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::{GemMaterial, zone_swatches},
    zoning::{ZoneAbsorption, ZonedAbsorption},
};
use indicatrix_cut_core::rough_plan::zoned_plan::{base_zone_bands, resolve_relative};
use indicatrix_vault::db::sqlite::Database;
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::sync::{Arc, Mutex};

/// One line of the "Zones..." list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneEntry {
    /// "Base zone" or "Zone 2: Cylinder".
    pub title: String,
    /// The shape's key numbers, or "Everywhere no zone covers" for the base.
    pub params: String,
    /// The colour of the zone at the face-up path, sRGB.
    pub swatch: [u8; 3],
}

fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The list of `zoning`: the base zone first, then every shaped zone, each with the swatch of its
/// colour for a stone `width_mm` wide (`0` means the default width).
#[must_use]
pub fn entries(zoning: &ZonedAbsorption, width_mm: f32) -> Vec<ZoneEntry> {
    let swatches = zone_swatches(
        &GemMaterial::diamond().with_zoning(zoning.clone()),
        width_mm,
    );
    zone_rows(zoning)
        .into_iter()
        .zip(swatches)
        .map(|(row, swatch)| ZoneEntry {
            title: row.title,
            params: row.summary,
            swatch: swatch.map(to_u8),
        })
        .collect()
}

/// The list for custom material `name`: empty when it has no stored zones or they cannot be read.
/// A "relative to stone" material is shown at `width_mm` (`0` means the default width).
#[must_use]
pub fn rows_for_material(db: &Database, name: &str, width_mm: f32) -> Vec<ZoneEntry> {
    let Ok(Some(StoredZoning {
        zoning,
        relative_to_stone,
    })) = load_material_zoning(db, name)
    else {
        return Vec::new();
    };
    let shown = if relative_to_stone {
        let width = if width_mm > 0.0 {
            f64::from(width_mm)
        } else {
            f64::from(indicatrix::render_setup::PHYSICS_DEFAULT_STONE_WIDTH_MM)
        };
        match resolve_relative(&zoning, width) {
            Some(scaled) => scaled,
            None => return Vec::new(),
        }
    } else {
        zoning
    };
    entries(&shown, width_mm)
}

/// Fills the dialog's "Zones..." list for the selected material `name` (clears it for a plain
/// material, a built-in one, or a name that is gone).
pub fn push_zone_rows(ui: &MainWindow, db: &Arc<Mutex<Database>>, name: &str) {
    let width = ui.global::<SettingsModel>().get_stone_width_mm();
    let entries = {
        let guard = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        rows_for_material(&guard, name, width)
    };
    let rows: Vec<MaterialZoneRow> = entries
        .into_iter()
        .map(|entry| MaterialZoneRow {
            title: entry.title.into(),
            params: entry.params.into(),
            swatch: Color::from_rgb_u8(entry.swatch[0], entry.swatch[1], entry.swatch[2]),
        })
        .collect();
    ui.global::<Zoning>()
        .set_material_zone_rows(ModelRc::new(VecModel::from(rows)));
}

/// `zoning` with its base zone replaced by the colour `bands` (`[centre_nm, width_nm, per_mm]`
/// rows, as the dialog's colour editor and the material row store them).
///
/// `None` when the bands are
/// what the base zone already holds (nothing was edited: a pleochroic or non-Gaussian base zone
/// must not be flattened by a plain re-save), or when the result does not validate.
#[must_use]
pub fn with_base_bands(zoning: &ZonedAbsorption, bands: &[[f32; 3]]) -> Option<ZonedAbsorption> {
    if base_zone_bands(&zoning.base) == bands {
        return None;
    }
    let tensor = AbsorptionTensor::isotropic(
        bands
            .iter()
            .map(|row| AbsorptionBand::new(row[0], row[1], row[2]))
            .collect(),
    );
    let mut updated = zoning.clone();
    updated.base = ZoneAbsorption::per_mm(tensor);
    updated.validate().ok()?;
    Some(updated)
}

/// Writes the edited body colour `bands` of custom material `name` into its stored base zone.
///
/// Returns whether the stored zones changed (`false`: the material has no zones, or the bands are
/// the base zone's own).
///
/// # Errors
///
/// The stored zones cannot be read, or the vault write fails.
pub fn write_back_base_colour(db: &Database, name: &str, bands: &[[f32; 3]]) -> Result<bool> {
    let Some(stored) = load_material_zoning(db, name)? else {
        return Ok(false);
    };
    if bands.iter().any(|row| row.iter().any(|v| !v.is_finite())) {
        bail!("The body colour of '{name}' has a band that is not a number");
    }
    let Some(updated) = with_base_bands(&stored.zoning, bands) else {
        return Ok(false);
    };
    save_material_zoning(db, name, &updated, stored.relative_to_stone)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_colour::store::{
        attach_zoning, save_material_zoning, save_relative_material_zoning, with_stored_zoning,
    };
    use glam::DVec3;
    use indicatrix::optics::zoning::{Zone, ZoneShape};

    fn db() -> Database {
        Database::new(Some(":memory:")).expect("in-memory vault")
    }

    fn absorber(centre_nm: f32, peak: f32) -> ZoneAbsorption {
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            centre_nm, 40.0, peak,
        )]))
    }

    fn watermelon() -> ZonedAbsorption {
        let mut zoned = ZonedAbsorption::new(absorber(480.0, 0.05));
        zoned.zones.push(Zone {
            shape: ZoneShape::CoaxialCylinder {
                axis_point: DVec3::new(0.0, -3.0, 0.0),
                axis_dir: DVec3::X,
                r_in: 0.0,
                r_out: 3.25,
            },
            absorption: absorber(600.0, 0.9),
        });
        zoned
    }

    #[test]
    fn the_list_has_a_row_and_a_swatch_per_zone() {
        let rows = entries(&watermelon(), 7.0);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "Base zone");
        assert_eq!(rows[1].title, "Zone 1: Cylinder");
        assert!(
            rows[1].params.contains("outer radius 3.25"),
            "{}",
            rows[1].params
        );
        assert_ne!(rows[0].swatch, rows[1].swatch);
    }

    #[test]
    fn a_material_without_zones_has_an_empty_list() {
        let db = db();
        assert_eq!(rows_for_material(&db, "Nothing", 7.0), [] as [ZoneEntry; 0]);
        save_material_zoning(&db, "Melon colour", &watermelon(), false).unwrap();
        assert_eq!(rows_for_material(&db, "melon colour", 7.0).len(), 2);
    }

    #[test]
    fn a_relative_material_is_listed_at_the_stone_width() {
        let db = db();
        save_relative_material_zoning(&db, "Layered", &watermelon(), 6.5).unwrap();
        let at_13 = rows_for_material(&db, "Layered", 13.0);
        assert!(
            at_13[1].params.contains("outer radius 6.5"),
            "{}",
            at_13[1].params
        );
        let at_6_5 = rows_for_material(&db, "Layered", 6.5);
        assert!(
            at_6_5[1].params.contains("outer radius 3.25"),
            "{}",
            at_6_5[1].params
        );
    }

    #[test]
    fn an_edited_body_colour_replaces_the_stored_base_zone_and_nothing_else() {
        let db = db();
        let original = watermelon();
        save_material_zoning(&db, "Melon colour", &original, false).unwrap();
        let edited = [[520.0_f32, 35.0, 0.4]];
        assert!(write_back_base_colour(&db, "Melon colour", &edited).unwrap());

        let stored = load_material_zoning(&db, "Melon colour").unwrap().unwrap();
        assert_eq!(base_zone_bands(&stored.zoning.base), edited.to_vec());
        assert_eq!(
            stored.zoning.zones, original.zones,
            "the shaped zones are untouched"
        );
        assert!(!stored.relative_to_stone);

        // The edit sticks: the zones that get attached when the list is built carry the new base.
        let attached = with_stored_zoning(&db, GemMaterial::diamond(), 0.0);
        assert!(
            attached.zoning.is_none(),
            "the name is the material's own: no zones for diamond"
        );
        let mut named = GemMaterial::diamond();
        named.name = "Melon colour".to_string();
        let attached = with_stored_zoning(&db, named, 0.0);
        let zoning = attached.zoning.expect("zones attached");
        assert_eq!(base_zone_bands(&zoning.base), edited.to_vec());
        assert_eq!(
            attached.absorption, zoning.base.tensor,
            "the plain absorption is the base zone"
        );
    }

    #[test]
    fn saving_the_colour_unchanged_leaves_the_stored_zones_alone() {
        let db = db();
        let original = watermelon();
        save_material_zoning(&db, "Melon colour", &original, false).unwrap();
        let same = base_zone_bands(&original.base);
        assert!(!write_back_base_colour(&db, "Melon colour", &same).unwrap());
        let stored = load_material_zoning(&db, "Melon colour").unwrap().unwrap();
        assert_eq!(stored.zoning, original);
    }

    #[test]
    fn a_plain_material_and_a_bad_band_are_not_written() {
        let db = db();
        assert!(!write_back_base_colour(&db, "Plain", &[[500.0, 30.0, 0.2]]).unwrap());
        save_material_zoning(&db, "Melon colour", &watermelon(), false).unwrap();
        assert!(write_back_base_colour(&db, "Melon colour", &[[f32::NAN, 30.0, 0.2]]).is_err());
        // A band the zone model refuses (width 0) leaves the stored zones as they were.
        assert!(!write_back_base_colour(&db, "Melon colour", &[[500.0, 0.0, 0.2]]).unwrap());
        let stored = load_material_zoning(&db, "Melon colour").unwrap().unwrap();
        assert_eq!(stored.zoning, watermelon());
    }

    #[test]
    fn a_relative_materials_edit_keeps_its_scaling_flag() {
        let db = db();
        save_relative_material_zoning(&db, "Layered", &watermelon(), 6.5).unwrap();
        assert!(write_back_base_colour(&db, "Layered", &[[510.0, 30.0, 0.3]]).unwrap());
        let stored = load_material_zoning(&db, "Layered").unwrap().unwrap();
        assert!(stored.relative_to_stone);
        // `attach_zoning` handles a list: the unrelated material keeps no zones.
        let mut list = vec![GemMaterial::diamond()];
        assert_eq!(attach_zoning(&db, &mut list, 0.0), 0);
    }
}
