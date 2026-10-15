//! Zoned materials in the material editor: the read-only "Zones..." list
//! of a zoned material, and writing an edited base colour back into the stored base zone.
//!
//! Window-free except [`push_zone_rows`], the one function that touches the dialog's global.
//!
//! A zoned custom material (an adopted "<plan> - <design> color", or a zoned library material) is an
//! ordinary row whose seven-band colour is the BASE zone, plus a `material_zoning` row holding all
//! the zones. When the cutter edits the colour in the dialog, the dialog saves new band rows into
//! the ordinary row; without the write-back the stored base zone would silently override them the
//! next time the zones are attached (`with_stored_zoning` installs the stored base zone as the
//! material's absorption). [`write_back_base_colour`] puts the edited bands into the stored base
//! zone, so the edit sticks, and leaves every shaped zone as it was.
//!
//! A built-in with zones (Ametrine) has no vault row: [`built_in_zoning`] reads its zones from the
//! material table, the dialog lists them read-only, and a save under a new name copies them.

use super::{
    store::{
        StoredZoning, load_material_zoning, save_material_zoning, save_relative_material_zoning,
        with_stored_zoning,
    },
    wizard::zone_rows::zone_rows,
};
use crate::{
    MainWindow, MaterialZoneRow, SettingsModel, Zoning, bridge::render_thread::RenderContext,
};
use anyhow::{Result, bail};
use indicatrix::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::{GemMaterial, zone_swatches},
    zoning::{ZoneAbsorption, ZonedAbsorption},
};
use indicatrix_cut_core::rough_plan::zoned_plan::{base_zone_bands, resolve_relative};
use indicatrix_vault::db::sqlite::Database;
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError},
};

thread_local! {
    /// The vault the main window was built with, for [`stone_width_applied`] (the stone-width
    /// handler has no vault of its own). Set by [`register_vault`].
    static VAULT: RefCell<Option<Arc<Mutex<Database>>>> = const { RefCell::new(None) };
}

/// Forgets the vault a previous main window registered, so its database connection can close.
/// See `gui::window_release`.
pub fn release_window_state() {
    let old = VAULT.with(|cell| cell.borrow_mut().take());
    drop(old);
}

/// Remembers the main window's vault (called when the custom-material list is built).
pub fn register_vault(db: &Arc<Mutex<Database>>) {
    VAULT.with(|cell| *cell.borrow_mut() = Some(Arc::clone(db)));
}

/// The stone width zone geometry is resolved at: `width_mm`, or the default width for `0`.
fn effective_width(width_mm: f32) -> f64 {
    if width_mm > 0.0 {
        f64::from(width_mm)
    } else {
        f64::from(indicatrix::render_setup::PHYSICS_DEFAULT_STONE_WIDTH_MM)
    }
}

/// The "relative to stone" materials of `materials`, re-scaled to a stone `width_mm` wide.
///
/// `0` means the default width. Materials with absolute (mm) zones, or none, are not in the
/// result.
#[must_use]
pub fn rescaled_materials(
    db: &Database,
    materials: &[GemMaterial],
    width_mm: f32,
) -> Vec<GemMaterial> {
    let relative: Vec<String> = match db.all_material_zonings() {
        Ok(rows) => rows
            .into_iter()
            .filter(|row| row.relative_to_stone)
            .map(|row| row.material_name)
            .collect(),
        Err(error) => {
            tracing::warn!("Material zones could not be listed for the new stone width: {error:#}");
            return Vec::new();
        }
    };
    if relative.is_empty() {
        return Vec::new();
    }
    materials
        .iter()
        .filter(|material| {
            relative
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&material.name))
        })
        .map(|material| with_stored_zoning(db, material.clone(), f64::from(width_mm)))
        .collect()
}

/// The stone width setting is now `width_mm`: re-scales the zone geometry of library materials.
///
/// Every "relative to stone" library material in the render context's list is re-scaled (owner
/// decision 4), so the next render traces the zones at the new size. Returns how many materials
/// changed.
///
/// Called when the design's girdle diameter changes (`editor::view::inspector::push_stone_fields`)
/// and when a library row takes the viewport (`library::detail::planes`, default width).
/// Does nothing before [`register_vault`] ran. The vault and the context are locked one after the
/// other, never nested.
pub fn stone_width_applied(render_ctx: &Arc<Mutex<RenderContext>>, width_mm: f32) -> usize {
    let Some(db) = VAULT.with(|cell| cell.borrow().clone()) else {
        return 0;
    };
    let snapshot = RenderContext::lock(render_ctx).custom_materials.clone();
    let rescaled = rescaled_materials(
        &db.lock().unwrap_or_else(PoisonError::into_inner),
        &snapshot,
        width_mm,
    );
    if rescaled.is_empty() {
        return 0;
    }
    let mut ctx = RenderContext::lock(render_ctx);
    let materials = Arc::make_mut(&mut ctx.custom_materials);
    for scaled in &rescaled {
        if let Some(slot) = materials
            .iter_mut()
            .find(|m| m.name.eq_ignore_ascii_case(&scaled.name))
        {
            *slot = scaled.clone();
        }
    }
    for scaled in &rescaled {
        if ctx
            .material_override
            .as_ref()
            .is_some_and(|m| m.name.eq_ignore_ascii_case(&scaled.name))
        {
            ctx.material_override = Some(scaled.clone());
        }
    }
    ctx.dirty = true;
    drop(ctx);
    rescaled.len()
}

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

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value clamped to 0..=1 and scaled to 0..=255 before the cast"
)]
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

/// The colour zones of the built-in material called `name` (ASCII case ignored), or `None` for a
/// built-in without zones (every one but Ametrine) and for any other name.
///
/// `GemMaterial::by_name` also matches substrings, so the name is compared again: a custom
/// material called "Ametrine Dream" must not borrow the zones of Ametrine. The geometry is in
/// millimetres through the stone's centre, so it needs no scaling.
#[must_use]
pub fn built_in_zoning(name: &str) -> Option<ZonedAbsorption> {
    GemMaterial::by_name(name)
        .filter(|material| material.name.eq_ignore_ascii_case(name))
        .and_then(|material| material.zoning)
}

/// The list for material `name`.
///
/// The zones stored for a custom material, else those of the
/// built-in of that name (Ametrine), else empty. An empty list is also the answer when the stored
/// zones cannot be read. A "relative to stone" material is shown at `width_mm` (`0` means the
/// default width); a built-in's zones are absolute (mm) and are shown as they are.
#[must_use]
pub fn rows_for_material(db: &Database, name: &str, width_mm: f32) -> Vec<ZoneEntry> {
    let stored = match load_material_zoning(db, name) {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            return built_in_zoning(name)
                .map_or_else(Vec::new, |zoning| entries(&zoning, width_mm));
        }
        Err(_) => return Vec::new(),
    };
    let shown = if stored.relative_to_stone {
        match resolve_relative(&stored.zoning, effective_width(width_mm)) {
            Some(scaled) => scaled,
            None => return Vec::new(),
        }
    } else {
        stored.zoning
    };
    entries(&shown, width_mm)
}

/// Fills the dialog's "Zones..." list for the selected material `name`.
///
/// The list is cleared for a plain material or a name that is gone; a built-in with zones
/// (Ametrine) lists them read-only. Also sets the "zone geometry relative to stone size" checkbox
/// (never ticked for a built-in) and the source name [`apply_on_save`] copies from.
pub fn push_zone_rows(ui: &MainWindow, db: &Arc<Mutex<Database>>, name: &str) {
    let width = ui.global::<SettingsModel>().get_stone_width_mm();
    let (entries, relative) = {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        let entries = rows_for_material(&guard, name, width);
        let relative = matches!(
            load_material_zoning(&guard, name),
            Ok(Some(StoredZoning {
                relative_to_stone: true,
                ..
            }))
        );
        drop(guard);
        (entries, relative)
    };
    let zoning = ui.global::<Zoning>();
    zoning.set_material_relative_to_stone(relative);
    zoning.set_material_zone_source(if entries.is_empty() {
        slint::SharedString::default()
    } else {
        name.into()
    });
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
        bail!("The body color of '{name}' has a band that is not a number");
    }
    let Some(updated) = with_base_bands(&stored.zoning, bands) else {
        return Ok(false);
    };
    save_material_zoning(db, name, &updated, stored.relative_to_stone)?;
    Ok(true)
}

/// Switches the stored zones of custom material `name` between absolute and relative geometry.
///
/// The choice is between absolute (mm) and "relative to stone" geometry. The geometry is
/// converted at a stone `width_mm` wide (`0` means the default width) so the stone looks the
/// same at that width.
///
/// Returns whether the stored row changed (`false`: the material has no zones, or they already
/// are in that form).
///
/// # Errors
///
/// The stored zones cannot be read or scaled, or the vault write fails.
pub fn set_relative_to_stone(
    db: &Database,
    name: &str,
    relative: bool,
    width_mm: f32,
) -> Result<bool> {
    let Some(stored) = load_material_zoning(db, name)? else {
        return Ok(false);
    };
    if stored.relative_to_stone == relative {
        return Ok(false);
    }
    let width = effective_width(width_mm);
    if relative {
        save_relative_material_zoning(db, name, &stored.zoning, width)?;
    } else {
        let Some(absolute) = resolve_relative(&stored.zoning, width) else {
            bail!("The zones of '{name}' cannot be scaled to a stone width of {width} mm");
        };
        save_material_zoning(db, name, &absolute, false)?;
    }
    Ok(true)
}

/// What the material editor's save asks of the stored zones.
#[derive(Debug, Clone, Copy)]
pub struct SaveZones<'a> {
    /// The (trimmed) name the material is saved under.
    pub name: &'a str,
    /// The material whose zones the dialog was showing (empty when it showed none).
    pub source: &'a str,
    /// The dialog's edited seven-band body colour, if it has one.
    pub bands: Option<&'a [[f32; 3]]>,
    /// The "zone geometry relative to stone size" checkbox (`None`: not asked, leave as stored).
    pub relative_to_stone: Option<bool>,
    /// The stone width setting, mm (`0` means the default width).
    pub stone_width_mm: f32,
}

/// The zones of the material the dialog showed (`source`): its vault row, else the built-in of
/// that name (Ametrine, absolute mm). `None` when it has neither.
fn source_zoning(db: &Database, source: &str) -> Result<Option<StoredZoning>> {
    Ok(load_material_zoning(db, source)?.or_else(|| {
        built_in_zoning(source).map(|zoning| StoredZoning {
            zoning,
            relative_to_stone: false,
        })
    }))
}

/// Brings the stored zones in line with a material save, after the material row itself was written:
/// 1. Saved under a new name, the zones of the material the dialog showed, stored or built in, are
///    copied to it (unless the new name has zones of its own), so the copy keeps its colour zones.
/// 2. An edited body colour goes into the stored base zone ([`write_back_base_colour`]).
/// 3. The checkbox converts the geometry between mm and relative to stone
///    ([`set_relative_to_stone`]).
///
/// # Errors
///
/// The sentence to show the cutter: the material was saved, but its zones could not be updated.
pub fn apply_on_save(db: &Database, request: SaveZones<'_>) -> Result<(), String> {
    let SaveZones {
        name,
        source,
        bands,
        relative_to_stone,
        stone_width_mm,
    } = request;
    let sentence = |error: &anyhow::Error| {
        format!(
            "'{name}' was saved, but its color zones could not be updated: {error:#}. \
             The zones keep their old settings."
        )
    };
    if !source.is_empty() && !source.eq_ignore_ascii_case(name) {
        // A copy of a zoned material (a stored one, or a built-in with zones such as Ametrine): it
        // keeps the zones, unless it already has its own.
        if matches!(load_material_zoning(db, name), Ok(None))
            && let Some(stored) = source_zoning(db, source).map_err(|e| sentence(&e))?
        {
            save_material_zoning(db, name, &stored.zoning, stored.relative_to_stone)
                .map_err(|e| sentence(&e))?;
        }
    }
    if let Some(bands) = bands {
        write_back_base_colour(db, name, bands).map_err(|e| sentence(&e))?;
    }
    if let Some(relative) = relative_to_stone {
        set_relative_to_stone(db, name, relative, stone_width_mm).map_err(|e| sentence(&e))?;
    }
    Ok(())
}

/// What the open material dialog showed about zones, read when its save callback fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogChoice {
    /// The material whose zones the dialog listed ("" for none).
    pub source: String,
    /// The state of the "zone geometry relative to stone size" checkbox.
    pub relative_to_stone: bool,
}

thread_local! {
    /// The choice [`stash_dialog_choice`] left for the save that follows.
    static DIALOG_CHOICE: RefCell<Option<DialogChoice>> = const { RefCell::new(None) };
}

/// Reads the dialog's zone state from the `Zoning` global and keeps it for the material save that
/// is about to run (the save callback's signature carries no room for it).
pub fn stash_dialog_choice(ui: &MainWindow) {
    let zoning = ui.global::<Zoning>();
    let choice = DialogChoice {
        source: zoning.get_material_zone_source().to_string(),
        relative_to_stone: zoning.get_material_relative_to_stone(),
    };
    DIALOG_CHOICE.with(|cell| *cell.borrow_mut() = Some(choice));
}

/// The choice [`stash_dialog_choice`] stored, once (`None` when nothing was stashed).
#[must_use]
pub fn take_dialog_choice() -> Option<DialogChoice> {
    DIALOG_CHOICE.with(|cell| cell.borrow_mut().take())
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
        assert_eq!(rows[0].title, "Base Zone");
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

    fn named(name: &str) -> GemMaterial {
        let mut material = GemMaterial::diamond();
        material.name = name.to_string();
        material
    }

    fn request<'a>(name: &'a str, source: &'a str) -> SaveZones<'a> {
        SaveZones {
            name,
            source,
            bands: None,
            relative_to_stone: None,
            stone_width_mm: 7.0,
        }
    }

    #[test]
    fn a_damaged_zone_row_makes_the_save_say_so() {
        let db = db();
        db.save_material_zoning(&indicatrix_vault::model::zoning::MaterialZoningRow {
            material_name: "Melon colour".to_string(),
            zoned_json: "{ not json".to_string(),
            relative_to_stone: false,
            version: indicatrix_cut_core::rough_plan::zoned_plan::ZONING_FORMAT_VERSION,
        })
        .unwrap();
        let edited = [[520.0_f32, 35.0, 0.4]];
        let refusal = apply_on_save(
            &db,
            SaveZones {
                bands: Some(&edited),
                ..request("Melon colour", "Melon colour")
            },
        )
        .expect_err("the damaged row is reported");
        assert!(
            refusal
                .starts_with("'Melon colour' was saved, but its color zones could not be updated"),
            "{refusal}"
        );
        assert!(
            refusal.ends_with("The zones keep their old settings."),
            "{refusal}"
        );
    }

    #[test]
    fn a_save_with_nothing_to_change_is_quiet() {
        let db = db();
        assert!(apply_on_save(&db, request("Plain", "")).is_ok());
        save_material_zoning(&db, "Melon colour", &watermelon(), false).unwrap();
        assert!(apply_on_save(&db, request("Melon colour", "Melon colour")).is_ok());
        let stored = load_material_zoning(&db, "Melon colour").unwrap().unwrap();
        assert_eq!(stored.zoning, watermelon());
        assert!(!stored.relative_to_stone);
    }

    #[test]
    fn saving_under_a_new_name_copies_the_zones() {
        let db = db();
        save_material_zoning(&db, "Melon colour", &watermelon(), false).unwrap();
        apply_on_save(&db, request("Melon copy", "Melon colour")).unwrap();
        let copy = load_material_zoning(&db, "Melon copy").unwrap().unwrap();
        assert_eq!(copy.zoning, watermelon());
        // The original keeps its own.
        assert!(load_material_zoning(&db, "Melon colour").unwrap().is_some());

        // A name that has zones of its own keeps them.
        save_material_zoning(
            &db,
            "Other",
            &ZonedAbsorption::new(absorber(500.0, 0.1)),
            false,
        )
        .unwrap();
        apply_on_save(&db, request("Other", "Melon colour")).unwrap();
        let other = load_material_zoning(&db, "Other").unwrap().unwrap();
        assert_eq!(other.zoning.zones.len(), 0);
    }

    #[test]
    fn the_checkbox_converts_between_mm_and_relative_geometry() {
        let db = db();
        save_material_zoning(&db, "Melon colour", &watermelon(), false).unwrap();
        let ticked = SaveZones {
            relative_to_stone: Some(true),
            stone_width_mm: 6.5,
            ..request("Melon colour", "Melon colour")
        };
        apply_on_save(&db, ticked).unwrap();
        let stored = load_material_zoning(&db, "Melon colour").unwrap().unwrap();
        assert!(stored.relative_to_stone);

        // Back to mm at another width: the geometry is that of a 13 mm stone.
        let unticked = SaveZones {
            relative_to_stone: Some(false),
            stone_width_mm: 13.0,
            ..request("Melon colour", "Melon colour")
        };
        apply_on_save(&db, unticked).unwrap();
        let rows = rows_for_material(&db, "Melon colour", 13.0);
        assert!(
            rows[1].params.contains("outer radius 6.5"),
            "{}",
            rows[1].params
        );
        assert!(
            !load_material_zoning(&db, "Melon colour")
                .unwrap()
                .unwrap()
                .relative_to_stone
        );
    }

    #[test]
    fn a_new_stone_width_rescales_only_the_relative_materials() {
        let db = db();
        save_relative_material_zoning(&db, "Layered", &watermelon(), 6.5).unwrap();
        save_material_zoning(&db, "Fixed", &watermelon(), false).unwrap();
        let list = vec![
            with_stored_zoning(&db, named("Layered"), 6.5),
            with_stored_zoning(&db, named("Fixed"), 6.5),
            named("Plain"),
        ];

        let at_13 = rescaled_materials(&db, &list, 13.0);
        assert_eq!(at_13.len(), 1, "only the relative material changes");
        assert_eq!(at_13[0].name, "Layered");
        let radius_of =
            |material: &GemMaterial| match &material.zoning.as_ref().unwrap().zones[0].shape {
                ZoneShape::CoaxialCylinder { r_out, .. } => *r_out,
                _ => panic!("the test zone is a cylinder"),
            };
        assert!((radius_of(&at_13[0]) - 6.5).abs() < 1e-9);
        assert!((radius_of(&list[0]) - 3.25).abs() < 1e-9);

        // `0` is the default width; with no relative rows nothing is returned.
        assert_eq!(rescaled_materials(&db, &list, 0.0).len(), 1);
        assert_eq!(rescaled_materials(&self::db(), &list, 13.0).len(), 0);
    }

    #[test]
    fn a_built_in_with_zones_is_listed() {
        let rows = rows_for_material(&db(), "Ametrine", 7.0);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "Base Zone");
        assert_eq!(rows[1].title, "Zone 1: Half Space");
        assert_ne!(rows[0].swatch, rows[1].swatch);
        assert!(rows[1].params.contains("offset 0"), "{}", rows[1].params);
    }

    #[test]
    fn a_plain_built_in_has_no_rows() {
        assert_eq!(rows_for_material(&db(), "Citrine", 7.0), []);
        assert!(built_in_zoning("Citrine").is_none());
    }

    #[test]
    fn a_custom_name_containing_a_built_in_name_does_not_borrow_its_zones() {
        assert_eq!(rows_for_material(&db(), "Ametrine Dream", 7.0), []);
        assert!(built_in_zoning("Ametrine Dream").is_none());
        assert!(built_in_zoning("ametrine").is_some(), "the case is ignored");
    }

    #[test]
    fn saving_a_copy_of_the_built_in_keeps_its_zones() {
        let db = db();
        apply_on_save(&db, request("My Ametrine", "Ametrine")).unwrap();
        let copy = load_material_zoning(&db, "My Ametrine").unwrap().unwrap();
        assert_eq!(copy.zoning.zones.len(), 1);
        assert!(!copy.relative_to_stone);
        assert_eq!(Some(copy.zoning), built_in_zoning("Ametrine"));

        let ticked = SaveZones {
            relative_to_stone: Some(true),
            stone_width_mm: 7.0,
            ..request("My Ametrine", "My Ametrine")
        };
        apply_on_save(&db, ticked).unwrap();
        let relative = load_material_zoning(&db, "My Ametrine").unwrap().unwrap();
        assert!(relative.relative_to_stone);
        assert_eq!(relative.zoning.zones.len(), 1);
    }
}
