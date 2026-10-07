//! A custom material's physics color through the design file: the vault row's typed
//! `ColorMode` becomes the native snapshot's recipe DTO, an older build's edit of the
//! top-level color is detected on open, and the older build itself still gets a color within
//! `DeltaE00` 15 of the recipe.

use super::save_helpers::custom_material_snapshot_for_save;
use crate::gui::optics::crystal_optics::{gem_material_from_row, save_gem_material};
use indicatrix::{
    color::body_color::{Illuminant, body_colors, delta_e_2000},
    optics::{
        chromophore::{ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve},
        materials::GemMaterial,
    },
};
use indicatrix_cut_core::{
    Design, FreshDesignSpec, MaterialSelection, PreformSpec,
    material::ColorMode,
    native::{SnapshotColor, gem_material_from_custom_snapshot, snapshot_color},
};
use indicatrix_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};

const NAME: &str = "Physics Ruby";

fn ruby_mode() -> ColorMode {
    let cat = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("corundum", cat.data_version);
    recipe.set_amount("Cr", 0.3);
    let (tensor, _) = resolve(&recipe, cat).expect("ruby resolves");
    recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    ColorMode::physics(recipe, [0.0; 3])
}

/// A database holding `NAME` with `mode` as the stored color, the way the editor saves it.
fn db_with(mode: &ColorMode) -> Arc<Mutex<Database>> {
    let db = Database::new(Some(":memory:")).expect("in-memory database");
    let fallback = mode.fallback_rgb();
    let material = GemMaterial::new_custom(NAME, 1.768, 0.018, -0.008, fallback);
    save_gem_material(
        &db,
        &material,
        1.768,
        0.018,
        -0.008,
        fallback,
        None,
        Some(&mode.to_json()),
    )
    .expect("save");
    Arc::new(Mutex::new(db))
}

fn design() -> Design {
    Design::fresh_from_spec(FreshDesignSpec {
        gear_teeth: 80,
        symmetry_order: 4,
        mirror: true,
        material: MaterialSelection {
            name: Some(NAME.to_string()),
            ..MaterialSelection::default()
        },
        preform: PreformSpec::cylinder(80, 1.4, 1.0, 1.3),
    })
}

/// Typed round trip: row -> `ColorMode` -> row material renders from the stored bands, and
/// the snapshot saved with a design carries both the recipe and the fallback color.
#[test]
fn the_vault_row_and_the_design_snapshot_carry_the_typed_recipe() {
    let mode = ruby_mode();
    let db = db_with(&mode);
    let row = db
        .lock()
        .unwrap()
        .get_custom_materials()
        .unwrap()
        .into_iter()
        .next()
        .expect("row");
    assert_eq!(
        ColorMode::from_json(row.color_recipe_json.as_deref().unwrap()),
        Some(mode.clone()),
        "the stored JSON is the typed ColorMode"
    );
    assert_eq!(
        row.absorption_rgb,
        mode.fallback_rgb(),
        "the older-build fallback is the top-level color"
    );
    let material = gem_material_from_row(&row);
    assert_eq!(
        material.absorption,
        mode.resolve_tensor(),
        "rendered from the stored resolved bands"
    );

    let snapshot = custom_material_snapshot_for_save(&design(), &db).expect("snapshot");
    assert!(matches!(
        snapshot_color(&snapshot),
        SnapshotColor::Physics(_)
    ));
    assert_eq!(
        gem_material_from_custom_snapshot(NAME, &snapshot).absorption,
        mode.resolve_tensor()
    );
}

/// An older build that opens the file sees only the top-level color: within `DeltaE00` 15.
#[test]
fn an_older_build_opening_the_saved_file_is_within_delta_e_15() {
    let mode = ruby_mode();
    let db = db_with(&mode);
    let snapshot = custom_material_snapshot_for_save(&design(), &db).expect("snapshot");
    let older = GemMaterial::new_custom(
        NAME,
        snapshot.mean_ri as f32,
        snapshot.dispersion_delta as f32,
        snapshot.birefringence_delta as f32,
        snapshot.body_color().expect("top-level color"),
    );
    let older_lab = body_colors(&older.absorption, 1.0, Illuminant::D65)
        .unpolarised
        .lab;
    let de = delta_e_2000(mode.recipe_lab().expect("recipe"), older_lab);
    assert!(de <= 15.0, "DeltaE00 {de:.1}");
}

/// The older build edited the color and saved: detected as fantasy-edited on open.
#[test]
fn an_edit_by_an_older_build_is_detected_on_open() {
    let mode = ruby_mode();
    let db = db_with(&mode);
    let snapshot = custom_material_snapshot_for_save(&design(), &db)
        .expect("snapshot")
        .with_body_color(Some([0.1, 0.2, 2.5]));
    assert!(matches!(
        snapshot_color(&snapshot),
        SnapshotColor::EditedElsewhere(_)
    ));
}

/// Saving in fantasy mode keeps the recipe in the vault row and the file.
#[test]
fn a_fantasy_save_keeps_the_recipe_in_both_stores() {
    let mut mode = ruby_mode();
    mode.fantasy_rgb = [0.3, 0.6, 1.2];
    mode.switch_to_fantasy();
    let db = db_with(&mode);
    let row = db.lock().unwrap().get_custom_materials().unwrap().remove(0);
    let stored = ColorMode::from_json(row.color_recipe_json.as_deref().unwrap()).unwrap();
    assert!(
        stored.last_recipe.is_some(),
        "the recipe survives a fantasy save"
    );
    assert_eq!(row.absorption_rgb, [0.3, 0.6, 1.2]);
    let snapshot = custom_material_snapshot_for_save(&design(), &db).expect("snapshot");
    assert!(snapshot.color_recipe.is_some(), "and travels in the file");
    assert_eq!(snapshot_color(&snapshot), SnapshotColor::Fantasy);
}
