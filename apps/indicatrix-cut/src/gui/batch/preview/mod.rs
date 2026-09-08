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
//! `RemoteOnly` runs one remote dispatcher; `LocalOnly` runs
//! [`batch_queue::local_lane_count`] local lanes; `Both` runs both, pulling from one
//! shared [`batch_queue::WorkQueue`] (see that module's doc comment for the queue
//! design). The unit handed out is one [`engine::PreviewItem`] -- a single view (front
//! or top) of one design, not both bundled -- since `PreviewImages` already allows the
//! two views to succeed/fail independently, unlike the tilt batch's per-design item
//! (whose 4 axes are all-or-nothing). `RemoteOnly` never falls back to local on failure
//! (tallied `failed` instead, so a remote-only user is never silently served a local
//! render); `Both` requeues a remote failure for guaranteed local processing instead
//! (see `batch_queue`'s doc comment for why that's local-only, never back to remote).
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
//! `preview_batch_local_lane_total` currently have an item claimed. The remote lane
//! stays singular (zero or one dispatcher), so `preview_batch_remote_title` remains a
//! meaningful current-item title.

mod engine;
mod scan;
mod wiring;

pub use engine::{RI_MATCH_TOLERANCE, seeded_random_unit, target_ri_for_design};
pub use wiring::{offer_batch_confirmation, setup_preview_batch_callbacks};
