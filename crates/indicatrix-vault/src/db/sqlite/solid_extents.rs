//! Storage for `diagram_solid_extents` -- see `Database::migrate_diagram_solid_extents_table`'s
//! doc comment (in `super::migrations`) for why this is a side table keyed by `entry_id`
//! rather than columns on `diagram_details`, and `crate::model::solid_extents` for the
//! types this module encodes into it and the measuring rule that fills it.

use super::{Database, solid_hull::write_hull};
use crate::model::{
    solid_extents::{SOLID_EXTENTS_VERSION, SolidExtents, SolidExtentsSource, StoredSolidExtents},
    solid_hull::SolidHull,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, Row, params, params_from_iter};
use std::{collections::BTreeMap, sync::atomic::Ordering};

/// Most `?` placeholders bound into one `IN (...)` list. A whole catalogue (about 3,300
/// ids) in one statement is otherwise close to SQLite's default variable limit on older
/// builds; one extra placeholder per statement carries the version filter.
const ID_CHUNK: usize = 500;

/// Reads one `SELECT entry_id, width_caliper, length_caliper, width_axis, length_axis,
/// height, volume, source` row. `None` for a row whose `source` text is not a known
/// [`SolidExtentsSource`] (treated as missing, so the scan re-measures it); extents are
/// `None` unless all six measurements are present.
fn read_row(row: &Row<'_>) -> rusqlite::Result<Option<(i64, StoredSolidExtents)>> {
    let entry_id: i64 = row.get(0)?;
    let mut dims: [Option<f64>; 6] = [None; 6];
    for (offset, dim) in dims.iter_mut().enumerate() {
        *dim = row.get(offset + 1)?;
    }
    let source_text: String = row.get(7)?;
    let Some(source) = SolidExtentsSource::parse(&source_text) else {
        return Ok(None);
    };
    let extents = match dims {
        [
            Some(width_caliper),
            Some(length_caliper),
            Some(width_axis),
            Some(length_axis),
            Some(height),
            Some(volume),
        ] => Some(SolidExtents {
            width_caliper,
            length_caliper,
            width_axis,
            length_axis,
            height,
            volume,
        }),
        _ => None,
    };
    Ok(Some((entry_id, StoredSolidExtents { extents, source })))
}

/// Upserts `entry_id`'s extents row on `conn` (a plain connection or an open transaction),
/// stamping it with [`SOLID_EXTENTS_VERSION`]. Every column is written together.
fn write_extents(
    conn: &Connection,
    entry_id: i64,
    extents: Option<SolidExtents>,
    source: SolidExtentsSource,
    measured_at_unix: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO diagram_solid_extents (
             entry_id, width_caliper, length_caliper, width_axis, length_axis,
             height, volume, source, measured_at, extents_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(entry_id) DO UPDATE SET
             width_caliper = excluded.width_caliper,
             length_caliper = excluded.length_caliper,
             width_axis = excluded.width_axis,
             length_axis = excluded.length_axis,
             height = excluded.height,
             volume = excluded.volume,
             source = excluded.source,
             measured_at = excluded.measured_at,
             extents_version = excluded.extents_version",
        params![
            entry_id,
            extents.map(|e| e.width_caliper),
            extents.map(|e| e.length_caliper),
            extents.map(|e| e.width_axis),
            extents.map(|e| e.length_axis),
            extents.map(|e| e.height),
            extents.map(|e| e.volume),
            source.as_str(),
            measured_at_unix,
            i64::from(SOLID_EXTENTS_VERSION),
        ],
    )
    .with_context(|| format!("Failed to save solid extents for entry_id: {entry_id}"))?;
    Ok(())
}

impl Database {
    /// Reads the cached [`StoredSolidExtents`] of every id in `ids` that has a current
    /// row, in batches of at most 500 ids per statement (one `?` per id).
    ///
    /// A missing row, or one whose `extents_version` differs from
    /// [`SOLID_EXTENTS_VERSION`], is ABSENT from the map -- the caller treats both as
    /// "not measured yet". A present entry whose `extents` is `None` means "measured,
    /// unusable" (its planes did not close), which a scan must not retry. A
    /// [`BTreeMap`], so iteration order is deterministic.
    ///
    /// # Errors
    ///
    /// Returns an error if any batch's `SELECT` fails.
    pub fn solid_extents_for(&self, ids: &[i64]) -> Result<BTreeMap<i64, StoredSolidExtents>> {
        let version = i64::from(SOLID_EXTENTS_VERSION);
        let mut found = BTreeMap::new();
        for chunk in ids.chunks(ID_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT entry_id, width_caliper, length_caliper, width_axis, length_axis,
                        height, volume, source
                 FROM diagram_solid_extents
                 WHERE extents_version = ? AND entry_id IN ({placeholders})"
            );
            let mut stmt = self
                .conn
                .prepare(&sql)
                .context("Failed to prepare the solid-extents lookup")?;
            let bound = std::iter::once(version).chain(chunk.iter().copied());
            let rows = stmt
                .query_map(params_from_iter(bound), read_row)
                .context("Failed to query solid extents")?;
            for row in rows {
                if let Some((entry_id, stored)) =
                    row.context("Failed to read a solid-extents row")?
                {
                    found.insert(entry_id, stored);
                }
            }
        }
        Ok(found)
    }

    /// Persists `entry_id`'s measured extents in one `INSERT ... ON CONFLICT DO UPDATE`,
    /// stamping the row with [`SOLID_EXTENTS_VERSION`].
    ///
    /// `extents == None` stores a "measured, unusable" row (all six measurements NULL),
    /// so a scan does not retry it every run. `measured_at_unix` is Unix seconds, like
    /// `diagram_tilt_curves.generated_at`. Every column is written together, so a row is
    /// never half-updated.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `INSERT ... ON CONFLICT` fails (for example,
    /// `entry_id` names no `diagram_entries` row).
    pub fn save_solid_extents(
        &self,
        entry_id: i64,
        extents: Option<SolidExtents>,
        source: SolidExtentsSource,
        measured_at_unix: i64,
    ) -> Result<()> {
        write_extents(&self.conn, entry_id, extents, source, measured_at_unix)
    }

    /// Persists a design's measured extents and its convex hull together, in ONE
    /// transaction: either both rows are written or neither is, so a crash or a failed
    /// second write can never leave extents without the hull that belongs to them.
    ///
    /// `hull == None` (a design measured without a usable outline) deletes any earlier
    /// hull row of `entry_id`, so a stale hull cannot outlive the extents that replaced it.
    /// The extents row is written exactly as [`Self::save_solid_extents`] does.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction cannot start or commit, or if either write
    /// fails (for example, `entry_id` names no `diagram_entries` row); nothing is kept then.
    pub fn save_solid_extents_and_hull(
        &self,
        entry_id: i64,
        extents: Option<SolidExtents>,
        source: SolidExtentsSource,
        hull: Option<&SolidHull>,
        measured_at_unix: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction().with_context(|| {
            format!("Failed to start transaction to save solid extents for entry_id: {entry_id}")
        })?;
        write_extents(&tx, entry_id, extents, source, measured_at_unix)?;
        match hull {
            Some(hull) => write_hull(&tx, entry_id, hull, measured_at_unix)?,
            None => {
                tx.execute(
                    "DELETE FROM diagram_solid_hull WHERE entry_id = ?1",
                    params![entry_id],
                )
                .with_context(|| {
                    format!("Failed to delete the stale solid hull for entry_id: {entry_id}")
                })?;
            }
        }
        tx.commit().with_context(|| {
            format!("Failed to commit solid extents and hull for entry_id: {entry_id}")
        })?;
        Ok(())
    }

    /// How many times cached extents were invalidated through
    /// [`Self::delete_solid_extents`] since this connection opened.
    ///
    /// A measurement reads it in the same lock hold that reads the design's geometry, and
    /// stores its result with [`Self::save_solid_extents_and_hull_if_current`], which
    /// refuses the write when the count moved in between (an import or a native save
    /// replaced the geometry meanwhile, so the figures are of the old one).
    #[must_use]
    pub fn solid_extents_epoch(&self) -> u64 {
        self.extents_epoch.load(Ordering::SeqCst)
    }

    /// [`Self::save_solid_extents_and_hull`], but only while
    /// [`Self::solid_extents_epoch`] still equals `observed_epoch`.
    ///
    /// The epoch check and the write happen in one call, so with the database behind one
    /// lock nothing can invalidate in between. Returns `Ok(true)` when the measurement
    /// was stored and `Ok(false)`, nothing written, when an invalidation happened since
    /// `observed_epoch` was read: the caller discards the figures and the next scan
    /// measures the design again.
    ///
    /// # Errors
    ///
    /// Same as [`Self::save_solid_extents_and_hull`].
    pub fn save_solid_extents_and_hull_if_current(
        &self,
        entry_id: i64,
        observed_epoch: u64,
        extents: Option<SolidExtents>,
        source: SolidExtentsSource,
        hull: Option<&SolidHull>,
        measured_at_unix: i64,
    ) -> Result<bool> {
        if self.solid_extents_epoch() != observed_epoch {
            return Ok(false);
        }
        self.save_solid_extents_and_hull(entry_id, extents, source, hull, measured_at_unix)?;
        Ok(true)
    }

    /// Deletes the design's cached extents and hull (`entry_id`'s `diagram_solid_extents`
    /// and `diagram_solid_hull` rows), if either exists.
    ///
    /// Like `diagram_previews`/`diagram_tilt_curves`, these side tables survive a
    /// `diagram_details` re-sync, so a change to a design's geometry (a re-import over
    /// an existing row, a native save write-back, a mirror sync of a changed design)
    /// must call this to force a re-measure instead of keeping extents or a hull
    /// measured from the OLD planes. Entry deletion needs no call: the rows cascade away.
    ///
    /// A missing row is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying transaction or `DELETE` statements fail.
    pub fn delete_solid_extents(&self, entry_id: i64) -> Result<()> {
        // Bumped before the delete so a measurement that read the geometry earlier can
        // never be stored over this invalidation, even if the delete itself fails.
        self.extents_epoch.fetch_add(1, Ordering::SeqCst);
        let tx = self.conn.unchecked_transaction().with_context(|| {
            format!("Failed to start transaction to delete solid extents for entry_id: {entry_id}")
        })?;
        tx.execute(
            "DELETE FROM diagram_solid_extents WHERE entry_id = ?1",
            params![entry_id],
        )
        .with_context(|| format!("Failed to delete solid extents for entry_id: {entry_id}"))?;
        tx.execute(
            "DELETE FROM diagram_solid_hull WHERE entry_id = ?1",
            params![entry_id],
        )
        .with_context(|| format!("Failed to delete solid hull for entry_id: {entry_id}"))?;
        tx.commit().with_context(|| {
            format!("Failed to commit deleting solid extents and hull for entry_id: {entry_id}")
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{detail::FacetingDiagramDetail, entry::FacetingDiagramEntry};

    fn temp_db() -> (Database, std::path::PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-solid-extents-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        (db, path)
    }

    fn add_entry(db: &Database, label: &str) -> i64 {
        db.save_diagram_entry(
            &FacetingDiagramEntry {
                title: format!("Extents Test {label}"),
                url: format!("local://extents-test-{label}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap()
    }

    fn sample(scale: f64) -> SolidExtents {
        SolidExtents {
            width_caliper: scale,
            length_caliper: 1.5 * scale,
            width_axis: 1.25 * scale,
            length_axis: 1.75 * scale,
            height: 0.75 * scale,
            volume: 0.5 * scale,
        }
    }

    fn cleanup(db: Database, path: &std::path::Path) {
        drop(db);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn save_and_read_round_trip_exact_values_and_sources() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        let b = add_entry(&db, "b");
        let c = add_entry(&db, "c");
        db.save_solid_extents(a, Some(sample(1.0)), SolidExtentsSource::DesignFile, 10)
            .unwrap();
        db.save_solid_extents(b, Some(sample(2.0)), SolidExtentsSource::AngleTable, 11)
            .unwrap();
        db.save_solid_extents(c, None, SolidExtentsSource::Unbounded, 12)
            .unwrap();

        let map = db.solid_extents_for(&[c, a, b]).unwrap();
        assert_eq!(
            map.keys().copied().collect::<Vec<_>>(),
            vec![a, b, c],
            "a BTreeMap iterates in ascending entry id order"
        );
        assert_eq!(
            map[&a],
            StoredSolidExtents {
                extents: Some(sample(1.0)),
                source: SolidExtentsSource::DesignFile
            }
        );
        assert_eq!(map[&b].source, SolidExtentsSource::AngleTable);
        assert_eq!(map[&b].extents, Some(sample(2.0)));
        assert_eq!(
            map[&c],
            StoredSolidExtents {
                extents: None,
                source: SolidExtentsSource::Unbounded
            },
            "an unusable design is present with no extents, not absent"
        );
        cleanup(db, &path);
    }

    #[test]
    fn missing_rows_and_an_empty_id_list_yield_an_empty_map() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        assert!(db.solid_extents_for(&[]).unwrap().is_empty());
        assert!(db.solid_extents_for(&[a, a + 1000]).unwrap().is_empty());
        cleanup(db, &path);
    }

    #[test]
    fn save_upserts_a_single_row() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        db.save_solid_extents(a, None, SolidExtentsSource::Unbounded, 1)
            .unwrap();
        db.save_solid_extents(a, Some(sample(3.0)), SolidExtentsSource::DesignFile, 2)
            .unwrap();

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM diagram_solid_extents", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        let measured_at: i64 = db
            .conn
            .query_row(
                "SELECT measured_at FROM diagram_solid_extents WHERE entry_id = ?1",
                params![a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(measured_at, 2);
        let stored = db.solid_extents_for(&[a]).unwrap()[&a];
        assert_eq!(stored.extents, Some(sample(3.0)));
        assert_eq!(stored.source, SolidExtentsSource::DesignFile);
        cleanup(db, &path);
    }

    #[test]
    fn a_row_with_a_stale_extents_version_counts_as_missing() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        let b = add_entry(&db, "b");
        db.save_solid_extents(a, Some(sample(1.0)), SolidExtentsSource::DesignFile, 1)
            .unwrap();
        db.save_solid_extents(b, Some(sample(2.0)), SolidExtentsSource::DesignFile, 1)
            .unwrap();
        db.conn
            .execute(
                "UPDATE diagram_solid_extents SET extents_version = ?1 WHERE entry_id = ?2",
                params![i64::from(SOLID_EXTENTS_VERSION) + 1, a],
            )
            .unwrap();

        let map = db.solid_extents_for(&[a, b]).unwrap();
        assert!(!map.contains_key(&a), "the stale-version row is absent");
        assert!(map.contains_key(&b));

        // Re-saving stamps the current version, so the row is current again.
        db.save_solid_extents(a, Some(sample(1.0)), SolidExtentsSource::DesignFile, 3)
            .unwrap();
        assert!(db.solid_extents_for(&[a]).unwrap().contains_key(&a));
        cleanup(db, &path);
    }

    #[test]
    fn a_version_two_row_with_a_convex_volume_is_re_measured_under_version_three() {
        // Version 2 cached the convex volume; version 3 caches the concave-carved one,
        // so such a row must read as missing and be measured again.
        assert_eq!(SOLID_EXTENTS_VERSION, 3);
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        db.save_solid_extents(a, Some(sample(1.0)), SolidExtentsSource::DesignFile, 1)
            .unwrap();
        db.conn
            .execute(
                "UPDATE diagram_solid_extents SET extents_version = 2 WHERE entry_id = ?1",
                params![a],
            )
            .unwrap();
        assert!(!db.solid_extents_for(&[a]).unwrap().contains_key(&a));
        cleanup(db, &path);
    }

    #[test]
    fn reads_span_more_than_one_id_chunk() {
        let (db, path) = temp_db();
        let real: Vec<i64> = ["a", "b", "c"].iter().map(|l| add_entry(&db, l)).collect();
        for (i, &id) in real.iter().enumerate() {
            db.save_solid_extents(
                id,
                Some(sample(1.0 + i as f64)),
                SolidExtentsSource::DesignFile,
                1,
            )
            .unwrap();
        }
        // 1,300 ids => three chunks of 500 + 500 + 300; one real id lands in each.
        let mut ids: Vec<i64> = (1_000_000..1_001_300).collect();
        ids[0] = real[0];
        ids[700] = real[1];
        ids[1299] = real[2];

        let map = db.solid_extents_for(&ids).unwrap();
        assert_eq!(map.keys().copied().collect::<Vec<_>>(), real);
        assert_eq!(map[&real[2]].extents, Some(sample(3.0)));
        cleanup(db, &path);
    }

    #[test]
    fn delete_removes_extents_and_hull_and_a_missing_row_is_not_an_error() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        db.delete_solid_extents(a).unwrap();
        db.save_solid_extents(a, Some(sample(1.0)), SolidExtentsSource::DesignFile, 1)
            .unwrap();
        db.save_solid_hull(
            a,
            &crate::model::solid_hull::SolidHull {
                vertices: vec![[1.0, 2.0, 3.0]],
            },
            1,
        )
        .unwrap();
        assert!(!db.solid_extents_for(&[a]).unwrap().is_empty());
        assert!(!db.solid_hulls_for(&[a]).unwrap().is_empty());

        db.delete_solid_extents(a).unwrap();
        assert!(db.solid_extents_for(&[a]).unwrap().is_empty());
        assert!(db.solid_hulls_for(&[a]).unwrap().is_empty());
        cleanup(db, &path);
    }

    /// The measurement-then-invalidation race: the epoch read with the geometry is stale by
    /// the time the figures are saved, so the save writes nothing; one read after the
    /// invalidation stores normally.
    #[test]
    fn a_save_after_an_invalidation_since_the_epoch_was_read_is_skipped() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        let hull = sample_hull();
        let observed = db.solid_extents_epoch();

        // An import or native save replaces the geometry while the measurement runs.
        db.delete_solid_extents(a).unwrap();
        assert_eq!(db.solid_extents_epoch(), observed + 1);

        let stored = db
            .save_solid_extents_and_hull_if_current(
                a,
                observed,
                Some(sample(1.0)),
                SolidExtentsSource::DesignFile,
                Some(&hull),
                1,
            )
            .unwrap();
        assert!(!stored, "the figures belong to the replaced geometry");
        assert!(db.solid_extents_for(&[a]).unwrap().is_empty());
        assert!(db.solid_hulls_for(&[a]).unwrap().is_empty());

        // Read again after the invalidation: the same save is accepted.
        let observed = db.solid_extents_epoch();
        assert!(
            db.save_solid_extents_and_hull_if_current(
                a,
                observed,
                Some(sample(2.0)),
                SolidExtentsSource::DesignFile,
                Some(&hull),
                2,
            )
            .unwrap()
        );
        assert_eq!(
            db.solid_extents_for(&[a]).unwrap()[&a].extents,
            Some(sample(2.0))
        );
        assert_eq!(db.solid_hulls_for(&[a]).unwrap()[&a], hull);
        assert_eq!(
            db.solid_extents_epoch(),
            observed,
            "saving does not invalidate"
        );
        cleanup(db, &path);
    }

    fn sample_hull() -> SolidHull {
        SolidHull {
            vertices: vec![
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [-1.0, -1.0, -1.0],
            ],
        }
    }

    #[test]
    fn extents_and_hull_are_saved_together() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        let hull = sample_hull();
        db.save_solid_extents_and_hull(
            a,
            Some(sample(1.0)),
            SolidExtentsSource::DesignFile,
            Some(&hull),
            7,
        )
        .unwrap();

        let stored = db.solid_extents_for(&[a]).unwrap()[&a];
        assert_eq!(stored.extents, Some(sample(1.0)));
        assert_eq!(stored.source, SolidExtentsSource::DesignFile);
        assert_eq!(db.solid_hulls_for(&[a]).unwrap()[&a], hull);
        cleanup(db, &path);
    }

    #[test]
    fn saving_without_a_hull_removes_the_old_hull_row_and_keeps_the_new_extents() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        let other = add_entry(&db, "other");
        for id in [a, other] {
            db.save_solid_extents_and_hull(
                id,
                Some(sample(1.0)),
                SolidExtentsSource::DesignFile,
                Some(&sample_hull()),
                1,
            )
            .unwrap();
        }

        db.save_solid_extents_and_hull(a, None, SolidExtentsSource::Unbounded, None, 2)
            .unwrap();

        assert!(
            !db.solid_hulls_for(&[a]).unwrap().contains_key(&a),
            "the stale hull is gone"
        );
        assert!(
            db.solid_hulls_for(&[other]).unwrap().contains_key(&other),
            "another design's hull stays"
        );
        assert_eq!(
            db.solid_extents_for(&[a]).unwrap()[&a],
            StoredSolidExtents {
                extents: None,
                source: SolidExtentsSource::Unbounded
            }
        );
        // Saving without a hull when none exists is not an error.
        let fresh = add_entry(&db, "fresh");
        db.save_solid_extents_and_hull(
            fresh,
            Some(sample(2.0)),
            SolidExtentsSource::AngleTable,
            None,
            3,
        )
        .unwrap();
        assert_eq!(
            db.solid_extents_for(&[fresh]).unwrap()[&fresh].extents,
            Some(sample(2.0))
        );
        cleanup(db, &path);
    }

    #[test]
    fn a_failing_hull_write_rolls_the_extents_write_back() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        db.save_solid_extents(a, Some(sample(1.0)), SolidExtentsSource::DesignFile, 1)
            .unwrap();
        // Break the second statement of the transaction.
        db.conn
            .execute("DROP TABLE diagram_solid_hull", [])
            .unwrap();

        assert!(
            db.save_solid_extents_and_hull(
                a,
                Some(sample(9.0)),
                SolidExtentsSource::DesignFile,
                Some(&sample_hull()),
                2,
            )
            .is_err()
        );
        let stored = db.solid_extents_for(&[a]).unwrap()[&a];
        assert_eq!(
            stored.extents,
            Some(sample(1.0)),
            "the extents write did not survive the failed hull write"
        );
        cleanup(db, &path);
    }

    #[test]
    fn saving_extents_and_hull_for_a_nonexistent_entry_is_rejected_and_leaves_nothing() {
        let (db, path) = temp_db();
        assert!(
            db.save_solid_extents_and_hull(
                9_999,
                Some(sample(1.0)),
                SolidExtentsSource::DesignFile,
                Some(&sample_hull()),
                1,
            )
            .is_err()
        );
        assert!(db.solid_extents_for(&[9_999]).unwrap().is_empty());
        assert!(db.solid_hulls_for(&[9_999]).unwrap().is_empty());
        cleanup(db, &path);
    }

    #[test]
    fn saving_for_a_nonexistent_entry_is_rejected_by_the_foreign_key() {
        let (db, path) = temp_db();
        assert!(
            db.save_solid_extents(9_999, None, SolidExtentsSource::Unbounded, 1)
                .is_err()
        );
        cleanup(db, &path);
    }

    /// Same cascade-vs-re-sync property as `tilt_curves.rs`' own test: keyed by
    /// `entry_id`, not by `diagram_details`' row id.
    #[test]
    fn extents_row_survives_a_diagram_details_re_sync_but_not_entry_deletion() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "a");
        db.save_diagram_detail(&FacetingDiagramDetail::default(), a)
            .unwrap();
        db.save_solid_extents(a, Some(sample(1.0)), SolidExtentsSource::DesignFile, 1)
            .unwrap();

        // A genuine re-sync deletes and reinserts diagram_details for this entry_id.
        db.save_diagram_detail(&FacetingDiagramDetail::default(), a)
            .unwrap();
        assert!(db.solid_extents_for(&[a]).unwrap().contains_key(&a));

        db.delete_diagram_entry(a).unwrap();
        assert!(db.solid_extents_for(&[a]).unwrap().is_empty());
        cleanup(db, &path);
    }
}
