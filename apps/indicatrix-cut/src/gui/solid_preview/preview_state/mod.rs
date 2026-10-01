//! The editor-side controller for the solid preview.
//!
//! Owns a [`mesh_cache::MeshCache`] and a [`raster::SolidRasterizer`] on one
//! dedicated worker thread, and coalesces a burst of redraw requests (an orbit
//! drag, a fast-typed edit) down to whichever was submitted LAST, via the same
//! [`RedrawGate`] contract `bridge::render_thread::frame_helpers::push_frame_to_ui`
//! already relies on for the path-traced view.
//!
//! # Threading
//!
//! [`SolidPreviewState::request_redraw`]/[`SolidPreviewState::request_replan`] must be
//! called from the UI thread and return immediately: they only submit into the
//! [`RedrawGate`] and, if this call won the race to be the one that must act, wake
//! the worker thread over a plain [`std::sync::mpsc`] channel. The worker thread
//! (spawned lazily on the first call) owns the [`mesh_cache::MeshCache`] and
//! [`raster::SolidRasterizer`] -- neither is `Sync`, and there is no reason for
//! either to be: exactly one thread ever touches them. Once a frame is ready, the
//! worker calls [`PreviewSink::apply`], whose only real (Slint-backed)
//! implementation hops back to the UI thread via `slint::Weak::upgrade_in_event_loop`
//! to do the actual image swap -- that hop lives on the trait implementation's
//! side, not in this module.
//!
//! # Planning happens on a WORKER thread, never the UI thread
//!
//! `resolve_dirty` was measured at 1.89s for a single-tier edit on the 103-tier
//! "CrackOtto-Step" design, and calling it on the UI thread would freeze the editor
//! that long on every keystroke. [`request::ReplanRequest`] carries everything
//! [`super::live_update::plan_preview`] needs (a `Design` clone, dirty tier set,
//! previous solve, camera/size/selection/RI/view-mode). The
//! UI thread never blocks on any of that; [`PreviewSink::apply`] hands the result
//! back asynchronously.
//!
//! # Two workers: planning vs. rendering
//!
//! Splitting `solve`/`resolve_dirty` and mesh-build/rasterize across separate
//! threads prevents `Reproject` requests from stalling behind slow solves.
//!
//! - The **PLAN worker** ([`plan_worker`]) is the ONLY
//!   thread that ever calls [`plan_worker::build_planned_frame`]/`live_update::plan_preview` --
//!   the expensive half. It has its own [`RedrawGate`] (`SolidPreviewState::
//!   plan_gate`), which coalesces a burst of edits to "whichever solve is
//!   currently running, then the latest one queued behind it" as it always did,
//!   except now that queue is entirely separate from the render worker's own.
//! - The **RENDER worker** ([`render`]) still owns the
//!   [`mesh_cache::MeshCache`]/[`raster::SolidRasterizer`] pair and is the only
//!   thread that ever touches them (neither is `Sync`) -- it now handles THREE
//!   request kinds instead of two: [`request::RedrawRequest::Reproject`],
//!   [`request::RedrawRequest::UpdateFacetOverlay`], and the plan worker's own finished
//!   [`request::RedrawRequest::Planned`] frame. None of the three ever calls
//!   `plan_preview`/`Design::solve`, so this worker's own queue is never blocked
//!   by a slow solve -- a camera drag submitted mid-solve renders against the
//!   last-known planes immediately, exactly like an ordinary `Reproject` always
//!   has.
//!
//! The mesh check for "Unbounded" status is split: [`plan_worker::build_planned_frame`]
//! (PLAN worker) does everything else; [`state::resolve_planned_state`] (RENDER worker)
//! finishes once it knows whether the arrangement closes, reusing the exact same
//! `mesh_cache.get_or_build` call immediately after -- guaranteed cache hit.
//!
//! The PLAN worker hands a finished [`request::PlannedFrame`] to the RENDER worker through
//! [`SolidPreviewState::submit`] -- the SAME lazy-spawn-and-wake path a
//! `Reproject`/`UpdateFacetOverlay` call already uses -- via a [`std::sync::Weak`] handle
//! back to this same `SolidPreviewState` ([`SolidPreviewState::self_weak`]),
//! rather than duplicating that spawn-on-first-use contract a second time.
//!
//! # Where `last_solved` lives
//!
//! `EditorState` is `Rc<RefCell<..>>`, which is why it cannot be reached from
//! [`PreviewSink::apply`]'s WORKER-thread call, and `PreviewSink` itself must stay
//! `Send + Sync` -- an `Rc`/`RefCell` cannot be a field of a type with that bound.
//! So the round trip goes through the wiring layer instead: [`PreviewSink::apply`]
//! is handed the freshly solved [`super::live_update::PreviewPlan::solved`] (`Send`
//! plain data), and `gui::mod`'s real [`PreviewSink`] stashes it into a plain
//! `Arc<Mutex<Option<Vec<SolvedTier>>>>` shared with `gui::editor`'s callbacks,
//! which read it back out building the next [`request::ReplanRequest`].
//!
//! # `PreviewSink`: kept generic over Slint on purpose
//!
//! [`SolidPreviewState`] is built against the [`PreviewSink`] trait rather than a
//! `slint::Weak<MainWindow>` directly, so a fake, synchronous sink can drive this
//! module's tests with no Slint window (see `tests::FakeSink`).

//!
//! # The pipeline itself lives in `indicatrix_solid::preview`
//!
//! The planner (`build_planned_frame`), the request types, the RENDER worker's
//! state machine (`WorkerMemory`/`resolve_request_state`) and its draw step
//! (`render_request`) moved to the shared crate so the web app runs the very same
//! functions on its main thread; this module keeps the threads, the gates and the
//! Slint pixel-buffer conversion, and re-exports the moved items at their old
//! paths. `tests::pins` holds the byte-identity pins recorded before the move.

#[cfg(test)]
use super::facet_map::FacetMap;
use super::{
    live_update, mesh_cache::MeshCache, raster::SolidRasterizer, to_diagram_pixel_buffer,
    to_pixel_buffer,
};

use crate::bridge::render_thread::RedrawGate;

mod controller;
mod plan_worker;
mod render;
mod request;
mod sink;
mod state;
#[cfg(test)]
mod tests;
mod types;

pub use controller::SolidPreviewState;
pub use request::ReplanRequest;
pub use sink::{DEFAULT_MESH_BOUNDING_RADIUS, PreviewFrame, PreviewSink};
pub use types::{
    CameraPose, FacetOverlay, FrameGeometry, PickBuffer, SolidLastSolved, SolidPickState,
};
