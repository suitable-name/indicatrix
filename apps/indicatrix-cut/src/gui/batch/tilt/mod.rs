//! Background batch computation of the catalogue's tilt-performance curves --
//! orchestration this batch shares in shape with `gui::batch::preview` (two-level
//! progress, cooperative cancellation, per-design panic isolation,
//! `RenderContext::export_active` viewport pause), but computing `indicatrix_vault::
//! model::tilt_curves::TiltPerformanceCurves` instead of front/top preview PNGs.
//!
//! Kept separate from `gui::batch::preview` rather than folding in: the unit of work
//! differs (one design's whole 4-axis sweep here, always one item, vs. two
//! independently-dispatchable views there), and the Slint dialogs are separate
//! components (`tilt_batch_dialog.slint` mirrors `preview_batch_dialog.slint` rather
//! than generalizing it). Both batches do share the concurrent local/remote dispatch
//! mechanism -- see `gui::batch::batch_queue` -- and this module calls into
//! `gui::batch::preview`'s `RI_MATCH_TOLERANCE`/`target_ri_for_design`/
//! `seeded_random_unit` rather than re-deriving them.
//!
//! `evaluate_full_axis_profile_at_azimuth` measures ~340ms/axis x4 = ~1.36s/design
//! single-threaded, roughly 72 minutes for the real ~3,187-design catalogue on one
//! core -- an order of magnitude longer than the preview batch's own ~2h for the whole
//! catalogue at 16 cores, so remote distribution matters more here.
//!
//! Lane distribution follows `gui::batch::batch_queue`'s shared-`WorkQueue` design
//! exactly as `gui::batch::preview` does (see that module's doc comment for the
//! `LiveComputeTarget`/`RemoteOnly`/`Both` mechanics); the queue's unit here is one
//! whole design, since a design's sweep is otherwise single-threaded
//! (`evaluate_full_axis_profile_at_azimuth` doesn't parallelise), so running N local
//! lanes -- rather than splitting work within one design's 4 axes -- is what actually
//! uses more than one core across the run.
//!
//! Cancellation is checked before each design any lane claims, and (local path only)
//! between each design's 4 axis sweeps inside
//! [`engine::compute_tilt_curves_locally`]'s loop. A `catch_unwind` wraps the whole
//! per-design attempt on every lane, so one bad design becomes a `failed` count rather
//! than a dead lane. Unlike preview images, a design's tilt curves are ALL-OR-NOTHING --
//! `TiltPerformanceCurves` requires all 4 axes to construct at all -- so a cancel
//! observed mid-design abandons that one design without saving anything for it; designs
//! that finished before the cancel stay saved.
//!
//! `TILT_CURVES` is a single blocking request/response (unlike `RENDER`'s progressive
//! `StreamEvent` stream), so a remote dispatch checks `cancel` once before sending and
//! then blocks for the whole round trip; `tilt_batch_remote_title` can only ever show
//! which design is in flight, never a sub-progress fraction.
//!
//! Progress is reported as a COMPLETED COUNT (`tilt_batch_design_index`), not a current
//! index, since local and remote lanes may each be mid-design simultaneously. With N
//! local lanes, no single title/axis-index can describe them, so
//! `tilt_batch_local_active` reports how many of `tilt_batch_local_lane_total` currently
//! have a design claimed. The remote lane stays singular, so `tilt_batch_remote_title`
//! remains meaningful.
//!
//! Split into [`scan`] (manually-triggered missing-curves scan), [`engine`] (per-design
//! resolve/compute/dispatch and the local/remote lane runners), and [`wiring`] (public
//! `spawn_tilt_batch`/`setup_tilt_batch_callbacks` UI glue) -- the same three-way seam
//! `gui::batch::preview` uses.

mod engine;
mod scan;
mod wiring;

pub use wiring::setup_tilt_batch_callbacks;
