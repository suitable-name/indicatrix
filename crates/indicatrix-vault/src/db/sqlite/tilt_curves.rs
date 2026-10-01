//! Storage for `diagram_tilt_curves` -- see `Database::migrate_diagram_tilt_curves_table`'s
//! doc comment (in `super::migrations`) for why this is a side table keyed by
//! `entry_id` rather than columns on `diagram_details`, and
//! `crate::model::tilt_curves`/`crate::model::performance` for the types this module
//! encodes into it.

use super::Database;
use crate::model::{
    performance::{Extreme, all_global_extreme_columns, global_extreme_column_name},
    tilt_curves::TiltPerformanceCurves,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, ToSql, params};

impl Database {
    /// Persists `entry_id`'s full tilt-performance record in one statement: the packed
    /// curve BLOB ([`TiltPerformanceCurves::to_bytes`]), the unix-seconds generation
    /// timestamp, `params_fingerprint` (what the sweep was computed with; see
    /// [`Self::entry_ids_missing_tilt_curves`]), and all 6 derived global-min/max
    /// columns ([`TiltPerformanceCurves::global_extremes`]) `crate::model::performance`'s
    /// search filters read back as a SOUND but incomplete SQL-level narrowing step --
    /// see that module's doc comment for why only these 6 survive as precomputed
    /// columns and why the exact per-filter test still has to run in Rust, over the
    /// decoded curve, once a candidate row is loaded.
    ///
    /// Every one of those 9 non-`entry_id` columns is written together, in the same
    /// `INSERT ... ON CONFLICT DO UPDATE`, specifically so `generated_at IS NOT NULL`
    /// can be trusted (by `crate::db::sqlite::search::build_search_predicate`'s
    /// exclusion-count query, and by [`Self::has_tilt_curves`](Database::has_tilt_curves))
    /// as meaning every derived column is populated too -- there is no code path in
    /// this crate that writes the curve BLOB without also writing its global extremes,
    /// or vice versa. The table's `curve_image` column is never written: nothing renders
    /// a stored graph, so it stays `NULL` (the column is kept only so existing databases
    /// need no rebuild).
    ///
    /// The 6 derived columns are located and bound via
    /// [`crate::model::performance::all_global_extreme_columns`]/
    /// [`crate::model::performance::global_extreme_column_name`] rather than hand-listed
    /// here -- the same single-source-of-truth reasoning as
    /// `Database::migrate_diagram_tilt_curves_table`'s own doc comment.
    ///
    /// # Compare-and-swap on the design's revision
    ///
    /// A sweep takes seconds with the database lock released, so the design can be
    /// re-imported, edited or re-synced in between; an unconditional write would then
    /// store curves of geometry that no longer exists, after the invalidation that
    /// should have removed them already ran. `expected_updated_at` is the
    /// `diagram_entries.updated_at` the caller read together with the record it swept
    /// ([`Self::entry_updated_at`]); the write happens only while the row still carries
    /// exactly that value (`None` matching a `NULL` stamp). The check and the upsert are
    /// one SQL statement, so nothing can slip between them.
    ///
    /// Returns `Ok(true)` when the curves were stored, and `Ok(false)` -- nothing
    /// written -- when the design changed since `expected_updated_at` was read or no
    /// longer exists. The caller should discard its result: the next scan sees the
    /// design as missing its curves and sweeps the current geometry.
    ///
    /// # Errors
    ///
    /// Returns an error if any sample of `curves` is not finite (a stored record must
    /// decode again, see [`TiltPerformanceCurves::from_bytes`]), or the underlying
    /// `INSERT ... ON CONFLICT` fails.
    pub fn save_tilt_curves(
        &self,
        entry_id: i64,
        curves: &TiltPerformanceCurves,
        generated_at_unix: i64,
        params_fingerprint: &str,
        expected_updated_at: Option<i64>,
    ) -> Result<bool> {
        ensure!(
            curves.is_finite(),
            "refusing to store tilt curves with a non-finite sample for entry_id: {entry_id}"
        );
        let extremes = curves.global_extremes();

        let mut columns: Vec<String> = vec![
            "entry_id".to_string(),
            "curves".to_string(),
            "generated_at".to_string(),
            "params_fingerprint".to_string(),
        ];
        let mut params: Vec<Box<dyn ToSql>> = vec![
            Box::new(entry_id),
            Box::new(curves.to_bytes()),
            Box::new(generated_at_unix),
            Box::new(params_fingerprint.to_string()),
        ];

        for (metric, extreme) in all_global_extreme_columns() {
            let global = extremes.get(metric);
            let value = match extreme {
                Extreme::Min => global.min,
                Extreme::Max => global.max,
            };
            columns.push(global_extreme_column_name(metric, extreme));
            params.push(Box::new(f64::from(value)));
        }
        // `?1` is the entry id (selected from `diagram_entries` below, which is also
        // where the revision check reads); the row values follow as `?2..`, and the
        // expected revision is the last parameter.
        params.push(Box::new(expected_updated_at));

        let values: Vec<String> = (2..=columns.len()).map(|i| format!("?{i}")).collect();
        // Every column except `entry_id` (index 0) is overwritten on conflict.
        let update_assignments: Vec<String> = columns[1..]
            .iter()
            .map(|c| format!("{c} = excluded.{c}"))
            .collect();
        let sql = format!(
            "INSERT INTO diagram_tilt_curves ({}) SELECT id, {} FROM diagram_entries
             WHERE id = ?1 AND updated_at IS ?{}
             ON CONFLICT(entry_id) DO UPDATE SET {}",
            columns.join(", "),
            values.join(", "),
            params.len(),
            update_assignments.join(", "),
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("Failed to prepare tilt-curve upsert")?;
        let bound: Vec<&dyn ToSql> = params.iter().map(std::convert::AsRef::as_ref).collect();
        let written = stmt
            .execute(bound.as_slice())
            .with_context(|| format!("Failed to save tilt curves for entry_id: {entry_id}"))?;
        Ok(written > 0)
    }

    /// Loads and decodes `entry_id`'s full packed tilt-curve record, or `None` if it
    /// has never been generated (no `diagram_tilt_curves` row, or a row whose `curves`
    /// column is `NULL`).
    ///
    /// This is what `crate::db::sqlite::search` calls to run a
    /// `PerformanceFilter`'s exact, arbitrary-radius test against every candidate row
    /// that survives SQL-level narrowing -- see that module's doc comment for the
    /// two-stage design.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails, or if the stored BLOB fails
    /// to decode (see [`TiltPerformanceCurves::from_bytes`] -- this would mean the
    /// stored bytes don't match this crate's current canonical shape, not an ordinary
    /// "not generated yet" state).
    pub fn get_tilt_curves(&self, entry_id: i64) -> Result<Option<TiltPerformanceCurves>> {
        let bytes: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT curves FROM diagram_tilt_curves WHERE entry_id = ?1",
                params![entry_id],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("Failed to load tilt curves for entry_id: {entry_id}"))?
            .flatten();
        bytes
            .map(|b| TiltPerformanceCurves::from_bytes(&b))
            .transpose()
    }

    /// Whether `entry_id` has ever had tilt curves generated -- `generated_at IS NOT
    /// NULL`, which [`Self::save_tilt_curves`](Database::save_tilt_curves)'s doc
    /// comment establishes as equivalent to "every derived global-extreme column is
    /// populated too". Cheaper than [`Self::get_tilt_curves`] for a caller that only
    /// needs the yes/no answer (e.g. deciding whether a design needs (re)generating),
    /// since it never reads or decodes the curve BLOB.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn has_tilt_curves(&self, entry_id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT generated_at FROM diagram_tilt_curves WHERE entry_id = ?1",
                params![entry_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .with_context(|| {
                format!("Failed to check tilt-curve presence for entry_id: {entry_id}")
            })?
            .flatten()
            .is_some())
    }

    /// Deletes `entry_id`'s entire `diagram_tilt_curves` row, if one exists.
    ///
    /// Like `diagram_previews`, this is a side table keyed by
    /// `entry_id` that survives a `diagram_details` re-sync (see
    /// [`Self::save_diagram_detail`](Database::save_diagram_detail)'s own doc
    /// comment) -- so re-importing a `.asc` over an existing row, which changes the
    /// design's actual geometry, otherwise leaves a stale tilt-performance curve
    /// computed from the OLD schedule sitting on the row unchanged. Called after a
    /// re-import collision so the curves are regenerated from the new geometry
    /// instead of silently kept.
    ///
    /// A missing row is not an error -- deleting nothing (a design that never had
    /// tilt curves computed) is the ordinary, expected outcome for most re-imports.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DELETE` fails.
    pub fn delete_tilt_curves(&self, entry_id: i64) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM diagram_tilt_curves WHERE entry_id = ?1",
                params![entry_id],
            )
            .with_context(|| format!("Failed to delete tilt curves for entry_id: {entry_id}"))?;
        Ok(())
    }

    /// Every non-ignored `diagram_entries.id` whose cached tilt-performance curves are
    /// missing or were computed with other parameters than the current ones -- same
    /// blob-free `LEFT JOIN` shape as
    /// [`Self::entry_ids_missing_previews`](Database::entry_ids_missing_previews), for
    /// a batch pass's (`gui::batch::tilt::scan`) equivalent "what needs
    /// (re)generating" check, without loading and decoding every design's packed curve
    /// BLOB just to test one timestamp.
    ///
    /// A design needs a sweep when `diagram_tilt_curves` has no row for it, the row's
    /// `generated_at` is `NULL`, or the row's `params_fingerprint` differs from
    /// `current_fingerprint(material)` -- the fingerprint a sweep made NOW would store.
    /// `material` is the design's persisted `preview_material` (`None` when it has none
    /// yet): the sweep is done in that material, and it lives in `diagram_previews`, not
    /// in this table. A row written before fingerprints existed carries `NULL` there
    /// and counts as outdated.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn entry_ids_missing_tilt_curves(
        &self,
        current_fingerprint: impl Fn(Option<&str>) -> String,
    ) -> Result<Vec<i64>> {
        self.outdated_entry_ids(
            "SELECT de.id, tc.generated_at, tc.params_fingerprint, p.preview_material
             FROM diagram_entries de
             LEFT JOIN diagram_tilt_curves tc ON tc.entry_id = de.id
             LEFT JOIN diagram_previews p ON p.entry_id = de.id
             WHERE de.ignored = 0
             ORDER BY de.id",
            current_fingerprint,
        )
        .context("Failed to read entry ids missing tilt curves")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        entry::FacetingDiagramEntry,
        tilt_curves::{AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS},
    };

    fn temp_db_with_one_entry() -> (Database, i64, std::path::PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-tilt-curves-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        let entry_id = db
            .save_diagram_entry(
                &FacetingDiagramEntry {
                    title: "Tilt Curve Test".to_string(),
                    url: "local://tilt-curve-test.asc".to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        (db, entry_id, path)
    }

    fn flat_curves(value: f32) -> TiltPerformanceCurves {
        TiltPerformanceCurves {
            axes: [AxisTiltCurves {
                brilliance_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
                extinction_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
                windowing_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
            }; TILT_CURVE_AXIS_COUNT],
        }
    }

    /// The fingerprint every test sweep is stored with.
    const FINGERPRINT: &str = "test-fingerprint";

    /// Saves `curves` from the design's CURRENT revision, as a sweep that raced with no
    /// other writer would.
    fn save_current(db: &Database, entry_id: i64, curves: &TiltPerformanceCurves, at: i64) {
        let revision = db.entry_updated_at(entry_id).unwrap();
        assert!(
            db.save_tilt_curves(entry_id, curves, at, FINGERPRINT, revision)
                .unwrap(),
            "a write at the current revision must be stored"
        );
    }

    /// Advances `entry_id`'s revision stamp, as a re-import or metadata edit does.
    fn bump_revision(db: &Database, entry_id: i64) {
        db.conn
            .execute(
                "UPDATE diagram_entries SET updated_at = COALESCE(updated_at, 0) + 1 WHERE id = ?1",
                params![entry_id],
            )
            .unwrap();
    }

    #[test]
    fn a_design_with_no_curves_reports_absence_consistently() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        assert_eq!(db.get_tilt_curves(entry_id).unwrap(), None);
        assert!(!db.has_tilt_curves(entry_id).unwrap());
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_get_round_trips_the_full_curve_record() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let curves = flat_curves(55.0);
        save_current(&db, entry_id, &curves, 1_700_000_000);

        let loaded = db.get_tilt_curves(entry_id).unwrap().unwrap();
        assert_eq!(loaded, curves);
        assert!(db.has_tilt_curves(entry_id).unwrap());
        // The graph-image column is never written.
        let image: Option<Vec<u8>> = db
            .conn
            .query_row(
                "SELECT curve_image FROM diagram_tilt_curves WHERE entry_id = ?1",
                params![entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(image, None);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_tilt_curves_writes_every_derived_global_extreme_column() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let curves = flat_curves(42.0);
        save_current(&db, entry_id, &curves, 1);

        // Flat data at 42.0 everywhere: every global min/max, for every metric, must
        // read back as exactly 42.0.
        for (metric, extreme) in all_global_extreme_columns() {
            let column = global_extreme_column_name(metric, extreme);
            let value: f64 = db
                .conn
                .query_row(
                    &format!("SELECT {column} FROM diagram_tilt_curves WHERE entry_id = ?1"),
                    params![entry_id],
                    |r| r.get(0),
                )
                .unwrap_or_else(|e| panic!("column {column} missing or unreadable: {e}"));
            assert!(
                (value - 42.0).abs() < 1e-4,
                "{column} was {value}, expected 42.0"
            );
        }
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_tilt_curves_overwrites_a_previous_generation_for_the_same_design() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        save_current(&db, entry_id, &flat_curves(10.0), 1);
        save_current(&db, entry_id, &flat_curves(90.0), 2);

        let loaded = db.get_tilt_curves(entry_id).unwrap().unwrap();
        assert_eq!(loaded, flat_curves(90.0));
        let global_max: f64 = db
            .conn
            .query_row(
                "SELECT perf_brilliance_global_max FROM diagram_tilt_curves WHERE entry_id = ?1",
                params![entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!((global_max - 90.0).abs() < 1e-4);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_tilt_curves_global_extremes_span_the_whole_ramp() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        // Ramp brilliance from 0 (index 0, tilt -90) to 180 (index 180, tilt +90).
        let mut axis = AxisTiltCurves {
            brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        };
        for (i, sample) in axis.brilliance_pct.iter_mut().enumerate() {
            *sample = i as f32;
        }
        let curves = TiltPerformanceCurves {
            axes: [axis; TILT_CURVE_AXIS_COUNT],
        };
        save_current(&db, entry_id, &curves, 1);

        let global_min: f64 = db
            .conn
            .query_row(
                "SELECT perf_brilliance_global_min FROM diagram_tilt_curves WHERE entry_id = ?1",
                params![entry_id],
                |r| r.get(0),
            )
            .unwrap();
        let global_max: f64 = db
            .conn
            .query_row(
                "SELECT perf_brilliance_global_max FROM diagram_tilt_curves WHERE entry_id = ?1",
                params![entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            (global_min - 0.0).abs() < 1e-4,
            "global_min was {global_min}"
        );
        assert!(
            (global_max - 180.0).abs() < 1e-4,
            "global_max was {global_max}"
        );

        // Sanity: the in-memory computation this table is derived from agrees.
        let extremes = curves.global_extremes();
        assert!((f64::from(extremes.brilliance.min) - global_min).abs() < 1e-4);
        assert!((f64::from(extremes.brilliance.max) - global_max).abs() < 1e-4);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn entry_ids_missing_tilt_curves_excludes_generated_and_ignored_entries() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let generated_entry = db
            .save_diagram_entry(
                &FacetingDiagramEntry {
                    title: "Has Curves".to_string(),
                    url: "local://has-curves.asc".to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        save_current(&db, generated_entry, &flat_curves(10.0), 1);
        let ignored_entry = db
            .save_diagram_entry(
                &FacetingDiagramEntry {
                    title: "Ignored".to_string(),
                    url: "local://ignored.asc".to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        db.set_diagram_ignored(ignored_entry, true).unwrap();

        let missing = db
            .entry_ids_missing_tilt_curves(|_| FINGERPRINT.to_string())
            .unwrap();
        assert_eq!(missing, vec![entry_id]);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// A sweep made with other parameters (or before fingerprints existed) is listed as
    /// missing; one made with the current parameters is not.
    #[test]
    fn entry_ids_missing_tilt_curves_lists_a_row_whose_fingerprint_differs() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        save_current(&db, entry_id, &flat_curves(10.0), 1);

        assert_eq!(
            db.entry_ids_missing_tilt_curves(|_| FINGERPRINT.to_string())
                .unwrap(),
            Vec::<i64>::new()
        );
        assert_eq!(
            db.entry_ids_missing_tilt_curves(|_| "other-fingerprint".to_string())
                .unwrap(),
            vec![entry_id]
        );

        db.conn
            .execute(
                "UPDATE diagram_tilt_curves SET params_fingerprint = NULL WHERE entry_id = ?1",
                params![entry_id],
            )
            .unwrap();
        assert_eq!(
            db.entry_ids_missing_tilt_curves(|_| FINGERPRINT.to_string())
                .unwrap(),
            vec![entry_id]
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// The material handed to the fingerprint function is the design's persisted
    /// `preview_material`, read from `diagram_previews`.
    #[test]
    fn entry_ids_missing_tilt_curves_hands_the_persisted_material_to_the_fingerprint_function() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let candidates = [crate::model::material_match::RiPresetCandidate {
            name: "Sapphire".to_string(),
            refractive_index: 1.762,
        }];
        let mut rng = || -> f64 { panic!("single match, must not be called") };
        db.ensure_preview_material(entry_id, 1.762, &candidates, 0.01, &mut rng)
            .unwrap();
        let revision = db.entry_updated_at(entry_id).unwrap();
        db.save_tilt_curves(
            entry_id,
            &flat_curves(10.0),
            1,
            "material=Sapphire",
            revision,
        )
        .unwrap();

        assert_eq!(
            db.entry_ids_missing_tilt_curves(|material| {
                format!("material={}", material.unwrap_or("none"))
            })
            .unwrap(),
            Vec::<i64>::new()
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// The refuse path: a sweep whose design changed while it was being computed must
    /// not overwrite anything.
    #[test]
    fn save_tilt_curves_refuses_a_write_after_the_design_changed() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        save_current(&db, entry_id, &flat_curves(10.0), 10);

        let read_before_sweep = db.entry_updated_at(entry_id).unwrap();
        bump_revision(&db, entry_id);
        let stored = db
            .save_tilt_curves(
                entry_id,
                &flat_curves(90.0),
                20,
                FINGERPRINT,
                read_before_sweep,
            )
            .unwrap();
        assert!(!stored, "the stale sweep must be refused");
        assert_eq!(
            db.get_tilt_curves(entry_id).unwrap().unwrap(),
            flat_curves(10.0),
            "the old curves stay"
        );

        // Reading the revision again and sweeping afresh is accepted.
        save_current(&db, entry_id, &flat_curves(90.0), 30);
        assert_eq!(
            db.get_tilt_curves(entry_id).unwrap().unwrap(),
            flat_curves(90.0)
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// A refused first write leaves no row behind, so the design still counts as
    /// missing its curves.
    #[test]
    fn a_refused_first_write_creates_no_row() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let read_before_sweep = db.entry_updated_at(entry_id).unwrap();
        bump_revision(&db, entry_id);

        assert!(
            !db.save_tilt_curves(
                entry_id,
                &flat_curves(1.0),
                5,
                FINGERPRINT,
                read_before_sweep
            )
            .unwrap()
        );
        assert!(!db.has_tilt_curves(entry_id).unwrap());
        assert_eq!(
            db.entry_ids_missing_tilt_curves(|_| FINGERPRINT.to_string())
                .unwrap(),
            vec![entry_id]
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// `None` is the legacy "never stamped" revision: it matches a `NULL`
    /// `updated_at` and nothing else.
    #[test]
    fn a_null_revision_matches_only_an_expected_none() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        db.conn
            .execute(
                "UPDATE diagram_entries SET updated_at = NULL WHERE id = ?1",
                params![entry_id],
            )
            .unwrap();

        assert!(
            !db.save_tilt_curves(entry_id, &flat_curves(1.0), 1, FINGERPRINT, Some(7))
                .unwrap()
        );
        assert!(
            db.save_tilt_curves(entry_id, &flat_curves(1.0), 1, FINGERPRINT, None)
                .unwrap()
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_tilt_curves_for_a_deleted_entry_writes_nothing() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let revision = db.entry_updated_at(entry_id).unwrap();
        db.delete_diagram_entry(entry_id).unwrap();

        assert!(
            !db.save_tilt_curves(entry_id, &flat_curves(1.0), 1, FINGERPRINT, revision)
                .unwrap()
        );
        assert!(!db.has_tilt_curves(entry_id).unwrap());

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// A record with a non-finite sample could not be decoded again, so it is never
    /// stored.
    #[test]
    fn save_tilt_curves_rejects_a_non_finite_sample() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let mut curves = flat_curves(10.0);
        curves.axes[2].extinction_pct[17] = f32::NAN;
        let revision = db.entry_updated_at(entry_id).unwrap();

        assert!(
            db.save_tilt_curves(entry_id, &curves, 1, FINGERPRINT, revision)
                .is_err()
        );
        assert!(!db.has_tilt_curves(entry_id).unwrap());

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// Same cascade-vs-re-sync property as `previews.rs`' own test -- see
    /// `Database::migrate_diagram_tilt_curves_table`'s doc comment for why this table
    /// is keyed by `entry_id` rather than living on `diagram_details`.
    #[test]
    fn tilt_curve_row_survives_a_diagram_details_re_sync_but_not_entry_deletion() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        // A first import, then the curves.
        db.save_diagram_detail(
            &crate::model::detail::FacetingDiagramDetail::default(),
            entry_id,
        )
        .unwrap();
        save_current(&db, entry_id, &flat_curves(50.0), 1);

        // A genuine re-sync: a second save_diagram_detail deletes and reinserts
        // diagram_details (a new row id) for this entry_id -- diagram_tilt_curves, keyed
        // by entry_id rather than diagram_details' own row id, must be unaffected.
        db.save_diagram_detail(
            &crate::model::detail::FacetingDiagramDetail::default(),
            entry_id,
        )
        .unwrap();
        assert!(db.has_tilt_curves(entry_id).unwrap());

        db.delete_diagram_entry(entry_id).unwrap();
        assert!(!db.has_tilt_curves(entry_id).unwrap());

        drop(db);
        std::fs::remove_file(&path).ok();
    }
}
