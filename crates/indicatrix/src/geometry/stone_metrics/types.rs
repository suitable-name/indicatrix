//! Data types [`measure_solid`](super::measure_solid) and
//! [`build_solid_mesh`](super::build_solid_mesh) report.
//!
//! They are the measured figures
//! themselves ([`SolidMetrics`]), the printed proportions they're checked
//! against ([`ExternalProportions`]), and the mesh-building outcome
//! ([`SolidStatus`], [`SolidMesh`]).

use glam::DVec3;

/// Everything [`measure_solid`](super::measure_solid) reports about one solid.
///
/// All figures are in the arrangement's own mast units; callers compare
/// dimensionless ratios (`volume / width^3`, `length / width`, ...) so the
/// unit never matters.
///
/// The Rough Planner caches the volume, height and caliper/axis extents in the
/// catalogue database; if the rule that measures them changes, bump
/// `indicatrix_vault::model::solid_extents::SOLID_EXTENTS_VERSION` (see
/// [`measure_solid`](super::measure_solid)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolidMetrics {
    /// Volume of the solid.
    pub volume: f64,
    /// Width: the smaller of the two axis-aligned horizontal (`x`/`z`) extents.
    pub width_axis: f64,
    /// Length: the larger of the two axis-aligned horizontal extents.
    pub length_axis: f64,
    /// Width by rotating calipers over the horizontal outline: the smallest
    /// directional extent over all outline-edge directions.
    pub width_caliper: f64,
    /// Length measured along the direction perpendicular to the caliper width.
    pub length_caliper: f64,
    /// Total height: full vertical (`y`) extent, table to culet, girdle included.
    pub total_height: f64,
    /// Crown height: top of the solid above the girdle band's top edge. `None`
    /// when the arrangement has no vertical girdle plane with a live facet.
    pub crown_height: Option<f64>,
    /// Pavilion depth: bottom of the solid below the girdle band's bottom edge.
    /// `None` when there is no live girdle facet.
    pub pavilion_depth: Option<f64>,
    /// Vertical extent of the girdle band itself. `None` without a live girdle.
    pub girdle_thickness: Option<f64>,
    /// Distinct vertices of the solid (after dedup).
    pub vertex_count: usize,
}

/// A design's printed proportion figures, as scraped into `diagram_details`.
///
/// Columns `volume`, `lw_ratio`, `cw_ratio`, `pw_ratio`, `hw_ratio`: the
/// external targets a candidate mast configuration must reproduce. Any subset
/// may be present.
///
/// Corpus calibration (full 2,881-design `.asc` corpus, true masts, measured
/// by a temporary corpus probe, deterministic run): with the **axis** width
/// convention
/// (`W` = the smaller of the two axis-aligned horizontal extents, which is the
/// convention that matched -- rotating calipers measured strictly worse on
/// every figure), each printed figure reproduces the true solid's measurement
/// with a median deviation of ~0.1% (`Vol/W^3` 95.8% of designs within 1%,
/// `L/W` 98.7%, `C/W` 88.6%, `P/W` 91.2%, `H/W` 96.8%).
#[derive(Debug, Clone, Copy, Default)]
pub struct ExternalProportions {
    /// Printed `Vol/W^3`.
    pub vol_w3: Option<f64>,
    /// Printed `L/W`.
    pub lw: Option<f64>,
    /// Printed `C/W` (crown height over width).
    pub cw: Option<f64>,
    /// Printed `P/W` (pavilion depth over width).
    pub pw: Option<f64>,
    /// Printed `H/W` (total height over width).
    pub hw: Option<f64>,
}

impl ExternalProportions {
    /// Mean relative deviation of `metrics` from the printed figures, over
    /// every figure both sides have (axis width convention -- see the type
    /// docs). `None` when nothing overlaps.
    #[must_use]
    pub fn combined_deviation(&self, metrics: &SolidMetrics) -> Option<f64> {
        let w = metrics.width_axis;
        if w < 1e-9 {
            return None;
        }
        let mut sum = 0.0_f64;
        let mut count = 0_usize;
        let mut add = |target: Option<f64>, measured: Option<f64>| {
            if let (Some(t), Some(v)) = (target, measured)
                && t > 1e-9
            {
                sum += (v - t).abs() / t;
                count += 1;
            }
        };
        add(self.vol_w3, Some(metrics.volume / (w * w * w)));
        add(self.lw, Some(metrics.length_axis / w));
        add(self.cw, metrics.crown_height.map(|c| c / w));
        add(self.pw, metrics.pavilion_depth.map(|p| p / w));
        add(self.hw, Some(metrics.total_height / w));
        if count == 0 {
            None
        } else {
            Some(sum / count as f64)
        }
    }
}

/// Everything a caller needs to render or reason about the solid a plane
/// arrangement bounds -- or, when it doesn't yet bound one, which planes are
/// responsible.
///
/// An editor calls [`build_solid_mesh`](super::build_solid_mesh) after every
/// keystroke that touches a tier. Unlike [`measure_solid`](super::measure_solid)'s
/// single `Option`, these three variants let the UI tell "not closed yet
/// because tier 34 is missing its closing neighbor" apart from "closed, but
/// numerically degenerate" apart from "here's your mesh" -- the first two are
/// ordinary states while someone is mid-edit, not error conditions that
/// should blank the viewport.
#[derive(Debug, Clone)]
pub enum SolidStatus {
    /// A finite, watertight solid.
    Closed(SolidMesh),
    /// At least one plane triple's intersection escapes to the bounding
    /// blank (see `BLANK_HALF_EXTENT`): the arrangement is missing
    /// whatever face(s) would have closed it up in that direction.
    ///
    /// `escaping` names the offending planes as indices into the slice
    /// [`build_solid_mesh`](super::build_solid_mesh) was called with (i.e.
    /// before this function's own internal plane-dedup call renumbers
    /// anything), sorted ascending and deduplicated, so the UI can say "plane
    /// 34 escapes the blank" instead of blanking the viewport.
    Unbounded {
        /// The offending plane indices, sorted ascending and deduplicated.
        escaping: Vec<usize>,
    },
    /// Bounded (no vertex reaches the blank) but not a valid solid: fewer
    /// than four distinct vertices, or a non-finite/non-positive volume.
    Degenerate {
        /// Distinct vertices found (see [`SolidMetrics::vertex_count`]).
        vertex_count: usize,
        /// `None` when the divergence-theorem sum itself was non-finite
        /// (NaN/infinite); `Some` (always `<= 0.0`, since a positive finite
        /// volume would have produced [`SolidStatus::Closed`] instead)
        /// when the sum was finite but not a real volume.
        volume: Option<f64>,
    },
}

/// A triangulated solid ready to hand to a renderer, plus the per-face
/// polygon rings an edge pass or picking wants instead of raw triangles.
///
/// Built by [`build_solid_mesh`](super::build_solid_mesh); see that
/// function's doc comment for the triangulation rule.
#[derive(Debug, Clone, Default)]
pub struct SolidMesh {
    /// One entry per mesh vertex. Vertices are duplicated across facets (see
    /// `normals`), so this is generally larger than the solid's true vertex
    /// count ([`SolidMetrics::vertex_count`]).
    pub positions: Vec<DVec3>,
    /// Per-vertex normal: exactly the owning facet's plane normal, repeated
    /// for every vertex of that facet's fan. Never averaged -- facets are
    /// flat by definition, and smooth-shading them would draw a stone that
    /// does not exist.
    pub normals: Vec<DVec3>,
    /// Per-vertex originating facet: an index into the slice
    /// [`build_solid_mesh`](super::build_solid_mesh) was called with. Gives
    /// hover/selection-by-facet for free, since every vertex already knows
    /// which facet it belongs to.
    pub facet_id: Vec<usize>,
    /// Triangle indices into `positions`/`normals`/`facet_id`, three per
    /// triangle. Each face is a centroid fan (see
    /// [`build_solid_mesh`](super::build_solid_mesh)).
    pub indices: Vec<u32>,
    /// Each face's ordered polygon ring in world space (the same ring
    /// `face_area` shoelaces internally), paired with that plane's index
    /// into the slice `build_solid_mesh` was called with. Omits faces cut
    /// away entirely (fewer than 3 vertices on the plane).
    pub rings: Vec<(usize, Vec<DVec3>)>,
    /// One outward normal per entry of `rings`; `None` on the planar path.
    ///
    /// A concave stone has several rings per `facet_id` (a facet is cut into
    /// convex pieces) and the pieces of a tool surface are curved, so the
    /// per-vertex flat normal no longer says which way a *piece* faces. This
    /// is the exact normal there: the facet's plane normal for a flat piece,
    /// the negated outward normal of the exact tool (not its polytope) at the
    /// piece centroid for a tool piece.
    pub piece_normals: Option<Vec<DVec3>>,
    /// Per ring, per segment `i -> i + 1`: whether that edge is drawn. `None`
    /// on the planar path, where every ring edge is a facet boundary.
    ///
    /// On a concave stone most ring edges are seams of the convex
    /// decomposition or between two tessellation planes of one tool, both of
    /// which sit inside a single facet and must stay invisible. An edge is
    /// visible only when the two sides belong to different `facet_id`s.
    pub edge_visible: Option<Vec<Vec<bool>>>,
}
