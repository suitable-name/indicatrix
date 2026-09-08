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
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, ToSql, params};

impl Database {
    /// Persists `entry_id`'s full tilt-performance record in one statement: the packed
    /// curve BLOB ([`TiltPerformanceCurves::to_bytes`]), the rendered graph PNG
    /// (`curve_image_png`, `None` if that render failed), the unix-seconds generation
    /// timestamp, and all 6 derived global-min/max columns
    /// ([`TiltPerformanceCurves::global_extremes`]) `crate::model::performance`'s
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
    /// or vice versa.
    ///
    /// The 6 derived columns are located and bound via
    /// [`crate::model::performance::all_global_extreme_columns`]/
    /// [`crate::model::performance::global_extreme_column_name`] rather than hand-listed
    /// here -- the same single-source-of-truth reasoning as
    /// `Database::migrate_diagram_tilt_curves_table`'s own doc comment.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `INSERT ... ON CONFLICT` fails.
    pub fn save_tilt_curves(
        &self,
        entry_id: i64,
        curves: &TiltPerformanceCurves,
        curve_image_png: Option<&[u8]>,
        generated_at_unix: i64,
    ) -> Result<()> {
        let extremes = curves.global_extremes();

        let mut columns: Vec<String> = vec![
            "entry_id".to_string(),
            "curves".to_string(),
            "curve_image".to_string(),
            "generated_at".to_string(),
        ];
        let mut params: Vec<Box<dyn ToSql>> = vec![
            Box::new(entry_id),
            Box::new(curves.to_bytes()),
            Box::new(curve_image_png.map(<[u8]>::to_vec)),
            Box::new(generated_at_unix),
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

        let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
        // Every column except `entry_id` (index 0) is overwritten on conflict.
        let update_assignments: Vec<String> = columns[1..]
            .iter()
            .map(|c| format!("{c} = excluded.{c}"))
            .collect();
        let sql = format!(
            "INSERT INTO diagram_tilt_curves ({}) VALUES ({})
             ON CONFLICT(entry_id) DO UPDATE SET {}",
            columns.join(", "),
            placeholders.join(", "),
            update_assignments.join(", "),
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("Failed to prepare tilt-curve upsert")?;
        let bound: Vec<&dyn ToSql> = params.iter().map(std::convert::AsRef::as_ref).collect();
        stmt.execute(bound.as_slice())
            .with_context(|| format!("Failed to save tilt curves for entry_id: {entry_id}"))?;
        Ok(())
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

    /// Loads just `entry_id`'s rendered tilt-curve graph PNG, without decoding the
    /// (much larger, 8,688-byte) packed curve BLOB at all -- the tilt-curve counterpart
    /// of [`Database::get_attachment_content`]'s "load only what was actually asked
    /// for" shape.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn get_tilt_curve_image(&self, entry_id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT curve_image FROM diagram_tilt_curves WHERE entry_id = ?1",
                params![entry_id],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .with_context(|| format!("Failed to load tilt curve image for entry_id: {entry_id}"))?
            .flatten())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        entry::FacetDiagramEntry,
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
                &FacetDiagramEntry {
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

    #[test]
    fn a_design_with_no_curves_reports_absence_consistently() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        assert_eq!(db.get_tilt_curves(entry_id).unwrap(), None);
        assert_eq!(db.get_tilt_curve_image(entry_id).unwrap(), None);
        assert!(!db.has_tilt_curves(entry_id).unwrap());
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_get_round_trips_the_full_curve_record() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let curves = flat_curves(55.0);
        db.save_tilt_curves(entry_id, &curves, Some(&[9, 8, 7]), 1_700_000_000)
            .unwrap();

        let loaded = db.get_tilt_curves(entry_id).unwrap().unwrap();
        assert_eq!(loaded, curves);
        assert_eq!(
            db.get_tilt_curve_image(entry_id).unwrap(),
            Some(vec![9, 8, 7])
        );
        assert!(db.has_tilt_curves(entry_id).unwrap());

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_tilt_curves_writes_every_derived_global_extreme_column() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let curves = flat_curves(42.0);
        db.save_tilt_curves(entry_id, &curves, None, 1).unwrap();

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
        db.save_tilt_curves(entry_id, &flat_curves(10.0), None, 1)
            .unwrap();
        db.save_tilt_curves(entry_id, &flat_curves(90.0), Some(&[1]), 2)
            .unwrap();

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
        db.save_tilt_curves(entry_id, &curves, None, 1).unwrap();

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

    /// Same cascade-vs-re-sync property as `previews.rs`' own test -- see
    /// `Database::migrate_diagram_tilt_curves_table`'s doc comment for why this table
    /// is keyed by `entry_id` rather than living on `diagram_details`.
    #[test]
    fn tilt_curve_row_survives_a_diagram_details_re_sync_but_not_entry_deletion() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        // A first import, then the curves.
        db.save_diagram_detail(
            &crate::model::detail::FacetDiagramDetail::default(),
            entry_id,
        )
        .unwrap();
        db.save_tilt_curves(entry_id, &flat_curves(50.0), None, 1)
            .unwrap();

        // A genuine re-sync: a second save_diagram_detail deletes and reinserts
        // diagram_details (a new row id) for this entry_id -- diagram_tilt_curves, keyed
        // by entry_id rather than diagram_details' own row id, must be unaffected.
        db.save_diagram_detail(
            &crate::model::detail::FacetDiagramDetail::default(),
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
