//! Storing computed tilt curves -- the one place a sweep result reaches the catalogue,
//! whichever lane produced it.

use crate::bridge::preview_render::{CacheKind, cache_fingerprint};
use indicatrix_vault::{db::sqlite::Database, model::tilt_curves::TiltPerformanceCurves};
use std::{
    sync::{Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::warn;

/// Persists `curves` for `entry_id`, stored with the fingerprint of the sweep
/// (`material_name` being the material it was swept in). Shared by both
/// `process_local_entry` and `process_remote_entry` -- whichever lane actually
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
    material_name: Option<&str>,
    expected_updated_at: Option<i64>,
) -> bool {
    // Unix seconds fits in i64 until well past the year 292 billion; the column this
    // feeds (`diagram_tilt_curves.generated_at`) is already declared INTEGER (i64) to
    // match.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let fingerprint = cache_fingerprint(CacheKind::TiltCurves, material_name);
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
/// and the curves are labelled with the design's persisted preview material -- the one
/// the batch sweeps use.
#[must_use]
pub fn save_tilt_curves_for_entry(
    db: &Mutex<Database>,
    entry_id: i64,
    curves: &TiltPerformanceCurves,
) -> bool {
    let (updated_at, material_name) = {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(updated_at) = guard.entry_updated_at(entry_id) else {
            return false;
        };
        (
            updated_at,
            guard.get_preview_material(entry_id).ok().flatten(),
        )
    };
    save_curves(db, entry_id, curves, material_name.as_deref(), updated_at)
}
