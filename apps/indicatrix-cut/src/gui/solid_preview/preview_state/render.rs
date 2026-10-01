//! The RENDER worker's own per-request draw call -- [`render_request`], a thin
//! wrapper over `indicatrix_solid::preview::render_request` (moved there, shared
//! with the web app) that turns its output into this desktop's Slint pixel
//! buffers -- and the [`super::SolidPreviewState`] methods that spawn and feed this
//! worker thread.

use super::{
    MeshCache, SolidRasterizer, controller::SolidPreviewState, request::RedrawRequest,
    sink::PreviewFrame, state::WorkerMemory, to_diagram_pixel_buffer, to_pixel_buffer,
    types::PickBuffer,
};
use std::sync::{
    Arc, PoisonError,
    mpsc::{self, Sender},
};

/// Renders one request against `mesh_cache`/`rasterizer`/`edges_rasterizer`, all
/// three owned by the worker thread for the process lifetime, plus `memory` --
/// see `indicatrix_solid::preview::render_request` (the moved body) and
/// `WorkerMemory`'s doc comment. `memory.solved_masts` matters most: a
/// `Reproject` request (every camera drag/zoom/pose button, and every
/// `Both`/`Diagram` redraw that isn't a fresh edit) chains the worker's own
/// last-known masts forward as its `solved` rather than reporting `None` --
/// `SlintSolidSink::apply` stores whatever `solved` it is handed with no
/// `Some`-check, so simply orbiting the stone would otherwise wipe the shared
/// `last_solved` cache the NEXT edit needs for a cheap subgraph `resolve_dirty`.
///
/// Copies the finished solid image (and, in "Both" mode, the edges layer) out of
/// the rasterizers and the diagram out of its frame into Slint pixel buffers.
///
/// Returns `None` if an `UpdateFacetOverlay` arrives before the first real frame
/// (nothing to redraw, so no frame is returned).
///
/// `planned` records whether the request was a `Planned` one -- see `PreviewFrame::planned`.
pub fn render_request(
    mesh_cache: &mut MeshCache,
    rasterizer: &mut SolidRasterizer,
    edges_rasterizer: &mut SolidRasterizer,
    memory: &mut WorkerMemory,
    request: RedrawRequest,
) -> Option<PreviewFrame> {
    let planned = matches!(request, RedrawRequest::Planned(_));
    let frame = indicatrix_solid::preview::render_request(
        mesh_cache,
        rasterizer,
        edges_rasterizer,
        memory,
        request,
    )?;
    let image = to_pixel_buffer(rasterizer);
    let edges_image = frame.has_edges.then(|| to_pixel_buffer(edges_rasterizer));
    let has_diagram = frame.diagram.is_some();
    let (diagram_image, diagram_pick, diagram_tooth_pick, diagram_panel_pick) =
        frame.diagram.map_or((None, None, None, None), |diagram| {
            let image = to_diagram_pixel_buffer(&diagram);
            // Which panel painted each pixel (a lookup yields 0/1/2 for Crown/
            // Pavilion/Profile), so a double-click can enlarge the panel under
            // the pointer -- see `diagram_wiring::setup_diagram_double_click_callback`.
            let panel_pick = PickBuffer {
                width: diagram.width,
                height: diagram.height,
                pick: diagram.panel_tags(),
            };
            // The index wheel's own tooth pick buffer, threaded through exactly
            // like the facet buffer -- both use the SAME `+1`/`0` encoding, so
            // `diagram_wiring`'s hover/click callbacks can query "which facet" and
            // "which tooth" from the same `(x, y)` with no new buffer type.
            let tooth_pick = PickBuffer {
                width: diagram.width,
                height: diagram.height,
                pick: diagram.tooth,
            };
            let pick = PickBuffer {
                width: diagram.width,
                height: diagram.height,
                pick: diagram.pick,
            };
            (Some(image), Some(pick), Some(tooth_pick), Some(panel_pick))
        });
    Some(PreviewFrame {
        image,
        has_solid: frame.has_solid,
        status: frame.status,
        solved: frame.solved,
        stale: frame.stale,
        pick: frame.pick,
        edges_image,
        diagram_image,
        has_diagram,
        diagram_pick,
        diagram_tooth_pick,
        diagram_panel_pick,
        diagram_hover_text: frame.diagram_hover_text,
        diagram_facet_tier: frame.diagram_facet_tier,
        planes: frame.planes,
        hover_text: frame.hover_text,
        facet_tier: frame.facet_tier,
        generation: frame.generation,
        planned,
        mesh_bounding_radius: frame.mesh_bounding_radius,
        geometry: frame.geometry,
    })
}

impl SolidPreviewState {
    /// Common submit path for the RENDER worker: lazily spawns it, then pushes
    /// `request` through its `RedrawGate` and wakes it if this call won the
    /// race. Also how `super::plan_worker`'s spawned worker hands off a finished
    /// `super::request::PlannedFrame` -- see that method's own doc comment.
    ///
    /// # Panics
    ///
    /// Never in practice: the `.expect(..)` below can only fail if another thread
    /// cleared `wake` between the check and this read, which never happens.
    pub(super) fn submit(&self, request: RedrawRequest) {
        let tx = {
            let mut guard = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.is_none() {
                *guard = Some(self.spawn_worker());
            }
            guard
                .clone()
                .expect("just initialized above if it was empty")
        };
        if self.gate.submit(request).is_some() {
            // A `send` failing here would mean the worker thread panicked. Not
            // fatal: the next submit call finds a dead channel and this call's
            // frame is silently skipped, rather than the UI thread panicking too.
            let _ = tx.send(());
        }
    }

    /// Spawns the RENDER worker thread. Called at most once, guarded by `wake`.
    /// Never calls `live_update::plan_preview` itself; every request it handles
    /// is cheap relative to a real solve.
    fn spawn_worker(&self) -> Sender<()> {
        let (tx, rx) = mpsc::channel::<()>();
        let sink = Arc::clone(&self.sink);
        let gate = Arc::clone(&self.gate);
        let outlines = Arc::clone(&self.outlines);
        std::thread::spawn(move || {
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(1, 1);
            let mut edges_rasterizer = SolidRasterizer::new(1, 1);
            // The provisional-slice and drag-follower outlines are read from the
            // controller's shared handle at draw time (`SolidPreviewState::outlines`).
            let mut memory = WorkerMemory {
                outlines: Some(outlines),
                ..WorkerMemory::default()
            };
            for () in rx {
                // May be newer than the request that caused this wake-up, if more
                // `submit` calls arrived while this thread was rendering the
                // previous one -- the coalescing this module exists for.
                let Some(request) = gate.take() else {
                    continue;
                };
                // `None` means an `UpdateFacetOverlay` arrived before the first
                // real frame; nothing is pushed to the sink.
                let Some(frame) = render_request(
                    &mut mesh_cache,
                    &mut rasterizer,
                    &mut edges_rasterizer,
                    &mut memory,
                    request,
                ) else {
                    continue;
                };
                sink.apply(frame);
            }
        });
        tx
    }
}
