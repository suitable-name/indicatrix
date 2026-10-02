//! Import against a catalogue created in the old column layout and migrated in place.

use super::{
    super::confirm::count_pending_collisions, run_import_catching_panics,
    verify_every_imported_id_reads_back,
};
use crate::gui::library::local::helpers::test_support::{
    VALID_ASC, open_temp_db, temp_db_path_for_test, temp_dir_for_test,
};

// --- Always-on: Import against a catalogue migrated from the OLD column layout ---
//
// Unlike the manual probes in `perf` (which need a real user catalogue, or are
// `#[ignore]`d stress runs), the test below builds its own tiny pre-migration-layout
// SQLite file by hand and always runs: the one CI-visible proof that `Database::new`'s
// migration chain and `import_path` still cooperate against a catalogue that started
// in the pre-`migrate_blob_columns_last`/pre-designer-split/pre-proportions shape a
// real `facet_diagrams.sqlite` predating this crate's own history would have had.

/// The pre-migration schema `Database::new`'s migration chain rebuilds, verbatim from
/// `indicatrix-vault`'s own `db::sqlite::tests::fixtures::seed_pre_migration_db`
/// (`diagram_entries`/`diagram_details`/`angle_settings`/`attached_files`/
/// `custom_gem_materials`) plus that same crate's `db::sqlite::tests::migrations::
/// seed_blob_columns_last_fixture`, which adds the blob-first `diagram_previews`/
/// `diagram_tilt_curves` shape `Database::migrate_blob_columns_last` also rebuilds.
/// Duplicated here rather than reused: both live behind `#[cfg(test)]` in
/// `indicatrix-vault`, which -- unlike an ordinary `pub` item -- is compiled OUT of the
/// library this crate links against (`--cfg test` is only set while `indicatrix-vault`
/// compiles itself as a test binary, never for a dependent crate's own `cargo test`),
/// so neither fixture is reachable from here. See
/// `indicatrix_vault::db::sqlite::Connection`'s own doc comment for the one-line
/// re-export that exists instead, so this file can still open its own raw connection
/// without `indicatrix-cut` needing a `rusqlite` dependency of its own.
///
/// `diagram_details.diagram_image_data` sits at column index 4 (`id`, `entry_id`,
/// `page_url`, `diagram_image_name`, `diagram_image_data`), exactly where a real
/// pre-fix `facet_diagrams.sqlite` had it.
const OLD_LAYOUT_SCHEMA_SQL: &str = "
    CREATE TABLE diagram_entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        title TEXT NOT NULL,
        url TEXT NOT NULL UNIQUE,
        design_id TEXT
    );
    CREATE TABLE diagram_details (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        entry_id INTEGER NOT NULL UNIQUE,
        page_url TEXT NOT NULL,
        diagram_image_name TEXT,
        diagram_image_data BLOB,
        competition_diagram TEXT,
        lw_ratio TEXT,
        refractive_index TEXT,
        index_gear TEXT,
        volume TEXT,
        facets_count TEXT,
        shape TEXT,
        designer_info TEXT,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
    CREATE TABLE angle_settings (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        order_idx INTEGER NOT NULL,
        detail_id INTEGER NOT NULL,
        facet TEXT NOT NULL,
        angle TEXT NOT NULL,
        index_val TEXT NOT NULL,
        notes TEXT NOT NULL,
        FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
    );
    CREATE TABLE attached_files (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        detail_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        url TEXT NOT NULL,
        content BLOB NOT NULL,
        FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
    );
    CREATE TABLE custom_gem_materials (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL UNIQUE,
        refractive_index REAL NOT NULL,
        dispersion REAL NOT NULL,
        birefringence REAL NOT NULL,
        absorption_r REAL NOT NULL,
        absorption_g REAL NOT NULL,
        absorption_b REAL NOT NULL
    );
    CREATE TABLE diagram_previews (
        entry_id INTEGER PRIMARY KEY,
        preview_front BLOB,
        preview_top BLOB,
        preview_material TEXT,
        preview_generated_at INTEGER,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
    CREATE TABLE diagram_tilt_curves (
        entry_id INTEGER PRIMARY KEY,
        curves BLOB,
        curve_image BLOB,
        generated_at INTEGER,
        perf_brilliance_global_min REAL,
        perf_brilliance_global_max REAL,
        perf_extinction_global_min REAL,
        perf_extinction_global_max REAL,
        perf_windowing_global_min REAL,
        perf_windowing_global_max REAL,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// Builds `path` as a fresh SQLite file already in [`OLD_LAYOUT_SCHEMA_SQL`]'s shape,
/// with three `diagram_entries`/`diagram_details` pairs, each carrying its own
/// `angle_settings` row:
///
/// 1. `index_val` holding a realistic `-`-separated legacy value (`"96-08-16"`).
/// 2. `designer_info`/`shape`/`refractive_index` all `NULL`.
/// 3. One `attached_files` row and one `diagram_previews` row (itself seeded in ITS
///    OWN pre-fix, blob-first column order), so the preview-cache rebuild is
///    exercised too, not just `diagram_details`'.
///
/// Returns the three seeded `diagram_entries.id`s, in insertion order -- read back via
/// `last_insert_rowid` rather than assumed, even though this is always a brand-new
/// file where they'd be `1..=3` in practice.
fn seed_old_layout_catalogue(path: &std::path::Path) -> [i64; 3] {
    let conn = indicatrix_vault::db::sqlite::Connection::open(path)
        .unwrap_or_else(|e| panic!("open raw pre-migration fixture connection: {e}"));
    conn.execute_batch(OLD_LAYOUT_SCHEMA_SQL)
        .unwrap_or_else(|e| panic!("create pre-migration schema: {e}"));

    // Entry 1: a realistic dash-separated legacy index_val.
    conn.execute_batch(
        "INSERT INTO diagram_entries (title, url, design_id)
         VALUES ('Legacy Dash Index', 'legacy://entry-1', NULL);",
    )
    .unwrap_or_else(|e| panic!("insert entry 1: {e}"));
    let entry_1 = conn.last_insert_rowid();
    conn.execute_batch(&format!(
        "INSERT INTO diagram_details (
             entry_id, page_url, diagram_image_name, competition_diagram, lw_ratio,
             refractive_index, index_gear, volume, facets_count, shape, designer_info
         ) VALUES ({entry_1}, '', NULL, NULL, '1.0', '1.62', '96', '1.1', '55+6', 'Round', 'J. Doe');"
    ))
    .unwrap_or_else(|e| panic!("insert detail 1: {e}"));
    let detail_1 = conn.last_insert_rowid();
    conn.execute_batch(&format!(
        "INSERT INTO angle_settings (order_idx, detail_id, facet, angle, index_val, notes)
         VALUES (0, {detail_1}, 'P1', '41.000000', '96-08-16', '');"
    ))
    .unwrap_or_else(|e| panic!("insert angle_settings for entry 1: {e}"));

    // Entry 2: NULL designer_info/shape/refractive_index.
    conn.execute_batch(
        "INSERT INTO diagram_entries (title, url, design_id)
         VALUES ('Legacy Null Fields', 'legacy://entry-2', NULL);",
    )
    .unwrap_or_else(|e| panic!("insert entry 2: {e}"));
    let entry_2 = conn.last_insert_rowid();
    conn.execute_batch(&format!(
        "INSERT INTO diagram_details (
             entry_id, page_url, diagram_image_name, competition_diagram, lw_ratio,
             refractive_index, index_gear, volume, facets_count, shape, designer_info
         ) VALUES ({entry_2}, '', NULL, NULL, '1.0', NULL, '96', '1.1', '55+6', NULL, NULL);"
    ))
    .unwrap_or_else(|e| panic!("insert detail 2: {e}"));
    let detail_2 = conn.last_insert_rowid();
    conn.execute_batch(&format!(
        "INSERT INTO angle_settings (order_idx, detail_id, facet, angle, index_val, notes)
         VALUES (0, {detail_2}, 'P1', '41.000000', '0', '');"
    ))
    .unwrap_or_else(|e| panic!("insert angle_settings for entry 2: {e}"));

    // Entry 3: one attached file and one preview row.
    conn.execute_batch(
        "INSERT INTO diagram_entries (title, url, design_id)
         VALUES ('Legacy With Attachment', 'legacy://entry-3', NULL);",
    )
    .unwrap_or_else(|e| panic!("insert entry 3: {e}"));
    let entry_3 = conn.last_insert_rowid();
    conn.execute_batch(&format!(
        "INSERT INTO diagram_details (
             entry_id, page_url, diagram_image_name, competition_diagram, lw_ratio,
             refractive_index, index_gear, volume, facets_count, shape, designer_info
         ) VALUES ({entry_3}, '', NULL, NULL, '1.0', '1.72', '96', '1.1', '55+6', 'Round', 'A. Cutter');"
    ))
    .unwrap_or_else(|e| panic!("insert detail 3: {e}"));
    let detail_3 = conn.last_insert_rowid();
    conn.execute_batch(&format!(
        "INSERT INTO angle_settings (order_idx, detail_id, facet, angle, index_val, notes)
         VALUES (0, {detail_3}, 'P1', '41.000000', '0', '');"
    ))
    .unwrap_or_else(|e| panic!("insert angle_settings for entry 3: {e}"));
    conn.execute_batch(&format!(
        "INSERT INTO attached_files (detail_id, name, url, content)
         VALUES ({detail_3}, 'legacy.asc', '', X'01020304');"
    ))
    .unwrap_or_else(|e| panic!("insert attached_files for entry 3: {e}"));
    conn.execute_batch(&format!(
        "INSERT INTO diagram_previews (
             entry_id, preview_front, preview_top, preview_material, preview_generated_at
         ) VALUES ({entry_3}, X'0102', X'0304', 'Quartz', 1700000000);"
    ))
    .unwrap_or_else(|e| panic!("insert diagram_previews for entry 3: {e}"));

    drop(conn);
    [entry_1, entry_2, entry_3]
}

/// Writes exactly one real `.asc` + `.indicatrix.toml` sidecar pair into `dir`, the
/// same way `write_sidecar_pairs` does for its two -- a solved
/// [`indicatrix_cut_core::design::Design`] from the first entry of
/// [`indicatrix_cut_core::templates::TEMPLATES`], written out via
/// [`indicatrix_cut_core::native::save_paired`]. Kept to exactly one pair (unlike
/// `write_sidecar_pairs`'s two) so this test's corpus imports exactly 2 files, not 3.
fn write_single_sidecar_pair(dir: &std::path::Path) {
    let spec = indicatrix_cut_core::templates::TEMPLATES
        .first()
        .expect("TEMPLATES is non-empty");
    let design = indicatrix_cut_core::design::Design::new(
        indicatrix_cut_core::preform::PreformSpec::cylinder(
            spec.gear_teeth.unsigned_abs() as usize,
            1.5,
            1.0,
            1.5,
        ),
        spec.schedule_meta(),
        spec.tiers(),
    );
    design
        .solve()
        .unwrap_or_else(|e| panic!("template {:?} failed to solve: {e:?}", spec.name));
    let asc_filename = "old_layout_sidecar.asc";
    let paired = indicatrix_cut_core::native::save_paired(
        &design,
        asc_filename.to_string(),
        None,
        None,
        None,
    )
    .unwrap_or_else(|e| panic!("save_paired for template {:?}: {e}", spec.name));
    std::fs::write(dir.join(asc_filename), &paired.asc_text)
        .unwrap_or_else(|e| panic!("write {asc_filename}: {e}"));
    std::fs::write(
        dir.join("old_layout_sidecar.indicatrix.toml"),
        &paired.native_toml,
    )
    .unwrap_or_else(|e| panic!("write old_layout_sidecar.indicatrix.toml: {e}"));
}

/// Proves `import_path` works against a catalogue that was created in the OLD
/// (pre-`migrate_blob_columns_last`/pre-designer-split/pre-proportions) column layout
/// and migrated in place by `Database::new` -- unlike the `manual_import_*` probes
/// in `perf`, this needs no external scratch file and always runs.
///
/// 1. Hand-writes [`OLD_LAYOUT_SCHEMA_SQL`] plus three entries via
///    [`seed_old_layout_catalogue`], then opens it with [`open_temp_db`] (i.e.
///    `Database::new`), which runs every migration -- including
///    `migrate_blob_columns_last` -- against this genuinely pre-fix data.
/// 2. Imports a directory holding one plain valid `.asc`, one `.asc` +
///    older `.indicatrix.toml` sidecar pair, and one empty (unparsable) `.asc`, via
///    the same [`super::run_import_catching_panics`] -> `import_path` entry point the
///    manual probes in `perf` use.
/// 3. Asserts no panic, the exact "2 imported, 1 skipped" summary shape, and that
///    every one of the 3 pre-existing rows AND the 2 newly-imported rows reads back
///    through `get_diagram_full`/`get_diagram_full_meta`/`get_preview_material`/the
///    library's search query (via [`super::verify_every_imported_id_reads_back`]) -- plus
///    that entry 3's pre-existing preview row specifically still reads back its
///    `preview_material`.
/// 4. Re-imports the same directory and asserts [`count_pending_collisions`] now
///    reports both real files (not the empty one) as collisions, and that the second
///    pass completes cleanly too.
#[test]
fn import_into_a_catalogue_created_in_the_old_column_layout_survives_the_migration_and_reads_back()
{
    let db_path = temp_db_path_for_test("old_layout_migrated");
    let [entry_1, entry_2, entry_3] = seed_old_layout_catalogue(&db_path);
    // `Database::new` runs every migration, including `migrate_blob_columns_last`,
    // against this genuinely pre-fix file.
    let db = open_temp_db(&db_path);

    let corpus_dir = temp_dir_for_test("old_layout_migrated_corpus");
    std::fs::write(corpus_dir.join("valid.asc"), VALID_ASC).expect("write valid.asc");
    write_single_sidecar_pair(&corpus_dir);
    std::fs::write(corpus_dir.join("bad.asc"), []).expect("write bad.asc (empty -> unparsable)");

    let outcome = run_import_catching_panics(&db, &corpus_dir).unwrap_or_else(|panic_msg| {
        panic!(
            "import_path PANICKED against a catalogue migrated from the old column \
             layout: {panic_msg}"
        )
    });
    assert_eq!(
        outcome.imported_ids.len(),
        2,
        "valid.asc + the sidecar pair's .asc must both import: {}",
        outcome.summary
    );
    assert!(
        outcome.had_failures,
        "the empty bad.asc must be reported as a failure/skip: {}",
        outcome.summary
    );
    assert!(
        outcome.summary.contains("Imported 2 file(s)"),
        "expected exactly 2 imported: {}",
        outcome.summary
    );
    assert!(
        outcome.summary.contains("1 skipped"),
        "expected exactly 1 skipped: {}",
        outcome.summary
    );
    assert!(
        !outcome.had_collision,
        "the first import must not collide with anything: {}",
        outcome.summary
    );

    // Every pre-existing row survives the migration AND reads back the way the
    // library itself reads every design.
    verify_every_imported_id_reads_back(&db, &[entry_1, entry_2, entry_3]);
    // Every newly-imported row too.
    verify_every_imported_id_reads_back(&db, &outcome.imported_ids);

    // Entry 3's pre-existing diagram_previews row specifically must still be there,
    // not just error-free -- migrate_blob_columns_last rebuilds this table too. The
    // lock guard is a chained temporary (not bound to a variable held across the
    // `assert_eq!` below) so it drops the moment `get_preview_material` returns.
    let preview_material = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_preview_material(entry_3)
        .unwrap_or_else(|e| panic!("get_preview_material({entry_3}) failed: {e}"));
    assert_eq!(
        preview_material.as_deref(),
        Some("Quartz"),
        "entry 3's pre-existing preview row must survive the migration"
    );

    // Re-importing the same folder must now see the two real files as collisions --
    // not the still-unparsable empty one -- and must itself complete cleanly.
    let (collisions, total) = count_pending_collisions(&db, &corpus_dir, true);
    assert_eq!(
        (collisions, total),
        (2, 3),
        "the 2 previously-imported files must collide on re-import; the empty file is \
         still a candidate but was never actually saved"
    );

    let second = run_import_catching_panics(&db, &corpus_dir).unwrap_or_else(|panic_msg| {
        panic!("import_path PANICKED on the second (collision) pass: {panic_msg}")
    });
    assert_eq!(
        second.imported_ids.len(),
        2,
        "the second pass replaces both real files: {}",
        second.summary
    );
    assert!(
        second.had_collision,
        "the second pass must report a collision: {}",
        second.summary
    );
    verify_every_imported_id_reads_back(&db, &second.imported_ids);

    let _ = std::fs::remove_dir_all(&corpus_dir);
    let _ = std::fs::remove_file(&db_path);
}
