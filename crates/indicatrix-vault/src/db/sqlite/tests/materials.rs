//! `custom_gem_materials` column migrations (crystal-optics, per-axis dispersion,
//! specific gravity) and the `save_custom_material`/`get_custom_materials` round trip
//! through them.

use super::{super::*, fixtures::temp_db_path};

#[test]
fn a_fresh_database_already_has_the_crystal_optics_columns() {
    let path = temp_db_path("fresh_crystal_optics");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    for column in [
        "crystal_system",
        "optical_character",
        "biaxial_delta_beta_alpha",
    ] {
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", column).unwrap(),
            "a fresh database's CREATE TABLE must already include {column}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

/// Seeds a `custom_gem_materials` row via the pre-crystal-optics schema directly
/// (bypassing `Database::new`): no `crystal_system`/`optical_character`/
/// `biaxial_delta_beta_alpha` columns at all.
fn seed_pre_crystal_optics_custom_material(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open raw connection for seeding");
    conn.execute_batch(
            "CREATE TABLE custom_gem_materials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                refractive_index REAL NOT NULL,
                dispersion REAL NOT NULL,
                birefringence REAL NOT NULL,
                absorption_r REAL NOT NULL,
                absorption_g REAL NOT NULL,
                absorption_b REAL NOT NULL
            );
            INSERT INTO custom_gem_materials
                (name, refractive_index, dispersion, birefringence, absorption_r, absorption_g, absorption_b)
                VALUES ('Legacy Custom Sapphire', 1.768, 0.018, -0.008, 2.8, 1.2, 0.1);",
        )
        .expect("create pre-crystal-optics custom_gem_materials and seed a row");
}

#[test]
fn crystal_optics_migration_adds_the_columns_and_leaves_existing_rows_nullable() {
    let path = temp_db_path("crystal_optics_migration");
    seed_pre_crystal_optics_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        for column in [
            "crystal_system",
            "optical_character",
            "biaxial_delta_beta_alpha",
        ] {
            assert!(
                Database::column_exists(&db.conn, "custom_gem_materials", column).unwrap(),
                "migration must add {column}"
            );
        }

        // Pre-existing row survives; new fields are None (fall back to
        // GemMaterial::new_custom's inference), not defaulted to a guessed value.
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        let m = &materials[0];
        assert_eq!(m.name, "Legacy Custom Sapphire");
        assert!((m.refractive_index - 1.768).abs() < 1e-6);
        assert_eq!(m.crystal_system, None);
        assert_eq!(m.optical_character, None);
        assert_eq!(m.biaxial_delta_beta_alpha, None);
    }

    // Second open: no-op, pre-existing row survives untouched.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2
        .get_custom_materials()
        .expect("read back materials again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].crystal_system, None);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn save_custom_material_round_trips_crystal_optics_fields() {
    let path = temp_db_path("crystal_optics_roundtrip");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    // Biaxial material: all three new fields set.
    db.save_custom_material(&CustomMaterialParams {
        name: "Custom Tanzanite",
        refractive_index: 1.691,
        dispersion: 0.030,
        birefringence: 0.0130,
        absorption_rgb: [1.8, 1.6, 0.2],
        crystal_system: Some("Orthorhombic"),
        optical_character: Some("BiaxialPositive"),
        biaxial_delta_beta_alpha: Some(0.0070),
        per_axis_dispersion_json: Some(r#"{"kind":"uniaxial_extraordinary","a":1.7,"b":0.01}"#),
        specific_gravity: Some(3.35),
        color_recipe_json: None,
        dispersion_model_json: None,
        absorption_bands_json: None,
    })
    .expect("save biaxial custom material");

    // Uniaxial material: crystal_system/optical_character set, biaxial delta None.
    db.save_custom_material(&CustomMaterialParams {
        name: "Custom Sapphire",
        refractive_index: 1.768,
        dispersion: 0.018,
        birefringence: -0.008,
        absorption_rgb: [2.8, 1.2, 0.1],
        crystal_system: Some("Trigonal"),
        optical_character: Some("UniaxialNegative"),
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
        specific_gravity: Some(4.00),
        color_recipe_json: None,
        dispersion_model_json: None,
        absorption_bands_json: None,
    })
    .expect("save uniaxial custom material");

    let materials = db.get_custom_materials().expect("read back materials");
    assert_eq!(materials.len(), 2);

    let tanzanite = materials
        .iter()
        .find(|m| m.name == "Custom Tanzanite")
        .expect("Custom Tanzanite present");
    assert_eq!(tanzanite.crystal_system.as_deref(), Some("Orthorhombic"));
    assert_eq!(
        tanzanite.optical_character.as_deref(),
        Some("BiaxialPositive")
    );
    assert!((tanzanite.biaxial_delta_beta_alpha.unwrap() - 0.0070).abs() < 1e-6);
    assert_eq!(
        tanzanite.per_axis_dispersion_json.as_deref(),
        Some(r#"{"kind":"uniaxial_extraordinary","a":1.7,"b":0.01}"#)
    );
    assert!((tanzanite.specific_gravity.unwrap() - 3.35).abs() < 1e-6);

    let sapphire = materials
        .iter()
        .find(|m| m.name == "Custom Sapphire")
        .expect("Custom Sapphire present");
    assert_eq!(sapphire.crystal_system.as_deref(), Some("Trigonal"));
    assert_eq!(
        sapphire.optical_character.as_deref(),
        Some("UniaxialNegative")
    );
    assert_eq!(sapphire.biaxial_delta_beta_alpha, None);
    assert_eq!(sapphire.per_axis_dispersion_json, None);
    assert!((sapphire.specific_gravity.unwrap() - 4.00).abs() < 1e-6);

    // Re-saving over the same name (upsert) must update crystal-optics columns too.
    db.save_custom_material(&CustomMaterialParams {
        name: "Custom Sapphire",
        refractive_index: 1.768,
        dispersion: 0.018,
        birefringence: -0.008,
        absorption_rgb: [2.8, 1.2, 0.1],
        crystal_system: None,
        optical_character: None,
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
        specific_gravity: None,
        color_recipe_json: None,
        dispersion_model_json: None,
        absorption_bands_json: None,
    })
    .expect("re-save clears crystal-optics fields");
    let materials = db.get_custom_materials().expect("read back after re-save");
    let sapphire = materials
        .iter()
        .find(|m| m.name == "Custom Sapphire")
        .expect("Custom Sapphire still present");
    assert_eq!(sapphire.crystal_system, None);
    assert_eq!(sapphire.optical_character, None);
    assert_eq!(sapphire.specific_gravity, None);

    let _ = std::fs::remove_file(&path);
}

/// Mirrors `a_fresh_database_already_has_the_crystal_optics_columns` for the new
/// `per_axis_dispersion_json` column.
#[test]
fn a_fresh_database_already_has_the_per_axis_dispersion_column() {
    let path = temp_db_path("fresh_per_axis_dispersion");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    assert!(
        Database::column_exists(&db.conn, "custom_gem_materials", "per_axis_dispersion_json")
            .unwrap(),
        "a fresh database's CREATE TABLE must already include per_axis_dispersion_json"
    );
    let _ = std::fs::remove_file(&path);
}

/// Seeds a `custom_gem_materials` row via the schema predating this column
/// (bypassing `Database::new`), including the crystal-optics columns
/// `migrate_crystal_optics_columns` already added -- this migration is purely
/// additive on top of that one.
fn seed_pre_per_axis_dispersion_custom_material(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open raw connection for seeding");
    conn.execute_batch(
            "CREATE TABLE custom_gem_materials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                refractive_index REAL NOT NULL,
                dispersion REAL NOT NULL,
                birefringence REAL NOT NULL,
                absorption_r REAL NOT NULL,
                absorption_g REAL NOT NULL,
                absorption_b REAL NOT NULL,
                crystal_system TEXT,
                optical_character TEXT,
                biaxial_delta_beta_alpha REAL
            );
            INSERT INTO custom_gem_materials
                (name, refractive_index, dispersion, birefringence, absorption_r, absorption_g, absorption_b,
                 crystal_system, optical_character, biaxial_delta_beta_alpha)
                VALUES ('Legacy Custom Quartz', 1.544, 0.013, 0.0091, 0.8, 1.8, 0.6,
                        'Trigonal', 'UniaxialPositive', NULL);",
        )
        .expect("create pre-per-axis-dispersion custom_gem_materials and seed a row");
}

/// Old schema (crystal-optics columns, no `per_axis_dispersion_json`) -> migrated ->
/// readable, mirroring `crystal_optics_migration_adds_the_columns_and_leaves_existing_rows_nullable`.
#[test]
fn per_axis_dispersion_migration_adds_the_column_and_leaves_existing_rows_nullable() {
    let path = temp_db_path("per_axis_dispersion_migration");
    seed_pre_per_axis_dispersion_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", "per_axis_dispersion_json")
                .unwrap(),
            "migration must add per_axis_dispersion_json"
        );

        // Pre-existing row survives, crystal-optics fields intact, new field None.
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        let m = &materials[0];
        assert_eq!(m.name, "Legacy Custom Quartz");
        assert!((m.refractive_index - 1.544).abs() < 1e-6);
        assert_eq!(m.crystal_system.as_deref(), Some("Trigonal"));
        assert_eq!(m.optical_character.as_deref(), Some("UniaxialPositive"));
        assert_eq!(m.per_axis_dispersion_json, None);
    }

    // Second open: no-op, pre-existing row survives untouched.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2
        .get_custom_materials()
        .expect("read back materials again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].per_axis_dispersion_json, None);

    let _ = std::fs::remove_file(&path);
}

/// Mirrors `a_fresh_database_already_has_the_per_axis_dispersion_column` for the
/// `specific_gravity` column.
#[test]
fn a_fresh_database_already_has_the_specific_gravity_column() {
    let path = temp_db_path("fresh_specific_gravity");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    assert!(
        Database::column_exists(&db.conn, "custom_gem_materials", "specific_gravity").unwrap(),
        "a fresh database's CREATE TABLE must already include specific_gravity"
    );
    let _ = std::fs::remove_file(&path);
}

/// Seeds a `custom_gem_materials` row via the schema predating this column
/// (bypassing `Database::new`), including every column
/// `migrate_per_axis_dispersion_column` already added -- this migration is purely
/// additive on top of that one.
fn seed_pre_specific_gravity_custom_material(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open raw connection for seeding");
    conn.execute_batch(
            "CREATE TABLE custom_gem_materials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                refractive_index REAL NOT NULL,
                dispersion REAL NOT NULL,
                birefringence REAL NOT NULL,
                absorption_r REAL NOT NULL,
                absorption_g REAL NOT NULL,
                absorption_b REAL NOT NULL,
                crystal_system TEXT,
                optical_character TEXT,
                biaxial_delta_beta_alpha REAL,
                per_axis_dispersion_json TEXT
            );
            INSERT INTO custom_gem_materials
                (name, refractive_index, dispersion, birefringence, absorption_r, absorption_g, absorption_b,
                 crystal_system, optical_character, biaxial_delta_beta_alpha, per_axis_dispersion_json)
                VALUES ('Legacy Custom Peridot', 1.690, 0.020, 0.0360, 0.4, 1.6, 0.2,
                        'Orthorhombic', 'BiaxialPositive', 0.0360, NULL);",
        )
        .expect("create pre-specific-gravity custom_gem_materials and seed a row");
}

/// Old schema (per-axis dispersion column, no `specific_gravity`) -> migrated ->
/// readable, mirroring `per_axis_dispersion_migration_adds_the_column_and_leaves_existing_rows_nullable`.
#[test]
fn specific_gravity_migration_adds_the_column_and_leaves_existing_rows_nullable() {
    let path = temp_db_path("specific_gravity_migration");
    seed_pre_specific_gravity_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", "specific_gravity").unwrap(),
            "migration must add specific_gravity"
        );

        // Pre-existing row survives, earlier columns intact, new field None.
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        let m = &materials[0];
        assert_eq!(m.name, "Legacy Custom Peridot");
        assert!((m.refractive_index - 1.690).abs() < 1e-6);
        assert_eq!(m.crystal_system.as_deref(), Some("Orthorhombic"));
        assert_eq!(m.specific_gravity, None);
    }

    // Second open: no-op, pre-existing row survives untouched.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2
        .get_custom_materials()
        .expect("read back materials again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].specific_gravity, None);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_fresh_database_already_has_the_color_recipe_column() {
    let path = temp_db_path("fresh_color_recipe");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    assert!(
        Database::column_exists(&db.conn, "custom_gem_materials", "color_recipe_json").unwrap(),
        "a fresh database's CREATE TABLE must already include color_recipe_json"
    );
    let _ = std::fs::remove_file(&path);
}

fn seed_pre_color_recipe_custom_material(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open raw connection for seeding");
    conn.execute_batch(
        "CREATE TABLE custom_gem_materials (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            refractive_index REAL NOT NULL,
            dispersion REAL NOT NULL,
            birefringence REAL NOT NULL,
            absorption_r REAL NOT NULL,
            absorption_g REAL NOT NULL,
            absorption_b REAL NOT NULL,
            crystal_system TEXT,
            optical_character TEXT,
            biaxial_delta_beta_alpha REAL,
            per_axis_dispersion_json TEXT,
            specific_gravity REAL
        );
        INSERT INTO custom_gem_materials
            (name, refractive_index, dispersion, birefringence, absorption_r, absorption_g, absorption_b,
             crystal_system, optical_character, biaxial_delta_beta_alpha, per_axis_dispersion_json, specific_gravity)
            VALUES ('Legacy Custom Ruby', 1.768, 0.018, -0.008, 0.1, 2.5, 2.0,
                    'Trigonal', 'UniaxialNegative', NULL, NULL, 4.0);",
    )
    .expect("create pre-color-recipe custom_gem_materials and seed a row");
}

#[test]
fn color_recipe_migration_adds_the_column_and_leaves_existing_rows_nullable() {
    let path = temp_db_path("color_recipe_migration");
    seed_pre_color_recipe_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", "color_recipe_json").unwrap(),
            "migration must add color_recipe_json"
        );

        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        let m = &materials[0];
        assert_eq!(m.name, "Legacy Custom Ruby");
        assert_eq!(m.color_recipe_json, None);
    }

    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2
        .get_custom_materials()
        .expect("read back materials again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].color_recipe_json, None);

    let _ = std::fs::remove_file(&path);
}

/// A real, resolved ruby recipe (the typed payload the desktop stores).
fn typed_ruby_recipe() -> indicatrix::optics::chromophore::ColorRecipe {
    use indicatrix::optics::chromophore::{
        ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve,
    };
    let cat = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("corundum", cat.data_version);
    recipe.set_amount("Cr", 0.3);
    let (tensor, _) = resolve(&recipe, cat).expect("ruby resolves");
    recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    recipe
}

/// The column is opaque text to this crate: whatever string is stored comes back byte for byte.
#[test]
fn custom_material_color_recipe_round_trip() {
    let path = temp_db_path("color_recipe_round_trip");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create db");

    // The real `ResolvedBands` field names (`o_ray`/`e_ray`/`beta_ray`/`is_pleochroic`).
    let recipe_json = r#"{"host":"corundum","data_version":2,"entries":[],"treatments":[],"strength":1.0,"reference_path_mm":5.0,"resolved_bands":{"o_ray":[],"e_ray":[],"is_pleochroic":true}}"#;
    assert!(
        serde_json::from_str::<indicatrix::optics::chromophore::ColorRecipe>(recipe_json).is_ok(),
        "the fixture JSON must deserialize into the real recipe type"
    );

    db.save_custom_material(&CustomMaterialParams {
        name: "Physics Sapphire",
        refractive_index: 1.768,
        dispersion: 0.018,
        birefringence: -0.008,
        absorption_rgb: [1.0, 0.5, 0.0],
        crystal_system: Some("Trigonal"),
        optical_character: Some("UniaxialNegative"),
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
        specific_gravity: Some(4.0),
        color_recipe_json: Some(recipe_json),
        dispersion_model_json: None,
        absorption_bands_json: None,
    })
    .expect("save custom material");

    let materials = db.get_custom_materials().expect("get custom materials");
    let mat = materials
        .iter()
        .find(|m| m.name == "Physics Sapphire")
        .expect("find mat");
    assert_eq!(mat.color_recipe_json.as_deref(), Some(recipe_json));

    let _ = std::fs::remove_file(&path);
}

/// Typed round trip: a real recipe (with its resolved bands and data version) serialised, stored
/// in the vault, read back and deserialised is equal to the original.
#[test]
fn typed_color_recipe_survives_the_vault() {
    use indicatrix::optics::chromophore::ColorRecipe;

    let path = temp_db_path("color_recipe_typed_round_trip");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create db");
    let recipe = typed_ruby_recipe();
    assert!(!recipe.resolved_bands.o_ray.is_empty(), "a ruby has bands");
    let json = serde_json::to_string(&recipe).expect("serialise recipe");

    db.save_custom_material(&CustomMaterialParams {
        name: "Typed Ruby",
        refractive_index: 1.768,
        dispersion: 0.018,
        birefringence: -0.008,
        absorption_rgb: [0.2, 1.4, 2.8],
        crystal_system: Some("Trigonal"),
        optical_character: Some("UniaxialNegative"),
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
        specific_gravity: None,
        color_recipe_json: Some(&json),
        dispersion_model_json: None,
        absorption_bands_json: None,
    })
    .expect("save custom material");

    let rows = db.get_custom_materials().expect("get custom materials");
    let stored = rows
        .iter()
        .find(|m| m.name == "Typed Ruby")
        .and_then(|m| m.color_recipe_json.as_deref())
        .expect("recipe stored");
    let back: ColorRecipe = serde_json::from_str(stored).expect("deserialise the stored recipe");
    assert_eq!(back, recipe);
    assert_eq!(back.data_version, recipe.data_version);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_fresh_database_already_has_the_dispersion_model_column() {
    let path = temp_db_path("fresh_dispersion_model");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    assert!(
        Database::column_exists(&db.conn, "custom_gem_materials", "dispersion_model_json").unwrap(),
        "a fresh database's CREATE TABLE must already include dispersion_model_json"
    );
    let _ = std::fs::remove_file(&path);
}

/// A database from before the column existed gains it as `NULL` on every row (so the row
/// keeps loading as the plain refractive-index path), and opening it again changes nothing.
#[test]
fn dispersion_model_migration_adds_the_column_and_leaves_existing_rows_null() {
    let path = temp_db_path("dispersion_model_migration");
    seed_pre_color_recipe_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", "dispersion_model_json")
                .unwrap(),
            "migration must add dispersion_model_json"
        );
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        assert_eq!(materials[0].name, "Legacy Custom Ruby");
        assert_eq!(materials[0].dispersion_model_json, None);
        assert!((materials[0].refractive_index - 1.768).abs() < 1e-6);
    }

    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2
        .get_custom_materials()
        .expect("read back materials again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].dispersion_model_json, None);

    let _ = std::fs::remove_file(&path);
}

/// The column is opaque text to this crate: the stored string comes back byte for byte, a
/// row saved without one reads `None`, and re-saving over a name clears or replaces it.
#[test]
fn dispersion_model_json_round_trips_and_a_resave_replaces_it() {
    let path = temp_db_path("dispersion_model_round_trip");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create db");
    let model_json = r#"{"kind":"sellmeier3","b":[1.0396122,0.23179235,1.0104694],"c":[0.006000699,0.020017914,103.56065]}"#;
    let save = |name: &str, json: Option<&str>| {
        db.save_custom_material(&CustomMaterialParams {
            name,
            refractive_index: 1.5168,
            dispersion: 0.008,
            birefringence: 0.0,
            absorption_rgb: [0.0, 0.0, 0.0],
            crystal_system: Some("Cubic"),
            optical_character: Some("Isotropic"),
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity: None,
            color_recipe_json: None,
            dispersion_model_json: json,
            absorption_bands_json: None,
        })
        .expect("save custom material");
    };
    let stored = |name: &str| {
        db.get_custom_materials()
            .expect("read back")
            .into_iter()
            .find(|m| m.name == name)
            .expect("row present")
    };

    save("With Model", Some(model_json));
    save("Plain", None);
    assert_eq!(
        stored("With Model").dispersion_model_json.as_deref(),
        Some(model_json)
    );
    assert_eq!(stored("Plain").dispersion_model_json, None);

    // Back to the plain path: the model is cleared.
    save("With Model", None);
    assert_eq!(stored("With Model").dispersion_model_json, None);
    // And on again with a different one.
    let cauchy = r#"{"kind":"cauchy","a":1.7,"b":0.006,"c":0.0}"#;
    save("With Model", Some(cauchy));
    assert_eq!(
        stored("With Model").dispersion_model_json.as_deref(),
        Some(cauchy)
    );

    let _ = std::fs::remove_file(&path);
}

/// A database from before the bands column existed opens unchanged: the column is added as
/// `NULL` (no bands) on every row, the legacy triple still reads, and a second open is a no-op.
#[test]
fn absorption_bands_migration_adds_the_column_and_leaves_existing_rows_null() {
    let path = temp_db_path("absorption_bands_migration");
    seed_pre_color_recipe_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", "absorption_bands_json")
                .unwrap(),
            "migration must add absorption_bands_json"
        );
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        assert_eq!(materials[0].absorption_bands_json, None);
        assert!((materials[0].refractive_index - 1.768).abs() < 1e-6);
    }

    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2.get_custom_materials().expect("read back again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].absorption_bands_json, None);

    let _ = std::fs::remove_file(&path);
}

/// A fresh database has the column in its CREATE TABLE, the stored text comes back byte for
/// byte beside the legacy triple, a row saved without bands reads `None`, and a re-save
/// replaces or clears it.
#[test]
fn absorption_bands_json_round_trips_beside_the_legacy_triple() {
    let path = temp_db_path("absorption_bands_round_trip");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create db");
    assert!(
        Database::column_exists(&db.conn, "custom_gem_materials", "absorption_bands_json").unwrap(),
        "a fresh database's CREATE TABLE must already include absorption_bands_json"
    );
    let bands = "[[460.0,45.0,0.25],[540.0,45.0,0.1]]";
    let save = |name: &str, bands: Option<&str>| {
        db.save_custom_material(&CustomMaterialParams {
            name,
            refractive_index: 1.5168,
            dispersion: 0.008,
            birefringence: 0.0,
            absorption_rgb: [0.3, 0.2, 0.1],
            crystal_system: Some("Cubic"),
            optical_character: Some("Isotropic"),
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity: None,
            color_recipe_json: None,
            dispersion_model_json: None,
            absorption_bands_json: bands,
        })
        .expect("save custom material");
    };
    let stored = |name: &str| {
        db.get_custom_materials()
            .expect("read back")
            .into_iter()
            .find(|m| m.name == name)
            .expect("row present")
    };

    save("Banded", Some(bands));
    save("Plain", None);
    assert_eq!(
        stored("Banded").absorption_bands_json.as_deref(),
        Some(bands)
    );
    assert_eq!(stored("Banded").absorption_rgb, [0.3, 0.2, 0.1]);
    assert_eq!(stored("Plain").absorption_bands_json, None);

    save("Banded", None);
    assert_eq!(stored("Banded").absorption_bands_json, None);

    let _ = std::fs::remove_file(&path);
}

// LEGACY_SOURCE_ID has no guard test here: what it must stay equal to isn't visible
// from this crate. See that constant's own doc comment.
