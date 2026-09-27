//! The RENDER worker's own per-request draw call: [`render_diagram`]'s style
//! resolution via [`super::state::resolve_request_state`], the mesh
//! build/rasterize/edges-layer/diagram passes, and the [`super::SolidPreviewState`]
//! methods that spawn and feed this worker thread.

use super::{
    CachedMesh, DiagramConfig, MeshCache, SolidMesh, SolidRasterizer,
    controller::SolidPreviewState,
    diagram2d, render_edges_layer,
    request::RedrawRequest,
    sink::PreviewFrame,
    state::{DiagramMemory, WorkerMemory, resolve_request_state},
    to_diagram_pixel_buffer, to_pixel_buffer,
    types::PickBuffer,
};
use indicatrix::optics::raytracer::Camera;
use std::sync::{
    Arc, PoisonError,
    mpsc::{self, Sender},
};

/// `super::render::build_diagram_outputs`'s return: the diagram image, whether it
/// was actually built, its own facet pick buffer, the index-wheel's own tooth
/// pick buffer, and the facet-id-indexed hover-text/tier tables -- see
/// `super::sink::PreviewFrame`'s matching fields for what each means.
type DiagramOutputs = (
    Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    bool,
    Option<PickBuffer>,
    Option<PickBuffer>,
    Option<Vec<String>>,
    Option<Vec<Option<usize>>>,
);

/// View mode 3's whole diagram-building step, split out of [`render_request`]
/// purely to keep that function short.
///
/// Camera-independent (never reads a camera pose) so a `RedrawRequest::
/// Reproject` request (an orbit drag on the OTHER viewport, which still
/// re-issues the current `view_mode`) simply rebuilds the diagram at the current
/// size from the worker's own `last_diagram` memory -- see `super::state::
/// DiagramMemory`'s doc comment. Builds nothing in any other view mode, matching
/// `edges_image`'s own "only when asked for" contract in [`render_request`].
fn build_diagram_outputs(
    mesh_cache: &mut MeshCache,
    planes: &[(glam::Vec3, f32)],
    size: (u32, u32),
    view_mode: u8,
    last_diagram: &DiagramMemory,
) -> DiagramOutputs {
    if view_mode != 3 {
        return (None, false, None, None, None, None);
    }
    let (image, has_diagram, pick, tooth_pick) =
        mesh_cache
            .get_or_build(planes)
            .map_or((None, false, None, None), |cached| {
                let config = DiagramConfig {
                    width: size.0,
                    height: size.1,
                    gear_teeth: last_diagram.gear_teeth,
                    gear_reference_angle: last_diagram.gear_reference_angle,
                    symmetry_order: last_diagram.symmetry_order,
                    mirror: last_diagram.mirror,
                };
                // The "enlarge this panel" mode: draw just that one panel filling
                // the whole frame instead of the ordinary three-column layout --
                // see `DiagramMemory::enlarged_panel`'s own doc comment.
                let diagram_frame = last_diagram.enlarged_panel.map_or_else(
                    || diagram2d::render_diagram(&cached.mesh, &config, &last_diagram.style),
                    |panel| {
                        diagram2d::render_diagram_single_panel(
                            &cached.mesh,
                            &config,
                            &last_diagram.style,
                            panel,
                        )
                    },
                );
                let image = to_diagram_pixel_buffer(&diagram_frame);
                // The index wheel's own tooth pick buffer, threaded through
                // exactly like `pick` (the facet buffer) above -- both are the SAME
                // `+1`/`0`-encoded shape (`DiagramFrame::tooth`'s own doc comment),
                // so `diagram_wiring`'s hover/click callbacks can query "which
                // facet" and "which tooth" from the same `(x, y)` with no new
                // buffer type.
                let tooth_pick = PickBuffer {
                    width: diagram_frame.width,
                    height: diagram_frame.height,
                    pick: diagram_frame.tooth,
                };
                let pick = PickBuffer {
                    width: diagram_frame.width,
                    height: diagram_frame.height,
                    pick: diagram_frame.pick,
                };
                (Some(image), true, Some(pick), Some(tooth_pick))
            });
    (
        image,
        has_diagram,
        pick,
        tooth_pick,
        Some(last_diagram.hover_text.clone()),
        Some(last_diagram.facet_tier.clone()),
    )
}

/// Renders one request against `mesh_cache`/`rasterizer`/`edges_rasterizer`, all
/// three owned by the worker thread for the process lifetime, plus `memory` --
/// see [`WorkerMemory`]'s doc comment. `memory.solved_masts` matters most: a
/// `Reproject` request (every camera drag/zoom/pose button, and every
/// `Both`/`Diagram` redraw that isn't a fresh edit) chains the worker's own
/// last-known masts forward as its `solved` rather than reporting `None` --
/// `SlintSolidSink::apply` stores whatever `solved` it is handed with no
/// `Some`-check, so simply orbiting the stone would otherwise wipe the shared
/// `last_solved` cache the NEXT edit needs for a cheap subgraph `resolve_dirty`,
/// silently downgrading it to a full `Design::solve()` (also zeroing every mast a
/// same-normal-direction facet lookup depends on).
///
/// Returns `None` if an `UpdateFacetOverlay` arrives before the first real frame
/// (nothing to redraw, so no frame is returned).
pub fn render_request(
    mesh_cache: &mut MeshCache,
    rasterizer: &mut SolidRasterizer,
    edges_rasterizer: &mut SolidRasterizer,
    memory: &mut WorkerMemory,
    request: RedrawRequest,
) -> Option<PreviewFrame> {
    let (planes, camera_pose, size, view_mode, style, solved, stale, unsolvable_status, generation) =
        resolve_request_state(memory, mesh_cache, request)?;

    rasterizer.resize(size.0, size.1);
    let camera = Camera::new(
        camera_pose.yaw,
        camera_pose.pitch,
        camera_pose.distance,
        42.0,
    );
    let (image, has_solid, status) = if let Some(cached) = mesh_cache.get_or_build(&planes) {
        rasterizer.render_prepared(cached, &camera, &style);
        (to_pixel_buffer(rasterizer), true, String::new())
    } else if let Some(cached) = mesh_cache.last_closed() {
        // This frame's arrangement doesn't close, but a real solid was built
        // before. Show that, dimmed, rather than blanking the viewport.
        let dimmed = super::state::dim_style(style.clone());
        rasterizer.render_prepared(cached, &camera, &dimmed);
        (
            to_pixel_buffer(rasterizer),
            true,
            mesh_cache.status_message(),
        )
    } else {
        // Never built a closed solid at all yet: still produce a real,
        // correctly-sized (background-colored) image -- the viewport must never
        // go blank/stale -- plus the reason, for the caller's status banner.
        rasterizer.render(&SolidMesh::default(), &camera, &style);
        (
            to_pixel_buffer(rasterizer),
            false,
            mesh_cache.status_message(),
        )
    };
    // A `preview_status` override (an `Unsolvable` replan, or `resolve_planned_state`'s
    // tier-mapped `Unbounded` text) always wins the status banner -- it names the
    // actual reason the CURRENT edit can't be shown, which matters whether or not
    // the held-over solid happens to still be closed (`has_solid` here reflects the
    // OLD/last-good planes, not necessarily this edit's own outcome).
    let status = unsolvable_status.unwrap_or(status);
    let pick = PickBuffer {
        width: rasterizer.width,
        height: rasterizer.height,
        pick: rasterizer.pick.clone(),
    };

    // "Both" mode: the transparent-fill/opaque-edges layer, composited by the Slint
    // side over the path-traced image -- only built when the toggle asks for it.
    let edges_image = if view_mode == 2 {
        mesh_cache.get_or_build(&planes).map(|cached| {
            edges_rasterizer.resize(size.0, size.1);
            render_edges_layer(edges_rasterizer, cached, &camera, &style);
            to_pixel_buffer(edges_rasterizer)
        })
    } else {
        None
    };

    let (
        diagram_image,
        has_diagram,
        diagram_pick,
        diagram_tooth_pick,
        diagram_hover_text,
        diagram_facet_tier,
    ) = build_diagram_outputs(mesh_cache, &planes, size, view_mode, &memory.diagram);
    // The Solid view's own hover/tier tables -- the SAME ones `diagram_hover_text`/
    // `diagram_facet_tier` above carry, just handed out unconditionally (not only
    // for a `view_mode == 3` request) since every view mode's facet ids come from
    // the same `FacetMap`. See `super::sink::PreviewFrame::hover_text`'s doc comment.
    let hover_text = memory.diagram.hover_text.clone();
    let facet_tier = memory.diagram.facet_tier.clone();

    // Mirrors the `mesh_cache.get_or_build(&planes).or_else(last_closed)`
    // fallback chain the image itself was rendered from above, so the distance
    // clamp always describes the SAME solid the viewport is actually showing --
    // never the live (possibly not-yet-closing) arrangement when a dimmed
    // `last_closed` mesh is what's on screen instead.
    // Each `.map(CachedMesh::bounding_radius)` converts the borrowed `&CachedMesh`
    // into an OWNED `f64` before the expression ends, so `mesh_cache`'s mutable
    // borrow from `get_or_build` is released before `last_closed` reborrows it in
    // the `or_else` closure -- chaining the two `Option<&CachedMesh>` calls
    // directly (or via `if let`/`else if let`) does not compile here, since the
    // first borrow would still be considered live.
    let mesh_bounding_radius = mesh_cache
        .get_or_build(&planes)
        .map(CachedMesh::bounding_radius)
        .or_else(|| mesh_cache.last_closed().map(CachedMesh::bounding_radius))
        .unwrap_or(super::sink::DEFAULT_MESH_BOUNDING_RADIUS);

    Some(PreviewFrame {
        image,
        has_solid,
        status,
        solved,
        stale,
        pick,
        edges_image,
        diagram_image,
        has_diagram,
        diagram_pick,
        diagram_tooth_pick,
        diagram_hover_text,
        diagram_facet_tier,
        planes,
        hover_text,
        facet_tier,
        generation,
        mesh_bounding_radius,
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
        std::thread::spawn(move || {
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(1, 1);
            let mut edges_rasterizer = SolidRasterizer::new(1, 1);
            let mut memory = WorkerMemory::default();
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
