//! The drawing half of a redraw: [`render_request`] resolves one
//! [`RedrawRequest`] (via [`resolve_request_state`]) and runs the mesh
//! build/rasterize/edges-layer/diagram passes, returning a [`RenderedFrame`].
//!
//! Moved from the desktop's `gui::solid_preview::preview_state::render`, minus the
//! Slint pixel-buffer conversions: the finished solid image stays in the
//! caller's `rasterizer` (and the "Both" layer in `edges_rasterizer`), the diagram
//! comes back as its own [`DiagramFrame`]. The desktop wraps this to build its
//! `PreviewFrame`; the web app pushes the same bytes into its own Slint images.

use super::{
    request::RedrawRequest,
    state::{DiagramMemory, WorkerMemory, dim_style, resolve_request_state},
    types::{CameraPose, FrameGeometry, PickBuffer, StoneGeometryBuf},
};
use crate::{
    diagram2d::{self, DiagramConfig, DiagramFrame},
    edges_layer::render_edges_layer,
    mesh_cache::{CachedMesh, MeshCache},
    raster::SolidRasterizer,
};
use indicatrix::{
    geometry::{meet_solver::SolvedTier, stone_metrics::SolidMesh},
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
};
use std::sync::Arc;

/// [`RenderedFrame::mesh_bounding_radius`]'s fallback before any arrangement has
/// ever closed.
///
/// Roughly a standard round brilliant's own half-width, so the very first frame's
/// distance clamp is already in a sensible range rather than `0.0` collapsing it.
pub const DEFAULT_MESH_BOUNDING_RADIUS: f64 = 1.5;

/// One finished redraw -- everything [`render_request`] produces besides the
/// pixels the caller's rasterizers now hold.
pub struct RenderedFrame {
    /// Whether the solid image shows a real (possibly held-over, dimmed) solid;
    /// `false` means a background-colored frame.
    pub has_solid: bool,
    /// A short reason when `!has_solid`, or the "Preview cannot be solved: ..." /
    /// "Unbounded: ..." banner for a held-over frame; empty otherwise.
    pub status: String,
    /// The planned masts (or the chained-forward ones); `None` when nothing solved.
    pub solved: Option<Vec<SolvedTier>>,
    /// `live_update::Freshness::Stale`: the drawn planes are the previous solve's.
    pub stale: bool,
    /// The solid raster's facet pick buffer.
    pub pick: PickBuffer,
    /// `true` when `edges_rasterizer` holds this frame's "Both" layer (view mode 2
    /// and a closed arrangement).
    pub has_edges: bool,
    /// View mode 3's diagram (three-panel, or one enlarged panel), built only when
    /// the arrangement closed.
    pub diagram: Option<DiagramFrame>,
    /// Facet id -> hover text for a Diagram-mode frame (`None` in other modes).
    pub diagram_hover_text: Option<Vec<String>>,
    /// Facet id -> owning tier for a Diagram-mode frame (`None` in other modes).
    pub diagram_facet_tier: Option<Vec<Option<usize>>>,
    /// The stone (planes plus concave tools) this frame was rendered from. Named
    /// `stone` rather than `geometry` because [`Self::geometry`] is the mesh-derived
    /// [`FrameGeometry`] the manipulation handles use.
    pub stone: StoneGeometryBuf,
    /// Facet id -> hover text, valid in every view mode.
    pub hover_text: Vec<String>,
    /// Facet id -> owning tier, valid in every view mode.
    pub facet_tier: Vec<Option<usize>>,
    /// The design generation this frame reflects.
    pub generation: u64,
    /// The shown solid's bounding radius, or [`DEFAULT_MESH_BOUNDING_RADIUS`].
    pub mesh_bounding_radius: f64,
    /// The geometry of the mesh this frame was actually drawn from (a fresh build, or
    /// the held-over `last_closed` one), with the camera pose and raster size used;
    /// `None` only when no mesh was ever built.
    pub geometry: Option<FrameGeometry>,
}

/// [`FrameGeometry`] for `cached`, drawn at `camera` / `size`.
fn frame_geometry(cached: &CachedMesh, camera: CameraPose, size: (u32, u32)) -> FrameGeometry {
    FrameGeometry {
        corner_points: Arc::clone(&cached.corner_points),
        facet_centroids: Arc::clone(&cached.facet_centroids),
        bounding_radius: cached.bounding_radius(),
        camera,
        size,
    }
}

/// View mode 3's diagram-building step: the diagram (three-panel, or the enlarged
/// panel) at `size` from the pipeline's `last_diagram` memory, or `None` when the
/// arrangement does not close. Camera-independent. Builds nothing in any other
/// view mode.
fn build_diagram(
    mesh_cache: &mut MeshCache,
    stone: &StoneGeometryBuf,
    size: (u32, u32),
    last_diagram: &DiagramMemory,
) -> Option<DiagramFrame> {
    mesh_cache.get_or_build_geometry(stone).map(|cached| {
        let config = DiagramConfig {
            width: size.0,
            height: size.1,
            gear_teeth: last_diagram.gear_teeth,
            gear_reference_angle: last_diagram.gear_reference_angle,
            symmetry_order: last_diagram.symmetry_order,
            mirror: last_diagram.mirror,
        };
        // The "enlarge this panel" mode: draw just that one panel filling the
        // whole frame instead of the ordinary three-column layout.
        last_diagram.enlarged_panel.map_or_else(
            || diagram2d::render_diagram(&cached.mesh, &config, &last_diagram.style),
            |panel| {
                diagram2d::render_diagram_single_panel(
                    &cached.mesh,
                    &config,
                    &last_diagram.style,
                    panel,
                )
            },
        )
    })
}

/// Renders one request against `mesh_cache`/`rasterizer`/`edges_rasterizer` plus
/// `memory` -- see [`WorkerMemory`]'s doc comment.
///
/// A `Reproject` request chains the
/// pipeline's own last-known masts forward as its `solved` rather than reporting
/// `None`, so orbiting never wipes a caller's `last_solved` cache.
///
/// Afterwards `rasterizer.color` holds the solid image (always sized to the
/// request, background-colored when nothing ever closed) and, when
/// [`RenderedFrame::has_edges`], `edges_rasterizer.color` the "Both" layer.
///
/// Returns `None` if an `UpdateFacetOverlay` arrives before the first real frame.
pub fn render_request(
    mesh_cache: &mut MeshCache,
    rasterizer: &mut SolidRasterizer,
    edges_rasterizer: &mut SolidRasterizer,
    memory: &mut WorkerMemory,
    request: RedrawRequest,
) -> Option<RenderedFrame> {
    let (stone, camera_pose, size, view_mode, style, solved, stale, unsolvable_status, generation) =
        resolve_request_state(memory, mesh_cache, request)?;
    // The app-owned outlines (slice tier, drag followers) are read NOW, not taken from
    // the request: a `Planned` request rebuilds the style without them and the gate
    // drops a superseded overlay update -- see `Outlines`.
    let style = memory.with_outlines(style);

    rasterizer.resize(size.0, size.1);
    let camera = Camera::new(
        camera_pose.yaw,
        camera_pose.pitch,
        camera_pose.distance,
        DEFAULT_FOV_DEG,
    );
    let (has_solid, status) = if let Some(cached) = mesh_cache.get_or_build_geometry(&stone) {
        rasterizer.render_prepared(cached, &camera, &style);
        (true, String::new())
    } else if let Some(cached) = mesh_cache.last_closed() {
        // This frame's arrangement doesn't close, but a real solid was built
        // before. Show that, dimmed, rather than blanking the viewport.
        let dimmed = dim_style(style.clone());
        rasterizer.render_prepared(cached, &camera, &dimmed);
        (true, mesh_cache.status_message())
    } else {
        // Never built a closed solid at all yet: still produce a real,
        // correctly-sized (background-colored) image -- the viewport must never
        // go blank/stale -- plus the reason, for the caller's status banner.
        rasterizer.render(&SolidMesh::default(), &camera, &style);
        (false, mesh_cache.status_message())
    };
    // An unsolvable/unbounded override always wins the status banner -- it names
    // the actual reason the CURRENT edit can't be shown.
    let status = unsolvable_status.unwrap_or(status);
    let pick = PickBuffer {
        width: rasterizer.width,
        height: rasterizer.height,
        pick: rasterizer.pick.clone(),
    };

    // "Both" mode: the transparent-fill/opaque-edges layer, composited by the
    // caller over the path-traced image -- only built when asked for.
    let has_edges = view_mode == 2
        && mesh_cache
            .get_or_build_geometry(&stone)
            .is_some_and(|cached| {
                edges_rasterizer.resize(size.0, size.1);
                render_edges_layer(edges_rasterizer, cached, &camera, &style);
                true
            });

    let (diagram, diagram_hover_text, diagram_facet_tier) = if view_mode == 3 {
        (
            build_diagram(mesh_cache, &stone, size, &memory.diagram),
            Some(memory.diagram.hover_text.clone()),
            Some(memory.diagram.facet_tier.clone()),
        )
    } else {
        (None, None, None)
    };
    // The Solid view's own hover/tier tables -- the SAME ones the diagram's
    // carry, handed out unconditionally since every view mode's facet ids come
    // from the same `FacetMap`.
    let hover_text = memory.diagram.hover_text.clone();
    let facet_tier = memory.diagram.facet_tier.clone();

    // Mirrors the `get_or_build(..).or_else(last_closed)` fallback chain the image
    // was rendered from, so the distance clamp (and the manipulation handles) always
    // describe the SAME solid the viewport is showing. Each `.map(..)` turns the
    // borrow into an owned value before `last_closed` reborrows the cache.
    let geometry = mesh_cache
        .get_or_build_geometry(&stone)
        .map(|cached| frame_geometry(cached, camera_pose, size))
        .or_else(|| {
            mesh_cache
                .last_closed()
                .map(|cached| frame_geometry(cached, camera_pose, size))
        });
    let mesh_bounding_radius = geometry
        .as_ref()
        .map_or(DEFAULT_MESH_BOUNDING_RADIUS, |shown| shown.bounding_radius);

    Some(RenderedFrame {
        has_solid,
        status,
        solved,
        stale,
        pick,
        has_edges,
        diagram,
        diagram_hover_text,
        diagram_facet_tier,
        stone,
        hover_text,
        facet_tier,
        generation,
        mesh_bounding_radius,
        geometry,
    })
}
