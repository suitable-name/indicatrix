//! Mesh of a stone carved by convex tools: `P \ (T1 u T2 u ..)`.
//!
//! The kernel's exact tools ([`ToolPrimitive`]) are tessellated into convex
//! polytopes `T~` (half-space lists). Because every `T~` is convex, "the part of
//! a convex polygon `A` outside `T~`" decomposes exactly into the convex
//! pieces `A n h1 n .. n h(k-1) n !hk`, each by one Sutherland-Hodgman clip, so
//! the whole construction needs no polygon-with-holes machinery:
//!
//! * the rings of the planar mesh are cut into the pieces outside every tool;
//! * each face of each `T~` is clipped to `P`, cut into the pieces outside every
//!   *other* tool, and reversed so it faces out of the stone.
//!
//! Edge visibility is tracked **by construction** rather than recovered from
//! endpoint coordinates afterwards. A cut edge created along plane `hk` is the
//! seam to the neighbouring pieces of the same facet except where the
//! neighbour is the removed tool volume, i.e. the part of the edge that lies in
//! `T~` itself; that part is found by clipping the edge against the later
//! planes. This is robust against the T-junctions the decomposition produces,
//! where an endpoint-keyed lookup would see an edge matched by several
//! fragments (or none).
//!
//! Only pieces that were actually cut have their vertices snapped to a
//! `1e-9 W` grid; rings and tool faces nothing touched keep their exact
//! `DVec3`s.

#![expect(
    clippy::many_single_char_names,
    reason = "polygon and plane algebra (n, m, p, q, t, d, u, v) mirrors the derivation in the plan"
)]

use std::collections::BTreeSet;

use glam::{DVec3, Vec3};

use super::{
    mesh::TOOL_SEGMENTS,
    types::{SolidMesh, SolidStatus},
    vertices::dedup_planes,
};
use crate::geometry::tool::ToolPrimitive;

mod tessellation;

use tessellation::Polytope;
pub use tessellation::ToolBounds;
pub(super) use tessellation::{bounds, polytope};

type Plane = (DVec3, f64);

/// Pieces whose area is below this fraction of `W^2` are dropped after
/// snapping: at that size the snap noise exceeds the piece and its winding is
/// meaningless, while the volume it carries is below `1e-10 W^3`.
const MIN_PIECE_AREA: f64 = 1e-10;

/// Snap grid as a fraction of the stone's bounding-box width `W`.
const SNAP_GRID: f64 = 1e-9;

/// Whether a polygon edge is drawn; see [`SolidMesh::edge_visible`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// Inside one facet: a seam of the decomposition or between two
    /// tessellation planes of one tool.
    Hidden,
    /// Between two different facets.
    Visible,
    /// Freshly cut along a tool plane; resolved into hidden/visible runs by
    /// [`Poly::resolve_cuts`] once the later planes are known.
    Cut,
}

/// A convex polygon with a visibility flag per edge `i -> i + 1`.
#[derive(Debug, Clone)]
struct Poly {
    v: Vec<DVec3>,
    e: Vec<Edge>,
    /// A vertex was created by clipping, so the polygon is snapped at the end.
    touched: bool,
}

impl Poly {
    fn new(v: Vec<DVec3>, edge: Edge) -> Self {
        let e = vec![edge; v.len()];
        Self {
            v,
            e,
            touched: false,
        }
    }

    fn vector_area(&self) -> DVec3 {
        let p0 = self.v[0];
        let mut s = DVec3::ZERO;
        for w in self.v[1..].windows(2) {
            s += (w[0] - p0).cross(w[1] - p0);
        }
        0.5 * s
    }

    /// Drops consecutive coincident vertices. The surviving vertex keeps the
    /// flag of the edge that actually leaves it.
    fn normalise(&mut self) {
        let mut v: Vec<DVec3> = Vec::with_capacity(self.v.len());
        let mut e: Vec<Edge> = Vec::with_capacity(self.v.len());
        for (&p, &f) in self.v.iter().zip(&self.e) {
            if v.last() == Some(&p) {
                if let Some(last) = e.last_mut() {
                    *last = f;
                }
            } else {
                v.push(p);
                e.push(f);
            }
        }
        while v.len() > 1 && v.first() == v.last() {
            v.pop();
            e.pop();
        }
        self.v = v;
        self.e = e;
    }

    fn snap(&mut self, grid: f64) {
        let s = |x: f64| (x / grid).round() * grid;
        for p in &mut self.v {
            *p = DVec3::new(s(p.x), s(p.y), s(p.z));
        }
    }

    /// Sutherland-Hodgman against `n . x <= m`; edges created along the plane
    /// get the flag `cut`. `None` when fewer than three vertices survive.
    ///
    /// The intersection is always computed from the edge's own start vertex, and
    /// the remainder's clip against `(n, m)` and the piece's against `(-n, -m)`
    /// see exactly negated distances, so both sides of a cut get bit-identical
    /// points.
    fn clip(&self, n: DVec3, m: f64, cut: Edge) -> Option<Self> {
        let len = self.v.len();
        let d: Vec<f64> = self.v.iter().map(|&p| n.dot(p) - m).collect();
        if d.iter().all(|&x| x <= 0.0) {
            return Some(self.clone());
        }
        let mut v = Vec::with_capacity(len + 2);
        let mut e = Vec::with_capacity(len + 2);
        let mut touched = self.touched;
        for i in 0..len {
            let j = (i + 1) % len;
            let (da, db) = (d[i], d[j]);
            let cross = || self.v[i] + (self.v[j] - self.v[i]) * (da / (da - db));
            if da <= 0.0 {
                v.push(self.v[i]);
                e.push(self.e[i]);
                if db > 0.0 {
                    v.push(cross());
                    e.push(cut);
                    touched = true;
                }
            } else if db <= 0.0 {
                v.push(cross());
                e.push(self.e[i]);
                touched = true;
            }
        }
        if v.len() < 3 {
            return None;
        }
        let mut out = Self { v, e, touched };
        out.normalise();
        (out.v.len() >= 3).then_some(out)
    }

    /// Replaces every [`Edge::Cut`] by hidden/visible runs: the part of the
    /// edge inside the later planes' polytope is where the neighbour across it
    /// is the removed volume (so a different facet); the rest borders another
    /// piece of the same facet.
    fn resolve_cuts(&mut self, later: &[Plane]) {
        if !self.e.contains(&Edge::Cut) {
            return;
        }
        let len = self.v.len();
        let mut v = Vec::with_capacity(len + 2);
        let mut e = Vec::with_capacity(len + 2);
        for i in 0..len {
            let (p, q) = (self.v[i], self.v[(i + 1) % len]);
            v.push(p);
            if self.e[i] != Edge::Cut {
                e.push(self.e[i]);
                continue;
            }
            let (t0, t1) = visible_interval(p, q, later);
            if t0 < t1 {
                if t0 > 0.0 {
                    e.push(Edge::Hidden);
                    v.push(p + (q - p) * t0);
                }
                e.push(Edge::Visible);
                if t1 < 1.0 {
                    v.push(p + (q - p) * t1);
                    e.push(Edge::Hidden);
                }
            } else {
                e.push(Edge::Hidden);
            }
        }
        self.v = v;
        self.e = e;
        self.normalise();
    }

    fn reversed(&self) -> Self {
        let n = self.v.len();
        let v: Vec<DVec3> = self.v.iter().rev().copied().collect();
        // Reversed edge `i -> i + 1` is the old edge between the same vertices.
        let e: Vec<Edge> = (0..n).map(|i| self.e[(2 * n - 2 - i) % n]).collect();
        Self {
            v,
            e,
            touched: self.touched,
        }
    }
}

/// The parameter range of segment `p -> q` lying inside every plane, empty
/// (`t0 >= t1`) when none does.
fn visible_interval(p: DVec3, q: DVec3, planes: &[Plane]) -> (f64, f64) {
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for &(n, m) in planes {
        let dp = n.dot(p) - m;
        let dq = n.dot(q) - m;
        if dp <= 0.0 && dq <= 0.0 {
            continue;
        }
        if dp > 0.0 && dq > 0.0 {
            return (1.0, 0.0);
        }
        let t = dp / (dp - dq);
        if dp > 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
    }
    (t0, t1)
}

/// The convex pieces of `poly` outside the convex polytope: `poly n h1 n .. n
/// h(k-1) n !hk` for `k = 1..m`, disjoint, their union `poly \ T~`.
///
/// A polygon wholly outside one plane is returned as is (this is also what
/// keeps an untouched ring's exact vertices).
fn split_outside(poly: Poly, tool: &Polytope) -> Vec<Poly> {
    if tool
        .planes
        .iter()
        .any(|&(n, m)| poly.v.iter().all(|&p| n.dot(p) - m > 0.0))
    {
        return vec![poly];
    }
    let mut out = Vec::new();
    let mut rem = poly;
    for (k, &(n, m)) in tool.planes.iter().enumerate() {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &p in &rem.v {
            let d = n.dot(p) - m;
            lo = lo.min(d);
            hi = hi.max(d);
        }
        if hi <= 0.0 {
            continue;
        }
        if lo >= 0.0 {
            out.push(rem);
            return out;
        }
        if let Some(mut piece) = rem.clip(-n, -m, Edge::Cut) {
            piece.resolve_cuts(&tool.planes[k + 1..]);
            out.push(piece);
        }
        match rem.clip(n, m, Edge::Hidden) {
            Some(r) => rem = r,
            None => return out,
        }
    }
    out
}

/// Faces of the polytope (one per plane that has area), wound counter-clockwise
/// about the outward normal.
///
/// Each is a large square in the plane clipped by every other plane, which is
/// exact for a convex region and needs no adjacency data.
fn faces(pt: &Polytope) -> Vec<Poly> {
    let big = 4.0 * pt.radius;
    let min_area = 1e-14 * pt.radius * pt.radius;
    let mut out = Vec::new();
    for (i, &(n, m)) in pt.planes.iter().enumerate() {
        let u = n.any_orthonormal_vector();
        let v = n.cross(u);
        let foot = pt.centre + n * (m - n.dot(pt.centre));
        // `u x v = n`, so this order is counter-clockwise about `n`.
        let mut poly = Some(Poly::new(
            vec![
                foot - u * big - v * big,
                foot + u * big - v * big,
                foot + u * big + v * big,
                foot - u * big + v * big,
            ],
            Edge::Hidden,
        ));
        for (j, &(n2, m2)) in pt.planes.iter().enumerate() {
            if j != i {
                poly = poly.and_then(|p| p.clip(n2, m2, Edge::Hidden));
            }
        }
        if let Some(mut p) = poly
            && p.vector_area().length() > min_area
        {
            // Generated geometry, not a stone vertex: nothing to keep exact.
            p.touched = false;
            out.push(p);
        }
    }
    out
}

/// Volume of the tessellated tool (test support: it is the "constant" a
/// dimple's removed volume is compared against).
#[cfg(test)]
pub(super) fn polytope_volume(tool: &ToolPrimitive, segments: usize) -> f64 {
    let Some(pt) = polytope(tool, segments) else {
        return 0.0;
    };
    let mut acc = 0.0;
    for f in faces(&pt) {
        acc += (f.v[0] - pt.centre).dot(f.vector_area());
    }
    acc / 3.0
}

fn centroid(v: &[DVec3]) -> DVec3 {
    v.iter().copied().sum::<DVec3>() / v.len() as f64
}

/// Appends one ring and its centroid fan the way `build_solid_mesh` does.
fn push_ring(out: &mut SolidMesh, facet: usize, ring: Vec<DVec3>, centre: DVec3, normal: DVec3) {
    let base = out.positions.len() as u32;
    out.positions.push(centre);
    out.normals.push(normal);
    out.facet_id.push(facet);
    for &v in &ring {
        out.positions.push(v);
        out.normals.push(normal);
        out.facet_id.push(facet);
    }
    let k = ring.len() as u32;
    for e in 0..k {
        out.indices.push(base);
        out.indices.push(base + 1 + e);
        out.indices.push(base + 1 + (e + 1) % k);
    }
    out.rings.push((facet, ring));
}

/// Builds the carved mesh from the closed planar `mesh` of `planes`.
#[expect(
    clippy::too_many_lines,
    reason = "two passes (flat facets, tool surfaces) sharing one output mesh and snap parameters"
)]
pub(super) fn build(planes: &[Plane], tools: &[ToolPrimitive], mesh: &SolidMesh) -> SolidStatus {
    let polytopes: Vec<Option<Polytope>> =
        tools.iter().map(|t| polytope(t, TOOL_SEGMENTS)).collect();

    let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for &p in &mesh.positions {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let width = (hi - lo).max_element();
    let grid = SNAP_GRID * width;
    let min_area = MIN_PIECE_AREA * width * width;
    let finalise = |mut p: Poly| -> Option<Poly> {
        if p.touched {
            p.snap(grid);
            p.normalise();
        }
        (p.v.len() >= 3 && p.vector_area().length() >= min_area).then_some(p)
    };

    let mut out = SolidMesh::default();
    let mut piece_normals: Vec<DVec3> = Vec::new();
    let mut edge_visible: Vec<Vec<bool>> = Vec::new();
    let visible = |p: &Poly| -> Vec<bool> { p.e.iter().map(|&f| f == Edge::Visible).collect() };

    // Flat facets: the pieces of each ring outside every tool.
    let mut base = 0usize;
    for (facet, ring) in &mesh.rings {
        let normal = mesh.normals[base];
        let fan_centre = mesh.positions[base];
        base += 1 + ring.len();
        let offset = normal.dot(ring[0]);
        let mut pieces = vec![Poly::new(ring.clone(), Edge::Visible)];
        for pt in polytopes.iter().flatten() {
            // The ring lies in its plane, so a plane that misses the bounding
            // sphere misses the tool; skipping also keeps round-off away.
            if (normal.dot(pt.centre) - offset).abs() > pt.radius {
                continue;
            }
            pieces = pieces
                .into_iter()
                .flat_map(|p| split_outside(p, pt))
                .collect();
        }
        if pieces.len() == 1 && !pieces[0].touched {
            // Untouched: the planar mesh's own ring and fan centre, bit for bit.
            edge_visible.push(vec![true; ring.len()]);
            piece_normals.push(normal);
            push_ring(&mut out, *facet, ring.clone(), fan_centre, normal);
            continue;
        }
        for p in pieces.into_iter().filter_map(finalise) {
            edge_visible.push(visible(&p));
            piece_normals.push(normal);
            let c = centroid(&p.v);
            push_ring(&mut out, *facet, p.v, c, normal);
        }
    }

    // Tool surfaces inside the stone, facing out of it.
    let p_planes = dedup_planes(planes);
    for (k, pt) in polytopes.iter().enumerate() {
        let Some(pt) = pt else { continue };
        if p_planes
            .iter()
            .any(|&(n, m)| n.dot(pt.centre) - m > pt.radius)
        {
            continue;
        }
        let facet = planes.len() + k;
        for face in faces(pt) {
            // Clip to P, then keep what lies outside every other tool.
            let mut inside_p = Some(face);
            for &(n, m) in &p_planes {
                if n.dot(pt.centre) - m < -pt.radius {
                    continue;
                }
                inside_p = inside_p.and_then(|p| p.clip(n, m, Edge::Visible));
            }
            let Some(inside_p) = inside_p else { continue };
            let mut pieces = vec![inside_p];
            for (j, other) in polytopes.iter().enumerate() {
                let Some(other) = other else { continue };
                if j == k || (other.centre - pt.centre).length() > other.radius + pt.radius {
                    continue;
                }
                pieces = pieces
                    .into_iter()
                    .flat_map(|p| split_outside(p, other))
                    .collect();
            }
            for p in pieces.into_iter().filter_map(finalise) {
                let p = p.reversed();
                let c = centroid(&p.v);
                let at = Vec3::new(c.x as f32, c.y as f32, c.z as f32);
                let n = tools[k].outward_normal(at);
                // The tool's outward normal points into the stone's material.
                let normal = -DVec3::new(f64::from(n.x), f64::from(n.y), f64::from(n.z));
                edge_visible.push(visible(&p));
                piece_normals.push(normal);
                push_ring(&mut out, facet, p.v, c, normal);
            }
        }
    }

    out.piece_normals = Some(piece_normals);
    out.edge_visible = Some(edge_visible);
    let volume = super::mesh::mesh_volume(&out);
    if volume.is_finite() && volume > 0.0 {
        SolidStatus::Closed(out)
    } else {
        let corners: BTreeSet<[u64; 3]> = out
            .rings
            .iter()
            .flat_map(|(_, r)| r.iter())
            .map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])
            .collect();
        SolidStatus::Degenerate {
            vertex_count: corners.len(),
            volume: volume.is_finite().then_some(volume),
        }
    }
}
