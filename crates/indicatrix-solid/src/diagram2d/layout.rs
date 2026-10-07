//! Panel visibility predicates, the crown/pavilion/profile orthographic
//! projections, and the fixed pixel layout ([`PanelLayout`]) each panel draws
//! into -- see the parent module's doc comment for the panel conventions this
//! implements.

use super::{NORMAL_EPS, PanelKind};
use glam::{DVec3, Vec3};
use indicatrix::geometry::stone_metrics::SolidMesh;

/// A facet is drawn top-facing (crown) when its normal points enough toward `+Y`
/// to not be considered edge-on. See the parent module's doc comment.
#[must_use]
pub fn crown_visible(normal: Vec3) -> bool {
    normal.y > NORMAL_EPS
}

/// The pavilion panel's mirror of [`crown_visible`].
#[must_use]
pub fn pavilion_visible(normal: Vec3) -> bool {
    normal.y < -NORMAL_EPS
}

/// A facet contributes to the profile (side elevation) panel when it has ANY
/// horizontal component to its normal -- i.e. it is not purely crown/pavilion
/// facing. See the parent module's doc comment for why this (rather than a single
/// backface-culled view direction) is what makes a profile view show the whole
/// stone's silhouette instead of just whichever single facet faces the camera.
#[must_use]
pub fn profile_visible(normal: Vec3) -> bool {
    normal.x.hypot(normal.z) > NORMAL_EPS
}

/// One panel's fixed layout in pixel space, computed once per [`super::render_diagram`]
/// call from the mesh's own extent so the drawing always fits regardless of the
/// design's physical scale.
///
/// Public through [`super::DiagramLayout`], so an embedding app can place hit tests
/// and drag handles on the very pixels the panel was drawn into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelLayout {
    /// Which panel this layout describes.
    pub kind: PanelKind,
    /// Horizontal panel centre, in pixels.
    pub center_x: f32,
    /// Vertical panel centre, in pixels.
    pub center_y: f32,
    /// World-units -> pixels scale factor for this panel.
    pub scale: f32,
    /// Radius (px) of the index-wheel tick ring (crown/pavilion only).
    pub wheel_radius_px: f32,
    /// Clip rectangle (inclusive) this panel is allowed to paint into, so a
    /// panel's fill/edges/labels never bleed into a neighboring column.
    pub clip: (i32, i32, i32, i32),
}

/// Converts a tooth index's azimuth into a unit screen-space direction
/// `(screen_right, screen_up)` for the index wheel and for the crown/pavilion
/// facet-body projections below -- see the parent module's doc comment for the
/// derivation. `mirrored` is `true` for the pavilion panel.
pub fn wheel_direction(phi: f32, mirrored: bool) -> (f32, f32) {
    let (sin_phi, cos_phi) = phi.sin_cos();
    let world_x = cos_phi;
    let world_z = sin_phi;
    let screen_right = if mirrored { -world_z } else { world_z };
    let screen_up = world_x;
    (screen_right, screen_up)
}

/// Projects a world point to `(screen_x, screen_y, depth)` for the crown panel
/// (looking down `-Y`). `depth` is smaller for a point nearer the camera (larger
/// `y`), matching `raster.rs`'s "smaller depth wins" convention.
pub const fn crown_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (p.x as f32, p.y as f32, p.z as f32);
    let screen_x = layout.center_x + world_z * layout.scale;
    let screen_y = layout.center_y - world_x * layout.scale;
    (screen_x, screen_y, -world_y)
}

/// The pavilion panel's mirror of [`crown_project`] (looking up `+Y`; see the
/// parent module's doc comment for why the horizontal axis flips).
pub const fn pavilion_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (p.x as f32, p.y as f32, p.z as f32);
    let screen_x = layout.center_x - world_z * layout.scale;
    let screen_y = layout.center_y - world_x * layout.scale;
    (screen_x, screen_y, world_y)
}

/// The profile (side elevation) panel: `world_x` horizontal, `world_y` (height)
/// vertical, `world_z` as depth (near wins) -- see the parent module's doc comment
/// for why [`profile_visible`] rather than a single dot-product cull decides what
/// reaches this projection.
pub const fn profile_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (p.x as f32, p.y as f32, p.z as f32);
    let screen_x = layout.center_x + world_x * layout.scale;
    let screen_y = layout.center_y - world_y * layout.scale;
    (screen_x, screen_y, -world_z)
}

/// Projects a world point with the projection of `layout.kind`: `(screen_x,
/// screen_y, depth)` in the frame's pixel space, exactly as the panel drew it.
#[must_use]
pub const fn project_point(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    match layout.kind {
        PanelKind::Crown => crown_project(p, layout),
        PanelKind::Pavilion => pavilion_project(p, layout),
        PanelKind::Profile => profile_project(p, layout),
    }
}

/// One flat normal per facet, indexed by `facet_id` -- identical in spirit to
/// `raster::SolidRasterizer::render`'s own local table.
pub fn facet_normals(mesh: &SolidMesh) -> Vec<Option<DVec3>> {
    let facet_count = mesh.rings.iter().map(|(id, _)| id + 1).max().unwrap_or(0);
    let mut facet_normal: Vec<Option<DVec3>> = vec![None; facet_count];
    for (&id, &normal) in mesh.facet_id.iter().zip(&mesh.normals) {
        if let Some(slot) = facet_normal.get_mut(id)
            && slot.is_none()
        {
            *slot = Some(normal);
        }
    }
    facet_normal
}

/// A mesh's own `(xz_radius, profile_radius)` -- the world-space radii
/// [`build_panel_layout`] scales the crown/pavilion and profile panels against,
/// shared between [`compute_layout`] (three columns) and
/// [`compute_single_panel_layout`] (one column spanning the whole frame).
fn mesh_radii(mesh: &SolidMesh) -> (f32, f32) {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut xz_radius = 1e-6f64;
    for p in &mesh.positions {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
        xz_radius = xz_radius.max(p.x.hypot(p.z));
    }
    let half_w = ((max_x - min_x) / 2.0).max(1e-6);
    let half_h = ((max_y - min_y) / 2.0).max(1e-6);
    (xz_radius as f32, half_w.max(half_h) as f32)
}

/// The column/row geometry every [`PanelLayout`] in a frame shares --
/// [`grid_metrics`] computes it once for `columns` equal-width columns spanning
/// the whole frame (`3` for [`compute_layout`], `1` for
/// [`compute_single_panel_layout`]).
struct GridMetrics {
    col_width: f32,
    panel_box: f32,
    center_y: f32,
}

fn grid_metrics(width: u32, height: u32, columns: u32) -> GridMetrics {
    let (width_f, height_f) = (width as f32, height as f32);
    let col_width = width_f / columns.max(1) as f32;
    let caption_height = (height_f * 0.06).clamp(10.0, 20.0);
    let avail_height = (height_f - caption_height).max(1.0);
    GridMetrics {
        col_width,
        panel_box: col_width.min(avail_height),
        center_y: caption_height + avail_height / 2.0,
    }
}

/// Builds one panel's [`PanelLayout`] at column `index` within `metrics`' grid.
fn build_panel_layout(
    metrics: &GridMetrics,
    index: usize,
    kind: PanelKind,
    world_radius: f32,
    body_frac: f32,
    height: u32,
) -> PanelLayout {
    let center_x = (index as f32).mul_add(metrics.col_width, metrics.col_width / 2.0);
    let wheel_radius_px = metrics.panel_box / 2.0 * 0.86;
    let body_radius_px = metrics.panel_box / 2.0 * body_frac;
    let scale = body_radius_px / world_radius.max(1e-6);
    let clip = (
        (index as f32 * metrics.col_width).floor() as i32,
        0,
        ((index as f32 + 1.0) * metrics.col_width).ceil() as i32 - 1,
        height as i32 - 1,
    );
    PanelLayout {
        kind,
        center_x,
        center_y: metrics.center_y,
        scale,
        wheel_radius_px,
        clip,
    }
}

/// Computes the three panels' fixed pixel layout from the mesh's own extent.
/// Falls back to a unit-scale layout for an empty mesh so a caller never has to
/// special-case "nothing to draw yet".
pub fn compute_layout(mesh: &SolidMesh, width: u32, height: u32) -> [PanelLayout; 3] {
    let (xz_radius, profile_radius) = mesh_radii(mesh);
    let metrics = grid_metrics(width, height, 3);
    [
        build_panel_layout(&metrics, 0, PanelKind::Crown, xz_radius, 0.62, height),
        build_panel_layout(&metrics, 1, PanelKind::Pavilion, xz_radius, 0.62, height),
        build_panel_layout(
            &metrics,
            2,
            PanelKind::Profile,
            profile_radius,
            0.85,
            height,
        ),
    ]
}

/// The single-panel counterpart to [`compute_layout`]: one column spanning
/// the WHOLE frame width instead of a third of it, for [`super::render_diagram_single_panel`]'s
/// "enlarge this panel" mode. Deliberately a fresh layout computation rather than
/// a crop of [`compute_layout`]'s output -- cropping would enlarge the panel's
/// PIXELS without changing its `scale`, leaving the facet body just as small
/// relative to the (now much bigger) available column as it was in the
/// three-column layout, defeating the point of "enlarge".
pub fn compute_single_panel_layout(
    mesh: &SolidMesh,
    width: u32,
    height: u32,
    kind: PanelKind,
) -> PanelLayout {
    let (xz_radius, profile_radius) = mesh_radii(mesh);
    let (world_radius, body_frac) = match kind {
        PanelKind::Crown | PanelKind::Pavilion => (xz_radius, 0.62),
        PanelKind::Profile => (profile_radius, 0.85),
    };
    let metrics = grid_metrics(width, height, 1);
    build_panel_layout(&metrics, 0, kind, world_radius, body_frac, height)
}
