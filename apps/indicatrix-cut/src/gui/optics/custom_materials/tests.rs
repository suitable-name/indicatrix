//! The material editor's save path: both color payloads reach the vault row and the render
//! context, whichever mode is active, and saving in fantasy mode never deletes the recipe.

use super::{CUSTOM_KEEP_COLOR_INDEX, CustomMaterialForm, apply_custom_material_save};
use crate::bridge::render_thread::RenderContext;
use indicatrix::optics::chromophore::{ChromophoreCatalogue, ResolvedBands, colorRecipe, resolve};
use indicatrix_cut_core::material::{Activecolor, colorMode};
use indicatrix_vault::db::sqlite::Database;
use slint::SharedString;
use std::sync::{Arc, Mutex};

fn ruby_mode() -> colorMode {
    let cat = ChromophoreCatalogue::global();
    let mut recipe = colorRecipe::new("corundum", cat.data_version);
    recipe.set_amount("Cr", 0.3);
    let (tensor, _) = resolve(&recipe, cat).expect("ruby resolves");
    recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    colorMode::physics(recipe, [0.1, 0.2, 0.3])
}

fn form(name: &str, color_idx: i32, json: &str) -> CustomMaterialForm {
    CustomMaterialForm {
        name: SharedString::from(name),
        ri: 1.768,
        disp: 0.018,
        biref: -0.008,
        color_idx,
        crystal_system_idx: 3,
        optical_character_idx: 2,
        biaxial_delta_beta_alpha: 0.0,
        specific_gravity: 0.0,
        apply_to_live_render: false,
        color_recipe_json: SharedString::from(json),
    }
}

fn setup() -> (Arc<Mutex<Database>>, Arc<Mutex<RenderContext>>) {
    let db = Database::new(Some(":memory:")).expect("in-memory database");
    (
        Arc::new(Mutex::new(db)),
        Arc::new(Mutex::new(RenderContext::default())),
    )
}

fn stored(
    db: &Arc<Mutex<Database>>,
    name: &str,
) -> indicatrix_vault::model::material::CustomMaterialRow {
    db.lock()
        .unwrap()
        .get_custom_materials()
        .unwrap()
        .into_iter()
        .find(|r| r.name == name)
        .expect("row saved")
}

#[test]
fn a_physics_save_stores_both_payloads_and_flags_the_material() {
    let (db, ctx) = setup();
    let mode = ruby_mode();
    apply_custom_material_save(
        &db,
        &ctx,
        form("Physics Ruby", CUSTOM_KEEP_COLOR_INDEX, &mode.to_json()),
    )
    .expect("saves");
    let row = stored(&db, "Physics Ruby");
    let back = colorMode::from_json(row.color_recipe_json.as_deref().unwrap()).unwrap();
    assert_eq!(back.active, Activecolor::Physics);
    assert_eq!(
        back.fantasy_rgb,
        [0.1, 0.2, 0.3],
        "the fantasy payload survives"
    );
    assert_eq!(back.last_recipe, mode.last_recipe);
    assert_eq!(
        row.absorption_rgb,
        mode.fallback_rgb(),
        "older builds read the fallback"
    );

    let guard = ctx.lock().unwrap();
    let material = guard
        .custom_materials
        .iter()
        .find(|m| m.name == "Physics Ruby")
        .unwrap();
    assert_eq!(
        material.absorption,
        mode.resolve_tensor(),
        "rendered from the stored bands"
    );
    assert!(
        guard
            .custom_material_physics
            .iter()
            .any(|n| n == "Physics Ruby")
    );
}

#[test]
fn saving_in_fantasy_mode_keeps_the_recipe_and_unflags_the_material() {
    let (db, ctx) = setup();
    let mut mode = ruby_mode();
    apply_custom_material_save(
        &db,
        &ctx,
        form("Parked", CUSTOM_KEEP_COLOR_INDEX, &mode.to_json()),
    )
    .expect("saves");
    // Physics -> fantasy -> save: the preset (2 = Red) is the fantasy payload now.
    mode.switch_to_fantasy();
    apply_custom_material_save(&db, &ctx, form("Parked", 2, &mode.to_json())).expect("saves");
    let row = stored(&db, "Parked");
    let back = colorMode::from_json(row.color_recipe_json.as_deref().unwrap()).unwrap();
    assert_eq!(back.active, Activecolor::Fantasy);
    assert!(back.last_recipe.is_some(), "the recipe was NOT deleted");
    assert_eq!(back.fantasy_rgb, row.absorption_rgb);
    assert_eq!(row.absorption_rgb, super::absorption_rgb_for_color_index(2));
    let guard = ctx.lock().unwrap();
    assert!(
        guard.custom_material_physics.is_empty(),
        "fantasy-active: no physics default width"
    );
}

#[test]
fn custom_keep_uses_the_stored_fantasy_payload_not_the_old_row() {
    let (db, ctx) = setup();
    let mut mode = colorMode::fantasy([0.4, 0.5, 0.6]);
    mode.last_recipe = ruby_mode().last_recipe;
    apply_custom_material_save(
        &db,
        &ctx,
        form("Keep", CUSTOM_KEEP_COLOR_INDEX, &mode.to_json()),
    )
    .expect("saves");
    assert_eq!(stored(&db, "Keep").absorption_rgb, [0.4, 0.5, 0.6]);
}

#[test]
fn a_plain_fantasy_save_has_no_recipe_column() {
    let (db, ctx) = setup();
    apply_custom_material_save(&db, &ctx, form("Plain", 3, "")).expect("saves");
    let row = stored(&db, "Plain");
    assert_eq!(row.color_recipe_json, None);
    assert_eq!(row.absorption_rgb, super::absorption_rgb_for_color_index(3));
}

#[test]
fn deleting_a_material_clears_its_physics_flag() {
    let (db, ctx) = setup();
    apply_custom_material_save(
        &db,
        &ctx,
        form("Gone", CUSTOM_KEEP_COLOR_INDEX, &ruby_mode().to_json()),
    )
    .expect("saves");
    super::apply_custom_material_delete(&db, &ctx, "Gone").expect("deletes");
    assert!(ctx.lock().unwrap().custom_material_physics.is_empty());
}
