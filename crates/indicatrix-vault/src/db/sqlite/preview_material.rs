//! How a design's cached renders are told to be outdated: the material, stone size and
//! provenance `diagram_previews` persists for a design
//! ([`crate::model::preview::PreviewMaterialState`]) and the two staleness scans that
//! compare a stored fingerprint with the one a render made now would carry
//! ([`Database::entry_ids_missing_previews`], [`Database::entry_ids_missing_tilt_curves`]).
//!
//! The image storage itself is in `super::previews`, the curve storage in
//! `super::tilt_curves`; see `Database::migrate_diagram_previews_table`'s doc comment (in
//! `super::migrations`) for why all of it is a side table keyed by `entry_id`.

use super::Database;
use crate::model::preview::{PreviewMaterialSource, PreviewMaterialState};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

impl Database {
    /// Records the material `entry_id`'s renders are made in, OVERWRITING whatever the
    /// row holds: the name, the girdle diameter the material is sized with (`None`, or a
    /// value that is not a finite positive number, is stored as "not set") and how the
    /// name was decided. Creates the `diagram_previews` row if the design has none yet;
    /// the images and the fingerprint of an existing row are left untouched.
    ///
    /// The overwrite is the point: a name taken from the design's own file
    /// ([`PreviewMaterialSource::DesignFile`]) is re-read from that file whenever the
    /// design is resolved, so the file is the authority and the stored copy never needs
    /// to outlive an edit. Only a refractive-index pick is "rolled once, reused
    /// forever" ([`Self::ensure_preview_material_by`]), and that method still returns
    /// whatever name is stored, a design file's included.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `INSERT ... ON CONFLICT` fails, including when
    /// `entry_id` names no `diagram_entries` row.
    pub fn set_preview_material(
        &self,
        entry_id: i64,
        name: &str,
        girdle_mm: Option<f64>,
        source: PreviewMaterialSource,
    ) -> Result<()> {
        let girdle_mm = girdle_mm.filter(|mm| mm.is_finite() && *mm > 0.0);
        self.conn
            .execute(
                "INSERT INTO diagram_previews (
                     entry_id, preview_material, preview_girdle_mm, preview_material_source
                 )
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(entry_id) DO UPDATE SET
                    preview_material = excluded.preview_material,
                    preview_girdle_mm = excluded.preview_girdle_mm,
                    preview_material_source = excluded.preview_material_source",
                params![entry_id, name, girdle_mm, source.as_column_text()],
            )
            .with_context(|| format!("Failed to set preview_material for entry_id: {entry_id}"))?;
        Ok(())
    }

    /// Forgets a material taken from the design's own file, so the next resolution picks
    /// afresh: when `entry_id`'s stored source is [`PreviewMaterialSource::DesignFile`],
    /// the name, the size and the source are cleared (the images and the fingerprint stay)
    /// and `Ok(true)` comes back. A refractive-index pick, a row without a material and a
    /// design with no row are left alone (`Ok(false)`).
    ///
    /// For a design whose file no longer names a material (or names one nothing resolves):
    /// without this, [`Self::ensure_preview_material_by`] would keep returning the name
    /// the file used to carry.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    pub fn clear_design_preview_material(&self, entry_id: i64) -> Result<bool> {
        let cleared = self
            .conn
            .execute(
                "UPDATE diagram_previews
                 SET preview_material = NULL,
                     preview_girdle_mm = NULL,
                     preview_material_source = NULL
                 WHERE entry_id = ?1 AND preview_material_source = ?2",
                params![entry_id, PreviewMaterialSource::DesignFile.as_column_text()],
            )
            .with_context(|| {
                format!("Failed to clear preview_material for entry_id: {entry_id}")
            })?;
        Ok(cleared > 0)
    }

    /// Records the girdle diameter `entry_id`'s material is sized with, beside a name that
    /// is kept as it is (a refractive-index pick, which stays write-once): `None`, or a
    /// value that is not a finite positive number, is stored as "not set". Only the
    /// size is written; the name, the source, the images and the fingerprint of the row are
    /// untouched, and a design with no `diagram_previews` row is left without one.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    pub fn set_preview_girdle_mm(&self, entry_id: i64, girdle_mm: Option<f64>) -> Result<()> {
        let girdle_mm = girdle_mm.filter(|mm| mm.is_finite() && *mm > 0.0);
        self.conn
            .execute(
                "UPDATE diagram_previews SET preview_girdle_mm = ?2 WHERE entry_id = ?1",
                params![entry_id, girdle_mm],
            )
            .with_context(|| format!("Failed to set preview_girdle_mm for entry_id: {entry_id}"))?;
        Ok(())
    }

    /// `entry_id`'s persisted material state: name, size and source. A design with no
    /// `diagram_previews` row, or a row written before the size and source columns
    /// existed, reads as [`PreviewMaterialState::default`] with whatever the row does
    /// hold (no size, a refractive-index source).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn get_preview_material_state(&self, entry_id: i64) -> Result<PreviewMaterialState> {
        let row = self
            .conn
            .query_row(
                "SELECT preview_material, preview_girdle_mm, preview_material_source
                 FROM diagram_previews WHERE entry_id = ?1",
                params![entry_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .with_context(|| {
                format!("Failed to read the preview material state for entry_id: {entry_id}")
            })?;
        Ok(row.map_or_else(
            PreviewMaterialState::default,
            |(name, girdle_mm, source)| PreviewMaterialState {
                name,
                girdle_mm,
                source: PreviewMaterialSource::from_column_text(source.as_deref()),
            },
        ))
    }

    /// Every non-ignored `diagram_entries.id` whose cached preview render is missing or
    /// was made with other parameters than the current ones. Ordered by id for a stable,
    /// resumable walk.
    ///
    /// A design needs a render when `diagram_previews` has no row for it, the row's
    /// `preview_generated_at` is `NULL`, EITHER view (`preview_front` / `preview_top`) is
    /// missing -- [`Self::save_preview_images`] stamps an attempt even when one view's
    /// render failed, so the stamp alone would call such a half-rendered row current --
    /// or the row's `params_fingerprint` differs from
    /// `current_fingerprint(material)` -- the fingerprint a render made NOW would store,
    /// given the row's persisted material state (`PreviewMaterialState::default()` when it
    /// has none). A row written before fingerprints existed carries `NULL` there and so
    /// counts as outdated: it is re-rendered once, then tracked like any other. The
    /// caller builds the string (renderer build, image size, sample count, bounce cap,
    /// lighting, material, stone size); this crate only compares it, so what goes into it
    /// never needs a schema change.
    ///
    /// The material state is what makes a design's own material and size count: a
    /// design file is never opened here (that would solve every design in the library),
    /// so the render that last used the file recorded its outcome with
    /// [`Self::set_preview_material`], and the fingerprint is compared against that.
    ///
    /// Replaces the "load every entry's preview images just to test one timestamp"
    /// scan a startup batch pass used to run (`gui::batch::preview::scan`): that
    /// pattern loads both 256x256 PNGs for EVERY entry in the catalogue merely to
    /// check `generated_at.is_none()` -- measured at 4.4s and 447 MB for one pass over
    /// the real catalogue's 3,299 entries -- when the same test is answered here by a
    /// `LEFT JOIN` that never selects a blob column at all. The "either view missing"
    /// test is `preview_front IS NULL OR preview_top IS NULL`, which reads only the
    /// record header, so the query stays blob-free.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn entry_ids_missing_previews(
        &self,
        current_fingerprint: impl Fn(&PreviewMaterialState) -> String,
    ) -> Result<Vec<i64>> {
        self.outdated_entry_ids(
            "SELECT de.id,
                    CASE WHEN p.preview_front IS NULL OR p.preview_top IS NULL
                         THEN NULL ELSE p.preview_generated_at END,
                    p.params_fingerprint, p.preview_material,
                    p.preview_girdle_mm, p.preview_material_source
             FROM diagram_entries de
             LEFT JOIN diagram_previews p ON p.entry_id = de.id
             WHERE de.ignored = 0
             ORDER BY de.id",
            current_fingerprint,
        )
        .context("Failed to read entry ids missing previews")
    }

    /// Runs `sql` -- one row per design to consider, as `(entry id, generated-at stamp,
    /// stored fingerprint, persisted material, persisted girdle diameter, persisted
    /// material source)`, each of the last five `NULL` when the cache row is absent (the
    /// caller may also null the stamp to mark an incomplete row, as
    /// [`Self::entry_ids_missing_previews`] does for a missing view) -- and returns, in row
    /// order, the ids whose cache entry is not current: no stamp, or a stored fingerprint
    /// other than `current_fingerprint(state)`, `state` being the last three columns
    /// read as a [`PreviewMaterialState`]. Shared by
    /// [`Self::entry_ids_missing_previews`] and
    /// [`Self::entry_ids_missing_tilt_curves`](Database::entry_ids_missing_tilt_curves),
    /// which differ only in the cache table they read.
    ///
    /// # Errors
    ///
    /// Returns an error if `sql` fails to prepare or run, or a row fails to decode.
    pub(super) fn outdated_entry_ids(
        &self,
        sql: &str,
        current_fingerprint: impl Fn(&PreviewMaterialState) -> String,
    ) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut outdated = Vec::new();
        for row in rows {
            let (entry_id, generated_at, stored, name, girdle_mm, source) = row?;
            let state = PreviewMaterialState {
                name,
                girdle_mm,
                source: PreviewMaterialSource::from_column_text(source.as_deref()),
            };
            let current = generated_at.is_some()
                && stored.as_deref() == Some(current_fingerprint(&state).as_str());
            if !current {
                outdated.push(entry_id);
            }
        }
        Ok(outdated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{entry::FacetingDiagramEntry, material_match::RiPresetCandidate};

    /// An in-memory database with one design.
    fn db_with_one_entry() -> (Database, i64) {
        let db = Database::new(Some(":memory:")).expect("an in-memory database opens");
        let entry_id = db
            .save_diagram_entry(
                &FacetingDiagramEntry {
                    title: "Material Test".to_string(),
                    url: "local://material-test.asc".to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .expect("the entry saves");
        (db, entry_id)
    }

    fn quartz_candidate() -> [RiPresetCandidate; 1] {
        [RiPresetCandidate {
            name: "Quartz".to_string(),
            refractive_index: 1.545,
        }]
    }

    /// The fingerprint the tests store: every part of the material state, spelled out.
    fn fingerprint_of(state: &PreviewMaterialState) -> String {
        format!(
            "material={};size={:?};source={}",
            state.name.as_deref().unwrap_or("none"),
            state.girdle_mm,
            state.source.as_column_text()
        )
    }

    /// Stores one finished render of the design under `fingerprint`.
    fn save_render(db: &Database, entry_id: i64, fingerprint: &str) {
        let revision = db.entry_updated_at(entry_id).expect("the entry exists");
        assert!(
            db.save_preview_images(entry_id, Some(&[1]), Some(&[2]), 1, fingerprint, revision)
                .expect("the images save"),
            "a write at the current revision is stored"
        );
    }

    #[test]
    fn a_design_with_no_row_has_the_default_material_state() {
        let (db, entry_id) = db_with_one_entry();
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap(),
            PreviewMaterialState::default()
        );
        assert_eq!(
            PreviewMaterialState::default().source,
            PreviewMaterialSource::RefractiveIndex
        );
    }

    /// `set_preview_material` replaces what the row holds, including a name the
    /// refractive-index pick wrote (which `ensure_preview_material_by` never replaces),
    /// and `ensure_preview_material_by` afterwards keeps the design file's name.
    #[test]
    fn set_preview_material_overwrites_and_ensure_then_keeps_the_design_files_name() {
        let (db, entry_id) = db_with_one_entry();
        let mut never_asked = || -> f64 { panic!("a single match needs no draw") };
        db.ensure_preview_material(entry_id, 1.544, &quartz_candidate(), 0.02, &mut never_asked)
            .unwrap();
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap(),
            PreviewMaterialState {
                name: Some("Quartz".to_string()),
                girdle_mm: None,
                source: PreviewMaterialSource::RefractiveIndex,
            },
            "a pick reads as a refractive-index source with no size"
        );

        db.set_preview_material(
            entry_id,
            "Sapphire",
            Some(9.0),
            PreviewMaterialSource::DesignFile,
        )
        .unwrap();
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap(),
            PreviewMaterialState {
                name: Some("Sapphire".to_string()),
                girdle_mm: Some(9.0),
                source: PreviewMaterialSource::DesignFile,
            }
        );
        assert_eq!(
            db.get_preview_material(entry_id).unwrap().as_deref(),
            Some("Sapphire")
        );

        let kept = db
            .ensure_preview_material_by(entry_id, 1.544, &quartz_candidate(), 0.02, &mut |_| {
                panic!("a stored material is never picked again")
            })
            .unwrap();
        assert_eq!(kept.as_deref(), Some("Sapphire"));

        // A second resolution overwrites again, with no size and the other source.
        db.set_preview_material(
            entry_id,
            "Ruby",
            None,
            PreviewMaterialSource::RefractiveIndex,
        )
        .unwrap();
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap(),
            PreviewMaterialState {
                name: Some("Ruby".to_string()),
                girdle_mm: None,
                source: PreviewMaterialSource::RefractiveIndex,
            }
        );
    }

    #[test]
    fn a_size_that_is_not_a_finite_positive_number_is_stored_as_not_set() {
        let (db, entry_id) = db_with_one_entry();
        for bad in [0.0, -3.0, f64::NAN, f64::INFINITY] {
            db.set_preview_material(
                entry_id,
                "Sapphire",
                Some(bad),
                PreviewMaterialSource::DesignFile,
            )
            .unwrap();
            assert_eq!(
                db.get_preview_material_state(entry_id).unwrap().girdle_mm,
                None,
                "{bad}"
            );
        }
    }

    /// Setting the material leaves a stored render and its fingerprint alone: only the
    /// next scan, comparing the new state against the old fingerprint, calls it outdated.
    #[test]
    fn set_preview_material_leaves_the_images_and_the_fingerprint_untouched() {
        let (db, entry_id) = db_with_one_entry();
        save_render(&db, entry_id, "stored");
        db.set_preview_material(
            entry_id,
            "Sapphire",
            Some(9.0),
            PreviewMaterialSource::DesignFile,
        )
        .unwrap();
        let images = db.get_preview_images(entry_id).unwrap();
        assert_eq!(images.front, Some(vec![1]));
        assert_eq!(images.top, Some(vec![2]));
        assert_eq!(images.generated_at, Some(1));
        assert_eq!(images.material.as_deref(), Some("Sapphire"));
        assert_eq!(
            db.entry_ids_missing_previews(|_| "stored".to_string())
                .unwrap(),
            Vec::<i64>::new()
        );
    }

    /// The size is recorded beside a pick that stays as it is, and a design with no row
    /// is not given one.
    #[test]
    fn set_preview_girdle_mm_records_only_the_size() {
        let (db, entry_id) = db_with_one_entry();
        db.set_preview_girdle_mm(entry_id, Some(9.0)).unwrap();
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap(),
            PreviewMaterialState::default(),
            "no row, nothing to size"
        );

        let mut never_asked = || -> f64 { panic!("a single match needs no draw") };
        db.ensure_preview_material(entry_id, 1.544, &quartz_candidate(), 0.02, &mut never_asked)
            .unwrap();
        db.set_preview_girdle_mm(entry_id, Some(9.0)).unwrap();
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap(),
            PreviewMaterialState {
                name: Some("Quartz".to_string()),
                girdle_mm: Some(9.0),
                source: PreviewMaterialSource::RefractiveIndex,
            }
        );
        db.set_preview_girdle_mm(entry_id, None).unwrap();
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap().girdle_mm,
            None
        );
    }

    #[test]
    fn clear_design_preview_material_clears_only_a_name_from_the_design_file() {
        let (db, entry_id) = db_with_one_entry();
        assert!(
            !db.clear_design_preview_material(entry_id).unwrap(),
            "no row, nothing to clear"
        );

        let mut never_asked = || -> f64 { panic!("a single match needs no draw") };
        db.ensure_preview_material(entry_id, 1.544, &quartz_candidate(), 0.02, &mut never_asked)
            .unwrap();
        assert!(
            !db.clear_design_preview_material(entry_id).unwrap(),
            "a refractive-index pick stays"
        );
        assert_eq!(
            db.get_preview_material(entry_id).unwrap().as_deref(),
            Some("Quartz")
        );

        db.set_preview_material(
            entry_id,
            "Sapphire",
            Some(9.0),
            PreviewMaterialSource::DesignFile,
        )
        .unwrap();
        assert!(db.clear_design_preview_material(entry_id).unwrap());
        assert_eq!(
            db.get_preview_material_state(entry_id).unwrap(),
            PreviewMaterialState::default()
        );

        // The pick is free again.
        let picked = db
            .ensure_preview_material(entry_id, 1.544, &quartz_candidate(), 0.02, &mut never_asked)
            .unwrap();
        assert_eq!(picked.as_deref(), Some("Quartz"));
    }

    /// The staleness scans see the persisted size and source, so a design that was
    /// re-rendered in another size, or whose name came from another source, is listed.
    #[test]
    fn the_scans_hand_the_persisted_size_and_source_to_the_fingerprint_function() {
        let (db, entry_id) = db_with_one_entry();
        db.set_preview_material(
            entry_id,
            "Sapphire",
            Some(9.0),
            PreviewMaterialSource::DesignFile,
        )
        .unwrap();
        save_render(
            &db,
            entry_id,
            "material=Sapphire;size=Some(9.0);source=design",
        );
        assert_eq!(
            db.entry_ids_missing_previews(fingerprint_of).unwrap(),
            Vec::<i64>::new(),
            "the render matches the persisted state"
        );

        db.set_preview_material(
            entry_id,
            "Sapphire",
            Some(10.5),
            PreviewMaterialSource::DesignFile,
        )
        .unwrap();
        assert_eq!(
            db.entry_ids_missing_previews(fingerprint_of).unwrap(),
            vec![entry_id],
            "another size makes the render outdated"
        );

        db.set_preview_material(
            entry_id,
            "Sapphire",
            Some(9.0),
            PreviewMaterialSource::RefractiveIndex,
        )
        .unwrap();
        assert_eq!(
            db.entry_ids_missing_previews(fingerprint_of).unwrap(),
            vec![entry_id],
            "another source makes the render outdated"
        );

        // Both scans read the same state: the tilt scan joins `diagram_previews` for it.
        assert_eq!(
            db.entry_ids_missing_tilt_curves(fingerprint_of).unwrap(),
            vec![entry_id],
            "a design with no curves is listed whatever its state"
        );
    }
}
