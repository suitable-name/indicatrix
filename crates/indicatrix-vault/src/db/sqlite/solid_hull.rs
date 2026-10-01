//! Storage for `diagram_solid_hull` -- see
//! `Database::migrate_diagram_solid_hull_table`'s doc comment (in `super::migrations`)
//! for why this is a side table keyed by `entry_id` rather than columns on
//! `diagram_details`, and `crate::model::solid_hull` for the types this module encodes
//! into it.

use super::Database;
use crate::model::solid_hull::{SOLID_HULL_VERSION, SolidHull};
use anyhow::{Context, Result};
use rusqlite::{Connection, Row, params, params_from_iter};
use std::collections::BTreeMap;

/// Most `?` placeholders bound into one `IN (...)` list. A whole catalogue (about 3,300
/// ids) in one statement is otherwise close to SQLite's default variable limit on older
/// builds; one extra placeholder per statement carries the version filter.
const ID_CHUNK: usize = 500;

/// Reads one `SELECT entry_id, vertices, vertex_count` row. `None` if the vertices BLOB does
/// not decode (bad length or a non-finite coordinate) or its vertex count disagrees with the
/// stored `vertex_count` (a truncation that happens to be a multiple of 12 bytes); such a row
/// is treated as missing, so the scan re-measures it.
fn read_row(row: &Row<'_>) -> rusqlite::Result<Option<(i64, SolidHull)>> {
    let entry_id: i64 = row.get(0)?;
    let blob: Vec<u8> = row.get(1)?;
    let stored_count: i64 = row.get(2)?;
    let Ok(hull) = SolidHull::from_bytes(&blob) else {
        return Ok(None);
    };
    if i64::try_from(hull.vertices.len()) != Ok(stored_count) {
        return Ok(None);
    }
    Ok(Some((entry_id, hull)))
}

/// Upserts `entry_id`'s hull row on `conn` (a plain connection or an open transaction),
/// stamping it with [`SOLID_HULL_VERSION`].
pub(super) fn write_hull(
    conn: &Connection,
    entry_id: i64,
    hull: &SolidHull,
    measured_at_unix: i64,
) -> Result<()> {
    let vertex_count =
        i64::try_from(hull.vertices.len()).context("vertex count exceeds i64 range")?;
    let blob = hull.to_bytes();
    conn.execute(
        "INSERT INTO diagram_solid_hull (
             entry_id, hull_version, vertex_count, measured_at, vertices
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(entry_id) DO UPDATE SET
             hull_version = excluded.hull_version,
             vertex_count = excluded.vertex_count,
             measured_at = excluded.measured_at,
             vertices = excluded.vertices",
        params![
            entry_id,
            i64::from(SOLID_HULL_VERSION),
            vertex_count,
            measured_at_unix,
            blob,
        ],
    )
    .with_context(|| format!("Failed to save solid hull for entry_id: {entry_id}"))?;
    Ok(())
}

impl Database {
    /// Reads the cached [`SolidHull`] of every id in `ids` that has a current row, in
    /// batches of at most 500 ids per statement (one `?` per id).
    ///
    /// A missing row, one whose `hull_version` differs from [`SOLID_HULL_VERSION`], or one
    /// with an invalid/corrupt blob, is ABSENT from the map -- the caller treats both as
    /// "not measured yet". A [`BTreeMap`], so iteration order is deterministic (ascending
    /// entry id).
    ///
    /// # Errors
    ///
    /// Returns an error if preparing the statement or querying fails.
    pub fn solid_hulls_for(&self, ids: &[i64]) -> Result<BTreeMap<i64, SolidHull>> {
        let version = i64::from(SOLID_HULL_VERSION);
        let mut found = BTreeMap::new();
        for chunk in ids.chunks(ID_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT entry_id, vertices, vertex_count
                 FROM diagram_solid_hull
                 WHERE hull_version = ? AND entry_id IN ({placeholders})"
            );
            let mut stmt = self
                .conn
                .prepare(&sql)
                .context("Failed to prepare the solid-hulls lookup")?;
            let bound = std::iter::once(version).chain(chunk.iter().copied());
            let rows = stmt
                .query_map(params_from_iter(bound), read_row)
                .context("Failed to query solid hulls")?;
            for row in rows {
                if let Some((entry_id, hull)) = row.context("Failed to read a solid-hull row")? {
                    found.insert(entry_id, hull);
                }
            }
        }
        Ok(found)
    }

    /// Persists `entry_id`'s measured convex hull in one `INSERT ... ON CONFLICT DO UPDATE`,
    /// stamping the row with [`SOLID_HULL_VERSION`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `INSERT ... ON CONFLICT` fails (for example,
    /// `entry_id` names no `diagram_entries` row).
    pub fn save_solid_hull(
        &self,
        entry_id: i64,
        hull: &SolidHull,
        measured_at_unix: i64,
    ) -> Result<()> {
        write_hull(&self.conn, entry_id, hull, measured_at_unix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::entry::FacetingDiagramEntry;

    fn temp_db() -> (Database, std::path::PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-solid-hull-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        (db, path)
    }

    fn add_entry(db: &Database, label: &str) -> i64 {
        db.save_diagram_entry(
            &FacetingDiagramEntry {
                title: format!("Hull Test {label}"),
                url: format!("local://hull-test-{label}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap()
    }

    fn sample_hull(scale: f32) -> SolidHull {
        SolidHull {
            vertices: vec![
                [scale, 0.0, 0.0],
                [0.0, scale, 0.0],
                [0.0, 0.0, scale],
                [-scale, -scale, -scale],
            ],
        }
    }

    fn cleanup(db: Database, path: &std::path::Path) {
        drop(db);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn save_and_read_round_trip() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        let b = add_entry(&db, "b");
        let hull_a = sample_hull(1.0);
        let hull_b = sample_hull(2.5);

        db.save_solid_hull(a, &hull_a, 100).unwrap();
        db.save_solid_hull(b, &hull_b, 200).unwrap();

        let map = db.solid_hulls_for(&[b, a]).unwrap();
        assert_eq!(map.keys().copied().collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(map[&a], hull_a);
        assert_eq!(map[&b], hull_b);

        cleanup(db, &path);
    }

    #[test]
    fn version_mismatch_reads_as_missing() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        let hull_a = sample_hull(1.0);
        db.save_solid_hull(a, &hull_a, 100).unwrap();

        // Stale version: update hull_version to something else
        db.conn
            .execute(
                "UPDATE diagram_solid_hull SET hull_version = ?1 WHERE entry_id = ?2",
                params![i64::from(SOLID_HULL_VERSION) + 1, a],
            )
            .unwrap();

        let map = db.solid_hulls_for(&[a]).unwrap();
        assert!(!map.contains_key(&a), "stale version must be omitted");

        // Re-saving updates the version back
        db.save_solid_hull(a, &hull_a, 101).unwrap();
        let map = db.solid_hulls_for(&[a]).unwrap();
        assert_eq!(map[&a], hull_a);

        cleanup(db, &path);
    }

    /// Overwrites a stored row's blob and vertex count behind the model's back.
    fn corrupt_row(db: &Database, entry_id: i64, blob: &[u8], vertex_count: i64) {
        db.conn
            .execute(
                "UPDATE diagram_solid_hull SET vertices = ?1, vertex_count = ?2
                 WHERE entry_id = ?3",
                params![blob, vertex_count, entry_id],
            )
            .unwrap();
    }

    #[test]
    fn corrupt_rows_read_as_missing() {
        let (db, path) = temp_db();
        let good = add_entry(&db, "good");
        let non_finite = add_entry(&db, "nan");
        let truncated = add_entry(&db, "truncated");
        let bad_length = add_entry(&db, "length");
        for id in [good, non_finite, truncated, bad_length] {
            db.save_solid_hull(id, &sample_hull(1.0), 100).unwrap();
        }

        let mut nan_hull = sample_hull(1.0);
        nan_hull.vertices[2][1] = f32::NAN;
        corrupt_row(&db, non_finite, &nan_hull.to_bytes(), 4);
        // Three whole vertices in the blob, but the row still claims four.
        let short = sample_hull(1.0).to_bytes()[..36].to_vec();
        corrupt_row(&db, truncated, &short, 4);
        corrupt_row(&db, bad_length, &[0u8; 13], 1);

        let map = db
            .solid_hulls_for(&[good, non_finite, truncated, bad_length])
            .unwrap();
        assert_eq!(map.keys().copied().collect::<Vec<_>>(), vec![good]);

        cleanup(db, &path);
    }

    #[test]
    fn cascade_on_entry_delete() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        db.save_solid_hull(a, &sample_hull(1.0), 100).unwrap();
        assert!(db.solid_hulls_for(&[a]).unwrap().contains_key(&a));

        db.delete_diagram_entry(a).unwrap();
        assert!(db.solid_hulls_for(&[a]).unwrap().is_empty());

        cleanup(db, &path);
    }

    #[test]
    fn invalid_foreign_key_fails() {
        let (db, path) = temp_db();
        assert!(db.save_solid_hull(99_999, &sample_hull(1.0), 100).is_err());
        cleanup(db, &path);
    }
}
