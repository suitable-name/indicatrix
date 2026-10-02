//! Background batch generation of the two cached catalogue-preview images (front +
//! top) for one or many designs at once -- the orchestration layer that sits on top of
//! `bridge::preview_render` (the actual rendering) and `indicatrix_vault`'s storage
//! (`Database::get_preview_images`/`save_preview_images`/`ensure_preview_material`).
//!
//! Split into [`scan`] (missing-previews library scan), [`engine`] (per-item
//! resolve/render/progress and the local/remote lane runners), and [`wiring`] (the
//! public `spawn_preview_batch`/`setup_preview_batch_callbacks` UI glue).
//!
//! Cancellation is checked between items, never mid-render (a render is ~1-2s, too
//! short for finer-grained cancellation to matter) -- designs already written via
//! `Database::save_preview_images` stay written, nothing rolls back.
//!
//! Every single-view render ([`engine::render_item_local`]/`_remote`) is wrapped in its
//! own `catch_unwind`, reusing the same fix `gui::library::local::import` applies to
//! import (`catch_file_panic`) for the same bug class (a malformed `.asc` angle-settings
//! row panicking inside `indicatrix`'s reconstruction/tracer) -- one item's panic can't
//! take down the rest of the batch or even the other view of the same design, matching
//! `PreviewImages`'s "one view can fail while the other succeeds" contract. An
//! unconditional `Drop` guard (the same `BusyGuard` pattern as `spawn_import`) clears
//! `RenderContext::export_active` and `preview_batch_running` regardless of panics.
//!
//! `RenderContext::export_active` is held for the whole batch's lifetime, the same flag
//! and reasoning `gui::render::render_export` uses for a high-res export: this is also
//! CPU/GPU-hungry background rendering that must not fight the live viewport for
//! raytracing capacity.
//!
//! `settings::model::LiveComputeTarget` (the same choice behind the viewport's "Live
//! Compute" pill) is read fresh each time a batch starts and governs lane count:
//! `RemoteOnly` runs [`batch_queue::remote_lane_count`] remote dispatchers; `LocalOnly`
//! runs [`batch_queue::local_lane_count`] local lanes; `Both` runs both, pulling from
//! one shared [`batch_queue::WorkQueue`] (see that module's doc comment for the queue
//! design). The unit handed out is one [`engine::PreviewItem`] -- a single view (front
//! or top) of one design, not both bundled -- since `PreviewImages` already allows the
//! two views to succeed/fail independently, unlike the tilt batch's per-design item
//! (whose 4 axes are all-or-nothing). `RemoteOnly` never falls back to local on failure
//! (tallied `failed` instead, so a remote-only user is never silently served a local
//! render); `Both` requeues a remote failure for guaranteed local processing instead
//! (see `batch_queue`'s doc comment for why that's local-only, never back to remote).
//!
//! The remote side runs `AppSettings::remote_batch_lanes` dispatchers at once, each
//! keeping one whole picture in flight on the remote -- one dispatcher waiting on one
//! round trip left a remote that renders a small picture in a fraction of it idle most of
//! the time. They all claim from the one queue (`claim_shared` is safe under any number
//! of claimants), each keeps its own failure backoff, and the batch's "remote lane done"
//! flag is raised when the LAST one ends (`gui::batch::remote_dispatch`).
//!
//! [`engine::resolve_design`] resolves per ITEM, not once per design behind a shared
//! cache: since a design's two views may be claimed by different lanes, there is no
//! single thread to naturally own a per-design resolve, and the duplicated
//! `get_diagram_full`/`ensure_preview_material` cost (~1ms) is negligible next to a
//! ~1-2s render -- not worth a lock-guarded cache to save it.
//!
//! Progress is reported as a COMPLETED COUNT (`preview_batch_design_index`), not a
//! current index, since local and remote lanes may each be mid-item on different
//! designs simultaneously. With N local lanes running concurrently, no single "current
//! title" can describe them, so `preview_batch_local_active` reports how many of
//! `preview_batch_local_lane_total` currently have an item claimed. The remote side is
//! several dispatchers too, so it reports how many items are on the remote
//! (`preview_batch_remote_in_flight`) and keeps `preview_batch_remote_title` as the item
//! a dispatcher started most recently.

mod engine;
mod import_choice;
mod remote_lane;
mod scan;
mod wiring;

pub use engine::{RI_MATCH_TOLERANCE, target_ri_for_design};
pub use import_choice::offer_import_previews;
pub use wiring::{offer_batch_confirmation, setup_preview_batch_callbacks};

#[cfg(test)]
mod tests {
    use super::engine::{BatchContext, record_planes};
    use crate::gui::batch::test_records::{
        ASC_MAST, ASC_TEXT, ENTRY_ID, angle_table_planes, angle_table_record, assert_gem_geometry,
        gem_record, has_plane_at, record_with,
    };
    use indicatrix::geometry::GpuFacetPlane;
    use indicatrix_vault::{db::sqlite::Database, model::entry::FullDiagramRecord};
    use std::{
        collections::BTreeSet,
        sync::{Mutex, atomic::AtomicBool},
    };

    /// The preview engine's own geometry step for `full`, plus the ids its batch
    /// context counted as angle-table fallbacks.
    fn preview_planes(full: &FullDiagramRecord) -> (Option<Vec<GpuFacetPlane>>, BTreeSet<i64>) {
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
        let gpu_retired = AtomicBool::new(false);
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            db: &db,
            material_candidates: &[],
            preview_size: 1,
            preview_spp: 1,
            solid: false,
            gpu_retired: &gpu_retired,
            angle_table_entries: &angle_table_entries,
        };
        let planes = record_planes(&ctx, full);
        let fallbacks = angle_table_entries
            .into_inner()
            .expect("the fallback set is never poisoned in a test");
        (planes, fallbacks)
    }

    /// A record whose only design file is a `.gem` renders the `.gem`'s real
    /// masts, not the angle table's, and is not counted as a fallback.
    #[test]
    fn preview_batch_renders_a_gem_only_record_from_its_design_file() {
        let full = gem_record();
        let (planes, fallbacks) = preview_planes(&full);
        let planes = planes.expect("the .gem yields planes");
        assert_gem_geometry(&planes);
        assert_ne!(planes, angle_table_planes(&full));
        assert!(fallbacks.is_empty());
        // The detail view resolves the same record to the same stone.
        assert_eq!(
            planes,
            crate::gui::editor::resolve_catalogue_planes(&full).planes
        );
    }

    /// A record whose `.asc` masts differ from its angle table renders the `.asc`.
    #[test]
    fn preview_batch_prefers_the_asc_over_a_differing_angle_table() {
        let full = record_with("design.asc", ASC_TEXT.as_bytes().to_vec());
        let (planes, fallbacks) = preview_planes(&full);
        let planes = planes.expect("the .asc yields planes");
        assert!(has_plane_at(&planes, ASC_MAST));
        assert_ne!(planes, angle_table_planes(&full));
        assert!(fallbacks.is_empty());
    }

    /// A record with no design file keeps the angle-table geometry the batch
    /// always rendered, and is counted as a fallback.
    #[test]
    fn preview_batch_renders_a_record_without_a_design_file_from_its_angle_table() {
        let full = angle_table_record();
        let (planes, fallbacks) = preview_planes(&full);
        assert_eq!(planes, Some(angle_table_planes(&full)));
        assert_eq!(fallbacks, BTreeSet::from([ENTRY_ID]));
    }

    /// A corrupt `.gem` falls back to the angle table without panicking.
    #[test]
    fn preview_batch_falls_back_to_the_angle_table_for_a_corrupt_gem() {
        let full = record_with("broken.gem", vec![1, 2, 3]);
        let (planes, fallbacks) = preview_planes(&full);
        assert_eq!(planes, Some(angle_table_planes(&full)));
        assert_eq!(fallbacks, BTreeSet::from([ENTRY_ID]));
    }
}
