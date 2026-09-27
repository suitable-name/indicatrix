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

// LEGACY_SOURCE_ID has no guard test here: what it must stay equal to isn't visible
// from this crate. See that constant's own doc comment.
