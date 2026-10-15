//! The material provenance migration (`diagram_previews.preview_girdle_mm` and
//! `preview_material_source`, `Database::migrate_preview_material_provenance`): the
//! columns appear ahead of the PNG blobs, every cached row survives, the cached results of
//! designs that carry a native design file are marked outdated in both cache tables, and
//! none of that repeats on a later open.

use super::{super::*, fixtures::temp_db_path};
use crate::model::{
    detail::FacetingDiagramDetail,
    entry::FacetingDiagramEntry,
    file::AttachedFile,
    preview::{PreviewMaterialSource, PreviewMaterialState},
};

/// Saves one design whose attachments are named `names` and returns its entry id.
fn save_design_with_files(db: &Database, title: &str, names: &[&str]) -> i64 {
    let entry_id = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: title.to_string(),
                url: format!("local://{title}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    let detail = FacetingDiagramDetail {
        attached_files: names
            .iter()
            .map(|name| AttachedFile {
                name: (*name).to_string(),
                url: String::new(),
                content: vec![1],
            })
            .collect(),
        ..FacetingDiagramDetail::default()
    };
    db.save_diagram_detail(&detail, entry_id).unwrap();
    entry_id
}

/// A database file in the shape before the provenance columns existed: three designs
/// (entry ids 1 to 3) -- one with a self-contained `.indicatrix` file (the legacy
/// extension), one with a `.isdd` file, one with an `.asc` and a PDF only -- each holding a finished
/// preview render and a tilt-curve row stored under the fingerprint `fp`. Closed, so the
/// next `Database::new` runs the migration.
fn seed_pre_provenance_db(label: &str) -> std::path::PathBuf {
    let path = temp_db_path(label);
    let db = Database::new(Some(path.to_str().unwrap())).unwrap();
    save_design_with_files(&db, "native", &["a.asc", "a.indicatrix"]);
    save_design_with_files(&db, "isdd", &["b.asc", "B.ISDD"]);
    save_design_with_files(&db, "plain", &["c.asc", "c.pdf"]);
    db.conn
        .execute_batch(
            "DROP TABLE diagram_previews;
             CREATE TABLE diagram_previews (
                 entry_id INTEGER PRIMARY KEY,
                 preview_material TEXT,
                 preview_generated_at INTEGER,
                 params_fingerprint TEXT,
                 preview_front BLOB,
                 preview_top BLOB,
                 FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
             );
             INSERT INTO diagram_previews
                 (entry_id, preview_material, preview_generated_at, params_fingerprint,
                  preview_front, preview_top)
             VALUES (1, 'Quartz', 10, 'fp', X'01', X'02'),
                    (2, 'Quartz', 10, 'fp', X'01', X'02'),
                    (3, 'Quartz', 10, 'fp', X'01', X'02');
             INSERT INTO diagram_tilt_curves (entry_id, generated_at, params_fingerprint)
             VALUES (1, 10, 'fp'), (2, 10, 'fp'), (3, 10, 'fp');",
        )
        .unwrap();
    drop(db);
    path
}

/// `table`'s column names in physical order.
fn column_names(db: &Database, table: &str) -> Vec<String> {
    let mut stmt = db
        .conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
}

/// `table`'s stored fingerprints in entry-id order.
fn fingerprints(db: &Database, table: &str) -> Vec<Option<String>> {
    let mut stmt = db
        .conn
        .prepare(&format!(
            "SELECT params_fingerprint FROM {table} ORDER BY entry_id"
        ))
        .unwrap();
    stmt.query_map([], |row| row.get::<_, Option<String>>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
}

/// Asserts both provenance columns exist in `db` and sit ahead of the PNG blobs.
fn assert_columns_before_the_blobs(db: &Database) {
    let names = column_names(db, "diagram_previews");
    let front = names
        .iter()
        .position(|name| name == "preview_front")
        .expect("the front image column exists");
    for column in ["preview_girdle_mm", "preview_material_source"] {
        let position = names
            .iter()
            .position(|name| name == column)
            .unwrap_or_else(|| panic!("{column} must exist: {names:?}"));
        assert!(
            position < front,
            "{column} must precede the blobs: {names:?}"
        );
    }
}

#[test]
fn a_database_without_the_provenance_columns_gains_them_and_keeps_every_row() {
    let path = seed_pre_provenance_db("provenance_gains_columns");
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");

    assert_columns_before_the_blobs(&db);
    for entry_id in 1..=3 {
        let images = db.get_preview_images(entry_id).unwrap();
        assert_eq!(images.front, Some(vec![1]), "entry {entry_id}");
        assert_eq!(images.top, Some(vec![2]), "entry {entry_id}");
        assert_eq!(images.generated_at, Some(10), "entry {entry_id}");
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap(),
            PreviewMaterialState {
                name: Some("Quartz".to_string()),
                girdle_mm: None,
                source: PreviewMaterialSource::RefractiveIndex,
            },
            "a row from before the columns reads as an unsized refractive-index pick"
        );
    }

    drop(db);
    std::fs::remove_file(&path).ok();
}

/// Previews and curves made before this change followed the refractive index alone, so the
/// designs that carry a native design file (the only ones that can name a material or a
/// size) lose their stored fingerprint in both cache tables and are listed as outdated;
/// the others keep theirs.
#[test]
fn designs_with_a_design_file_are_marked_outdated_once_and_the_others_are_not() {
    let path = seed_pre_provenance_db("provenance_marks_design_files");
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");

    for table in ["diagram_previews", "diagram_tilt_curves"] {
        assert_eq!(
            fingerprints(&db, table),
            vec![None, None, Some("fp".to_string())],
            "{table}: the .indicatrix and the .isdd design lose it, the plain .asc keeps it"
        );
    }
    assert_eq!(
        db.entry_ids_missing_previews(|_| "fp".to_string()).unwrap(),
        vec![1, 2]
    );
    assert_eq!(
        db.entry_ids_missing_tilt_curves(|_| "fp".to_string())
            .unwrap(),
        vec![1, 2]
    );

    // A render made after the migration is current, and a later open leaves it alone.
    db.conn
        .execute_batch(
            "UPDATE diagram_previews SET params_fingerprint = 'fresh' WHERE entry_id = 1;
             UPDATE diagram_tilt_curves SET params_fingerprint = 'fresh' WHERE entry_id = 1;",
        )
        .unwrap();
    drop(db);
    let db = Database::new(Some(path.to_str().unwrap())).expect("the second open is a no-op");
    for table in ["diagram_previews", "diagram_tilt_curves"] {
        assert_eq!(
            fingerprints(&db, table),
            vec![Some("fresh".to_string()), None, Some("fp".to_string())],
            "{table}: the migration does not run again"
        );
    }
    assert_columns_before_the_blobs(&db);

    drop(db);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_fresh_database_has_the_provenance_columns_before_the_blobs() {
    let db = Database::new(Some(":memory:")).expect("an in-memory database opens");
    assert_columns_before_the_blobs(&db);
}
