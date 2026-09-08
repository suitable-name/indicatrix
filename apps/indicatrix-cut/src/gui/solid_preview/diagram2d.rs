//! A GemCAD-style 2D facet diagram: three orthographic panels laid out side by side.
//!
//! Crown, pavilion, and profile, each with a per-pixel facet-picking buffer so the
//! diagram mode reuses the same hover/click/selection plumbing the solid
//! rasterizer already has.
//!
//! Pure Rust, no Slint types anywhere in this file -- see `raster.rs`'s own doc
//! comment for why that separation matters; [`super::to_pixel_buffer`] stays the
//! only bridge into Slint types.
//!
//! # Panel conventions
//!
//! The stone's main axis is world `+Y` (see `facet_map.rs`'s candidate-normal
//! construction: a tier's elevation fixes `normal.y`, azimuth splits the rest
//! between `x`/`z`). Three fixed orthographic panels are drawn, none of them tied
//! to the orbit camera (a 2D facet diagram is not a perspective view of anything):
//!
//! - **Crown**: looking straight down `-Y` from above. Visible facets are those
//!   with `normal.y` above a small edge-on threshold (see [`crown_visible`]).
//! - **Pavilion**: looking straight up `+Y` from below. Visible facets have
//!   `normal.y` below the negated threshold (see [`pavilion_visible`]).
//! - **Profile**: a side elevation. Rather than a single backface-culled view
//!   direction (which would show only the one facet directly facing the camera),
//!   this shows every facet that is not purely crown/pavilion-facing -- any
//!   horizontal component to its normal at all (see [`profile_visible`]) --
//!   projected onto the `(world_x, world_y)` plane with `world_z` as depth (near
//!   wins). A facet edge-on to that projection (e.g. a cube's left/right faces)
//!   degenerates to a zero-area vertical line, which is exactly the silhouette
//!   edge an elevation drawing wants; a facet facing the viewer (e.g. a cube's
//!   front face) fills the panel and hides whatever is behind it. This reads as a
//!   real side elevation of the whole stone, not just its front face.
//!
//! ## Screen mapping and the index wheel's "index 0 at the top" convention
//!
//! [`StandardGemCuts::index_to_azimuth`](indicatrix::geometry::cuts::StandardGemCuts::index_to_azimuth)
//! documents `phi = 2*pi*(index + gear_reference_angle) / gear_teeth`, and a
//! facet's own outward horizontal direction is `(cos(phi), sin(phi))` in
//! `(world_x, world_z)` (see `facet_map.rs`'s candidate normal construction: at
//! `phi = 0` the horizontal part of the normal is pure `+X`). To put index `0` at
//! the top of the crown panel, this module maps world `+X` to screen "up" and
//! world `+Z` to screen "right": `screen_u (right) = world_z`, `screen_v (up) =
//! world_x`. Under that mapping, increasing `phi` sweeps top -> right -> bottom ->
//! left, i.e. **clockwise** (index 24 of 96 lands at 3 o'clock) -- see
//! [`wheel_direction`] and [`crown_project`].
//!
//! The pavilion panel is a view from the OPPOSITE side of the same horizontal
//! plane (looking up instead of down), which mirrors the horizontal screen axis
//! while keeping "up" (world `+X`) the same -- exactly like looking at a page from
//! behind. So the pavilion panel uses `screen_u = -world_z`, keeping index `0` at
//! the top but sweeping **counter-clockwise** (index 24 lands at 9 o'clock) -- see
//! [`pavilion_project`]. This is not an arbitrary stylistic mirror: a tier at a
//! given index has the IDENTICAL `(world_x, world_z)` horizontal direction whether
//! it is a crown or pavilion tier (only `normal.y`'s sign differs), so mirroring
//! the pavilion's screen axis is what makes an orthographic "view from below"
//! project those same physical points correctly rather than drawing an X-ray
//! view through the stone.
//!
//! The profile panel has no index wheel (a side elevation has no azimuth to mark).

use super::raster::simplify_ring;
use glam::{DVec3, Vec3};
use indicatrix::geometry::stone_metrics::SolidMesh;

/// Normal-component threshold below which a facet is treated as edge-on rather
/// than facing/away for a panel's visibility test -- see [`crown_visible`],
/// [`pavilion_visible`], [`profile_visible`].
const NORMAL_EPS: f32 = 1e-3;

/// Screen-space period, in pixels, of the diagonal hatch stripes over a flagged
/// (critical-angle-risk) facet -- mirrors `raster::HATCH_PERIOD`.
const HATCH_PERIOD: i32 = 6;

/// A facet's projected on-screen span (in either axis) must be at least this many
/// pixels before [`render_diagram`] bothers drawing its tier-name label.
const MIN_LABEL_SPAN: f32 = 22.0;

/// A facet is drawn top-facing (crown) when its normal points enough toward `+Y`
/// to not be considered edge-on. See this module's doc comment.
#[must_use]
fn crown_visible(normal: Vec3) -> bool {
    normal.y > NORMAL_EPS
}

/// The pavilion panel's mirror of [`crown_visible`].
#[must_use]
fn pavilion_visible(normal: Vec3) -> bool {
    normal.y < -NORMAL_EPS
}

/// A facet contributes to the profile (side elevation) panel when it has ANY
/// horizontal component to its normal -- i.e. it is not purely crown/pavilion
/// facing. See this module's doc comment for why this (rather than a single
/// backface-culled view direction) is what makes a profile view show the whole
/// stone's silhouette instead of just whichever single facet faces the camera.
#[must_use]
fn profile_visible(normal: Vec3) -> bool {
    normal.x.hypot(normal.z) > NORMAL_EPS
}

/// Which of the three fixed panels a pixel (or a facet) belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    Crown,
    Pavilion,
    Profile,
}

impl PanelKind {
    const fn tag(self) -> u8 {
        match self {
            Self::Crown => 1,
            Self::Pavilion => 2,
            Self::Profile => 3,
        }
    }

    const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Crown),
            2 => Some(Self::Pavilion),
            3 => Some(Self::Profile),
            _ => None,
        }
    }

    const fn has_index_wheel(self) -> bool {
        matches!(self, Self::Crown | Self::Pavilion)
    }

    const fn caption(self) -> &'static str {
        match self {
            Self::Crown => "CROWN",
            Self::Pavilion => "PAVILION",
            Self::Profile => "PROFILE",
        }
    }
}

/// What [`render_diagram`] needs beyond the mesh: the frame size and the index
/// wheel's tooth count/reference angle.
///
/// Mirrors `indicatrix_cut_core::design::tier::ScheduleMeta::gear_teeth_abs`/
/// `gear_reference_angle`, widened to the `f32` this module works in -- kept as
/// plain primitives here so this module stays independent of
/// `indicatrix-cut-core`, matching `mesh_cache`'s own boundary.
#[derive(Debug, Clone, Copy)]
pub struct DiagramConfig {
    pub width: u32,
    pub height: u32,
    pub gear_teeth: u32,
    pub gear_reference_angle: f32,
}

/// Everything [`render_diagram`] needs beyond the mesh/config to style and label
/// the diagram.
///
/// Mirrors `raster::SolidStyle`'s flagged/pending/selected overlay contract
/// (indexed by `facet_id`, a facet past the end of a vector is unset) so
/// `preview_state` can build both from the very same
/// `facet_map::FacetMap::overlay_flags` call.
#[derive(Debug, Clone)]
pub struct DiagramStyle {
    pub base_color: [u8; 3],
    pub edge_color: [u8; 3],
    pub pending_color: [u8; 3],
    pub hatch_color: [u8; 3],
    pub selected_color: [u8; 3],
    pub background: [u8; 4],
    pub flagged: Vec<bool>,
    pub pending: Vec<bool>,
    pub selected: Vec<bool>,
    /// Facet id -> short on-diagram label (`facet_map::FacetMap::facet_label`,
    /// typically the tier name); empty string suppresses the label.
    pub facet_labels: Vec<String>,
}

impl Default for DiagramStyle {
    fn default() -> Self {
        Self {
            base_color: [200, 205, 215],
            edge_color: [30, 32, 38],
            pending_color: [235, 170, 40],
            hatch_color: [90, 40, 40],
            selected_color: [70, 160, 235],
            background: [12, 14, 20, 255],
            flagged: Vec::new(),
            pending: Vec::new(),
            selected: Vec::new(),
            facet_labels: Vec::new(),
        }
    }
}

/// A finished three-panel 2D facet diagram: an RGBA8 pixel buffer plus a
/// per-pixel facet-picking buffer.
///
/// Exactly like [`super::raster::SolidRasterizer`] but orthographic and split
/// across three fixed panels instead of one perspective camera view.
#[derive(Debug, Clone)]
pub struct DiagramFrame {
    pub width: u32,
    pub height: u32,
    pub color: Vec<u8>,
    /// `facet_id + 1` per pixel, `0` meaning background/unpainted -- see
    /// [`Self::pick_at`]. `pub(crate)` so `preview_state` can lift it straight
    /// into a [`super::preview_state::PickBuffer`] without a public constructor.
    pub(crate) pick: Vec<u32>,
    /// Which panel a pixel's fill came from (0 = none) -- see [`Self::panel_at`].
    panel: Vec<u8>,
    /// View-space depth of the closest fill written to each pixel so far, within
    /// that pixel's own panel (panels never share a clip rect, so cross-panel
    /// depth comparisons never happen even though the depth CONVENTION differs
    /// per panel) -- `f32::INFINITY` where unpainted, mirroring
    /// `raster::SolidRasterizer::depth`.
    depth: Vec<f32>,
}

impl DiagramFrame {
    fn blank(width: u32, height: u32, background: [u8; 4]) -> Self {
        let n = (width as usize) * (height as usize);
        let mut color = vec![0u8; n * 4];
        let (pixels, _remainder) = color.as_chunks_mut::<4>();
        for px in pixels {
            px.copy_from_slice(&background);
        }
        Self {
            width,
            height,
            color,
            pick: vec![0u32; n],
            panel: vec![0u8; n],
            depth: vec![f32::INFINITY; n],
        }
    }

    /// Same contract as [`super::raster::SolidRasterizer::pick_at`].
    #[must_use]
    pub fn pick_at(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let v = self.pick[(y * self.width + x) as usize];
        (v != 0).then(|| v - 1)
    }

    /// Which panel (crown/pavilion/profile) painted pixel `(x, y)`, or `None` for
    /// background/out-of-bounds. A click on the pavilion panel and a click on the
    /// crown panel can never be confused with each other even though both may
    /// paint the same `facet_id` space -- this is what lets a caller show
    /// panel-aware feedback if it ever wants to, though [`Self::pick_at`] alone
    /// already resolves to the correct globally-unique facet id regardless.
    #[must_use]
    pub fn panel_at(&self, x: u32, y: u32) -> Option<PanelKind> {
        if x >= self.width || y >= self.height {
            return None;
        }
        PanelKind::from_tag(self.panel[(y * self.width + x) as usize])
    }

    const fn idx(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return None;
        }
        Some((y as u32 * self.width + x as u32) as usize)
    }
}

/// One panel's fixed layout in pixel space, computed once per [`render_diagram`]
/// call from the mesh's own extent so the drawing always fits regardless of the
/// design's physical scale.
struct PanelLayout {
    kind: PanelKind,
    center_x: f32,
    center_y: f32,
    /// World-units -> pixels scale factor for this panel.
    scale: f32,
    /// Radius (px) of the index-wheel tick ring (crown/pavilion only).
    wheel_radius_px: f32,
    /// Clip rectangle (inclusive) this panel is allowed to paint into, so a
    /// panel's fill/edges/labels never bleed into a neighboring column.
    clip: (i32, i32, i32, i32),
}

/// Converts a tooth index's azimuth into a unit screen-space direction
/// `(screen_right, screen_up)` for the index wheel and for the crown/pavilion
/// facet-body projections below -- see this module's doc comment for the
/// derivation. `mirrored` is `true` for the pavilion panel.
fn wheel_direction(phi: f32, mirrored: bool) -> (f32, f32) {
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
const fn crown_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (p.x as f32, p.y as f32, p.z as f32);
    let screen_x = layout.center_x + world_z * layout.scale;
    let screen_y = layout.center_y - world_x * layout.scale;
    (screen_x, screen_y, -world_y)
}

/// The pavilion panel's mirror of [`crown_project`] (looking up `+Y`; see this
/// module's doc comment for why the horizontal axis flips).
const fn pavilion_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (p.x as f32, p.y as f32, p.z as f32);
    let screen_x = layout.center_x - world_z * layout.scale;
    let screen_y = layout.center_y - world_x * layout.scale;
    (screen_x, screen_y, world_y)
}

/// The profile (side elevation) panel: `world_x` horizontal, `world_y` (height)
/// vertical, `world_z` as depth (near wins) -- see this module's doc comment for
/// why [`profile_visible`] rather than a single dot-product cull decides what
/// reaches this projection.
const fn profile_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (p.x as f32, p.y as f32, p.z as f32);
    let screen_x = layout.center_x + world_x * layout.scale;
    let screen_y = layout.center_y - world_y * layout.scale;
    (screen_x, screen_y, -world_z)
}

/// One flat normal per facet, indexed by `facet_id` -- identical in spirit to
/// `raster::SolidRasterizer::render`'s own local table.
fn facet_normals(mesh: &SolidMesh) -> Vec<Option<DVec3>> {
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

/// Computes the three panels' fixed pixel layout from the mesh's own extent.
/// Falls back to a unit-scale layout for an empty mesh so a caller never has to
/// special-case "nothing to draw yet".
fn compute_layout(mesh: &SolidMesh, width: u32, height: u32) -> [PanelLayout; 3] {
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
    let profile_radius = half_w.max(half_h) as f32;
    let xz_radius = xz_radius as f32;

    let (width_f, height_f) = (width as f32, height as f32);
    let col_width = width_f / 3.0;
    let caption_height = (height_f * 0.06).clamp(10.0, 20.0);
    let avail_height = (height_f - caption_height).max(1.0);
    let panel_box = col_width.min(avail_height);
    let center_y = caption_height + avail_height / 2.0;

    let make = |index: usize, kind: PanelKind, world_radius: f32, body_frac: f32| {
        let center_x = (index as f32).mul_add(col_width, col_width / 2.0);
        let wheel_radius_px = panel_box / 2.0 * 0.86;
        let body_radius_px = panel_box / 2.0 * body_frac;
        let scale = body_radius_px / world_radius.max(1e-6);
        let clip = (
            (index as f32 * col_width).floor() as i32,
            0,
            ((index as f32 + 1.0) * col_width).ceil() as i32 - 1,
            height as i32 - 1,
        );
        PanelLayout {
            kind,
            center_x,
            center_y,
            scale,
            wheel_radius_px,
            clip,
        }
    };

    [
        make(0, PanelKind::Crown, xz_radius, 0.62),
        make(1, PanelKind::Pavilion, xz_radius, 0.62),
        make(2, PanelKind::Profile, profile_radius, 0.85),
    ]
}

/// Renders `mesh` into a three-panel [`DiagramFrame`] per this module's doc
/// comment.
///
/// Never panics on an empty mesh (draws background only, per-panel captions and
/// index wheels still appear).
#[must_use]
pub fn render_diagram(
    mesh: &SolidMesh,
    config: &DiagramConfig,
    style: &DiagramStyle,
) -> DiagramFrame {
    let mut frame = DiagramFrame::blank(config.width, config.height, style.background);
    if config.width == 0 || config.height == 0 {
        return frame;
    }
    let layouts = compute_layout(mesh, config.width, config.height);
    let facet_normal = facet_normals(mesh);
    let gear_teeth = config.gear_teeth.max(1);

    let mut dedup_scratch: Vec<DVec3> = Vec::new();
    let mut ring_scratch: Vec<DVec3> = Vec::new();

    for layout in &layouts {
        render_panel(
            &mut frame,
            mesh,
            layout,
            &facet_normal,
            gear_teeth,
            config.gear_reference_angle,
            style,
            &mut dedup_scratch,
            &mut ring_scratch,
        );
    }

    frame
}

/// One panel's whole draw pass: caption, index wheel, facet fill/edges, labels --
/// split out of [`render_diagram`] purely to keep that function short; see this
/// module's doc comment for the panel conventions this implements.
#[expect(
    clippy::too_many_arguments,
    reason = "a flat parameter list mirroring render_diagram's own locals -- there is \
              exactly one call site, so a bundling struct would only add indirection"
)]
fn render_panel(
    frame: &mut DiagramFrame,
    mesh: &SolidMesh,
    layout: &PanelLayout,
    facet_normal: &[Option<DVec3>],
    gear_teeth: u32,
    gear_reference_angle: f32,
    style: &DiagramStyle,
    dedup_scratch: &mut Vec<DVec3>,
    ring_scratch: &mut Vec<DVec3>,
) {
    let (cap_w, _) = text_size(layout.kind.caption(), 1);
    draw_text(
        frame,
        layout.center_x - cap_w as f32 / 2.0,
        2.0,
        layout.kind.caption(),
        1,
        style.edge_color,
        None,
    );

    if layout.kind.has_index_wheel() {
        draw_index_wheel(
            frame,
            layout,
            gear_teeth,
            gear_reference_angle,
            style.edge_color,
        );
    }

    let visible: fn(Vec3) -> bool = match layout.kind {
        PanelKind::Crown => crown_visible,
        PanelKind::Pavilion => pavilion_visible,
        PanelKind::Profile => profile_visible,
    };
    let project: fn(DVec3, &PanelLayout) -> (f32, f32, f32) = match layout.kind {
        PanelKind::Crown => crown_project,
        PanelKind::Pavilion => pavilion_project,
        PanelKind::Profile => profile_project,
    };

    let fill_ctx = PanelRenderCtx {
        mesh,
        layout,
        facet_normal,
        style,
        visible,
        project,
    };
    let (edge_points, edge_ranges, label_spots) =
        fill_panel_facets(frame, &fill_ctx, dedup_scratch, ring_scratch);
    draw_panel_edges(frame, layout, style, &edge_points, &edge_ranges);
    draw_panel_labels(frame, layout, style, label_spots);
}

/// The immutable per-panel context [`fill_panel_facets`] needs, bundled purely to
/// keep that function (and its caller, [`render_panel`]) under clippy's
/// argument-count limit.
struct PanelRenderCtx<'a> {
    mesh: &'a SolidMesh,
    layout: &'a PanelLayout,
    facet_normal: &'a [Option<DVec3>],
    style: &'a DiagramStyle,
    visible: fn(Vec3) -> bool,
    project: fn(DVec3, &PanelLayout) -> (f32, f32, f32),
}

/// [`fill_panel_facets`]'s own output: the flattened edge-segment points, each
/// facet's `(facet_id, start, count)` range into them, and each facet's
/// `(facet_id, cx, cy, w, h)` label spot. Named purely so that function's return
/// type doesn't trip clippy's `type_complexity` lint.
type PanelFillOutput = (
    Vec<(f32, f32, f32)>,
    Vec<(usize, usize, usize)>,
    Vec<(usize, f32, f32, f32, f32)>, // facet_id, cx, cy, w, h
);

/// The facet fill pass of [`render_panel`]'s "two passes, exactly like
/// `raster::SolidRasterizer::render`" scheme: fills every visible facet, and
/// collects the edge segments/label spots the caller's later passes
/// ([`draw_panel_edges`]/[`draw_panel_labels`]) draw on top -- so an edge is
/// depth-tested against the complete panel rather than only the facets drawn before
/// it. Split out of `render_panel` purely to keep that function under clippy's
/// function-length lint.
fn fill_panel_facets(
    frame: &mut DiagramFrame,
    ctx: &PanelRenderCtx<'_>,
    dedup_scratch: &mut Vec<DVec3>,
    ring_scratch: &mut Vec<DVec3>,
) -> PanelFillOutput {
    let mut edge_points: Vec<(f32, f32, f32)> = Vec::new();
    let mut edge_ranges: Vec<(usize, usize, usize)> = Vec::new();
    let mut label_spots: Vec<(usize, f32, f32, f32, f32)> = Vec::new();

    for (facet_id, ring) in &ctx.mesh.rings {
        let Some(Some(normal_f64)) = ctx.facet_normal.get(*facet_id).copied() else {
            continue;
        };
        let normal = to_vec3(normal_f64);
        if !(ctx.visible)(normal) {
            continue;
        }
        simplify_ring(ring, dedup_scratch, ring_scratch);
        if ring_scratch.len() < 3 {
            continue;
        }
        let screen: Vec<(f32, f32, f32)> = ring_scratch
            .iter()
            .map(|&p| (ctx.project)(p, ctx.layout))
            .collect();

        let color = shade(normal, ctx.style);
        fill_polygon(
            frame,
            &screen,
            *facet_id,
            color,
            ctx.style,
            ctx.layout.clip,
            ctx.layout.kind.tag(),
        );
        edge_ranges.push((*facet_id, edge_points.len(), screen.len()));
        edge_points.extend_from_slice(&screen);

        let (min_x, max_x, min_y, max_y) = bounds(&screen);
        let (cx, cy) = (min_x.midpoint(max_x), min_y.midpoint(max_y));
        label_spots.push((*facet_id, cx, cy, max_x - min_x, max_y - min_y));
    }

    (edge_points, edge_ranges, label_spots)
}

/// The edge-draw pass of [`render_panel`]'s "two passes" scheme -- see
/// [`fill_panel_facets`]'s doc comment. Split out of `render_panel` purely to keep
/// that function under clippy's function-length lint.
fn draw_panel_edges(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    style: &DiagramStyle,
    edge_points: &[(f32, f32, f32)],
    edge_ranges: &[(usize, usize, usize)],
) {
    for &(facet_id, start, count) in edge_ranges {
        let edge_color = if style.pending.get(facet_id).copied().unwrap_or(false) {
            style.pending_color
        } else if style.selected.get(facet_id).copied().unwrap_or(false) {
            style.selected_color
        } else {
            style.edge_color
        };
        let pts = &edge_points[start..start + count];
        for (i, &a) in pts.iter().enumerate() {
            let b = pts[(i + 1) % count];
            draw_edge(frame, a, b, edge_color, layout.clip);
        }
    }
}

/// The facet-label pass of `render_panel`: only where there's real room, and only
/// where this facet actually won the depth test at its own centroid (never label an
/// occluded facet). Split out of `render_panel` purely to keep that function under
/// clippy's function-length lint.
fn draw_panel_labels(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    style: &DiagramStyle,
    label_spots: Vec<(usize, f32, f32, f32, f32)>,
) {
    for (facet_id, cx, cy, w, h) in label_spots {
        if w < MIN_LABEL_SPAN || h < MIN_LABEL_SPAN {
            continue;
        }
        let Some(label) = style.facet_labels.get(facet_id) else {
            continue;
        };
        if label.is_empty() {
            continue;
        }
        let visible_here = frame
            .idx(cx.round() as i32, cy.round() as i32)
            .is_some_and(|idx| frame.pick[idx] == facet_id as u32 + 1);
        if !visible_here {
            continue;
        }
        let (label_w, label_h) = text_size(label, 1);
        draw_text(
            frame,
            cx - label_w as f32 / 2.0,
            cy - label_h as f32 / 2.0,
            label,
            1,
            style.edge_color,
            Some(layout.clip),
        );
    }
}

fn bounds(pts: &[(f32, f32, f32)]) -> (f32, f32, f32, f32) {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for &(x, y, _) in pts {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    (min_x, max_x, min_y, max_y)
}

const fn to_vec3(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Flat, simple lighting for the diagram: no camera-relative rim term (there is
/// no single camera in an orthographic multi-panel diagram), just a fixed key
/// light so facets at different elevations read as visually distinct.
fn shade(normal: Vec3, style: &DiagramStyle) -> [u8; 3] {
    const KEY_LIGHT: Vec3 = Vec3::new(0.35, 0.8, 0.48);
    let light = KEY_LIGHT.normalize();
    let n_dot_l = normal.normalize().dot(light).max(0.0);
    let intensity = 0.45f32.mul_add(n_dot_l, 0.55).clamp(0.0, 1.0);
    std::array::from_fn(|i| {
        (f32::from(style.base_color[i]) * intensity)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

fn blend_toward(base: [u8; 3], target: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|i| {
        let b = f32::from(base[i]);
        let t = f32::from(target[i]);
        amount.mul_add(t - b, b).round().clamp(0.0, 255.0) as u8
    })
}

/// Twice the signed area of triangle `(ax,ay)-(bx,by)-(px,py)` -- same helper as
/// `raster::edge_fn`, reimplemented here so this module stays free-standing.
fn edge_fn(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (by - ay).mul_add(-(px - ax), (bx - ax) * (py - ay))
}

/// Depth-tested scanline span fill of one screen-space convex polygon, clipped to
/// `clip` (inclusive `x0,y0,x1,y1`). Unlike `raster::SolidRasterizer::
/// fill_convex_polygon`, `depth` is already affine in screen position (an
/// orthographic projection of a flat facet), so the fitted plane is used
/// directly with no `1/depth` step.
fn fill_polygon(
    frame: &mut DiagramFrame,
    verts: &[(f32, f32, f32)],
    facet_id: usize,
    color: [u8; 3],
    style: &DiagramStyle,
    clip: (i32, i32, i32, i32),
    panel_tag: u8,
) {
    let n = verts.len();
    if n < 3 {
        return;
    }
    let (x0, y0, d0) = verts[0];
    let mut best = (1usize, 2usize);
    let mut best_area = 0.0f32;
    for i in 1..n - 1 {
        let (xi, yi, _) = verts[i];
        let (xj, yj, _) = verts[i + 1];
        let area = edge_fn(x0, y0, xi, yi, xj, yj);
        if area.abs() > best_area.abs() {
            best_area = area;
            best = (i, i + 1);
        }
    }
    if best_area.abs() < 1e-8 {
        return;
    }
    let (x1, y1, d1) = verts[best.0];
    let (x2, y2, d2) = verts[best.1];
    let inv_area = 1.0 / best_area;
    let grad_x = (y1 - y2).mul_add(d0, (y2 - y0).mul_add(d1, (y0 - y1) * d2)) * inv_area;
    let grad_y = (x2 - x1).mul_add(d0, (x0 - x2).mul_add(d1, (x1 - x0) * d2)) * inv_area;
    let plane_c = d0 - grad_x.mul_add(x0, grad_y * y0);

    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for &(_, y, _) in verts {
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let (clip_x0, clip_y0, clip_x1, clip_y1) = clip;
    let row_start = (min_y.floor() as i32).max(clip_y0);
    let row_end = (max_y.ceil() as i32).min(clip_y1);
    if row_start > row_end {
        return;
    }

    let flagged = style.flagged.get(facet_id).copied().unwrap_or(false);
    let selected = style.selected.get(facet_id).copied().unwrap_or(false);
    let selected_fill = selected.then(|| blend_toward(color, style.selected_color, 0.35));

    for py in row_start..=row_end {
        let sy = py as f32 + 0.5;
        let (mut x_left, mut x_right) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut crossings = 0u32;
        for (i, &(px_a, py_a, _)) in verts.iter().enumerate() {
            let (px_b, py_b, _) = verts[(i + 1) % n];
            if (py_a <= sy) == (py_b <= sy) {
                continue;
            }
            let t = (sy - py_a) / (py_b - py_a);
            let x = t.mul_add(px_b - px_a, px_a);
            x_left = x_left.min(x);
            x_right = x_right.max(x);
            crossings += 1;
        }
        if crossings < 2 {
            continue;
        }
        let col_start = ((x_left - 0.5).ceil() as i32).max(clip_x0);
        let col_end = ((x_right - 0.5).floor() as i32).min(clip_x1);
        if col_start > col_end {
            continue;
        }
        for px in col_start..=col_end {
            let sx = px as f32 + 0.5;
            let depth = grad_x.mul_add(sx, grad_y.mul_add(sy, plane_c));
            let Some(idx) = frame.idx(px, py) else {
                continue;
            };
            let stripe = (px + py).rem_euclid(HATCH_PERIOD * 2) < HATCH_PERIOD;
            if depth < frame.depth[idx] {
                frame.depth[idx] = depth;
                frame.pick[idx] = facet_id as u32 + 1;
                frame.panel[idx] = panel_tag;
                let painted = if flagged && stripe {
                    style.hatch_color
                } else {
                    selected_fill.unwrap_or(color)
                };
                let o = idx * 4;
                frame.color[o] = painted[0];
                frame.color[o + 1] = painted[1];
                frame.color[o + 2] = painted[2];
                frame.color[o + 3] = 255;
            }
        }
    }
}

/// Draws one 1px edge segment, depth-tested against the fill pass -- see
/// `raster::SolidRasterizer::draw_edge`'s doc comment for [`EDGE_DEPTH_BIAS`]'s
/// purpose, reused here at the same value.
const EDGE_DEPTH_BIAS: f32 = 5e-2;

fn draw_edge(
    frame: &mut DiagramFrame,
    from: (f32, f32, f32),
    to: (f32, f32, f32),
    color: [u8; 3],
    clip: (i32, i32, i32, i32),
) {
    let (x0, y0, d0) = (from.0.round() as i32, from.1.round() as i32, from.2);
    let (x1, y1, d1) = (to.0.round() as i32, to.1.round() as i32, to.2);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let steps_total = dx.max(-dy).max(1);

    let (mut cx, mut cy) = (x0, y0);
    let mut err = dx + dy;
    let mut step = 0;
    loop {
        if cx >= clip.0 && cy >= clip.1 && cx <= clip.2 && cy <= clip.3 {
            let frac = step as f32 / steps_total as f32;
            let depth = (1.0 - frac).mul_add(d0, frac * d1);
            if let Some(idx) = frame.idx(cx, cy) {
                let current = frame.depth[idx];
                if depth <= current + EDGE_DEPTH_BIAS {
                    let offset = idx * 4;
                    frame.color[offset] = color[0];
                    frame.color[offset + 1] = color[1];
                    frame.color[offset + 2] = color[2];
                    frame.color[offset + 3] = 255;
                }
            }
        }
        if cx == x1 && cy == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
        step += 1;
    }
}

/// Draws the index wheel: one tick per tooth, a longer tick every 8th (every 4th
/// for a gear of 64 teeth or fewer, so a small wheel doesn't end up with only one
/// or two labeled ticks), with the tooth number at every long tick.
fn draw_index_wheel(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    gear_teeth: u32,
    gear_reference_angle: f32,
    color: [u8; 3],
) {
    let mirrored = matches!(layout.kind, PanelKind::Pavilion);
    let major_every = if gear_teeth <= 64 { 4 } else { 8 };
    let inner_r = layout.wheel_radius_px * 0.94;
    let minor_outer_r = layout.wheel_radius_px;
    let major_outer_r = layout.wheel_radius_px * 1.08;
    let label_r = layout.wheel_radius_px * 1.2;

    for tooth in 0..gear_teeth {
        let phi =
            2.0 * std::f32::consts::PI * (tooth as f32 + gear_reference_angle) / gear_teeth as f32;
        let (su, sv) = wheel_direction(phi, mirrored);
        let is_major = tooth % major_every == 0;
        let outer_r = if is_major {
            major_outer_r
        } else {
            minor_outer_r
        };
        let from = (
            su.mul_add(inner_r, layout.center_x),
            sv.mul_add(-inner_r, layout.center_y),
            0.0,
        );
        let to = (
            su.mul_add(outer_r, layout.center_x),
            sv.mul_add(-outer_r, layout.center_y),
            0.0,
        );
        draw_edge(frame, from, to, color, layout.clip);

        if is_major {
            let label = tooth.to_string();
            let (w, h) = text_size(&label, 1);
            let lx = su.mul_add(label_r, layout.center_x) - w as f32 / 2.0;
            let ly = sv.mul_add(-label_r, layout.center_y) - h as f32 / 2.0;
            draw_text(frame, lx, ly, &label, 1, color, Some(layout.clip));
        }
    }
}

// --- Minimal built-in 5x7 bitmap font ---------------------------------------

const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;

/// Converts a 5-character `'X'`/`'.'` row into a bitmask (bit 4 = leftmost column).
fn row_bits(row: &str) -> u8 {
    let mut bits = 0u8;
    for (i, ch) in row.chars().take(GLYPH_WIDTH).enumerate() {
        if ch != '.' {
            bits |= 1 << (GLYPH_WIDTH - 1 - i);
        }
    }
    bits
}

/// The punctuation/whitespace arms of [`glyph_rows`]'s lookup, split out purely to
/// keep that function under clippy's function-length lint -- `c` is already
/// uppercased by the caller.
const fn glyph_rows_symbols(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        ' ' => [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
        '-' => [
            ".....", ".....", ".....", "XXXXX", ".....", ".....", ".....",
        ],
        '.' => [
            ".....", ".....", ".....", ".....", ".....", ".XX..", ".XX..",
        ],
        '\'' => [
            ".X...", ".X...", ".....", ".....", ".....", ".....", ".....",
        ],
        _ => return None,
    })
}

/// The digit arms of [`glyph_rows`]'s lookup, split out purely to keep that function
/// under clippy's function-length lint -- `c` is already uppercased by the caller.
const fn glyph_rows_digits(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        '0' => [
            "XXXXX", "X...X", "X..XX", "X.X.X", "XX..X", "X...X", "XXXXX",
        ],
        '1' => [
            "..X..", ".XX..", "..X..", "..X..", "..X..", "..X..", ".XXX.",
        ],
        '2' => [
            ".XXX.", "X...X", "....X", "...X.", "..X..", ".X...", "XXXXX",
        ],
        '3' => [
            "XXXXX", "...X.", "..X..", "...X.", "....X", "X...X", ".XXX.",
        ],
        '4' => [
            "...X.", "..XX.", ".X.X.", "X..X.", "XXXXX", "...X.", "...X.",
        ],
        '5' => [
            "XXXXX", "X....", "XXXX.", "....X", "....X", "X...X", ".XXX.",
        ],
        '6' => [
            "..XX.", ".X...", "X....", "XXXX.", "X...X", "X...X", ".XXX.",
        ],
        '7' => [
            "XXXXX", "....X", "...X.", "..X..", ".X...", ".X...", ".X...",
        ],
        '8' => [
            ".XXX.", "X...X", "X...X", ".XXX.", "X...X", "X...X", ".XXX.",
        ],
        '9' => [
            ".XXX.", "X...X", "X...X", ".XXXX", "....X", "...X.", ".XX..",
        ],
        _ => return None,
    })
}

/// The `A`-`M` letter arms of [`glyph_rows`]'s lookup, split out purely to keep that
/// function under clippy's function-length lint -- `c` is already uppercased by the
/// caller.
const fn glyph_rows_letters_a_to_m(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        'A' => [
            "..X..", ".X.X.", "X...X", "X...X", "XXXXX", "X...X", "X...X",
        ],
        'B' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X...X", "X...X", "XXXX.",
        ],
        'C' => [
            ".XXXX", "X....", "X....", "X....", "X....", "X....", ".XXXX",
        ],
        'D' => [
            "XXXX.", "X...X", "X...X", "X...X", "X...X", "X...X", "XXXX.",
        ],
        'E' => [
            "XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "XXXXX",
        ],
        'F' => [
            "XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "X....",
        ],
        'G' => [
            ".XXXX", "X....", "X....", "X.XXX", "X...X", "X...X", ".XXXX",
        ],
        'H' => [
            "X...X", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X",
        ],
        'I' => [
            "XXXXX", "..X..", "..X..", "..X..", "..X..", "..X..", "XXXXX",
        ],
        'J' => [
            "....X", "....X", "....X", "....X", "X...X", "X...X", ".XXX.",
        ],
        'K' => [
            "X...X", "X..X.", "X.X..", "XX...", "X.X..", "X..X.", "X...X",
        ],
        'L' => [
            "X....", "X....", "X....", "X....", "X....", "X....", "XXXXX",
        ],
        'M' => [
            "X...X", "XX.XX", "X.X.X", "X...X", "X...X", "X...X", "X...X",
        ],
        _ => return None,
    })
}

/// The `N`-`Z` letter arms of [`glyph_rows`]'s lookup, split out purely to keep that
/// function under clippy's function-length lint -- `c` is already uppercased by the
/// caller.
const fn glyph_rows_letters_n_to_z(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        'N' => [
            "X...X", "XX..X", "X.X.X", "X..XX", "X...X", "X...X", "X...X",
        ],
        'O' => [
            ".XXX.", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX.",
        ],
        'P' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X....", "X....", "X....",
        ],
        'Q' => [
            ".XXX.", "X...X", "X...X", "X...X", "X.X.X", "X..X.", ".XX.X",
        ],
        'R' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X.X..", "X..X.", "X...X",
        ],
        'S' => [
            ".XXXX", "X....", "X....", ".XXX.", "....X", "....X", "XXXX.",
        ],
        'T' => [
            "XXXXX", "..X..", "..X..", "..X..", "..X..", "..X..", "..X..",
        ],
        'U' => [
            "X...X", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX.",
        ],
        'V' => [
            "X...X", "X...X", "X...X", "X...X", "X...X", ".X.X.", "..X..",
        ],
        'W' => [
            "X...X", "X...X", "X...X", "X.X.X", "X.X.X", "XX.XX", "X...X",
        ],
        'X' => [
            "X...X", "X...X", ".X.X.", "..X..", ".X.X.", "X...X", "X...X",
        ],
        'Y' => [
            "X...X", "X...X", ".X.X.", "..X..", "..X..", "..X..", "..X..",
        ],
        'Z' => [
            "XXXXX", "....X", "...X.", "..X..", ".X...", "X....", "XXXXX",
        ],
        _ => return None,
    })
}

/// Returns the glyph rows for `c` (case-insensitive letters), or `None` for an
/// unsupported character -- callers skip it rather than drawing a placeholder.
/// Dispatches across [`glyph_rows_symbols`]/[`glyph_rows_digits`]/
/// [`glyph_rows_letters_a_to_m`]/[`glyph_rows_letters_n_to_z`], split out purely to
/// keep this lookup table's own function under clippy's function-length lint.
const fn glyph_rows(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    let c = c.to_ascii_uppercase();
    if let Some(rows) = glyph_rows_symbols(c) {
        return Some(rows);
    }
    if let Some(rows) = glyph_rows_digits(c) {
        return Some(rows);
    }
    if let Some(rows) = glyph_rows_letters_a_to_m(c) {
        return Some(rows);
    }
    glyph_rows_letters_n_to_z(c)
}

fn glyph(c: char) -> [u8; GLYPH_HEIGHT] {
    let rows = glyph_rows(c).unwrap_or([
        ".....", ".....", ".....", ".....", ".....", ".....", ".....",
    ]);
    std::array::from_fn(|i| row_bits(rows[i]))
}

/// The pixel size `text` occupies at `scale` (1 device pixel per glyph pixel at
/// `scale == 1`), including inter-character spacing but not a trailing gap.
fn text_size(text: &str, scale: u32) -> (u32, u32) {
    let len = text.chars().count() as u32;
    if len == 0 {
        return (0, (GLYPH_HEIGHT as u32) * scale);
    }
    let width = (len * (GLYPH_WIDTH as u32 + 1) - 1) * scale;
    (width, (GLYPH_HEIGHT as u32) * scale)
}

/// Draws `text` with its top-left at `(x, y)`, one glyph pixel per `scale`
/// device pixels, clipped to `clip` when given (otherwise the whole frame).
fn draw_text(
    frame: &mut DiagramFrame,
    x: f32,
    y: f32,
    text: &str,
    scale: u32,
    color: [u8; 3],
    clip: Option<(i32, i32, i32, i32)>,
) {
    let clip = clip.unwrap_or((0, 0, frame.width as i32 - 1, frame.height as i32 - 1));
    let scale = scale.max(1) as i32;
    let mut pen_x = x.round() as i32;
    let pen_y = y.round() as i32;
    for ch in text.chars() {
        let rows = glyph(ch);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..GLYPH_WIDTH {
                if bits & (1 << (GLYPH_WIDTH - 1 - col)) == 0 {
                    continue;
                }
                let px0 = pen_x + col as i32 * scale;
                let py0 = pen_y + row as i32 * scale;
                for sy in 0..scale {
                    for sx in 0..scale {
                        let (px, py) = (px0 + sx, py0 + sy);
                        if px < clip.0 || py < clip.1 || px > clip.2 || py > clip.3 {
                            continue;
                        }
                        if let Some(idx) = frame.idx(px, py) {
                            let o = idx * 4;
                            frame.color[o] = color[0];
                            frame.color[o + 1] = color[1];
                            frame.color[o + 2] = color[2];
                            frame.color[o + 3] = 255;
                        }
                    }
                }
            }
        }
        pen_x += (GLYPH_WIDTH as i32 + 1) * scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::stone_metrics::{SolidStatus, build_solid_mesh};

    /// "Visible" here means "selected by the panel's own facet-visibility
    /// predicate" (see this module's doc comment) -- a cube's front profile face
    /// still fully occludes its back face once rasterized, exactly like
    /// `raster.rs`'s own crown/pavilion visibility test does before its depth
    /// test runs. This is the count the feature brief's "4 visible side facets in
    /// profile and 1 in crown" describes.
    #[test]
    fn cube_visibility_predicates_select_one_crown_facet_and_four_profile_facets() {
        let cube_normals = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ];
        let crown_count = cube_normals.iter().filter(|&&n| crown_visible(n)).count();
        let pavilion_count = cube_normals
            .iter()
            .filter(|&&n| pavilion_visible(n))
            .count();
        let profile_count = cube_normals.iter().filter(|&&n| profile_visible(n)).count();

        assert_eq!(crown_count, 1, "only +Y should read as crown-facing");
        assert_eq!(pavilion_count, 1, "only -Y should read as pavilion-facing");
        assert_eq!(
            profile_count, 4,
            "the four vertical faces (+-X, +-Z) should all read as profile-facing"
        );
    }

    /// Gear 96, no reference-angle offset: index 0 must land at 12 o'clock
    /// (straight up) on the crown panel, and index 24 (a quarter turn) at 3
    /// o'clock -- see this module's doc comment for why crown sweeps clockwise.
    #[test]
    fn crown_index_wheel_places_index_0_at_top_and_index_24_at_three_oclock() {
        let gear_teeth = 96.0f32;
        let phi0 = 2.0 * std::f32::consts::PI * 0.0 / gear_teeth;
        let phi24 = 2.0 * std::f32::consts::PI * 24.0 / gear_teeth;

        let (su0, sv0) = wheel_direction(phi0, false);
        assert!(su0.abs() < 1e-5, "index 0 must have no horizontal offset");
        assert!(sv0 > 0.99, "index 0 must point straight up (screen_up > 0)");

        let (su24, sv24) = wheel_direction(phi24, false);
        assert!(
            su24 > 0.99,
            "index 24 must point straight right (3 o'clock)"
        );
        assert!(sv24.abs() < 1e-5, "index 24 must have no vertical offset");
    }

    /// The pavilion panel mirrors the horizontal axis, so the same index lands on
    /// the OPPOSITE side (9 o'clock instead of 3 o'clock) while index 0 stays at
    /// the top on both panels.
    #[test]
    fn pavilion_index_wheel_mirrors_the_horizontal_axis() {
        let gear_teeth = 96.0f32;
        let phi24 = 2.0 * std::f32::consts::PI * 24.0 / gear_teeth;
        let (su24, sv24) = wheel_direction(phi24, true);
        assert!(
            su24 < -0.99,
            "index 24 must point left (9 o'clock) when mirrored"
        );
        assert!(sv24.abs() < 1e-5);
    }

    /// A closed box's crown/pavilion/profile panels each pick back the expected
    /// facet id at their own panel center -- the pick-buffer round trip the
    /// hover/click wiring depends on.
    #[test]
    fn pick_buffer_round_trips_each_panels_center_to_the_right_facet() {
        let mesh = match build_solid_mesh(&[
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::NEG_Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ]) {
            SolidStatus::Closed(mesh) => mesh,
            other => panic!("box fixture must close: {other:?}"),
        };
        let config = DiagramConfig {
            width: 300,
            height: 120,
            gear_teeth: 96,
            gear_reference_angle: 0.0,
        };
        let style = DiagramStyle::default();
        let frame = render_diagram(&mesh, &config, &style);

        // Panel centers sit at column midpoints (300 / 3 = 100px per column).
        assert_eq!(
            frame.pick_at(50, 60),
            Some(2),
            "crown panel center must show the +Y facet (id 2)"
        );
        assert_eq!(
            frame.pick_at(150, 60),
            Some(3),
            "pavilion panel center must show the -Y facet (id 3)"
        );
        assert_eq!(
            frame.pick_at(250, 60),
            Some(4),
            "profile panel center must show the nearer of +-Z (id 4, +Z)"
        );

        assert_eq!(frame.panel_at(50, 60), Some(PanelKind::Crown));
        assert_eq!(frame.panel_at(150, 60), Some(PanelKind::Pavilion));
        assert_eq!(frame.panel_at(250, 60), Some(PanelKind::Profile));
    }

    #[test]
    fn an_empty_or_zero_sized_config_never_panics() {
        let mesh = SolidMesh::default();
        let config = DiagramConfig {
            width: 0,
            height: 0,
            gear_teeth: 96,
            gear_reference_angle: 0.0,
        };
        let frame = render_diagram(&mesh, &config, &DiagramStyle::default());
        assert_eq!(frame.width, 0);
        assert_eq!(frame.pick_at(0, 0), None);
    }
}
