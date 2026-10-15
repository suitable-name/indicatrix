//! Storing computed tilt curves -- the one place a sweep result reaches the catalogue,
//! whichever lane produced it.

use crate::bridge::preview_render::{CacheKind, FingerprintMaterial, cache_fingerprint};
use indicatrix_vault::{db::sqlite::Database, model::tilt_curves::TiltPerformanceCurves};
use std::{
    sync::{Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::warn;

/// Persists `curves` for `entry_id`, stored with the fingerprint of the sweep
/// (`material` being the material, stone size and provenance it was swept with). Shared
/// by both `process_local_entry` and `process_remote_entry` -- whichever lane actually
/// produced the curves, the save itself is identical. Returns whether the curves were
/// stored.
///
/// `expected_updated_at` is the catalogue row's revision stamp the design was resolved
/// at (`ResolvedDesign::updated_at`): the write is refused, and logged, when the row
/// has moved on since -- the sweep took seconds with the lock released, and a re-import
/// or metadata edit in that time made its curves describe a design that no longer
/// exists.
pub(super) fn save_curves(
    db: &Mutex<Database>,
    entry_id: i64,
    curves: &TiltPerformanceCurves,
    material: FingerprintMaterial<'_>,
    expected_updated_at: Option<i64>,
) -> bool {
    // Unix seconds fits in i64 until well past the year 292 billion; the column this
    // feeds (`diagram_tilt_curves.generated_at`) is already declared INTEGER (i64) to
    // match.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let fingerprint = cache_fingerprint(CacheKind::TiltCurves, material);
    let stored = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .save_tilt_curves(entry_id, curves, now, &fingerprint, expected_updated_at);
    match stored {
        Ok(true) => true,
        Ok(false) => {
            warn!(
                "Discarded the tilt curves computed for entry {entry_id}: the design changed \
                 while they were computed"
            );
            false
        }
        Err(e) => {
            warn!("Failed to save the tilt curves for entry {entry_id}: {e}");
            false
        }
    }
}

/// [`save_curves`], exposed for a caller outside this module -- the single-design
/// counterpart the Edit tab needs (its own "save tilt curves for this design" action,
/// once a design has been saved into the catalogue and so has a real `entry_id` to
/// save against).
///
/// The editor computed `curves` from its own in-memory design, not from the catalogue
/// row, so there is no record read to compare against: the save is checked against the
/// row's revision as of this call (which only fails for an entry that does not exist),
/// and the curves are labelled with the design's persisted preview material, stone size
/// and provenance -- the ones the batch sweeps use.
#[must_use]
pub fn save_tilt_curves_for_entry(
    db: &Mutex<Database>,
    entry_id: i64,
    curves: &TiltPerformanceCurves,
) -> bool {
    let (updated_at, material) = {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(updated_at) = guard.entry_updated_at(entry_id) else {
            return false;
        };
        (
            updated_at,
            guard
                .get_preview_material_state(entry_id)
                .unwrap_or_default(),
        )
    };
    save_curves(
        db,
        entry_id,
        curves,
        FingerprintMaterial::from_state(&material),
        updated_at,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        super::{BatchContext, resolve_design},
        *,
    };
    use crate::gui::batch::{
        catalogue_material::CatalogueMaterialSource,
        test_records::{design_file_text, store_design_file},
    };
    use indicatrix::{optics::materials::GemMaterial, render_setup};
    use indicatrix_cut_core::material::MaterialSelection;
    use indicatrix_vault::model::{
        material_match::RiPresetCandidate,
        preview::PreviewMaterialState,
        tilt_curves::{AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS},
    };
    use std::collections::BTreeSet;

    fn blank_curves() -> TiltPerformanceCurves {
        let axis = AxisTiltCurves {
            brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        };
        TiltPerformanceCurves {
            axes: [axis; TILT_CURVE_AXIS_COUNT],
        }
    }

    /// The designs the tilt scan lists for the fingerprint `material_of` builds from each
    /// persisted row.
    fn missing_with(
        db: &Mutex<Database>,
        material_of: impl Fn(&PreviewMaterialState) -> String,
    ) -> Vec<i64> {
        db.lock()
            .expect("the database lock is not poisoned")
            .entry_ids_missing_tilt_curves(material_of)
            .expect("the scan runs")
    }

    /// The fingerprint the tilt scan builds from a persisted row.
    fn scan_fingerprint(state: &PreviewMaterialState) -> String {
        cache_fingerprint(
            CacheKind::TiltCurves,
            FingerprintMaterial::from_state(state),
        )
    }

    /// A design whose file names Sapphire at a 9 mm girdle diameter is swept in that
    /// material at that size; its curves are stored under a fingerprint the scan rebuilds
    /// from the persisted row, so the scan finds them current, and the fingerprint the
    /// design had before the size and the source were tracked reads as stale.
    #[test]
    fn a_design_files_material_and_size_are_part_of_the_stored_fingerprint() {
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
        let selection = MaterialSelection {
            name: Some("Sapphire".to_string()),
            ..MaterialSelection::none()
        };
        let text = design_file_text(selection, Some(9.0), None);
        let entry_id = store_design_file(
            &db.lock().expect("the database lock is not poisoned"),
            "sapphire.isdd",
            text.into_bytes(),
        );
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            db: &db,
            material_candidates: &[],
            custom_materials: &[],
            angle_table_entries: &angle_table_entries,
        };

        let resolved = resolve_design(&ctx, entry_id).expect("the design resolves");
        assert_eq!(resolved.material_name, "Sapphire");
        assert_eq!(resolved.girdle_mm, 9.0);
        assert_eq!(
            resolved.material_source,
            CatalogueMaterialSource::DesignFile
        );
        assert_eq!(
            resolved.material,
            render_setup::material_for_stone(
                GemMaterial::by_name("Sapphire").expect("a built-in"),
                9.0,
                &resolved.planes
            )
        );

        assert_eq!(missing_with(&db, scan_fingerprint), vec![entry_id]);
        assert!(save_curves(
            &db,
            entry_id,
            &blank_curves(),
            resolved.fingerprint_material(),
            resolved.updated_at
        ));
        assert_eq!(missing_with(&db, scan_fingerprint), Vec::<i64>::new());

        let legacy = cache_fingerprint(
            CacheKind::TiltCurves,
            FingerprintMaterial::picked(Some("Sapphire")),
        );
        assert_eq!(missing_with(&db, |_| legacy.clone()), vec![entry_id]);
    }

    /// A design whose file names no material and no size keeps the refractive-index pick,
    /// unsized, and the exact fingerprint its curves had before the size and the source
    /// were tracked -- so those curves are not computed again.
    #[test]
    fn an_unnamed_unsized_design_keeps_the_old_fingerprint() {
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
        let text = design_file_text(MaterialSelection::none(), None, None);
        let entry_id = store_design_file(
            &db.lock().expect("the database lock is not poisoned"),
            "plain.isdd",
            text.into_bytes(),
        );
        let candidates = [RiPresetCandidate {
            name: "Quartz".to_string(),
            refractive_index: 1.545,
        }];
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            db: &db,
            material_candidates: &candidates,
            custom_materials: &[],
            angle_table_entries: &angle_table_entries,
        };

        let resolved = resolve_design(&ctx, entry_id).expect("the pick resolves");
        assert_eq!(resolved.material_name, "Quartz");
        assert_eq!(resolved.girdle_mm, 0.0);
        assert_eq!(
            resolved.material_source,
            CatalogueMaterialSource::RefractiveIndex
        );
        assert_eq!(
            resolved.fingerprint_material(),
            FingerprintMaterial::picked(Some("Quartz"))
        );
        assert!(save_curves(
            &db,
            entry_id,
            &blank_curves(),
            resolved.fingerprint_material(),
            resolved.updated_at
        ));
        let old = cache_fingerprint(
            CacheKind::TiltCurves,
            FingerprintMaterial::picked(Some("Quartz")),
        );
        assert_eq!(missing_with(&db, |_| old.clone()), Vec::<i64>::new());
        assert_eq!(missing_with(&db, scan_fingerprint), Vec::<i64>::new());
    }
}
