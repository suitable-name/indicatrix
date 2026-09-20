//! Storage for `diagram_previews` -- see
//! `Database::migrate_diagram_previews_table`'s doc comment (in `super::migrations`)
//! for why this is a side table keyed by `entry_id` rather than columns on
//! `diagram_details`, and `crate::model::preview::PreviewImages` for the read-side
//! shape this module hands back.

use super::Database;
use crate::model::{
    material_match::{RiPresetCandidate, pick_ri_preset},
    preview::PreviewImages,
};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

impl Database {
    /// Returns `entry_id`'s persisted `preview_material` if one is already on file,
    /// without consulting `candidates`/`tolerance`/`random_unit` at all. Otherwise picks
    /// one via `crate::model::material_match::pick_ri_preset`, persists it, and returns
    /// it. Returns `Ok(None)` only when `candidates` is empty.
    ///
    /// # The "rolled once, reused forever" contract
    ///
    /// [`pick_ri_preset`] itself has no memory -- called twice it can answer
    /// differently. This method makes `preview_material` stable for a design's whole
    /// lifetime: the existing-value check runs first and returns immediately without
    /// calling `random_unit` if found, so a second call is a cheap read, never a second
    /// roll. `ON CONFLICT ... DO UPDATE SET preview_material = COALESCE(...)` makes the
    /// write itself race-safe (existing value always wins), followed by a read-back of
    /// whatever actually persisted -- so concurrent callers for the same `entry_id`
    /// can't disagree or clobber each other, even though this crate's actual
    /// single-connection usage never triggers that race.
    ///
    /// `target_ri` is the design's own refractive index, evaluated by the caller (see
    /// `crate::model::material_match`'s module doc for why evaluating a `GemMaterial`'s
    /// dispersion curve at the sodium D line is the caller's job, not this crate's).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT`/`INSERT ... ON CONFLICT` fails.
    pub fn ensure_preview_material(
        &self,
        entry_id: i64,
        target_ri: f64,
        candidates: &[RiPresetCandidate],
        tolerance: f64,
        random_unit: &mut dyn FnMut() -> f64,
    ) -> Result<Option<String>> {
        if let Some(existing) = self.read_preview_material(entry_id)? {
            return Ok(Some(existing));
        }

        let Some(picked) = pick_ri_preset(target_ri, candidates, tolerance, random_unit) else {
            return Ok(None);
        };

        self.conn
            .execute(
                "INSERT INTO diagram_previews (entry_id, preview_material)
                 VALUES (?1, ?2)
                 ON CONFLICT(entry_id) DO UPDATE SET
                    preview_material = COALESCE(diagram_previews.preview_material, excluded.preview_material)",
                params![entry_id, picked.name],
            )
            .with_context(|| format!("Failed to persist preview_material for entry_id: {entry_id}"))?;

        // Re-read rather than trusting `picked.name`: under the race this method's own
        // doc comment describes, another call may have won the `COALESCE` and this is
        // the only way to report what actually ended up persisted.
        self.read_preview_material(entry_id)
    }

    fn read_preview_material(&self, entry_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT preview_material FROM diagram_previews WHERE entry_id = ?1",
                params![entry_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .with_context(|| format!("Failed to read preview_material for entry_id: {entry_id}"))?
            .flatten())
    }

    /// Records `front_png`/`top_png` (PNG bytes, independently `None` if that view's
    /// render failed -- see [`crate::model::preview::PreviewImages`]'s doc comment) and
    /// the unix-seconds timestamp generation was attempted at. Creates `entry_id`'s
    /// `diagram_previews` row if it doesn't exist yet, or overwrites these three
    /// columns if it does -- `preview_material`, if already set on an existing row, is
    /// left completely untouched (this statement's `ON CONFLICT` clause never mentions
    /// it), so calling this after
    /// [`Self::ensure_preview_material`](Database::ensure_preview_material) can never
    /// clobber the material that call picked.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `INSERT ... ON CONFLICT` fails.
    pub fn save_preview_images(
        &self,
        entry_id: i64,
        front_png: Option<&[u8]>,
        top_png: Option<&[u8]>,
        generated_at_unix: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO diagram_previews (entry_id, preview_front, preview_top, preview_generated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(entry_id) DO UPDATE SET
                    preview_front = excluded.preview_front,
                    preview_top = excluded.preview_top,
                    preview_generated_at = excluded.preview_generated_at",
                params![entry_id, front_png, top_png, generated_at_unix],
            )
            .with_context(|| format!("Failed to save preview images for entry_id: {entry_id}"))?;
        Ok(())
    }

    /// Loads `entry_id`'s cached preview state. A design with no `diagram_previews` row
    /// at all (preview generation never attempted, and
    /// [`Self::ensure_preview_material`](Database::ensure_preview_material) never
    /// called either) returns [`PreviewImages::default`] -- every field `None` -- not
    /// an error; "no previews yet" is an ordinary, expected state for the large
    /// majority of a freshly-migrated real catalogue, not a failure.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn get_preview_images(&self, entry_id: i64) -> Result<PreviewImages> {
        let row = self
            .conn
            .query_row(
                "SELECT preview_front, preview_top, preview_material, preview_generated_at
                 FROM diagram_previews WHERE entry_id = ?1",
                params![entry_id],
                |row| {
                    Ok(PreviewImages {
                        front: row.get(0)?,
                        top: row.get(1)?,
                        material: row.get(2)?,
                        generated_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to load preview images for entry_id: {entry_id}"))?;
        Ok(row.unwrap_or_default())
    }

    /// Deletes `entry_id`'s entire `diagram_previews` row (images, generation
    /// timestamp, AND the persisted `preview_material` choice), if one exists.
    ///
    /// CAD audit item 97: `diagram_previews` is a side table keyed by `entry_id` (see
    /// this module's own doc comment), so re-importing a `.asc` over an existing row
    /// -- which fully replaces `diagram_details` via
    /// [`Self::save_diagram_detail`](Database::save_diagram_detail) -- leaves whatever
    /// preview images were generated from the OLD geometry sitting there unchanged,
    /// now silently describing a design that no longer exists. Called after a
    /// re-import collision so the next preview-generation pass has a clean slate to
    /// regenerate into, rather than a thumbnail that looks plausible but is wrong.
    ///
    /// A missing row is not an error -- deleting nothing (a design that never had
    /// previews generated) is the ordinary, expected outcome for most re-imports.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DELETE` fails.
    pub fn delete_preview_images(&self, entry_id: i64) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM diagram_previews WHERE entry_id = ?1",
                params![entry_id],
            )
            .with_context(|| format!("Failed to delete preview images for entry_id: {entry_id}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{entry::FacetDiagramEntry, material_match::RiPresetCandidate};

    fn temp_db_with_one_entry() -> (Database, i64, std::path::PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-previews-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        let entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: "Preview Test".to_string(),
                    url: "local://preview-test.asc".to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        (db, entry_id, path)
    }

    #[test]
    fn get_preview_images_on_a_design_with_no_row_returns_all_none() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        assert_eq!(
            db.get_preview_images(entry_id).unwrap(),
            crate::model::preview::PreviewImages::default()
        );
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn ensure_preview_material_with_a_single_match_persists_it_without_the_rng() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let candidates = [RiPresetCandidate {
            name: "Quartz".to_string(),
            refractive_index: 1.545,
        }];
        let mut rng = || -> f64 { panic!("must not be called for a single match") };
        let picked = db
            .ensure_preview_material(entry_id, 1.544, &candidates, 0.01, &mut rng)
            .unwrap();
        assert_eq!(picked.as_deref(), Some("Quartz"));
        assert_eq!(
            db.get_preview_images(entry_id).unwrap().material.as_deref(),
            Some("Quartz")
        );
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn ensure_preview_material_never_re_rolls_once_persisted() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let first_round = [
            RiPresetCandidate {
                name: "A".to_string(),
                refractive_index: 1.540,
            },
            RiPresetCandidate {
                name: "B".to_string(),
                refractive_index: 1.541,
            },
        ];
        let mut rng_low = || -> f64 { 0.0 };
        let first = db
            .ensure_preview_material(entry_id, 1.5405, &first_round, 0.01, &mut rng_low)
            .unwrap();
        assert_eq!(first.as_deref(), Some("A"));

        // Same design, a DIFFERENT candidate list and an RNG that would pick
        // differently if consulted -- the already-persisted value must win untouched,
        // and the RNG must never even be called.
        let second_round = [RiPresetCandidate {
            name: "Should Never Be Picked".to_string(),
            refractive_index: 1.5405,
        }];
        let mut rng_panics = || -> f64 { panic!("must not be called: material already on file") };
        let second = db
            .ensure_preview_material(entry_id, 1.5405, &second_round, 0.01, &mut rng_panics)
            .unwrap();
        assert_eq!(second.as_deref(), Some("A"));

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn ensure_preview_material_with_no_candidates_returns_none_and_persists_nothing() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let mut rng = || -> f64 { panic!("must not be called: nothing to pick from") };
        let picked = db
            .ensure_preview_material(entry_id, 1.54, &[], 0.01, &mut rng)
            .unwrap();
        assert_eq!(picked, None);
        assert_eq!(db.get_preview_images(entry_id).unwrap().material, None);
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_preview_images_round_trips_and_leaves_material_untouched() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let candidates = [RiPresetCandidate {
            name: "Sapphire".to_string(),
            refractive_index: 1.762,
        }];
        let mut rng = || -> f64 { panic!("single match, must not be called") };
        db.ensure_preview_material(entry_id, 1.762, &candidates, 0.01, &mut rng)
            .unwrap();

        let front = vec![1u8, 2, 3, 4];
        let top = vec![5u8, 6, 7, 8];
        db.save_preview_images(entry_id, Some(&front), Some(&top), 1_700_000_000)
            .unwrap();

        let loaded = db.get_preview_images(entry_id).unwrap();
        assert_eq!(loaded.front, Some(front));
        assert_eq!(loaded.top, Some(top));
        assert_eq!(loaded.generated_at, Some(1_700_000_000));
        // The material picked before this call must survive it untouched.
        assert_eq!(loaded.material.as_deref(), Some("Sapphire"));

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_preview_images_allows_one_view_to_be_none_when_its_render_failed() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        let front = vec![9u8, 9, 9];
        db.save_preview_images(entry_id, Some(&front), None, 1_700_000_001)
            .unwrap();

        let loaded = db.get_preview_images(entry_id).unwrap();
        assert_eq!(loaded.front, Some(front));
        assert_eq!(loaded.top, None);
        // "attempted" is still recorded even though one view failed -- this is exactly
        // the distinction PreviewImages::generated_at exists to preserve.
        assert_eq!(loaded.generated_at, Some(1_700_000_001));

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// Deleting the owning `diagram_entries` row must cascade to `diagram_previews`
    /// (its `FOREIGN KEY ... ON DELETE CASCADE`), while a `diagram_details` re-sync
    /// (delete + reinsert of that unrelated row) must NOT touch it -- the entire reason
    /// this table is keyed by `entry_id` rather than living as columns on
    /// `diagram_details` (see `Database::migrate_diagram_previews_table`'s doc
    /// comment).
    #[test]
    fn preview_row_survives_a_diagram_details_re_sync_but_not_entry_deletion() {
        let (db, entry_id, path) = temp_db_with_one_entry();
        // A first import: writes diagram_details for the first time.
        db.save_diagram_detail(
            &crate::model::detail::FacetDiagramDetail::default(),
            entry_id,
        )
        .unwrap();
        db.save_preview_images(entry_id, Some(&[1, 2, 3]), Some(&[4, 5, 6]), 42)
            .unwrap();

        // A "re-sync": save_diagram_detail's own doc comment establishes that a second
        // call for the same entry_id first DELETEs the existing diagram_details row
        // (cascading to angle_settings/attached_files) and then reinserts a fresh one
        // with a new id -- exactly what a real re-scrape does. diagram_previews must
        // not be affected by that delete at all, since it is keyed by entry_id
        // (diagram_entries), not by diagram_details' own row id.
        db.save_diagram_detail(
            &crate::model::detail::FacetDiagramDetail::default(),
            entry_id,
        )
        .unwrap();
        let after_resync = db.get_preview_images(entry_id).unwrap();
        assert_eq!(after_resync.front, Some(vec![1, 2, 3]));

        db.delete_diagram_entry(entry_id).unwrap();
        let after_delete = db.get_preview_images(entry_id).unwrap();
        assert_eq!(
            after_delete,
            crate::model::preview::PreviewImages::default()
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }
}
