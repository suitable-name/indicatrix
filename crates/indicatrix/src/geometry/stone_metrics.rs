//! Deterministic physical measurement of a faceted stone from its plane
//! arrangement.
//!
//! [`super::meet_solver`]'s corpus work established that wrong mast
//! configurations are *self-consistent*: every tier of a wrong solve is still
//! vertex-incident, so nothing internal to the arrangement separates right from
//! wrong. The proportions printed on a real diagram (`Vol/W^3`, `L/W`, `C/W`,
//! `P/W`, `H/W`) are **external** constraints: a candidate configuration either
//! reproduces them or it does not. [`measure_solid`] computes those same figures
//! from a plane arrangement -- deterministically, in `f64`, with no convex-hull
//! library ([`super::brep`]'s `chull` hull is nondeterministic and must stay out
//! of every solver decision path).
//!
//! The mechanism: enumerate the solid's vertices as feasible triple
//! intersections of the planes (the same primitive `meet_solver` uses), then
//! reconstruct each facet's polygon by collecting the vertices on its plane and
//! ordering them by angle. Volume comes from the divergence theorem
//! (`V = (1/3) * sum over faces of offset * area`, exact for outward-oriented
//! planes), heights from vertex `y` extents against the girdle band, and
//! width/length from the girdle outline's `x`/`z` extents (both axis-aligned
//! and rotating-caliper are computed; corpus measurement settled that the
//! printed figures use the axis convention -- see [`ExternalProportions`]).

use glam::DVec3;

/// Half-extent of the bounding box standing in for the uncut rough. Matches
/// `meet_solver`'s own blank; a solid that reaches it is unbounded (a schedule
/// missing its closing planes), which [`measure_solid`] reports as `None`.
const BLANK_HALF_EXTENT: f64 = 64.0;

/// Feasibility slack: a vertex may poke this far (absolute; masts are ~1) beyond
/// a plane and still count as part of the solid. Matches `meet_solver::EPS_FEAS`.
const EPS_FEAS: f64 = 1e-5;

/// A vertex within this absolute distance of a plane counts as lying on its
/// face. Sized to cover [`EPS_FEAS`] feasibility drift and [`VERTEX_DEDUP`]
/// merging; the resulting polygon-corner error is quadratically small in it.
const EPS_FACE: f64 = 2e-5;

/// Minimum `|determinant|` for a triple of unit plane normals to define a
/// candidate vertex. Matches `meet_solver::MIN_TRIPLE_DET`.
const MIN_TRIPLE_DET: f64 = 1e-6;

/// Two candidate vertices within this distance (per axis) are one vertex.
const VERTEX_DEDUP: f64 = 1e-6;

/// `|normal.y|` at or below this means a vertical (girdle) plane -- the same
/// threshold `meet_solver::classify_blocks` uses.
const GIRDLE_NY: f64 = 1e-6;

/// `normal.y` at or above this counts as a flat table facet: an outward
/// normal within this of straight up (`+Y`) -- the horizontal-plane
/// counterpart to [`GIRDLE_NY`]'s vertical-plane threshold.
const TABLE_NY: f64 = 1.0 - 1e-6;

/// Everything [`measure_solid`] reports about one solid.
///
/// All figures are in the arrangement's own mast units; callers compare
/// dimensionless ratios (`volume / width^3`, `length / width`, ...) so the
/// unit never matters.
#[derive(Debug, Clone, Copy)]
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
/// An editor calls [`build_solid_mesh`] after every keystroke that touches a
/// tier. Unlike [`measure_solid`]'s single `Option`, these three variants let
/// the UI tell "not closed yet because tier 34 is missing its closing
/// neighbor" apart from "closed, but numerically degenerate" apart from
/// "here's your mesh" -- the first two are ordinary states while someone is
/// mid-edit, not error conditions that should blank the viewport.
#[derive(Debug, Clone)]
pub enum SolidStatus {
    /// A finite, watertight solid.
    Closed(SolidMesh),
    /// At least one plane triple's intersection escapes to the bounding
    /// blank (see [`BLANK_HALF_EXTENT`]): the arrangement is missing
    /// whatever face(s) would have closed it up in that direction.
    ///
    /// `escaping` names the offending planes as indices into the slice
    /// [`build_solid_mesh`] was called with (i.e. before this function's own
    /// internal [`dedup_planes`] call renumbers anything), sorted ascending
    /// and deduplicated, so the UI can say "plane 34 escapes the blank"
    /// instead of blanking the viewport.
    Unbounded { escaping: Vec<usize> },
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
/// Built by [`build_solid_mesh`]; see that function's doc comment for the
/// triangulation rule.
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
    /// [`build_solid_mesh`] was called with. Gives hover/selection-by-facet
    /// for free, since every vertex already knows which facet it belongs to.
    pub facet_id: Vec<usize>,
    /// Triangle indices into `positions`/`normals`/`facet_id`, three per
    /// triangle. Each face is a centroid fan (see [`build_solid_mesh`]).
    pub indices: Vec<u32>,
    /// Each face's ordered polygon ring in world space (the same ring
    /// [`face_area`] shoelaces internally), paired with that plane's index
    /// into the slice `build_solid_mesh` was called with. Omits faces cut
    /// away entirely (fewer than 3 vertices on the plane).
    pub rings: Vec<(usize, Vec<DVec3>)>,
}

/// One deduplicated vertex of the solid.
struct SolidVertex {
    v: DVec3,
}

/// Accepted vertices, in first-seen order, plus an index over them sorted by
/// `x` so a new candidate's duplicate check only has to scan the vertices
/// that could possibly be within [`VERTEX_DEDUP`] of it on every axis instead
/// of the full accepted set.
///
/// `verts`' order is exactly the push order of [`insert_if_new`](Self::insert_if_new)
/// calls (the divergence-theorem sum in [`measure_solid`] depends on it);
/// `by_x` is a lookup structure only, never observed by callers.
#[derive(Default)]
struct VertexAccumulator {
    verts: Vec<SolidVertex>,
    /// Indices into `verts`, kept sorted ascending by `verts[i].v.x`.
    by_x: Vec<usize>,
}

impl VertexAccumulator {
    /// Inserts `v` unless some already-accepted vertex is within
    /// [`VERTEX_DEDUP`] of it on every axis -- identical to a linear scan
    /// testing `(s.v - v).abs().max_element() < VERTEX_DEDUP` against every
    /// prior vertex, just restricted up front to the `x`-sorted window that
    /// could possibly match (any vertex outside `[v.x - VERTEX_DEDUP, v.x +
    /// VERTEX_DEDUP]` fails the `x`-axis check alone, so narrowing to that
    /// window changes no accept/reject decision).
    fn insert_if_new(&mut self, v: DVec3) {
        let verts = &self.verts;
        let lo = self
            .by_x
            .partition_point(|&i| verts[i].v.x < v.x - VERTEX_DEDUP);
        let hi = self
            .by_x
            .partition_point(|&i| verts[i].v.x <= v.x + VERTEX_DEDUP);
        for &i in &self.by_x[lo..hi] {
            if (self.verts[i].v - v).abs().max_element() < VERTEX_DEDUP {
                return;
            }
        }
        let idx = self.verts.len();
        self.verts.push(SolidVertex { v });
        let pos = self.by_x.partition_point(|&i| self.verts[i].v.x < v.x);
        self.by_x.insert(pos, idx);
    }
}

/// Measures the solid bounded by `planes` (`n . x <= m`, unit outward normals).
///
/// Returns `None` when the solid is degenerate or unbounded: fewer than four
/// distinct vertices, zero/negative volume, or any vertex escaping to the
/// bounding blank (a schedule missing its closing planes).
///
/// Deterministic by construction: plain nested loops over the given plane
/// order, total-order sorts, no hashing, no convex-hull library. Two calls with
/// identical inputs produce byte-identical results.
#[must_use]
pub fn measure_solid(planes: &[(DVec3, f64)]) -> Option<SolidMetrics> {
    let planes = dedup_planes(planes);
    let verts = feasible_vertices(&planes)?;
    if verts.len() < 4 {
        return None;
    }

    let volume: f64 = planes
        .iter()
        .map(|&(n, m)| m * face_area(n, m, &verts) / 3.0)
        .sum();
    if !(volume.is_finite() && volume > 0.0) {
        return None;
    }

    let y_max = verts
        .iter()
        .map(|s| s.v.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let y_min = verts.iter().map(|s| s.v.y).fold(f64::INFINITY, f64::min);

    // Girdle band: vertical extent of the vertices lying on any vertical plane.
    let mut girdle_top = f64::NEG_INFINITY;
    let mut girdle_bottom = f64::INFINITY;
    for &(n, m) in planes.iter().filter(|(n, _)| n.y.abs() <= GIRDLE_NY) {
        for s in &verts {
            if (n.dot(s.v) - m).abs() <= EPS_FACE {
                girdle_top = girdle_top.max(s.v.y);
                girdle_bottom = girdle_bottom.min(s.v.y);
            }
        }
    }
    let has_girdle = girdle_top >= girdle_bottom;

    let x_max = verts
        .iter()
        .map(|s| s.v.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let x_min = verts.iter().map(|s| s.v.x).fold(f64::INFINITY, f64::min);
    let z_max = verts
        .iter()
        .map(|s| s.v.z)
        .fold(f64::NEG_INFINITY, f64::max);
    let z_min = verts.iter().map(|s| s.v.z).fold(f64::INFINITY, f64::min);
    let (dx, dz) = (x_max - x_min, z_max - z_min);
    let (width_axis, length_axis) = if dx <= dz { (dx, dz) } else { (dz, dx) };

    let outline: Vec<(f64, f64)> = verts.iter().map(|s| (s.v.x, s.v.z)).collect();
    let (width_caliper, length_caliper) =
        caliper_extents(&outline).unwrap_or((width_axis, length_axis));

    Some(SolidMetrics {
        volume,
        width_axis,
        length_axis,
        width_caliper,
        length_caliper,
        total_height: y_max - y_min,
        crown_height: has_girdle.then_some(y_max - girdle_top),
        pavilion_depth: has_girdle.then_some(girdle_bottom - y_min),
        girdle_thickness: has_girdle.then_some(girdle_top - girdle_bottom),
        vertex_count: verts.len(),
    })
}

/// A design's proportion readouts the way a cutter quotes them off a
/// finished stone.
///
/// Table size as a percentage of width, crown height, pavilion depth, total
/// depth, girdle thickness, and the ratios (L/W, C/W, P/W) every faceting
/// diagram prints alongside them.
///
/// All lengths are in the arrangement's own mast units, the same convention
/// [`SolidMetrics`] uses -- multiply by a real millimetres-per-unit scale
/// (e.g. `indicatrix_cut_core::yield_metrics::mm_per_unit`) for a physical
/// figure, or use [`Self::to_mm`]. `table_percent`, `length_to_width` and the
/// `_to_width_percent` fields are already scale-invariant ratios/percentages
/// and never need that conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoneProportions {
    /// The table facet's own horizontal extent, as a percentage of the
    /// stone's [`SolidMetrics::width_axis`] -- the single figure every
    /// faceting diagram prints for a round or near-round cut. `None` when
    /// the arrangement has no single facet with an outward normal near
    /// straight up (e.g. a pointed crown authored with no table at all).
    pub table_percent: Option<f64>,
    /// [`SolidMetrics::crown_height`], unchanged.
    pub crown_height: Option<f64>,
    /// [`SolidMetrics::pavilion_depth`], unchanged.
    pub pavilion_depth: Option<f64>,
    /// [`SolidMetrics::girdle_thickness`], unchanged. `None` under the exact
    /// same condition as `crown_height`/`pavilion_depth`: no vertical girdle
    /// plane with a live facet (e.g. the manual's chapter-7 0-degree-"girdle"
    /// example, which classifies as a second table instead -- see
    /// `crates/indicatrix-cut-core`'s CAD audit item 115 notes).
    pub girdle_thickness: Option<f64>,
    /// [`SolidMetrics::total_height`], under the name a cutter actually uses
    /// for it ("total depth": table to culet).
    pub total_depth: f64,
    /// [`SolidMetrics::length_axis`] over [`SolidMetrics::width_axis`].
    /// `None` only when `width_axis` is not positive -- a degenerate solid
    /// that should never reach here in practice, since [`measure_solid`]
    /// already refuses to return a non-positive-volume arrangement, but the
    /// ratio is still guarded rather than dividing by zero.
    pub length_to_width: Option<f64>,
    /// `crown_height` as a percentage of `width_axis` -- the printed "C/W"
    /// figure. `None` whenever `crown_height` itself is `None`, or
    /// `width_axis` is not positive.
    pub crown_to_width_percent: Option<f64>,
    /// `pavilion_depth` as a percentage of `width_axis` -- the printed "P/W"
    /// figure. `None` whenever `pavilion_depth` itself is `None`, or
    /// `width_axis` is not positive.
    pub pavilion_to_width_percent: Option<f64>,
    /// `girdle_thickness` as a percentage of `width_axis` -- the girdle
    /// thickness figure every faceter checks alongside table/crown/pavilion.
    /// `None` whenever `girdle_thickness` itself is `None`, or `width_axis`
    /// is not positive.
    pub girdle_to_width_percent: Option<f64>,
}

impl StoneProportions {
    /// Derives every figure from an already-measured solid: `metrics` for
    /// the extents, `mesh` and `planes` (the same slice [`build_solid_mesh`]
    /// was called with, in the same order) for the table facet's own ring,
    /// which `metrics` alone does not carry.
    #[must_use]
    pub fn from_solid(metrics: &SolidMetrics, mesh: &SolidMesh, planes: &[(DVec3, f64)]) -> Self {
        let width = (metrics.width_axis > 1e-9).then_some(metrics.width_axis);
        let to_width_percent =
            |value: Option<f64>| Option::zip(value, width).map(|(v, w)| 100.0 * v / w);
        Self {
            table_percent: table_percent(metrics, mesh, planes),
            crown_height: metrics.crown_height,
            pavilion_depth: metrics.pavilion_depth,
            girdle_thickness: metrics.girdle_thickness,
            total_depth: metrics.total_height,
            length_to_width: width.map(|w| metrics.length_axis / w),
            crown_to_width_percent: to_width_percent(metrics.crown_height),
            pavilion_to_width_percent: to_width_percent(metrics.pavilion_depth),
            girdle_to_width_percent: to_width_percent(metrics.girdle_thickness),
        }
    }

    /// Scales the absolute-length fields (crown height, pavilion depth,
    /// girdle thickness, total depth) by `mm_per_unit`; every ratio/percentage
    /// field (`table_percent`, `length_to_width`, the three `_to_width_percent`
    /// fields) passes through unchanged -- the same linear-factor convention
    /// `indicatrix_cut_core::yield_metrics::PreformFit::to_mm` uses.
    #[must_use]
    pub fn to_mm(&self, mm_per_unit: f64) -> Self {
        Self {
            table_percent: self.table_percent,
            crown_height: self.crown_height.map(|v| v * mm_per_unit),
            pavilion_depth: self.pavilion_depth.map(|v| v * mm_per_unit),
            girdle_thickness: self.girdle_thickness.map(|v| v * mm_per_unit),
            total_depth: self.total_depth * mm_per_unit,
            length_to_width: self.length_to_width,
            crown_to_width_percent: self.crown_to_width_percent,
            pavilion_to_width_percent: self.pavilion_to_width_percent,
            girdle_to_width_percent: self.girdle_to_width_percent,
        }
    }
}

/// The table facet's own horizontal extent, as a percentage of the solid's
/// width -- the geometric half of [`StoneProportions::table_percent`].
///
/// Finds the ring in `mesh.rings` whose originating plane (looked up in
/// `planes`) has an outward normal within [`TABLE_NY`] of straight up
/// (`+Y`); among any that match (there should be exactly one on a normal
/// faceted stone), picks the one whose vertices sit highest, breaking a tie
/// by plane order for determinism. Measures that ring's own horizontal
/// extent with the same axis convention [`measure_solid`] uses for the whole
/// stone's `width_axis` (the smaller of the two axis-aligned extents), so
/// `table_percent` and `width_axis` are always directly comparable.
fn table_percent(metrics: &SolidMetrics, mesh: &SolidMesh, planes: &[(DVec3, f64)]) -> Option<f64> {
    if metrics.width_axis <= 1e-9 {
        return None;
    }
    let mut best: Option<(f64, &Vec<DVec3>)> = None;
    for (plane_idx, ring) in &mesh.rings {
        let Some(&(normal, _)) = planes.get(*plane_idx) else {
            continue;
        };
        if normal.y < TABLE_NY || ring.len() < 3 {
            continue;
        }
        let top = ring.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max);
        if best.is_none_or(|(best_top, _)| top > best_top) {
            best = Some((top, ring));
        }
    }
    let (_, ring) = best?;
    let x_max = ring.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max);
    let x_min = ring.iter().map(|v| v.x).fold(f64::INFINITY, f64::min);
    let z_max = ring.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
    let z_min = ring.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
    let table_width = (x_max - x_min).min(z_max - z_min);
    Some(100.0 * table_width / metrics.width_axis)
}

/// Drops duplicate planes (same normal and offset within tight tolerance) so a
/// tier that lists the same index twice can't double-count its face's area.
fn dedup_planes(planes: &[(DVec3, f64)]) -> Vec<(DVec3, f64)> {
    let mut out: Vec<(DVec3, f64)> = Vec::with_capacity(planes.len());
    for &(n, m) in planes {
        let dup = out
            .iter()
            .any(|&(n2, m2)| n.dot(n2) > 1.0 - 1e-12 && (m - m2).abs() < 1e-9);
        if !dup {
            out.push((n, m));
        }
    }
    out
}

/// Drains one full (or final partial) [`crate::simd::TripleBatch`] solve into
/// `verts`, in ascending lane order. Returns `None` the moment a vertex
/// escapes to the blank box, propagated by the caller via `?` -- matching the
/// original loop's immediate `return None`. Shared by
/// [`feasible_vertices`]'s batching loop.
fn flush_solid_batch(
    batch: &crate::simd::TripleBatch,
    soa: &crate::simd::PlanesSoA64,
    acc: &mut VertexAccumulator,
) -> Option<()> {
    let sol = crate::simd::solve_triple_batch(batch);
    for lane in 0..batch.len {
        if sol.det[lane].abs() < MIN_TRIPLE_DET {
            continue;
        }
        let v = DVec3::new(sol.vx[lane], sol.vy[lane], sol.vz[lane]);
        if v.abs().max_element() > BLANK_HALF_EXTENT + 1.0 {
            continue;
        }
        if crate::simd::any_violation(soa, v, EPS_FEAS) {
            continue;
        }
        // A feasible vertex at the blank box means the real planes never
        // closed the solid up -- there is no finite stone to measure.
        if v.abs().max_element() > BLANK_HALF_EXTENT - 1.0 {
            return None;
        }
        acc.insert_if_new(v);
    }
    Some(())
}

/// Enumerates the solid's distinct vertices: every well-conditioned plane triple
/// whose intersection satisfies all half-spaces (within [`EPS_FEAS`]), then
/// deduplicated by position. Returns `None` when any vertex reaches the bounding
/// blank (the real planes don't bound a finite solid).
///
/// Batched through `crate::simd`, matching
/// `meet_solver::enumerate_candidate_vertices`: one `PlanesSoA64` built up
/// front (owner is irrelevant to this owner-free scan, so every plane is
/// pushed with owner 0), triples solved via `solve_triple_batch` via
/// [`flush_solid_batch`], and the `any()` feasibility scan replaced by
/// `any_violation` -- bit-identical per lane to the `glam` `DMat3` sequence
/// and scalar scan they replace (see `src/simd.rs`'s determinism contract).
/// Lanes are drained in ascending order and triples are still generated by
/// the same nested loops in the same order, so vertex order and every
/// decision here (determinant check, bounds check, feasibility,
/// blank-escape) match the unbatched scalar version exactly.
fn feasible_vertices(planes: &[(DVec3, f64)]) -> Option<Vec<SolidVertex>> {
    let mut all: Vec<(DVec3, f64)> = planes.to_vec();
    for n in [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ] {
        all.push((n, BLANK_HALF_EXTENT));
    }

    let mut soa = crate::simd::PlanesSoA64::with_capacity(all.len());
    for &(n, m) in &all {
        soa.push(n, m, 0);
    }

    let p = all.len();
    let mut acc = VertexAccumulator::default();
    let mut batch = crate::simd::TripleBatch::default();
    for a in 0..p {
        for b in (a + 1)..p {
            for c in (b + 1)..p {
                let (pa, pb, pc) = (all[a], all[b], all[c]);
                if batch.push((pa.0, pa.1), (pb.0, pb.1), (pc.0, pc.1)) {
                    flush_solid_batch(&batch, &soa, &mut acc)?;
                    batch = crate::simd::TripleBatch::default();
                }
            }
        }
    }
    if batch.len > 0 {
        flush_solid_batch(&batch, &soa, &mut acc)?;
    }
    Some(acc.verts)
}

/// Maps each plane of `deduped` (in order) back to its index in `original`.
///
/// Valid because [`dedup_planes`] is a pure filter: it copies each retained
/// plane through unchanged (no numeric transform) and never reorders
/// anything, only drops exact repeats -- so `deduped` is, element for
/// element, an order-preserving subsequence of `original`. A single lockstep
/// scan (advancing `original`'s cursor past every plane it consumes, whether
/// kept or skipped) recovers the mapping without re-implementing
/// `dedup_planes`'s own equality test, and without ever revisiting an
/// `original` index already assigned to an earlier `deduped` entry (so two
/// bit-identical original planes are told apart by position, not silently
/// both mapped to the first).
fn dedup_origin_indices(original: &[(DVec3, f64)], deduped: &[(DVec3, f64)]) -> Vec<usize> {
    let same = |a: (DVec3, f64), b: (DVec3, f64)| {
        a.0.x.to_bits() == b.0.x.to_bits()
            && a.0.y.to_bits() == b.0.y.to_bits()
            && a.0.z.to_bits() == b.0.z.to_bits()
            && a.1.to_bits() == b.1.to_bits()
    };
    let mut out = Vec::with_capacity(deduped.len());
    let mut cursor = 0usize;
    for &d in deduped {
        while cursor < original.len() && !same(original[cursor], d) {
            cursor += 1;
        }
        // `cursor == original.len()` here would mean `deduped` contains a
        // plane `dedup_planes` could not have produced from `original` --
        // an invariant violation, not a real runtime case. Clamping instead
        // of panicking keeps this diagnostic helper infallible even if that
        // invariant is ever broken by a future edit to `dedup_planes`.
        out.push(cursor.min(original.len().saturating_sub(1)));
        cursor += 1;
    }
    out
}

/// Diagnostic re-scan of `planes` (already deduped), used only when
/// [`feasible_vertices`] reports the arrangement unbounded: replays the same
/// augmented-triple enumeration and the same escape test
/// (`flush_solid_batch`'s `v.abs().max_element() > BLANK_HALF_EXTENT - 1.0`),
/// but instead of stopping at the first escaping vertex, visits every triple
/// and collects which of the REAL (non-blank) plane indices participate in
/// at least one.
///
/// Not SIMD-batched, unlike [`feasible_vertices`]: this only runs on an
/// editor's invalid intermediate states, never on the success path measured
/// by any perf budget, so a plain `glam` solve per triple (the same
/// arithmetic `feasible_vertices`'s batches compute, just one triple at a
/// time) is the right tradeoff here -- obviously correct beats fast.
fn escaping_plane_indices(planes: &[(DVec3, f64)]) -> Vec<usize> {
    let real_count = planes.len();
    let mut all: Vec<(DVec3, f64)> = planes.to_vec();
    for n in [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ] {
        all.push((n, BLANK_HALF_EXTENT));
    }

    let p = all.len();
    let mut escaping: Vec<usize> = Vec::new();
    for a in 0..p {
        for b in (a + 1)..p {
            for c in (b + 1)..p {
                let (na, ma) = all[a];
                let (nb, mb) = all[b];
                let (nc, mc) = all[c];
                let mat = glam::DMat3::from_cols(na, nb, nc).transpose();
                let det = mat.determinant();
                if det.abs() < MIN_TRIPLE_DET {
                    continue;
                }
                let v = mat.inverse() * DVec3::new(ma, mb, mc);
                if v.abs().max_element() > BLANK_HALF_EXTENT + 1.0 {
                    continue;
                }
                if all.iter().any(|&(n, m)| n.dot(v) - m > EPS_FEAS) {
                    continue;
                }
                if v.abs().max_element() > BLANK_HALF_EXTENT - 1.0 {
                    for idx in [a, b, c] {
                        if idx < real_count && !escaping.contains(&idx) {
                            escaping.push(idx);
                        }
                    }
                }
            }
        }
    }
    escaping.sort_unstable();
    escaping
}

/// Builds a triangulated, watertight mesh of the solid `planes` bounds -- or
/// reports why it doesn't bound one yet (see [`SolidStatus`]).
///
/// Only the *target* differs from [`measure_solid`]; the vertex enumeration
/// is identical (the same [`dedup_planes`] then [`feasible_vertices`] call,
/// the same tolerances), so a [`SolidStatus::Closed`] mesh's own
/// divergence-theorem volume (computed from its triangles) matches
/// `measure_solid`'s figure for the same `planes` -- exercised in this
/// module's tests.
///
/// # Triangulation
///
/// Each face is fan-triangulated **about its own centroid**, not its first
/// ring vertex. A schedule's facets are frequently thin slivers (a tier
/// meeting a crease at a shallow angle), and fanning from a corner of a thin
/// polygon produces triangles whose long edge is nearly the whole polygon
/// diagonal and whose opposite angle is near zero -- exactly the
/// degenerate-triangle shape rasterizers and normal-dependent shading handle
/// worst. A centroid fan instead produces exactly `k` triangles for a
/// `k`-gon, each spanning one ring edge and the centroid, so no triangle's
/// area can exceed roughly `1/k` of the face's -- bounded regardless of how
/// thin the polygon is.
#[must_use]
pub fn build_solid_mesh(planes: &[(DVec3, f64)]) -> SolidStatus {
    let deduped = dedup_planes(planes);
    let origin = dedup_origin_indices(planes, &deduped);

    let Some(verts) = feasible_vertices(&deduped) else {
        let mut escaping: Vec<usize> = escaping_plane_indices(&deduped)
            .into_iter()
            .map(|i| origin[i])
            .collect();
        escaping.sort_unstable();
        escaping.dedup();
        return SolidStatus::Unbounded { escaping };
    };
    if verts.len() < 4 {
        return SolidStatus::Degenerate {
            vertex_count: verts.len(),
            volume: None,
        };
    }

    let volume: f64 = deduped
        .iter()
        .map(|&(n, m)| m * face_area(n, m, &verts) / 3.0)
        .sum();
    if !(volume.is_finite() && volume > 0.0) {
        return SolidStatus::Degenerate {
            vertex_count: verts.len(),
            volume: volume.is_finite().then_some(volume),
        };
    }

    let mut mesh = SolidMesh::default();
    for (i, &(normal, offset)) in deduped.iter().enumerate() {
        let Some((ring, centroid)) = face_ring(normal, offset, &verts) else {
            continue;
        };
        let facet_idx = origin[i];
        let base = mesh.positions.len() as u32;

        // Centroid vertex first, then the ring in angular order -- see this
        // function's doc comment for why the fan pivots on the centroid
        // rather than `ring[0]`.
        mesh.positions.push(centroid);
        mesh.normals.push(normal);
        mesh.facet_id.push(facet_idx);
        for &v in &ring {
            mesh.positions.push(v);
            mesh.normals.push(normal);
            mesh.facet_id.push(facet_idx);
        }

        let k = ring.len() as u32;
        for e in 0..k {
            let a = base + 1 + e;
            let b = base + 1 + (e + 1) % k;
            mesh.indices.push(base);
            mesh.indices.push(a);
            mesh.indices.push(b);
        }
        mesh.rings.push((facet_idx, ring));
    }

    SolidStatus::Closed(mesh)
}

/// Ordered polygon ring of the face polygon that plane `(normal, offset)`
/// contributes to the solid: the vertices lying on the plane (within
/// [`EPS_FACE`]), sorted by angle about their centroid using a deterministic
/// in-plane basis (the world axis least aligned with the normal). `None`
/// when fewer than three vertices lie on the plane -- the facet was cut away
/// entirely, the same case [`face_area`] reports as zero area.
///
/// This is the vertex-collection-and-ordering half of what used to be
/// `face_area`'s whole body, pulled out so [`build_solid_mesh`] can reuse the
/// ring itself (for triangulation, and for an edge/picking pass) instead of
/// only the scalar area `face_area` shoelaces it down to.
fn face_ring(normal: DVec3, offset: f64, verts: &[SolidVertex]) -> Option<(Vec<DVec3>, DVec3)> {
    let on_face: Vec<DVec3> = verts
        .iter()
        .map(|s| s.v)
        .filter(|v| (normal.dot(*v) - offset).abs() <= EPS_FACE)
        .collect();
    if on_face.len() < 3 {
        return None;
    }

    // Deterministic in-plane basis: start from the world axis least aligned
    // with the normal.
    let seed = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
        DVec3::X
    } else if normal.y.abs() <= normal.z.abs() {
        DVec3::Y
    } else {
        DVec3::Z
    };
    let basis_u = (seed - normal * normal.dot(seed)).normalize();
    let basis_v = normal.cross(basis_u);

    let centroid = on_face.iter().copied().sum::<DVec3>() / on_face.len() as f64;
    let mut angled: Vec<(f64, DVec3)> = on_face
        .into_iter()
        .map(|vert| {
            let d = vert - centroid;
            (basis_v.dot(d).atan2(basis_u.dot(d)), vert)
        })
        .collect();
    angled.sort_by(|x, y| x.0.total_cmp(&y.0));
    Some((angled.into_iter().map(|(_, v)| v).collect(), centroid))
}

/// Area of the face polygon that plane `(normal, offset)` contributes to the
/// solid: [`face_ring`]'s ordered polygon, shoelace-summed about its
/// centroid. Zero when fewer than three vertices lie on the plane (the facet
/// was cut away entirely).
///
/// Takes the centroid from [`face_ring`] rather than recomputing it from the
/// returned ring. That is not a convenience: `face_ring` sorts the ring by
/// angle, and float addition is not associative, so summing the same points
/// in post-sort order can differ in the last bit from the pre-sort
/// (as-filtered) order this function used before `face_ring` was split out.
/// `measure_solid`'s volume is a sum of `offset * face_area(...) / 3` over
/// every plane, so that last bit is observable -- the volume is contractually
/// byte-identical across that refactor (see this module's tests). `face_ring`
/// computing the centroid before it sorts is what preserves the order for
/// free; recomputing it here from the sorted ring would silently break the
/// guarantee, and recomputing it from a second filter pass would cost an
/// extra scan and allocation per face on a function that runs over every
/// plane of every design in the catalogue.
fn face_area(normal: DVec3, offset: f64, verts: &[SolidVertex]) -> f64 {
    let Some((ring, centroid)) = face_ring(normal, offset, verts) else {
        return 0.0;
    };

    let mut cross_sum = DVec3::ZERO;
    for i in 0..ring.len() {
        let a = ring[i] - centroid;
        let b = ring[(i + 1) % ring.len()] - centroid;
        cross_sum += a.cross(b);
    }
    0.5 * normal.dot(cross_sum).abs()
}

/// Rotating-caliper width and length of a 2D point set: the smallest directional
/// extent over all convex-outline edge directions, and the extent along the
/// perpendicular direction. Returns `None` when the outline is degenerate
/// (fewer than three distinct hull points).
fn caliper_extents(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let hull = convex_hull_2d(points);
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f64, f64)> = None;
    for i in 0..hull.len() {
        let (px, pz) = hull[i];
        let (qx, qz) = hull[(i + 1) % hull.len()];
        let (ex, ez) = (qx - px, qz - pz);
        let len = ex.hypot(ez);
        if len < 1e-12 {
            continue;
        }
        let (dx, dz) = (ex / len, ez / len);
        let mut along = (f64::INFINITY, f64::NEG_INFINITY);
        let mut across = (f64::INFINITY, f64::NEG_INFINITY);
        for &(x, z) in &hull {
            let a = x.mul_add(dx, z * dz);
            let c = x.mul_add(-dz, z * dx);
            along = (along.0.min(a), along.1.max(a));
            across = (across.0.min(c), across.1.max(c));
        }
        let width = across.1 - across.0;
        let length = along.1 - along.0;
        if best.is_none_or(|(bw, _)| width < bw) {
            best = Some((width, length));
        }
    }
    best.map(|(w, l)| if w <= l { (w, l) } else { (l, w) })
}

/// Andrew's monotone-chain convex hull over 2D points, counterclockwise.
/// Deterministic: total-order lexicographic sort, no hashing.
fn convex_hull_2d(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = points.to_vec();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| -> f64 {
        (a.0 - o.0).mul_add(b.1 - o.1, -((a.1 - o.1) * (b.0 - o.0)))
    };
    let mut lower: Vec<(f64, f64)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axis-aligned box `[-1,1] x [-0.6,0.6] x [-1,1]`: volume 4.8, width 2,
    /// length 2, height 1.2. The four vertical walls are girdle planes cut by
    /// nothing, so the girdle band spans the full height and crown/pavilion are
    /// zero.
    #[test]
    fn measures_a_plain_box() {
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::NEG_Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        let m = measure_solid(&planes).expect("box must measure");
        assert!((m.volume - 4.8).abs() < 1e-9, "volume {}", m.volume);
        assert!((m.width_axis - 2.0).abs() < 1e-9);
        assert!((m.length_axis - 2.0).abs() < 1e-9);
        assert!((m.width_caliper - 2.0).abs() < 1e-9);
        assert!((m.total_height - 1.2).abs() < 1e-9);
        assert_eq!(m.vertex_count, 8);
        assert!((m.crown_height.expect("girdle present")).abs() < 1e-9);
        assert!((m.pavilion_depth.expect("girdle present")).abs() < 1e-9);
        assert!((m.girdle_thickness.expect("girdle present") - 1.2).abs() < 1e-9);
    }

    /// A hip-roofed block: square girdle walls at `|x|,|z| <= 1`, flat floor at
    /// `y = -0.5`, and four 45-degree crown planes `y <= 1 - |x|`, `y <= 1 - |z|`.
    /// Hand-computed: volume `2 + 4/3`, ridge apex at `y = 1`, girdle band
    /// clipped at `y = 0`, so crown height 1, pavilion depth 0, girdle 0.5.
    #[test]
    fn measures_crown_height_against_the_girdle_band() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
            (DVec3::NEG_Y, 0.5),
            // 45-degree crown planes: n = (+-s, s, 0) and (0, s, +-s), m = s,
            // i.e. x + y = 1 etc.
            (DVec3::new(s, s, 0.0), s),
            (DVec3::new(-s, s, 0.0), s),
            (DVec3::new(0.0, s, s), s),
            (DVec3::new(0.0, s, -s), s),
        ];
        let m = measure_solid(&planes).expect("roofed block must measure");
        assert!(
            (m.volume - (2.0 + 4.0 / 3.0)).abs() < 1e-9,
            "volume {}",
            m.volume
        );
        assert!((m.total_height - 1.5).abs() < 1e-9);
        assert!((m.crown_height.expect("girdle present") - 1.0).abs() < 1e-9);
        assert!((m.pavilion_depth.expect("girdle present")).abs() < 1e-9);
        assert!((m.girdle_thickness.expect("girdle present") - 0.5).abs() < 1e-9);
        assert!((m.width_axis - 2.0).abs() < 1e-9);
    }

    /// The plain box's flat top IS its table (normal exactly `+Y`), spanning
    /// the whole width -- `table_percent` must read 100%, and
    /// `length_to_width` must be 1.0 (a square footprint).
    #[test]
    fn stone_proportions_reads_100_percent_table_on_a_plain_box() {
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::NEG_Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        let metrics = measure_solid(&planes).expect("box must measure");
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            panic!("box must close");
        };
        let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
        assert!((proportions.table_percent.expect("box top is a table") - 100.0).abs() < 1e-6);
        assert!((proportions.length_to_width.expect("positive width") - 1.0).abs() < 1e-9);
        assert!((proportions.total_depth - metrics.total_height).abs() < 1e-9);
        // The box's four side planes are all girdle (normal.y == 0), spanning the
        // whole height, so crown/pavilion are both zero and the girdle band is the
        // box's whole 1.2-unit height over a width of 2.0 -- 60%.
        assert!((proportions.crown_to_width_percent.expect("girdle present")).abs() < 1e-9);
        assert!(
            (proportions
                .pavilion_to_width_percent
                .expect("girdle present"))
            .abs()
                < 1e-9
        );
        assert!((proportions.girdle_to_width_percent.expect("girdle present") - 60.0).abs() < 1e-6);

        let mm = proportions.to_mm(2.0);
        assert!((proportions.total_depth.mul_add(-2.0, mm.total_depth)).abs() < 1e-9);
        assert_eq!(mm.table_percent, proportions.table_percent);
        assert!(
            (proportions
                .girdle_thickness
                .expect("girdle present")
                .mul_add(-2.0, mm.girdle_thickness.expect("girdle present")))
            .abs()
                < 1e-9
        );
        // Percentages are scale-invariant: `to_mm` must leave them untouched.
        assert_eq!(
            mm.crown_to_width_percent,
            proportions.crown_to_width_percent
        );
        assert_eq!(
            mm.pavilion_to_width_percent,
            proportions.pavilion_to_width_percent
        );
        assert_eq!(
            mm.girdle_to_width_percent,
            proportions.girdle_to_width_percent
        );
    }

    /// The hip-roofed block has no facet with an outward normal near
    /// straight up -- its top is a ridge, not a table -- so `table_percent`
    /// must be `None`.
    #[test]
    fn stone_proportions_reports_no_table_on_a_hip_roofed_block() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
            (DVec3::NEG_Y, 0.5),
            (DVec3::new(s, s, 0.0), s),
            (DVec3::new(-s, s, 0.0), s),
            (DVec3::new(0.0, s, s), s),
            (DVec3::new(0.0, s, -s), s),
        ];
        let metrics = measure_solid(&planes).expect("roofed block must measure");
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            panic!("roofed block must close");
        };
        let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
        assert!(proportions.table_percent.is_none());
    }

    /// A solid the real planes never close (no floor): must report `None`, not a
    /// blank-box-clipped volume.
    #[test]
    fn unbounded_solid_reports_none() {
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        assert!(measure_solid(&planes).is_none());
    }

    /// A duplicated plane (same normal and offset listed twice) must not
    /// double-count its face's area.
    #[test]
    fn duplicate_planes_do_not_double_count() {
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::NEG_Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        let m = measure_solid(&planes).expect("box must measure");
        assert!((m.volume - 4.8).abs() < 1e-9, "volume {}", m.volume);
    }

    /// A 45-degree-rotated square girdle: axis extents see the diagonal
    /// (`2*sqrt(2)`), calipers must recover the true side length 2.
    #[test]
    fn caliper_width_beats_axis_width_on_a_rotated_outline() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let planes = vec![
            (DVec3::new(s, 0.0, s), 1.0),
            (DVec3::new(-s, 0.0, s), 1.0),
            (DVec3::new(s, 0.0, -s), 1.0),
            (DVec3::new(-s, 0.0, -s), 1.0),
            (DVec3::Y, 0.5),
            (DVec3::NEG_Y, 0.5),
        ];
        let m = measure_solid(&planes).expect("rotated box must measure");
        let diag = 2.0 * std::f64::consts::SQRT_2;
        assert!((m.width_axis - diag).abs() < 1e-9, "axis {}", m.width_axis);
        assert!(
            (m.width_caliper - 2.0).abs() < 1e-9,
            "caliper {}",
            m.width_caliper
        );
        // Side-2 square cross-section, height 1.
        assert!((m.volume - 4.0).abs() < 1e-9, "volume {}", m.volume);
    }

    /// Byte-identical determinism across repeated calls.
    #[test]
    fn measurement_is_deterministic() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
            (DVec3::NEG_Y, 0.5),
            (DVec3::new(s, s, 0.0), s),
            (DVec3::new(-s, s, 0.0), s),
            (DVec3::new(0.0, s, s), s),
            (DVec3::new(0.0, s, -s), s),
        ];
        let a = measure_solid(&planes).expect("must measure");
        let b = measure_solid(&planes).expect("must measure");
        assert_eq!(a.volume.to_bits(), b.volume.to_bits());
        assert_eq!(a.width_caliper.to_bits(), b.width_caliper.to_bits());
        assert_eq!(a.total_height.to_bits(), b.total_height.to_bits());
    }

    // -----------------------------------------------------------------------
    // The `VertexAccumulator` x-sorted index in `insert_if_new` must make the
    // exact same accept/reject decision, in the exact same insertion order,
    // as the O(V^2) linear scan it replaces. Proven here by running both side
    // by side over the module's own fixtures plus real cutting schedules and
    // comparing the resulting vertex lists bit-for-bit.
    // -----------------------------------------------------------------------

    /// Pre-optimization dedup, kept only as a reference: a duplicate is any
    /// already-accepted vertex within [`VERTEX_DEDUP`] on every axis, found
    /// by scanning the full accepted set (this is exactly the body
    /// `flush_solid_batch` had before `VertexAccumulator` existed).
    fn flush_solid_batch_linear_reference(
        batch: &crate::simd::TripleBatch,
        soa: &crate::simd::PlanesSoA64,
        verts: &mut Vec<SolidVertex>,
    ) -> Option<()> {
        let sol = crate::simd::solve_triple_batch(batch);
        for lane in 0..batch.len {
            if sol.det[lane].abs() < MIN_TRIPLE_DET {
                continue;
            }
            let v = DVec3::new(sol.vx[lane], sol.vy[lane], sol.vz[lane]);
            if v.abs().max_element() > BLANK_HALF_EXTENT + 1.0 {
                continue;
            }
            if crate::simd::any_violation(soa, v, EPS_FEAS) {
                continue;
            }
            if v.abs().max_element() > BLANK_HALF_EXTENT - 1.0 {
                return None;
            }
            let dup = verts
                .iter()
                .any(|s| (s.v - v).abs().max_element() < VERTEX_DEDUP);
            if !dup {
                verts.push(SolidVertex { v });
            }
        }
        Some(())
    }

    /// [`feasible_vertices`], but deduped by [`flush_solid_batch_linear_reference`]
    /// instead of [`VertexAccumulator`]. Otherwise byte-for-byte the same
    /// function (same plane augmentation, same batching loop).
    fn feasible_vertices_linear_reference(planes: &[(DVec3, f64)]) -> Option<Vec<DVec3>> {
        let mut all: Vec<(DVec3, f64)> = planes.to_vec();
        for n in [
            DVec3::X,
            DVec3::NEG_X,
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::NEG_Z,
        ] {
            all.push((n, BLANK_HALF_EXTENT));
        }

        let mut soa = crate::simd::PlanesSoA64::with_capacity(all.len());
        for &(n, m) in &all {
            soa.push(n, m, 0);
        }

        let p = all.len();
        let mut verts: Vec<SolidVertex> = Vec::new();
        let mut batch = crate::simd::TripleBatch::default();
        for a in 0..p {
            for b in (a + 1)..p {
                for c in (b + 1)..p {
                    let (pa, pb, pc) = (all[a], all[b], all[c]);
                    if batch.push((pa.0, pa.1), (pb.0, pb.1), (pc.0, pc.1)) {
                        flush_solid_batch_linear_reference(&batch, &soa, &mut verts)?;
                        batch = crate::simd::TripleBatch::default();
                    }
                }
            }
        }
        if batch.len > 0 {
            flush_solid_batch_linear_reference(&batch, &soa, &mut verts)?;
        }
        Some(verts.into_iter().map(|s| s.v).collect())
    }

    /// Runs both the production (`VertexAccumulator`-indexed) and reference
    /// (linear-scan) dedup over `planes` and asserts byte-identical vertex
    /// lists, in order.
    fn assert_dedup_matches_reference(planes: &[(DVec3, f64)], label: &str) {
        let deduped = dedup_planes(planes);
        let fast =
            feasible_vertices(&deduped).map(|v| v.into_iter().map(|s| s.v).collect::<Vec<_>>());
        let reference = feasible_vertices_linear_reference(&deduped);

        match (fast, reference) {
            (None, None) => {}
            (Some(f), Some(r)) => {
                assert_eq!(
                    f.len(),
                    r.len(),
                    "{label}: vertex count differs (indexed {} vs linear-scan reference {})",
                    f.len(),
                    r.len()
                );
                for (i, (fv, rv)) in f.iter().zip(r.iter()).enumerate() {
                    assert_eq!(
                        (fv.x.to_bits(), fv.y.to_bits(), fv.z.to_bits()),
                        (rv.x.to_bits(), rv.y.to_bits(), rv.z.to_bits()),
                        "{label}: vertex {i} differs (indexed {fv:?} vs reference {rv:?})"
                    );
                }
            }
            (f, r) => panic!(
                "{label}: indexed and reference dedup disagree on boundedness (indexed {:?}, reference {:?})",
                f.is_some(),
                r.is_some()
            ),
        }
    }

    #[test]
    fn dedup_matches_linear_scan_reference_on_module_fixtures() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        assert_dedup_matches_reference(
            &[
                (DVec3::X, 1.0),
                (DVec3::NEG_X, 1.0),
                (DVec3::Y, 0.6),
                (DVec3::NEG_Y, 0.6),
                (DVec3::Z, 1.0),
                (DVec3::NEG_Z, 1.0),
            ],
            "plain box",
        );
        assert_dedup_matches_reference(
            &[
                (DVec3::X, 1.0),
                (DVec3::NEG_X, 1.0),
                (DVec3::Z, 1.0),
                (DVec3::NEG_Z, 1.0),
                (DVec3::NEG_Y, 0.5),
                (DVec3::new(s, s, 0.0), s),
                (DVec3::new(-s, s, 0.0), s),
                (DVec3::new(0.0, s, s), s),
                (DVec3::new(0.0, s, -s), s),
            ],
            "hip-roofed block",
        );
        assert_dedup_matches_reference(
            &[
                (DVec3::new(s, 0.0, s), 1.0),
                (DVec3::new(-s, 0.0, s), 1.0),
                (DVec3::new(s, 0.0, -s), 1.0),
                (DVec3::new(-s, 0.0, -s), 1.0),
                (DVec3::Y, 0.5),
                (DVec3::NEG_Y, 0.5),
            ],
            "rotated square girdle",
        );
    }

    /// Builds a real cutting schedule's plane arrangement (tier normals via
    /// `meet_solver::tier_instance_normals`, offsets from `solve_meet_points`'s
    /// solved masts) the same way `SolveContext::config_score` does when the
    /// solver scores a candidate configuration against a design's printed
    /// proportions -- this is the actual production caller of `measure_solid`
    /// that makes `feasible_vertices` dedup real numbers of colliding
    /// candidate vertices, not just the module's small hand-built fixtures.
    fn planes_from_asc_schedule(text: &str) -> Vec<(DVec3, f64)> {
        let schedule = indicatrix_formats::asc::parse_asc(text).expect("fixture schedule parses");
        let mut tiers = crate::geometry::meet_solver::meet_tier_inputs_from_asc(&schedule);
        for j in [0usize, 1, 2] {
            if let Some(t) = schedule.tiers.get(j) {
                tiers[j].constraint =
                    crate::geometry::meet_solver::MeetConstraint::ScaleReference(t.mast);
            }
        }
        let normals =
            crate::geometry::meet_solver::tier_instance_normals(schedule.gear_teeth_abs(), &tiers);
        let solved =
            crate::geometry::meet_solver::solve_meet_points(schedule.gear_teeth_abs(), &tiers);
        normals
            .iter()
            .zip(solved.iter().map(|s| s.mast))
            .flat_map(|(ns, m)| ns.iter().map(move |&n| (n, m)))
            .collect()
    }

    #[test]
    fn dedup_matches_linear_scan_reference_on_real_schedules() {
        // Same fixture text as `examples/simd_bench.rs`'s solver benchmark
        // ("Bench design" / pgo_train.rs's "Train A"): a 96-tooth round with
        // two crown tiers, table, and two pavilion tiers plus culet.
        assert_dedup_matches_reference(
            &planes_from_asc_schedule(
                "GemCad 5.0\n\
                 g 96 0.0\n\
                 y 6 y\n\
                 I 1.72\n\
                 H Bench design\n\
                 a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
                 a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
                 a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
                 a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
                 a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
                 a 0.000000 0.44000000 n T\n",
            ),
            "real schedule: Train A (96-tooth round)",
        );
        // pgo_train.rs's "Train B": mixed 96/6-tooth tiers, a heavier real
        // schedule with a different symmetry split.
        assert_dedup_matches_reference(
            &planes_from_asc_schedule(
                "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\nH Train B\n\
                 a -43.000000 0.70000000 96 n P1 12 24 36 48 60 72 84\n\
                 a -41.000000 0.68000000 6 n P2 18 30 42 54 66 78 90\n\
                 a -90.000000 1.00000000 96 n G 12 24 36 48 60 72 84\n\
                 a -90.000000 1.00000000 6 n G2 18 30 42 54 66 78 90\n\
                 a 42.000000 0.72000000 96 n C1 12 24 36 48 60 72 84\n\
                 a 27.000000 0.62000000 6 n C2 18 30 42 54 66 78 90\n\
                 a 0.000000 0.40000000 n T\n",
            ),
            "real schedule: Train B (mixed 96/6-tooth)",
        );
        // pgo_train.rs's "Train C": a simpler 4-fold real schedule.
        assert_dedup_matches_reference(
            &planes_from_asc_schedule(
                "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.62\nH Train C\n\
                 a -45.000000 0.75000000 96 n 1 24 48 72\n\
                 a -40.000000 0.70000000 12 n 2 36 60 84\n\
                 a -90.000000 1.05000000 96 n G 24 48 72\n\
                 a -90.000000 1.05000000 12 n G2 36 60 84\n\
                 a 35.000000 0.70000000 96 n 3 24 48 72\n\
                 a 20.000000 0.58000000 12 n 4 36 60 84\n\
                 a 0.000000 0.42000000 n T\n",
            ),
            "real schedule: Train C (4-fold)",
        );
    }

    // -----------------------------------------------------------------------
    // `build_solid_mesh`: mesh extraction from the plane arrangement
    // -----------------------------------------------------------------------

    /// Signed volume via the standard triangle-mesh divergence theorem (`V =
    /// (1/6) * sum over triangles of v0 . (v1 x v2)`), valid when every
    /// triangle winds counterclockwise as seen from outside the solid --
    /// exactly what a centroid fan over [`face_ring`]'s angle-sorted ring
    /// produces (the ring itself is already wound that way; see
    /// `build_solid_mesh`'s doc comment on triangulation and this module's
    /// own reasoning about `basis_u x basis_v = normal`). Used only by tests,
    /// as an independent cross-check against [`measure_solid`]'s
    /// plane-offset-and-area formula.
    fn mesh_divergence_volume(mesh: &SolidMesh) -> f64 {
        let mut acc = 0.0f64;
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v0 = mesh.positions[tri[0] as usize];
            let v1 = mesh.positions[tri[1] as usize];
            let v2 = mesh.positions[tri[2] as usize];
            acc += v0.dot(v1.cross(v2));
        }
        acc / 6.0
    }

    /// Every undirected edge of a triangle mesh must be shared by exactly two
    /// triangles for the mesh to be watertight (closed, manifold).
    ///
    /// Compares edges by vertex *position*, not by index: `build_solid_mesh`
    /// deliberately duplicates each vertex once per owning facet so every
    /// copy can carry that facet's own flat normal (see `SolidMesh::normals`
    /// doc comment), so the two triangles on either side of a real edge
    /// almost always reference two DIFFERENT index pairs that happen to sit
    /// at the same position (one pair from each facet's own vertex block) --
    /// only a spoke edge internal to one facet's centroid fan reuses the
    /// same indices on both its triangles. Position comparison treats both
    /// cases uniformly.
    ///
    /// `O(n^2)` plain-loop matching -- meshes in this module's tests are a
    /// few dozen triangles, so this trades asymptotic efficiency for staying
    /// in the same plain-loops-no-hashing style as the production code it's
    /// checking.
    fn assert_watertight(mesh: &SolidMesh, label: &str) {
        type PosKey = (u64, u64, u64);
        let key = |i: u32| -> PosKey {
            let p = mesh.positions[i as usize];
            (p.x.to_bits(), p.y.to_bits(), p.z.to_bits())
        };
        let mut edges: Vec<(PosKey, PosKey)> = Vec::new();
        for tri in mesh.indices.as_chunks::<3>().0 {
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let (ka, kb) = (key(a), key(b));
                edges.push(if ka <= kb { (ka, kb) } else { (kb, ka) });
            }
        }
        for e in &edges {
            let count = edges.iter().filter(|other| *other == e).count();
            assert_eq!(
                count, 2,
                "{label}: edge {e:?} is shared by {count} triangles, not 2 (not watertight)"
            );
        }
    }

    /// Builds the plane-arrangement vertex list the same way `measure_solid`
    /// does internally (`dedup_planes` then `feasible_vertices`), for tests
    /// that need to call the private `face_area` directly as an oracle.
    fn verts_for(planes: &[(DVec3, f64)]) -> Vec<SolidVertex> {
        feasible_vertices(&dedup_planes(planes)).expect("fixture must be bounded")
    }

    /// A `Closed` mesh's per-face triangle-area sum must equal
    /// [`face_area`]'s figure for that plane, and the mesh's own
    /// divergence-theorem volume must equal [`measure_solid`]'s -- both
    /// within a tight tolerance (not bit-exact: the mesh sums triangle areas
    /// in a different order than `face_area`'s single shoelace pass, so
    /// floating-point round-off can differ in the last few bits even though
    /// both compute the same polygon's area).
    fn assert_mesh_matches_measure_solid(planes: &[(DVec3, f64)], label: &str) {
        let status = build_solid_mesh(planes);
        let SolidStatus::Closed(mesh) = status else {
            panic!("{label}: expected a closed mesh, got {status:?}");
        };
        assert_watertight(&mesh, label);

        let verts = verts_for(planes);
        for &(facet_idx, ref ring) in &mesh.rings {
            let (normal, offset) = planes[facet_idx];
            let want_area = face_area(normal, offset, &verts);
            let centroid = ring.iter().copied().sum::<DVec3>() / ring.len() as f64;
            let mut got_area = 0.0f64;
            for i in 0..ring.len() {
                let a = ring[i] - centroid;
                let b = ring[(i + 1) % ring.len()] - centroid;
                got_area = 0.5f64.mul_add(a.cross(b).length(), got_area);
            }
            assert!(
                (got_area - want_area).abs() < 1e-9,
                "{label}: facet {facet_idx} triangle-area sum {got_area} != face_area {want_area}"
            );
        }

        let want_volume = measure_solid(planes).expect("must measure").volume;
        let got_volume = mesh_divergence_volume(&mesh);
        assert!(
            (got_volume - want_volume).abs() < 1e-6,
            "{label}: mesh divergence volume {got_volume} != measure_solid volume {want_volume}"
        );
    }

    #[test]
    fn build_solid_mesh_matches_measure_solid_on_a_plain_box() {
        assert_mesh_matches_measure_solid(
            &[
                (DVec3::X, 1.0),
                (DVec3::NEG_X, 1.0),
                (DVec3::Y, 0.6),
                (DVec3::NEG_Y, 0.6),
                (DVec3::Z, 1.0),
                (DVec3::NEG_Z, 1.0),
            ],
            "plain box",
        );
    }

    #[test]
    fn build_solid_mesh_matches_measure_solid_on_a_hip_roofed_block() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        assert_mesh_matches_measure_solid(
            &[
                (DVec3::X, 1.0),
                (DVec3::NEG_X, 1.0),
                (DVec3::Z, 1.0),
                (DVec3::NEG_Z, 1.0),
                (DVec3::NEG_Y, 0.5),
                (DVec3::new(s, s, 0.0), s),
                (DVec3::new(-s, s, 0.0), s),
                (DVec3::new(0.0, s, s), s),
                (DVec3::new(0.0, s, -s), s),
            ],
            "hip-roofed block",
        );
    }

    #[test]
    fn build_solid_mesh_matches_measure_solid_on_a_real_schedule() {
        assert_mesh_matches_measure_solid(
            &planes_from_asc_schedule(
                "GemCad 5.0\n\
                 g 96 0.0\n\
                 y 6 y\n\
                 I 1.72\n\
                 H Bench design\n\
                 a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
                 a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
                 a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
                 a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
                 a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
                 a 0.000000 0.44000000 n T\n",
            ),
            "real schedule: Bench design",
        );
    }

    /// The vertex-id-only fields (`facet_id`, `indices`) and geometry
    /// (`positions`, `normals`) must be byte-identical (not just
    /// numerically close) across two calls with the same input -- the same
    /// determinism contract `measurement_is_deterministic` checks for
    /// `measure_solid`.
    #[test]
    fn build_solid_mesh_is_deterministic() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
            (DVec3::NEG_Y, 0.5),
            (DVec3::new(s, s, 0.0), s),
            (DVec3::new(-s, s, 0.0), s),
            (DVec3::new(0.0, s, s), s),
            (DVec3::new(0.0, s, -s), s),
        ];
        let (SolidStatus::Closed(a), SolidStatus::Closed(b)) =
            (build_solid_mesh(&planes), build_solid_mesh(&planes))
        else {
            panic!("fixture must close");
        };
        assert_eq!(a.facet_id, b.facet_id);
        assert_eq!(a.indices, b.indices);
        assert_eq!(a.positions.len(), b.positions.len());
        for (pa, pb) in a.positions.iter().zip(&b.positions) {
            assert_eq!(
                (pa.x.to_bits(), pa.y.to_bits(), pa.z.to_bits()),
                (pb.x.to_bits(), pb.y.to_bits(), pb.z.to_bits())
            );
        }
        for (na, nb) in a.normals.iter().zip(&b.normals) {
            assert_eq!(
                (na.x.to_bits(), na.y.to_bits(), na.z.to_bits()),
                (nb.x.to_bits(), nb.y.to_bits(), nb.z.to_bits())
            );
        }
    }

    /// A schedule missing its closing planes (no floor) must report
    /// `Unbounded` naming at least one real (non-blank) plane index, not
    /// silently blank or panic -- the editor-facing case this status exists
    /// for.
    #[test]
    fn build_solid_mesh_reports_unbounded_with_escaping_planes() {
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        match build_solid_mesh(&planes) {
            SolidStatus::Unbounded { escaping } => {
                assert!(!escaping.is_empty(), "expected at least one escaping plane");
                assert!(escaping.iter().all(|&i| i < planes.len()));
                let mut sorted = escaping.clone();
                sorted.sort_unstable();
                sorted.dedup();
                assert_eq!(
                    escaping, sorted,
                    "escaping indices must be sorted and deduped"
                );
            }
            other => panic!("expected Unbounded, got {other:?}"),
        }
    }

    /// Six planes all through the origin collapse the "solid" to a single
    /// point: bounded (nothing escapes the blank), but with 1 distinct
    /// vertex, far short of the 4 a real polytope needs.
    #[test]
    fn build_solid_mesh_reports_degenerate_for_too_few_vertices() {
        let planes = vec![
            (DVec3::X, 0.0),
            (DVec3::NEG_X, 0.0),
            (DVec3::Y, 0.0),
            (DVec3::NEG_Y, 0.0),
            (DVec3::Z, 0.0),
            (DVec3::NEG_Z, 0.0),
        ];
        match build_solid_mesh(&planes) {
            SolidStatus::Degenerate {
                vertex_count,
                volume,
            } => {
                assert!(vertex_count < 4, "vertex_count {vertex_count}");
                assert!(volume.is_none() || volume == Some(0.0));
            }
            other => panic!("expected Degenerate, got {other:?}"),
        }
    }

    /// A box flattened to zero height (`y` pinned to exactly 0 by both the
    /// `+Y` and `-Y` planes) has 4 distinct vertices -- enough to pass the
    /// vertex-count gate -- but zero volume: bounded, not too few vertices,
    /// yet still not a real solid.
    #[test]
    fn build_solid_mesh_reports_degenerate_for_zero_volume() {
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.0),
            (DVec3::NEG_Y, 0.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        match build_solid_mesh(&planes) {
            SolidStatus::Degenerate {
                vertex_count,
                volume,
            } => {
                assert_eq!(vertex_count, 4);
                assert_eq!(volume, Some(0.0));
            }
            other => panic!("expected Degenerate, got {other:?}"),
        }
    }

    /// A duplicated plane (the same normal and offset listed twice, e.g. a
    /// tier appearing in two rows) must not produce duplicate overlapping
    /// geometry -- `build_solid_mesh` must dedup exactly like `measure_solid`
    /// does, and the escaping/facet indices it reports must still refer to
    /// the ORIGINAL (pre-dedup) plane list position, not the deduped one.
    #[test]
    fn build_solid_mesh_dedups_planes_and_maps_indices_to_the_original_list() {
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::X, 1.0), // duplicate of index 0
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::NEG_Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            panic!("fixture must close");
        };
        assert_watertight(&mesh, "duplicate-plane box");
        // Exactly one of the two bit-identical `+X` planes (index 0 or 1)
        // should own a face -- never both, and never any index >= len.
        let x_owners: Vec<usize> = mesh
            .rings
            .iter()
            .map(|&(idx, _)| idx)
            .filter(|&idx| idx == 0 || idx == 1)
            .collect();
        assert_eq!(
            x_owners.len(),
            1,
            "expected exactly one +X facet owner among indices [0, 1], got {x_owners:?}"
        );
        assert!(mesh.facet_id.iter().all(|&f| f < planes.len()));
        let want_volume = measure_solid(&planes).expect("must measure").volume;
        let got_volume = mesh_divergence_volume(&mesh);
        assert!((got_volume - want_volume).abs() < 1e-9);
    }
}
