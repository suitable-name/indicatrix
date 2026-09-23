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

use super::{
    super::pixel_font::{GLYPH_HEIGHT, GLYPH_WIDTH, glyph},
    raster::simplify_ring,
};
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
/// pixels before [`draw_panel_labels`] draws its label directly ON the facet. Kept
/// low enough that most facets still get a label even at the crown/pavilion body
/// radius typical of an 8-fold design in a ~700px-wide viewport. A facet still too
/// small for this gets a leader line instead (see [`LEADER_LINE_MIN_SPAN`]) rather
/// than being dropped silently.
const MIN_LABEL_SPAN: f32 = 10.0;

/// Below this on-screen span (in either axis) a facet is too small even to anchor
/// a leader line legibly -- a near-zero-area sliver -- so [`draw_panel_labels`]
/// drops its label entirely. Facets between this and [`MIN_LABEL_SPAN`] get a
/// leader line rather than nothing.
const LEADER_LINE_MIN_SPAN: f32 = 2.5;

/// Leader-line length in pixels, radiating from a too-small facet's centroid away
/// from the panel center, with the label drawn at its far end.
const LEADER_LINE_LENGTH: f32 = 22.0;

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
    /// The schedule's own rotational symmetry order (`ScheduleMeta::
    /// symmetry_order`) -- the symmetry-line overlay draws this many evenly
    /// spaced radial guides on the crown/pavilion panels. `1` (or `0`, treated
    /// the same) draws none: a design with no rotational symmetry has no sector
    /// lines to show.
    pub symmetry_order: u32,
    /// The schedule's own mirror flag (`ScheduleMeta::mirror`) -- when set, the
    /// overlay also draws the mirror axis (through index 0, the same "up"
    /// direction on every panel per this module's own doc comment) in its own
    /// distinct color.
    pub mirror: bool,
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
    /// Matches `raster::SolidStyle::pending_color` / `ui/theme.slint`'s
    /// `accent-amber` -- see that field's doc comment.
    pub pending_color: [u8; 3],
    pub hatch_color: [u8; 3],
    /// Matches `raster::SolidStyle::selected_color` / `ui/theme.slint`'s `primary`
    /// -- see that field's doc comment.
    pub selected_color: [u8; 3],
    /// Matches `raster::SolidStyle::hover_color` -- see that field's doc comment.
    pub hover_color: [u8; 3],
    /// Matches `raster::SolidStyle::selected_facet_color` -- see that field's doc
    /// comment.
    pub selected_facet_color: [u8; 3],
    /// Matches `raster::SolidStyle::multi_selected_color` -- see that field's doc
    /// comment.
    pub multi_selected_color: [u8; 3],
    pub background: [u8; 4],
    pub flagged: Vec<bool>,
    pub pending: Vec<bool>,
    pub selected: Vec<bool>,
    /// Facet id -> short on-diagram label (`facet_map::FacetMap::facet_label`,
    /// typically the tier name); empty string suppresses the label.
    pub facet_labels: Vec<String>,
    /// Matches `raster::SolidStyle::hovered` -- see that field's doc comment.
    pub hovered: Option<u32>,
    /// Matches `raster::SolidStyle::selected_facet` -- see that field's doc comment.
    pub selected_facet: Option<u32>,
    /// Matches `raster::SolidStyle::multi_selected` -- see that field's doc comment.
    pub multi_selected: Vec<u32>,
    /// Facet id -> index-wheel tooth (`facet_map::FacetMap::index_on_gear`), for
    /// the radial-line pass: whenever a facet on a crown/pavilion panel is
    /// selected/hovered/multi-selected, a line is drawn from the panel centre
    /// through this tooth, linking the facet on screen to its own position on
    /// the index wheel. Unset (default empty) entries simply draw no radial.
    pub facet_index_on_gear: Vec<u32>,
    /// Facet-id pairs to mark with a meet-point dot on the crown/pavilion panels:
    /// `facet_map::FacetMap::meeting_facet_pairs`' output. Resolved to actual
    /// world-space points by [`meet_marker_points`] against the SAME mesh
    /// `render_diagram` is already drawing, since `facet_meets` only names
    /// tiers, not geometry -- see that function's own doc comment.
    pub meet_marker_pairs: Vec<(u32, u32)>,
    /// Marker color for a resolved meet point -- deliberately distinct from
    /// every edge/selection color so it reads as its own kind of annotation
    /// rather than another highlight.
    pub meet_marker_color: [u8; 3],
    /// Radial-guide color for `DiagramConfig::symmetry_order`'s sector lines --
    /// muted, since these are a background reference, not a highlight.
    pub symmetry_line_color: [u8; 3],
    /// Axis color for `DiagramConfig::mirror`'s mirror line -- distinct from
    /// `symmetry_line_color` so a mirrored design's own axis still stands out
    /// among the ordinary sector guides.
    pub mirror_line_color: [u8; 3],
}

impl Default for DiagramStyle {
    fn default() -> Self {
        Self {
            base_color: [200, 205, 215],
            edge_color: [30, 32, 38],
            pending_color: [245, 158, 11],
            hatch_color: [90, 40, 40],
            selected_color: [59, 130, 246],
            hover_color: [226, 232, 240],
            selected_facet_color: [168, 85, 247],
            multi_selected_color: [56, 189, 248],
            background: [12, 14, 20, 255],
            flagged: Vec::new(),
            pending: Vec::new(),
            selected: Vec::new(),
            facet_labels: Vec::new(),
            hovered: None,
            selected_facet: None,
            multi_selected: Vec::new(),
            facet_index_on_gear: Vec::new(),
            meet_marker_pairs: Vec::new(),
            meet_marker_color: [250, 250, 250],
            symmetry_line_color: [70, 74, 84],
            mirror_line_color: [45, 212, 191],
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
    /// `tooth + 1` per pixel within an index-wheel tick's hit region, `0`
    /// elsewhere -- see [`Self::tooth_at`]. A wheel tick is 1px wide, an
    /// unusably small click/hover target, so [`draw_index_wheel`] tags a small
    /// box around each tick's midpoint here rather than relying on the 1px
    /// stroke itself.
    ///
    /// `pub(crate)` (like [`Self::pick`]) so `preview_state` can lift it
    /// straight into a [`super::preview_state::PickBuffer`] -- its `+1`/`0`
    /// encoding is bit-for-bit the same convention `PickBuffer::facet_at`
    /// already reads, so this buffer is threaded through as one without a
    /// second accessor type.
    pub(crate) tooth: Vec<u32>,
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
            tooth: vec![0u32; n],
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

    /// The index-wheel tooth whose hit region covers pixel `(x, y)`, or `None`
    /// well outside every tick (see [`Self::tooth`]'s own doc comment for the hit
    /// region's size). Exposed so a caller (`gui::solid_preview::
    /// diagram_wiring`) can turn a hover/click over the wheel into "which tooth",
    /// exactly like [`Self::pick_at`] already does for a facet.
    #[must_use]
    pub fn tooth_at(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let v = self.tooth[(y * self.width + x) as usize];
        (v != 0).then(|| v - 1)
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
fn compute_layout(mesh: &SolidMesh, width: u32, height: u32) -> [PanelLayout; 3] {
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
/// the WHOLE frame width instead of a third of it, for [`render_diagram_single_panel`]'s
/// "enlarge this panel" mode. Deliberately a fresh layout computation rather than
/// a crop of [`compute_layout`]'s output -- cropping would enlarge the panel's
/// PIXELS without changing its `scale`, leaving the facet body just as small
/// relative to the (now much bigger) available column as it was in the
/// three-column layout, defeating the point of "enlarge".
fn compute_single_panel_layout(
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
    let wheel = WheelConfig {
        gear_teeth: config.gear_teeth.max(1),
        gear_reference_angle: config.gear_reference_angle,
        symmetry_order: config.symmetry_order,
        mirror: config.mirror,
    };
    // Resolved once, panel-agnostic -- `render_panel` below only needs to
    // filter by which side of the girdle a point falls on, not recompute it.
    let marker_points =
        meet_marker_points(mesh, &style.meet_marker_pairs, mesh_diagonal(mesh) * 1e-4);

    let mut dedup_scratch: Vec<DVec3> = Vec::new();
    let mut ring_scratch: Vec<DVec3> = Vec::new();

    for layout in &layouts {
        render_panel(
            &mut frame,
            mesh,
            layout,
            &facet_normal,
            wheel,
            style,
            &mut dedup_scratch,
            &mut ring_scratch,
            &marker_points,
        );
    }

    frame
}

/// The "enlarge this panel" mode: renders `mesh` as a SINGLE panel filling the
/// whole frame.
///
/// Everything else matches [`render_diagram`]'s fixed three-column layout: the same
/// style, the same meet-marker and index-radial passes, and the same pick, panel and
/// tooth buffer contract. `gui::solid_preview::diagram_wiring`'s hover and click
/// callbacks therefore work against a frame from either function identically, since
/// both go through the same [`render_panel`] into the same [`DiagramFrame`].
#[must_use]
pub fn render_diagram_single_panel(
    mesh: &SolidMesh,
    config: &DiagramConfig,
    style: &DiagramStyle,
    panel: PanelKind,
) -> DiagramFrame {
    let mut frame = DiagramFrame::blank(config.width, config.height, style.background);
    if config.width == 0 || config.height == 0 {
        return frame;
    }
    let layout = compute_single_panel_layout(mesh, config.width, config.height, panel);
    let facet_normal = facet_normals(mesh);
    let wheel = WheelConfig {
        gear_teeth: config.gear_teeth.max(1),
        gear_reference_angle: config.gear_reference_angle,
        symmetry_order: config.symmetry_order,
        mirror: config.mirror,
    };
    let marker_points =
        meet_marker_points(mesh, &style.meet_marker_pairs, mesh_diagonal(mesh) * 1e-4);

    let mut dedup_scratch: Vec<DVec3> = Vec::new();
    let mut ring_scratch: Vec<DVec3> = Vec::new();
    render_panel(
        &mut frame,
        mesh,
        &layout,
        &facet_normal,
        wheel,
        style,
        &mut dedup_scratch,
        &mut ring_scratch,
        &marker_points,
    );
    frame
}

/// The mesh's own bounding-box diagonal, in world units -- the scale
/// [`meet_marker_points`]'s vertex-matching tolerance is fractioned from
/// (mirrors [`simplify_ring`]'s own scale-relative merge tolerance). `1e-6` for
/// an empty mesh so a zero tolerance never turns into "everything matches".
fn mesh_diagonal(mesh: &SolidMesh) -> f64 {
    let Some(&first) = mesh.positions.first() else {
        return 1e-6;
    };
    let (mut lo, mut hi) = (first, first);
    for &p in &mesh.positions {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    (hi - lo).length().max(1e-6)
}

/// The index-wheel/symmetry parameters every panel draw needs, bundled purely
/// so [`render_panel`]'s own parameter list doesn't keep growing every time a
/// new wheel-relative overlay (the symmetry/mirror lines, on top of the index
/// wheel itself) needs another `DiagramConfig` field mirrored down.
#[derive(Debug, Clone, Copy)]
struct WheelConfig {
    gear_teeth: u32,
    gear_reference_angle: f32,
    symmetry_order: u32,
    mirror: bool,
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
    wheel: WheelConfig,
    style: &DiagramStyle,
    dedup_scratch: &mut Vec<DVec3>,
    ring_scratch: &mut Vec<DVec3>,
    marker_points: &[DVec3],
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
        draw_symmetry_and_mirror_lines(frame, layout, wheel, style);
        draw_index_wheel(
            frame,
            layout,
            wheel.gear_teeth,
            wheel.gear_reference_angle,
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

    if layout.kind.has_index_wheel() {
        draw_selected_index_radials(
            frame,
            layout,
            wheel.gear_teeth,
            wheel.gear_reference_angle,
            style,
            &edge_ranges,
        );
    }

    // Meet-point markers -- crown-side points on the crown panel,
    // pavilion-side points on the pavilion panel (a profile panel shows no
    // index-wheel-relative content and is skipped, same set as the radials
    // above). See `meet_marker_points`'s own doc comment for how a point here
    // was resolved from `DiagramStyle::meet_marker_pairs`.
    let on_this_panel: fn(f64) -> bool = match layout.kind {
        PanelKind::Crown => |y| y >= 0.0,
        PanelKind::Pavilion => |y| y <= 0.0,
        PanelKind::Profile => |_| false,
    };
    for &point in marker_points {
        if !on_this_panel(point.y) {
            continue;
        }
        let screen = project(point, layout);
        draw_meet_marker(frame, screen, style.meet_marker_color, layout.clip);
    }
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

/// Which of [`draw_panel_edges`]'s ordered passes an edge segment belongs to --
/// mirrors `raster::EdgePass` exactly (see that type's doc comment for why later
/// passes must win a shared edge, and for the pass ordering rationale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgePass {
    /// None of the below: the plain `edge_color`, drawn first.
    Ordinary,
    /// The one facet named in `style.hovered`.
    Hovered,
    /// A facet listed in `style.multi_selected`, not otherwise highlighted.
    MultiSelected,
    /// `selected` (a whole tier) and not otherwise highlighted.
    Selected,
    /// The one facet named in `style.selected_facet`.
    SelectedFacet,
    /// `pending`: drawn last, so it wins over every other pass too.
    Pending,
}

/// The edge-draw pass of [`render_panel`]'s "two passes" scheme -- see
/// [`fill_panel_facets`]'s doc comment. Split out of `render_panel` purely to keep
/// that function under clippy's function-length lint.
///
/// Draws in [`EdgePass`]'s six ordered sub-passes rather than once per facet, and
/// every highlighted sub-pass uses a wider stroke, so a highlighted facet's
/// boundary reads as a clean, intentional outline instead of being
/// half-overpainted by a neighbouring ordinary facet's dark edge.
fn draw_panel_edges(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    style: &DiagramStyle,
    edge_points: &[(f32, f32, f32)],
    edge_ranges: &[(usize, usize, usize)],
) {
    let is_selected = |facet_id: usize| style.selected.get(facet_id).copied().unwrap_or(false);
    let is_pending = |facet_id: usize| style.pending.get(facet_id).copied().unwrap_or(false);
    let is_hovered = |facet_id: usize| style.hovered == Some(facet_id as u32);
    let is_selected_facet = |facet_id: usize| style.selected_facet == Some(facet_id as u32);
    let is_multi_selected = |facet_id: usize| style.multi_selected.contains(&(facet_id as u32));

    for pass in [
        EdgePass::Ordinary,
        EdgePass::Hovered,
        EdgePass::MultiSelected,
        EdgePass::Selected,
        EdgePass::SelectedFacet,
        EdgePass::Pending,
    ] {
        for &(facet_id, start, count) in edge_ranges {
            let selected = is_selected(facet_id);
            let pending = is_pending(facet_id);
            let hovered = is_hovered(facet_id);
            let selected_facet = is_selected_facet(facet_id);
            let multi_selected = is_multi_selected(facet_id);
            let highlighted = selected || pending || hovered || selected_facet;
            let (draw_this_pass, color, width) = match pass {
                EdgePass::Ordinary => (!highlighted && !multi_selected, style.edge_color, 1),
                EdgePass::Hovered => (hovered && !pending && !selected_facet, style.hover_color, 2),
                EdgePass::MultiSelected => (
                    multi_selected && !selected && !pending && !selected_facet,
                    style.multi_selected_color,
                    2,
                ),
                EdgePass::Selected => (
                    selected && !pending && !selected_facet,
                    style.selected_color,
                    2,
                ),
                EdgePass::SelectedFacet => {
                    (selected_facet && !pending, style.selected_facet_color, 3)
                }
                EdgePass::Pending => (pending, style.pending_color, 2),
            };
            if !draw_this_pass {
                continue;
            }
            let pts = &edge_points[start..start + count];
            for (i, &a) in pts.iter().enumerate() {
                let b = pts[(i + 1) % count];
                draw_edge(frame, a, b, color, layout.clip, width);
            }
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
        if w < LEADER_LINE_MIN_SPAN || h < LEADER_LINE_MIN_SPAN {
            continue;
        }
        let Some(label) = style.facet_labels.get(facet_id) else {
            continue;
        };
        if label.is_empty() {
            continue;
        }
        let Some(idx) = frame.idx(cx.round() as i32, cy.round() as i32) else {
            continue;
        };
        if frame.pick[idx] != facet_id as u32 + 1 {
            continue;
        }

        // Too small to hold the label directly on the facet: draw a short leader
        // line radiating away from the panel center instead of dropping the
        // label, so a cutter can still trace it back to its facet. Reuses the
        // facet's own depth so the leader is occluded by anything truly in front
        // of it but always wins against the (f32::INFINITY) background.
        let (label_cx, label_cy) = if w < MIN_LABEL_SPAN || h < MIN_LABEL_SPAN {
            let (dir_x, dir_y) = {
                let (dx, dy) = (cx - layout.center_x, cy - layout.center_y);
                let dist = dx.hypot(dy).max(1e-3);
                (dx / dist, dy / dist)
            };
            let leader_end = (
                dir_x.mul_add(LEADER_LINE_LENGTH, cx),
                dir_y.mul_add(LEADER_LINE_LENGTH, cy),
            );
            let depth_here = frame.depth[idx];
            draw_edge(
                frame,
                (cx, cy, depth_here),
                (leader_end.0, leader_end.1, depth_here),
                style.edge_color,
                layout.clip,
                1,
            );
            leader_end
        } else {
            (cx, cy)
        };

        let (label_w, label_h) = text_size(label, 1);
        draw_text(
            frame,
            label_cx - label_w as f32 / 2.0,
            label_cy - label_h as f32 / 2.0,
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
    // Blended well toward the highlight color (58%) so a selected facet reads
    // clearly against its neighbors -- see `raster::SolidRasterizer::
    // fill_convex_polygon`'s matching comment.
    let selected_fill = selected.then(|| blend_toward(color, style.selected_color, 0.58));

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

/// Draws one edge segment `width` pixels wide (`1` for an ordinary facet
/// boundary, `2` for a `selected`/`pending` highlight -- see [`draw_panel_edges`]),
/// depth-tested against the fill pass -- see `raster::SolidRasterizer::draw_edge`'s
/// doc comment for [`EDGE_DEPTH_BIAS`]'s purpose, reused here at the same value.
const EDGE_DEPTH_BIAS: f32 = 5e-2;

fn draw_edge(
    frame: &mut DiagramFrame,
    from: (f32, f32, f32),
    to: (f32, f32, f32),
    color: [u8; 3],
    clip: (i32, i32, i32, i32),
    width: i32,
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
        let frac = step as f32 / steps_total as f32;
        let depth = (1.0 - frac).mul_add(d0, frac * d1);
        try_paint_edge_pixel(frame, cx, cy, depth, color, clip);
        // A broadened brush for the priority passes -- see `raster::
        // SolidRasterizer::draw_edge`'s matching comment.
        if width >= 2 {
            try_paint_edge_pixel(frame, cx + 1, cy, depth, color, clip);
            try_paint_edge_pixel(frame, cx, cy + 1, depth, color, clip);
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

/// Paints one edge pixel at `(x, y)` with `color` when inside `clip` and `depth`
/// passes [`EDGE_DEPTH_BIAS`]'s slack against the fill pass -- the single-pixel
/// primitive [`draw_edge`]'s Bresenham walk (and its width-2 broadening) both go
/// through.
fn try_paint_edge_pixel(
    frame: &mut DiagramFrame,
    x: i32,
    y: i32,
    depth: f32,
    color: [u8; 3],
    clip: (i32, i32, i32, i32),
) {
    if x < clip.0 || y < clip.1 || x > clip.2 || y > clip.3 {
        return;
    }
    let Some(idx) = frame.idx(x, y) else {
        return;
    };
    if depth <= frame.depth[idx] + EDGE_DEPTH_BIAS {
        let offset = idx * 4;
        frame.color[offset] = color[0];
        frame.color[offset + 1] = color[1];
        frame.color[offset + 2] = color[2];
        frame.color[offset + 3] = 255;
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
        draw_edge(frame, from, to, color, layout.clip, 1);
        // Tag a small hit region at this tick's midpoint so hover/click can
        // resolve "which tooth" from a generous target, not the 1px stroke.
        let mid_r = inner_r.midpoint(outer_r);
        let mid_x = su.mul_add(mid_r, layout.center_x).round() as i32;
        let mid_y = sv.mul_add(-mid_r, layout.center_y).round() as i32;
        tag_tooth_hit(frame, mid_x, mid_y, tooth, 4);

        if is_major {
            let label = tooth.to_string();
            let (w, h) = text_size(&label, 1);
            let lx = su.mul_add(label_r, layout.center_x) - w as f32 / 2.0;
            let ly = sv.mul_add(-label_r, layout.center_y) - h as f32 / 2.0;
            // `label_r` alone puts the two horizontal ticks' labels (3 and 9
            // o'clock) past the panel's own column edge -- `wheel_radius_px*1.2`
            // exceeds `col_width/2` whenever the wheel is sized against a
            // width-limited panel. The ticks themselves stay exactly where they
            // are; only the label's drawing origin is pulled back inside the
            // clip rect so the full glyph string survives rather than being cut
            // off mid-digit.
            let lx = lx.clamp(layout.clip.0 as f32, (layout.clip.2 as f32) - w as f32);
            let ly = ly.clamp(layout.clip.1 as f32, (layout.clip.3 as f32) - h as f32);
            draw_text(frame, lx, ly, &label, 1, color, Some(layout.clip));
        }
    }
}

/// Fills a `(2*radius+1)` square of [`DiagramFrame::tooth`] around `(cx, cy)`
/// with `tooth + 1` -- the hit-region primitive [`draw_index_wheel`]'s tagging
/// pass uses per tick. Out-of-bounds pixels are silently skipped, same as every
/// other per-pixel primitive in this module.
fn tag_tooth_hit(frame: &mut DiagramFrame, cx: i32, cy: i32, tooth: u32, radius: i32) {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if let Some(idx) = frame.idx(cx + dx, cy + dy) {
                frame.tooth[idx] = tooth + 1;
            }
        }
    }
}

/// Draws `wheel.symmetry_order` evenly spaced radial guide lines
/// (the schedule's own rotational symmetry, `ScheduleMeta::symmetry_order`) and,
/// when `wheel.mirror` is set, the design's mirror axis -- always the vertical
/// line through the panel centre, since index 0 sits at screen "up" on BOTH
/// panels regardless of `mirrored` (see this module's own doc comment: only
/// `screen_right` flips between crown and pavilion, never `screen_up`).
/// Drawn before the index wheel and the facet body so both sit on top of these
/// as background reference lines, not the other way around.
fn draw_symmetry_and_mirror_lines(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    wheel: WheelConfig,
    style: &DiagramStyle,
) {
    let mirrored = matches!(layout.kind, PanelKind::Pavilion);
    let line_len = layout.wheel_radius_px * 1.05;
    let center = (layout.center_x, layout.center_y, 0.0);

    if wheel.symmetry_order > 1 {
        for k in 0..wheel.symmetry_order {
            let phi = 2.0 * std::f32::consts::PI * (k as f32) / (wheel.symmetry_order as f32);
            let (su, sv) = wheel_direction(phi, mirrored);
            let tip = (
                su.mul_add(line_len, layout.center_x),
                sv.mul_add(-line_len, layout.center_y),
                0.0,
            );
            draw_edge(
                frame,
                center,
                tip,
                style.symmetry_line_color,
                layout.clip,
                1,
            );
        }
    }

    if wheel.mirror {
        let top = (layout.center_x, layout.center_y - line_len, 0.0);
        let bottom = (layout.center_x, layout.center_y + line_len, 0.0);
        draw_edge(frame, top, bottom, style.mirror_line_color, layout.clip, 2);
    }
}

/// Draws a radial line from the panel centre to a highlighted facet's own
/// index-wheel tooth (`DiagramStyle::facet_index_on_gear`), linking "here is
/// tooth N on the wheel" to "here is the facet sitting at that tooth". Only
/// ever called for a crown/pavilion panel (see [`render_panel`]): a profile
/// panel has no index wheel to point at.
///
/// One radial per highlighted facet, in the SAME highlight color
/// [`draw_panel_edges`] would give that facet's own boundary, at the same
/// precedence (a more specific highlight wins over a coarser one).
fn draw_selected_index_radials(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    gear_teeth: u32,
    gear_reference_angle: f32,
    style: &DiagramStyle,
    edge_ranges: &[(usize, usize, usize)],
) {
    let mirrored = matches!(layout.kind, PanelKind::Pavilion);
    for &(facet_id, _start, _count) in edge_ranges {
        let color = if style.selected_facet == Some(facet_id as u32) {
            style.selected_facet_color
        } else if style.selected.get(facet_id).copied().unwrap_or(false) {
            style.selected_color
        } else if style.multi_selected.contains(&(facet_id as u32)) {
            style.multi_selected_color
        } else {
            continue;
        };
        let Some(&index) = style.facet_index_on_gear.get(facet_id) else {
            continue;
        };
        let phi = 2.0 * std::f32::consts::PI * (index as f32 + gear_reference_angle)
            / gear_teeth.max(1) as f32;
        let (su, sv) = wheel_direction(phi, mirrored);
        let center = (layout.center_x, layout.center_y, 0.0);
        let tip = (
            su.mul_add(layout.wheel_radius_px, layout.center_x),
            sv.mul_add(-layout.wheel_radius_px, layout.center_y),
            0.0,
        );
        draw_edge(frame, center, tip, color, layout.clip, 2);
    }
}

/// Resolves [`DiagramStyle::meet_marker_pairs`] (facet-id pairs `Design::
/// facet_meets`' tier-level resolution names) to actual world-space points: the
/// vertex the two facets' mesh rings genuinely share, found by nearest-point
/// matching rather than trusting index alignment -- `facet_meets` only says
/// WHICH tiers meet, never where. A pair with no ring vertex closer than
/// `tolerance` contributes nothing (a generously over-listed candidate pair,
/// see `FacetMap::meeting_facet_pairs`'s own doc comment, not a real shared
/// vertex).
///
/// `tolerance` should scale with the mesh's own size -- [`render_diagram`] uses
/// a fraction of its bounding-box diagonal, mirroring [`simplify_ring`]'s own
/// scale-relative merge tolerance.
fn meet_marker_points(mesh: &SolidMesh, pairs: &[(u32, u32)], tolerance: f64) -> Vec<DVec3> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let ring_of = |facet_id: u32| -> Option<&Vec<DVec3>> {
        mesh.rings
            .iter()
            .find(|(id, _)| *id == facet_id as usize)
            .map(|(_, ring)| ring)
    };
    let mut points: Vec<DVec3> = Vec::new();
    for &(facet_a, facet_b) in pairs {
        let (Some(ring_a), Some(ring_b)) = (ring_of(facet_a), ring_of(facet_b)) else {
            continue;
        };
        for &pa in ring_a {
            for &pb in ring_b {
                if (pa - pb).length() <= tolerance {
                    // Dedup against points already found for an earlier pair --
                    // a shared vertex between more than two facets (a table/star
                    // apex, say) would otherwise get one marker per pair.
                    if !points.iter().any(|&p| (p - pa).length() <= tolerance) {
                        points.push(pa);
                    }
                }
            }
        }
    }
    points
}

/// Draws one meet-point marker: a small filled diamond, depth-tested against
/// `frame`'s fill pass with the same slack [`draw_edge`] uses, so a marker on
/// the far side of the stone from this panel's view stays hidden rather than
/// drawing through the solid.
fn draw_meet_marker(
    frame: &mut DiagramFrame,
    point: (f32, f32, f32),
    color: [u8; 3],
    clip: (i32, i32, i32, i32),
) {
    let (cx, cy, depth) = point;
    let (cx, cy) = (cx.round() as i32, cy.round() as i32);
    for dy in -2..=2i32 {
        for dx in -2..=2i32 {
            if dx.abs() + dy.abs() > 2 {
                continue; // Diamond, not a square.
            }
            try_paint_edge_pixel(frame, cx + dx, cy + dy, depth, color, clip);
        }
    }
}

// The glyph lookup itself (`glyph`/`GLYPH_WIDTH`/`GLYPH_HEIGHT`) lives in
// `gui::pixel_font`, shared with `gui::tilt::video_export::overlay` -- see that
// module's own doc comment. Everything below is this panel's own scaling/drawing
// code, which stays here.

/// The pixel size `text` occupies at `scale` (1 device pixel per glyph pixel at
/// `scale == 1`), including inter-character spacing but not a trailing gap.
///
/// `pub(super)` -- `raster.rs`'s orientation-marker/facet-label overlay needs
/// the same measurement to center its own labels.
pub(super) fn text_size(text: &str, scale: u32) -> (u32, u32) {
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
    let (width, height) = (frame.width, frame.height);
    draw_text_into_buffer(
        TextCanvas {
            color_buf: &mut frame.color,
            width,
            height,
        },
        x,
        y,
        text,
        scale,
        color,
        clip,
    );
}

/// The buffer-generic sibling of [`draw_text`]: draws `text` straight into a raw
/// RGBA8 `width x height` buffer rather than a [`DiagramFrame`]. `pub(super)` so
/// `raster.rs`'s orientation-marker/facet-label overlay can share this module's
/// one bitmap font instead of carrying a second copy -- see that module's own
/// doc comment for what it draws.
/// One RGBA8 drawing target for [`draw_text_into_buffer`]: the pixels plus the two
/// dimensions needed to index them.
///
/// Bundled because the three always travel together and describe one thing, which
/// also keeps that function's parameter list within the workspace's own limit.
pub(super) struct TextCanvas<'a> {
    /// RGBA8 pixels, `width * height * 4` bytes long.
    pub(super) color_buf: &'a mut [u8],
    /// Row width in pixels.
    pub(super) width: u32,
    /// Row count.
    pub(super) height: u32,
}

pub(super) fn draw_text_into_buffer(
    canvas: TextCanvas<'_>,
    x: f32,
    y: f32,
    text: &str,
    scale: u32,
    color: [u8; 3],
    clip: Option<(i32, i32, i32, i32)>,
) {
    let TextCanvas {
        color_buf,
        width,
        height,
    } = canvas;
    let clip = clip.unwrap_or((0, 0, width as i32 - 1, height as i32 - 1));
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
                        if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
                            continue;
                        }
                        let idx = (py as u32 * width + px as u32) as usize;
                        let o = idx * 4;
                        color_buf[o] = color[0];
                        color_buf[o + 1] = color[1];
                        color_buf[o + 2] = color[2];
                        color_buf[o + 3] = 255;
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
            symmetry_order: 8,
            mirror: true,
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
            symmetry_order: 8,
            mirror: false,
        };
        let frame = render_diagram(&mesh, &config, &DiagramStyle::default());
        assert_eq!(frame.width, 0);
        assert_eq!(frame.pick_at(0, 0), None);
    }

    /// The enlarged panel must fill the WHOLE frame width, not the one-third
    /// column [`render_diagram`] gives it -- otherwise "enlarge this panel"
    /// would just be a relabeled crop of the existing image.
    #[test]
    fn render_diagram_single_panel_fills_the_whole_frame() {
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
            symmetry_order: 8,
            mirror: false,
        };
        let style = DiagramStyle::default();
        let frame = render_diagram_single_panel(&mesh, &config, &style, PanelKind::Crown);

        // The crown panel's own center, now the FRAME's center (150, 60) rather
        // than a one-third column's (50, 60) -- see `render_diagram`'s own
        // matching assertion at column-center (50, 60).
        assert_eq!(
            frame.pick_at(150, 60),
            Some(2),
            "the enlarged panel's own center must show the +Y facet"
        );
        assert_eq!(frame.panel_at(150, 60), Some(PanelKind::Crown));
        // "Fills the whole frame" in the sense the API can actually express:
        // `panel_at` tags only pixels a facet paints (see its own doc comment), so a
        // background pixel is `None` by contract, not `Some`. What must hold is that
        // no pixel anywhere belongs to a DIFFERENT panel -- in the three-column
        // layout, x = 200 would have been the pavilion column.
        for y in 0..config.height {
            for x in 0..config.width {
                let tag = frame.panel_at(x, y);
                assert!(
                    tag.is_none() || tag == Some(PanelKind::Crown),
                    "pixel ({x}, {y}) is tagged {tag:?}, but only the enlarged crown                      panel may paint in a single-panel frame"
                );
            }
        }
    }

    /// [`meet_marker_points`] must find the actual shared vertices between two
    /// touching facets, not merely trust that a candidate pair (from
    /// `FacetMap::meeting_facet_pairs`) really touches.
    #[test]
    fn meet_marker_points_finds_the_shared_edge_between_two_touching_facets() {
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
        // Facet 0 (+X) and facet 2 (+Y) share the edge x=1, y=0.6, z in [-1, 1] --
        // two corners, (1, 0.6, -1) and (1, 0.6, 1).
        let points = meet_marker_points(&mesh, &[(0, 2)], 1e-4);
        assert_eq!(points.len(), 2, "got: {points:?}");
        for expected_z in [-1.0, 1.0] {
            assert!(
                points.iter().any(|p| (p.x - 1.0).abs() < 1e-6
                    && (p.y - 0.6).abs() < 1e-6
                    && (p.z - expected_z).abs() < 1e-6),
                "expected a marker at z={expected_z}, got: {points:?}"
            );
        }

        // An unrelated pair (facets that never touch) must find nothing.
        assert_eq!(meet_marker_points(&mesh, &[(0, 1)], 1e-4).len(), 0);
    }
}
