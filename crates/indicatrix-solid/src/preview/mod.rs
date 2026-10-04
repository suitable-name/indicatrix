//! The Solid/Diagram frame pipeline both apps drive.
//!
//! Plan a design into a plane
//! arrangement and a facet style, then rasterize it (solid, "Both" edges layer,
//! or the 2D diagram) with hover/selection overlays.
//!
//! Everything here moved from the desktop's `gui::solid_preview::preview_state`
//! (types, requests, the planner, the request-resolution state machine and the
//! render step) or is a new pure helper around it, so the desktop's worker
//! threads and the web app's main thread run the very same functions:
//!
//! - [`build_planned_frame`] (the planner, around [`crate::live_update::plan_preview`],
//!   with an injected [`crate::live_update::Clock`]);
//! - [`render_request`] (resolve one [`RedrawRequest`] against [`WorkerMemory`],
//!   rasterize, build the diagram) returning a [`RenderedFrame`];
//! - [`PreviewPipeline`], the two run back to back on one thread (the web app);
//! - [`camera`]'s orbit/zoom/pose math and [`view`]'s hit testing, raster sizing,
//!   selection stepping and dirty-tier diff.
//!
//! The desktop keeps its threads, its `RedrawGate` coalescing and the Slint
//! pixel-buffer conversion, and re-exports the moved items at their old paths.
//! The identity pins recorded before the move (`preview_state::tests::pins` in the
//! desktop) are asserted again by this module's own tests through
//! [`PreviewPipeline`].

pub mod camera;
mod pipeline;
mod plan;
mod render;
mod request;
mod state;
#[cfg(test)]
mod tests;
mod types;
pub mod view;

pub use pipeline::PreviewPipeline;
pub use plan::build_planned_frame;
pub use render::{DEFAULT_MESH_BOUNDING_RADIUS, RenderedFrame, render_request};
pub use request::{PlanJob, PlannedFrame, RedrawRequest, panel_kind_from_index};
pub use state::{
    DiagramMemory, Outlines, RequestState, SharedOutlines, WorkerMemory, dim_style,
    escaping_tier_label, resolve_request_state,
};
pub use types::{CameraPose, FacetOverlay, FrameGeometry, PickBuffer, StoneGeometryBuf};
