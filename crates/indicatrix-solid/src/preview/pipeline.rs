//! [`PreviewPipeline`]: the frame pipeline run synchronously on one thread (the
//! web app's main thread). It owns exactly what the desktop's RENDER worker owns
//! for the process lifetime -- a [`MeshCache`], the solid and edges
//! [`SolidRasterizer`]s and the [`WorkerMemory`] -- and drives the SAME
//! [`build_planned_frame`]/[`render_request`] functions the desktop's PLAN and
//! RENDER workers call, so the two apps produce the same bytes for the same
//! request (see this module's identity test).

use super::{
    plan::build_planned_frame,
    render::{RenderedFrame, render_request},
    request::{PlanJob, PlannedFrame, RedrawRequest},
    state::WorkerMemory,
    types::{CameraPose, FacetOverlay},
};
use crate::{live_update::Clock, mesh_cache::MeshCache, raster::SolidRasterizer};
use std::time::Duration;

/// The single-threaded frame pipeline -- see the module doc comment.
pub struct PreviewPipeline {
    mesh_cache: MeshCache,
    rasterizer: SolidRasterizer,
    edges_rasterizer: SolidRasterizer,
    memory: WorkerMemory,
}

impl Default for PreviewPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewPipeline {
    /// An empty pipeline: nothing planned, 1 x 1 rasterizers (the desktop worker's
    /// own starting state).
    #[must_use]
    pub fn new() -> Self {
        Self {
            mesh_cache: MeshCache::default(),
            rasterizer: SolidRasterizer::new(1, 1),
            edges_rasterizer: SolidRasterizer::new(1, 1),
            memory: WorkerMemory::default(),
        }
    }

    /// Plans `job` ([`build_planned_frame`] with `budget` and `clock`) without
    /// drawing it, so a caller can inspect [`PlannedFrame::unsolvable_status`]/
    /// [`PlannedFrame::solved`] before handing it to [`Self::render_planned`].
    #[must_use]
    pub fn plan(job: PlanJob, budget: Duration, clock: &dyn Clock) -> PlannedFrame {
        build_planned_frame(job, budget, clock)
    }

    /// Draws a planned frame (the desktop's `RedrawRequest::Planned`).
    pub fn render_planned(&mut self, planned: PlannedFrame) -> Option<RenderedFrame> {
        self.render(RedrawRequest::Planned(Box::new(planned)))
    }

    /// [`Self::plan`] then [`Self::render_planned`].
    pub fn replan(
        &mut self,
        job: PlanJob,
        budget: Duration,
        clock: &dyn Clock,
    ) -> Option<RenderedFrame> {
        let planned = Self::plan(job, budget, clock);
        self.render_planned(planned)
    }

    /// Re-renders the last planned (or reprojected) planes at a new camera pose,
    /// size or view mode (the desktop's `RedrawRequest::Reproject`). `None` before
    /// anything was planned.
    pub fn reproject(
        &mut self,
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
        gear: Option<(u32, f32)>,
    ) -> Option<RenderedFrame> {
        let geometry = self.memory.geometry()?;
        self.render(RedrawRequest::Reproject {
            geometry,
            camera,
            size,
            view_mode,
            gear,
        })
    }

    /// Re-renders with a new hover/click/multi-select highlight (the desktop's
    /// `RedrawRequest::UpdateFacetOverlay`). `None` before the first frame.
    pub fn update_overlay(&mut self, overlay: FacetOverlay) -> Option<RenderedFrame> {
        self.render(RedrawRequest::UpdateFacetOverlay(overlay))
    }

    /// Runs any request through [`render_request`].
    pub fn render(&mut self, request: RedrawRequest) -> Option<RenderedFrame> {
        render_request(
            &mut self.mesh_cache,
            &mut self.rasterizer,
            &mut self.edges_rasterizer,
            &mut self.memory,
            request,
        )
    }

    /// The last frame's solid image: RGBA8, row-major, `size()` pixels.
    #[must_use]
    pub fn solid_rgba(&self) -> &[u8] {
        &self.rasterizer.color
    }

    /// The last frame's solid image size in pixels.
    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        (self.rasterizer.width, self.rasterizer.height)
    }

    /// The last "Both" edges layer: RGBA8, only meaningful after a frame with
    /// [`RenderedFrame::has_edges`].
    #[must_use]
    pub fn edges_rgba(&self) -> &[u8] {
        &self.edges_rasterizer.color
    }

    /// What the pipeline remembers between requests (the last planes, pose,
    /// size, generation and styles).
    #[must_use]
    pub const fn memory(&self) -> &WorkerMemory {
        &self.memory
    }
}
