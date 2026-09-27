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
//!   with `normal.y` above a small edge-on threshold (see [`layout::crown_visible`]).
//! - **Pavilion**: looking straight up `+Y` from below. Visible facets have
//!   `normal.y` below the negated threshold (see [`layout::pavilion_visible`]).
//! - **Profile**: a side elevation. Rather than a single backface-culled view
//!   direction (which would show only the one facet directly facing the camera),
//!   this shows every facet that is not purely crown/pavilion-facing -- any
//!   horizontal component to its normal at all (see [`layout::profile_visible`]) --
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
//! [`layout::wheel_direction`] and [`layout::crown_project`].
//!
//! The pavilion panel is a view from the OPPOSITE side of the same horizontal
//! plane (looking up instead of down), which mirrors the horizontal screen axis
//! while keeping "up" (world `+X`) the same -- exactly like looking at a page from
//! behind. So the pavilion panel uses `screen_u = -world_z`, keeping index `0` at
//! the top but sweeping **counter-clockwise** (index 24 lands at 9 o'clock) -- see
//! [`layout::pavilion_project`]. This is not an arbitrary stylistic mirror: a tier at a
//! given index has the IDENTICAL `(world_x, world_z)` horizontal direction whether
//! it is a crown or pavilion tier (only `normal.y`'s sign differs), so mirroring
//! the pavilion's screen axis is what makes an orthographic "view from below"
//! project those same physical points correctly rather than drawing an X-ray
//! view through the stone.
//!
//! The profile panel has no index wheel (a side elevation has no azimuth to mark).

use super::raster::simplify_ring;

mod fill;
mod labels;
mod layout;
mod legend;
mod render;
#[cfg(test)]
mod tests;

pub use render::{render_diagram, render_diagram_single_panel};
// `raster.rs`'s orientation-marker/facet-label overlay shares this module's one
// bitmap font rather than carrying a second copy -- see `labels`'s own doc
// comment.
pub(super) use labels::{TextCanvas, draw_text_into_buffer, text_size};

/// Normal-component threshold below which a facet is treated as edge-on rather
/// than facing/away for a panel's visibility test -- see [`layout::crown_visible`],
/// [`layout::pavilion_visible`], [`layout::profile_visible`].
const NORMAL_EPS: f32 = 1e-3;

/// Screen-space period, in pixels, of the diagonal hatch stripes over a flagged
/// (critical-angle-risk) facet -- mirrors `raster::HATCH_PERIOD`.
const HATCH_PERIOD: i32 = 6;

/// A facet's projected on-screen span (in either axis) must be at least this many
/// pixels before [`labels::draw_panel_labels`] draws its label directly ON the facet. Kept
/// low enough that most facets still get a label even at the crown/pavilion body
/// radius typical of an 8-fold design in a ~700px-wide viewport. A facet still too
/// small for this gets a leader line instead (see [`LEADER_LINE_MIN_SPAN`]) rather
/// than being dropped silently.
const MIN_LABEL_SPAN: f32 = 10.0;

/// Below this on-screen span (in either axis) a facet is too small even to anchor
/// a leader line legibly -- a near-zero-area sliver -- so [`labels::draw_panel_labels`]
/// drops its label entirely. Facets between this and [`MIN_LABEL_SPAN`] get a
/// leader line rather than nothing.
const LEADER_LINE_MIN_SPAN: f32 = 2.5;

/// Leader-line length in pixels, radiating from a too-small facet's centroid away
/// from the panel center, with the label drawn at its far end.
const LEADER_LINE_LENGTH: f32 = 22.0;

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
    /// world-space points by [`fill::meet_marker_points`] against the SAME mesh
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
    /// unusably small click/hover target, so [`legend::draw_index_wheel`] tags a small
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
