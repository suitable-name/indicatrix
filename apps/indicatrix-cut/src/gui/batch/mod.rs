//! The two catalogue-wide background batches -- generating cached preview thumbnails
//! ([`preview`]) and computing tilt-performance curves ([`tilt`]) -- plus what they
//! share: a local/remote work-distribution queue ([`batch_queue`]), the bookkeeping for
//! the several remote dispatchers each batch runs at once ([`remote_dispatch`], sized by
//! the setting [`remote_lanes_setting`] edits), the decoded preview-thumbnail cache the
//! design list reads from ([`preview_cache`]), and the one geometry step both engines
//! take for a record ([`record_planes_for_batch`]).
//!
//! Was 4 flat top-level `gui` files (`preview_batch.rs`, `tilt_batch.rs`,
//! `batch_queue.rs`, `preview_cache.rs`); grouped here since the two batches are
//! deliberately parallel in shape (see [`preview`]'s and [`tilt`]'s own module doc
//! comments for exactly what they share vs. why they stayed separate modules) and both
//! were already, individually, well over this codebase's ~700-line split threshold.

pub mod batch_queue;
pub mod material_choice;
pub mod preview;
pub mod preview_cache;
pub mod regenerate_all;
pub mod remote_backoff;
pub mod remote_dispatch;
pub mod remote_lanes_setting;
#[cfg(test)]
mod test_records;
pub mod tilt;

use indicatrix::geometry::GpuFacetPlane;
use indicatrix_vault::model::entry::FullDiagramRecord;
use std::{
    collections::BTreeSet,
    sync::{Mutex, PoisonError},
};

/// The facet planes a batch engine renders or sweeps for `full`, or `None` when
/// there are none to use.
///
/// Resolved by `gui::editor::resolve_catalogue_planes`, the same function the
/// library detail view's 3D preview uses: the design file (`.asc`, else `.gem`,
/// else `.gcs`) first, the angle table only when the record has no usable design
/// file. A record that took the angle-table fallback is added to
/// `angle_table_entries` (a set, so a design resolved more than once -- two
/// preview views, or a remote attempt retried locally -- counts once), which the
/// batch's summary line reports.
#[must_use]
pub fn record_planes_for_batch(
    full: &FullDiagramRecord,
    angle_table_entries: &Mutex<BTreeSet<i64>>,
) -> Option<Vec<GpuFacetPlane>> {
    let resolved = crate::gui::editor::resolve_catalogue_planes(full);
    if resolved.source == crate::gui::editor::CataloguePlanesSource::AngleTable {
        angle_table_entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(full.entry_id);
    }
    (!resolved.planes.is_empty()).then_some(resolved.planes)
}

/// How many designs [`record_planes_for_batch`] recorded in `angle_table_entries`.
#[must_use]
pub fn angle_table_count(angle_table_entries: &Mutex<BTreeSet<i64>>) -> usize {
    angle_table_entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len()
}

/// The summary-line clause for `count` designs built from the angle table, empty
/// when there were none -- shared by both batches' summary lines.
#[must_use]
pub fn angle_table_summary(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(", {count} from the angle table (no usable design file)")
    }
}
