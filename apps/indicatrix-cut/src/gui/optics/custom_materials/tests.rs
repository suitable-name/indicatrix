//! The material editor's save path: both color payloads reach the vault row and the render
//! context, whichever mode is active, and saving in fantasy mode never deletes the recipe.

use super::{
    CUSTOM_KEEP_COLOR_INDEX, CustomMaterialForm, absorption_rgb_for_color_index,
    apply_custom_material_save,
};
use crate::{bridge::render_thread::RenderContext, gui::optics::physics_state::PhysicsState};
use indicatrix::optics::{
    chromophore::{ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve},
    dispersion::DispersionModel,
    materials::GemMaterial,
};
use indicatrix_cut_core::{
    material::{ActiveColor, ColorMode},
    native::dispersion_model_to_json,
};
use indicatrix_vault::db::sqlite::Database;
use slint::SharedString;
use std::sync::{Arc, Mutex};

fn ruby_mode() -> ColorMode {
    let cat = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("corundum", cat.data_version);
    recipe.set_amount("Cr", 0.3);
    let (tensor, _) = resolve(&recipe, cat).expect("ruby resolves");
    recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    ColorMode::physics(recipe, [0.1, 0.2, 0.3])
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
        dispersion_model_json: SharedString::default(),
        body_bands: None,
    }
}

/// A plain-swatch form carrying the Dispersion section's model text.
fn form_with_model(name: &str, model_json: &str) -> CustomMaterialForm {
    CustomMaterialForm {
        dispersion_model_json: SharedString::from(model_json),
        ..form(name, 3, "")
    }
}

/// The live copy of the saved material called `name`, taken so the render context's lock is
/// released at once.
fn live(ctx: &Arc<Mutex<RenderContext>>, name: &str) -> GemMaterial {
    ctx.lock()
        .unwrap()
        .custom_materials
        .iter()
        .find(|m| m.name == name)
        .cloned()
        .expect("the material is in the live list")
}

/// The N-BK7 glass curve: a three-term Sellmeier that no refractive index and dispersion pair
/// can describe.
fn bk7() -> DispersionModel {
    GemMaterial::by_name("Glass (N-BK7)")
        .expect("N-BK7 is a built-in")
        .dispersion
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
    let back = ColorMode::from_json(row.color_recipe_json.as_deref().unwrap()).unwrap();
    assert_eq!(back.active, ActiveColor::Physics);
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

    let material = live(&ctx, "Physics Ruby");
    assert_eq!(
        material.absorption,
        mode.resolve_tensor(),
        "rendered from the stored bands"
    );
    assert!(
        ctx.lock()
            .unwrap()
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
    let back = ColorMode::from_json(row.color_recipe_json.as_deref().unwrap()).unwrap();
    assert_eq!(back.active, ActiveColor::Fantasy);
    assert!(back.last_recipe.is_some(), "the recipe was NOT deleted");
    assert_eq!(back.fantasy_rgb, row.absorption_rgb);
    assert_eq!(row.absorption_rgb, super::absorption_rgb_for_color_index(2));
    assert!(
        ctx.lock().unwrap().custom_material_physics.is_empty(),
        "fantasy-active: no physics default width"
    );
}

#[test]
fn custom_keep_uses_the_stored_fantasy_payload_not_the_old_row() {
    let (db, ctx) = setup();
    let mut mode = ColorMode::fantasy([0.4, 0.5, 0.6]);
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

/// The swatch the dialog preselects for a stored material (`push_selected_custom_material_fields`).
fn preselected_swatch(stored_mode: &ColorMode) -> i32 {
    super::color_index_for_absorption_rgb(stored_mode.fantasy_rgb)
        .unwrap_or(CUSTOM_KEEP_COLOR_INDEX)
}

/// Without the physics editor the dialog still opens a stored physics material (Fantasy
/// controls shown) and Save hands the state's JSON and the preselected swatch back: nothing the
/// user did not touch may change -- not the mode, not the recipe, not one byte of the JSON.
#[test]
fn opening_and_saving_a_physics_material_untouched_keeps_it_byte_identical() {
    let cat = ChromophoreCatalogue::global();
    let preset_two = super::absorption_rgb_for_color_index(2);
    for fantasy_rgb in [[0.1, 0.2, 0.3], preset_two] {
        let (db, ctx) = setup();
        let mut mode = ruby_mode();
        mode.fantasy_rgb = fantasy_rgb;
        apply_custom_material_save(
            &db,
            &ctx,
            form("Hidden Editor", CUSTOM_KEEP_COLOR_INDEX, &mode.to_json()),
        )
        .expect("saves");
        let before = stored(&db, "Hidden Editor");
        let before_json = before
            .color_recipe_json
            .clone()
            .expect("a recipe is stored");

        // Open exactly as the dialog does, then save without touching the color.
        let state = PhysicsState::open(
            &before_json,
            before.absorption_rgb,
            "Hidden Editor",
            0.0,
            cat,
            0,
        );
        assert!(state.is_physics() && !state.is_dirty());
        let idx = preselected_swatch(&mode);
        apply_custom_material_save(&db, &ctx, form("Hidden Editor", idx, &state.mode_json()))
            .expect("saves");

        let after = stored(&db, "Hidden Editor");
        assert_eq!(after.color_recipe_json, before.color_recipe_json);
        assert_eq!(after.absorption_rgb, before.absorption_rgb);
        assert!(
            ctx.lock()
                .unwrap()
                .custom_material_physics
                .iter()
                .any(|n| n == "Hidden Editor"),
            "it still renders from its recipe"
        );
    }
}

/// A color text the swatch row leaves alone is stored exactly as it came -- even when it is not
/// the canonical serialisation -- and a changed swatch re-serialises it.
#[test]
fn a_color_text_nobody_changed_is_written_back_verbatim() {
    let (db, ctx) = setup();
    let mode = ruby_mode();
    let pretty = serde_json::to_string_pretty(&mode).expect("serialises");
    assert_ne!(
        pretty,
        mode.to_json(),
        "the test needs a non-canonical text"
    );
    apply_custom_material_save(
        &db,
        &ctx,
        form("Verbatim", CUSTOM_KEEP_COLOR_INDEX, &pretty),
    )
    .expect("saves");
    assert_eq!(
        stored(&db, "Verbatim").color_recipe_json.as_deref(),
        Some(pretty.as_str())
    );

    // The same text with a different swatch is a change: it is re-serialised.
    apply_custom_material_save(&db, &ctx, form("Verbatim", 2, &pretty)).expect("saves");
    let row = stored(&db, "Verbatim");
    let back = ColorMode::from_json(row.color_recipe_json.as_deref().unwrap()).unwrap();
    assert_eq!(back.fantasy_rgb, super::absorption_rgb_for_color_index(2));
    assert_eq!(row.color_recipe_json, Some(back.to_json()));
}

/// Picking a color over a stored recipe (editor hidden) saves a fixed color, keeps the recipe in
/// the JSON, and the material stops rendering from it.
#[test]
fn picking_a_color_over_a_stored_recipe_saves_a_fixed_color_and_keeps_the_recipe() {
    let cat = ChromophoreCatalogue::global();
    let (db, ctx) = setup();
    let mode = ruby_mode();
    apply_custom_material_save(
        &db,
        &ctx,
        form("Picked Over", CUSTOM_KEEP_COLOR_INDEX, &mode.to_json()),
    )
    .expect("saves");
    let stored_json = stored(&db, "Picked Over")
        .color_recipe_json
        .expect("a recipe is stored");

    let mut state = PhysicsState::open(&stored_json, mode.fantasy_rgb, "Picked Over", 0.0, cat, 0);
    // The "Clear" swatch (index 0) is an explicit pick: all-zero, never re-seeded.
    state.choose_fixed_color(Some(super::absorption_rgb_for_color_index(0)));
    assert!(state.is_dirty());
    apply_custom_material_save(&db, &ctx, form("Picked Over", 0, &state.mode_json()))
        .expect("saves");

    let row = stored(&db, "Picked Over");
    let back = ColorMode::from_json(row.color_recipe_json.as_deref().unwrap()).unwrap();
    assert_eq!(back.active, ActiveColor::Fantasy);
    assert_eq!(back.fantasy_rgb, super::absorption_rgb_for_color_index(0));
    assert_eq!(back.last_recipe, mode.last_recipe, "the recipe stays saved");
    assert_eq!(row.absorption_rgb, super::absorption_rgb_for_color_index(0));
    assert!(ctx.lock().unwrap().custom_material_physics.is_empty());
}

/// A coefficient model in the form is the curve the stone is traced with: the row keeps the
/// model text and its own `n_d` / `n_F - n_C` (not the numbers the sliders still held), and
/// the live material carries the identical curve.
#[test]
fn a_coefficient_model_is_stored_and_becomes_the_materials_curve() {
    let (db, ctx) = setup();
    let model = bk7();
    let json = dispersion_model_to_json(&model);
    apply_custom_material_save(&db, &ctx, form_with_model("Coefficients", &json)).expect("saves");

    let row = stored(&db, "Coefficients");
    assert_eq!(row.dispersion_model_json.as_deref(), Some(json.as_str()));
    assert_eq!(row.refractive_index.to_bits(), model.n_d().to_bits());
    assert_eq!(row.dispersion.to_bits(), model.delta_f_c().to_bits());

    let material = live(&ctx, "Coefficients");
    assert_eq!(material.dispersion, model);
    assert_eq!(
        material.birefringence_delta, -0.008,
        "the other crystal-optics numbers are the form's"
    );
}

/// The text is re-serialised on the way in, so the column always holds the canonical form.
#[test]
fn the_stored_model_text_is_canonical_whatever_the_dialog_sent() {
    let (db, ctx) = setup();
    let model = bk7();
    let canonical = dispersion_model_to_json(&model);
    let spaced = format!("  {}  ", canonical.replace(',', ", "));
    apply_custom_material_save(&db, &ctx, form_with_model("Spaced", &spaced)).expect("saves");
    assert_eq!(
        stored(&db, "Spaced").dispersion_model_json.as_deref(),
        Some(canonical.as_str())
    );
}

/// A model the engine would refuse is never written, whoever sent it: the toast says why and the
/// vault and the live list stay as they were.
#[test]
fn an_unusable_model_is_refused_and_nothing_is_written() {
    for bad in [
        "not json at all",
        r#"{"kind":"sellmeier1","b1":1.0,"c1":0.36}"#,
        r#"{"kind":"cauchy","a":0.9,"b":0.0,"c":0.0}"#,
    ] {
        let (db, ctx) = setup();
        let refusal = apply_custom_material_save(&db, &ctx, form_with_model("Refused", bad))
            .err()
            .unwrap_or_else(|| panic!("{bad} should be refused"));
        assert!(refusal.contains("cannot be used"), "{refusal}");
        let rows = db.lock().unwrap().get_custom_materials().unwrap();
        assert_eq!(rows.len(), 0, "nothing reached the vault");
        assert!(ctx.lock().unwrap().custom_materials.is_empty());
    }
}

/// Saving in the Simple mode (blank model text) over a material that had a model drops the
/// model: the row is the plain numbers again and the curve the Cauchy fit of them.
#[test]
fn saving_without_a_model_over_a_modelled_material_returns_to_the_plain_path() {
    let (db, ctx) = setup();
    let json = dispersion_model_to_json(&bk7());
    apply_custom_material_save(&db, &ctx, form_with_model("Switch", &json)).expect("saves");
    assert!(stored(&db, "Switch").dispersion_model_json.is_some());

    apply_custom_material_save(&db, &ctx, form("Switch", 3, "")).expect("saves");
    let row = stored(&db, "Switch");
    assert_eq!(row.dispersion_model_json, None);
    assert_eq!(row.refractive_index, 1.768);
    assert_eq!(row.dispersion, 0.018);
    let plain = GemMaterial::new_custom("Switch", 1.768, 0.018, -0.008, [0.0; 3]);
    assert_eq!(live(&ctx, "Switch").dispersion, plain.dispersion);
}

/// A plain save (the existing path) never writes a model column.
#[test]
fn a_plain_save_has_no_dispersion_model_column() {
    let (db, ctx) = setup();
    apply_custom_material_save(&db, &ctx, form("Numbers", 3, "")).expect("saves");
    assert_eq!(stored(&db, "Numbers").dispersion_model_json, None);
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

const BANDS: [[f32; 3]; 2] = [[460.0, 45.0, 0.25], [540.0, 45.0, 0.1]];

#[test]
fn the_lch_editors_bands_are_stored_beside_the_triple_and_colour_the_live_material() {
    let (db, ctx) = setup();
    let banded = CustomMaterialForm {
        body_bands: Some(BANDS.to_vec()),
        ..form("Banded Sapphire", 3, "")
    };
    apply_custom_material_save(&db, &ctx, banded).expect("saves");
    let row = stored(&db, "Banded Sapphire");
    let json = row.absorption_bands_json.as_deref().expect("bands stored");
    assert_eq!(
        indicatrix_cut_core::material::absorption_bands_from_json(json),
        Some(BANDS.to_vec())
    );
    assert_eq!(
        row.absorption_rgb,
        absorption_rgb_for_color_index(3),
        "the legacy triple is still written"
    );
    let plain = live(&ctx, "Banded Sapphire");
    assert!(
        plain.absorption_path_scale > 1.0,
        "the live copy renders from the bands"
    );

    // Saving again without bands (a preset was picked) clears the column.
    apply_custom_material_save(&db, &ctx, form("Banded Sapphire", 3, "")).expect("saves");
    assert_eq!(stored(&db, "Banded Sapphire").absorption_bands_json, None);
}

#[test]
fn a_physics_recipe_wins_over_bands() {
    let (db, ctx) = setup();
    let form = CustomMaterialForm {
        body_bands: Some(BANDS.to_vec()),
        ..form(
            "Physics Banded",
            CUSTOM_KEEP_COLOR_INDEX,
            &ruby_mode().to_json(),
        )
    };
    apply_custom_material_save(&db, &ctx, form).expect("saves");
    assert_eq!(stored(&db, "Physics Banded").absorption_bands_json, None);
}
