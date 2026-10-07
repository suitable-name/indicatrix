//! A custom material's coefficient-based dispersion through the design file: the vault row's
//! model becomes the native snapshot's `dispersion_model` table, and the snapshot rebuilds the
//! same curve; a plain row leaves the snapshot exactly as it was before the table existed.

use super::save_helpers::custom_material_snapshot_for_save;
use crate::gui::optics::crystal_optics::{
    MaterialSaveFields, save_gem_material, save_gem_material_fields,
};
use indicatrix::optics::{dispersion::DispersionModel, materials::GemMaterial};
use indicatrix_cut_core::{
    Design, FreshDesignSpec, MaterialSelection, PreformSpec,
    native::{dispersion_model_dto, dispersion_model_to_json, gem_material_from_custom_snapshot},
};
use indicatrix_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};

const NAME: &str = "Probe Glass";

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

fn bk7() -> DispersionModel {
    GemMaterial::by_name("Glass (N-BK7)")
        .expect("N-BK7 is a built-in")
        .dispersion
}

/// A database holding `NAME` saved the way the editor saves it: with `model` when given, as
/// the plain refractive-index-and-dispersion row otherwise.
fn db_with(model: Option<DispersionModel>) -> Arc<Mutex<Database>> {
    let db = Database::new(Some(":memory:")).expect("in-memory database");
    if let Some(model) = model {
        let material = GemMaterial::new_custom_with_dispersion(NAME, model, 0.0, [0.0; 3]);
        let json = dispersion_model_to_json(&model);
        save_gem_material_fields(
            &db,
            &material,
            &MaterialSaveFields {
                ri: model.n_d(),
                dispersion: model.delta_f_c(),
                birefringence: 0.0,
                absorption_rgb: [0.0; 3],
                specific_gravity: None,
                color_recipe_json: None,
                dispersion_model_json: Some(&json),
                absorption_bands_json: None,
            },
        )
        .expect("save");
    } else {
        let material = GemMaterial::new_custom(NAME, 1.62, 0.02, 0.0, [0.0; 3]);
        save_gem_material(&db, &material, 1.62, 0.02, 0.0, [0.0; 3], None, None).expect("save");
    }
    Arc::new(Mutex::new(db))
}

/// The snapshot saved with a design carries the row's model, the plain numbers beside it are
/// the model's own, and the rebuilt material has the identical curve.
#[test]
fn the_design_snapshot_carries_the_rows_dispersion_model() {
    let model = bk7();
    let db = db_with(Some(model));
    let snapshot = custom_material_snapshot_for_save(&design(), &db).expect("snapshot");
    assert_eq!(
        snapshot.dispersion_model,
        Some(dispersion_model_dto(&model)),
        "the table is the row's model"
    );
    assert_eq!(snapshot.mean_ri as f32, model.n_d());
    assert!((snapshot.dispersion_delta as f32 - model.delta_f_c()).abs() < 1e-6);
    assert_eq!(
        gem_material_from_custom_snapshot(NAME, &snapshot).dispersion,
        model
    );
}

/// A row without a model leaves the snapshot without a table, and rebuilds the Cauchy fit.
#[test]
fn a_plain_row_leaves_the_snapshot_without_a_model() {
    let db = db_with(None);
    let snapshot = custom_material_snapshot_for_save(&design(), &db).expect("snapshot");
    assert_eq!(snapshot.dispersion_model, None);
    let rebuilt = gem_material_from_custom_snapshot(NAME, &snapshot);
    let expected = GemMaterial::new_custom(NAME, 1.62, 0.02, 0.0, [0.0; 3]);
    assert_eq!(rebuilt.dispersion, expected.dispersion);
}
