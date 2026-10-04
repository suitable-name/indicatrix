//! A rough given as a closed triangle mesh, which may be non-convex.
//!
//! The planner's rough model is a convex polytope. A scanned rough with a notch or a
//! hollow is not, and planning inside its convex hull would place stones in air. A
//! [`RoughMesh`] is the exact solid: the hull still bounds the planner's grid and region
//! (so convex roughs are untouched), and the mesh is consulted only to REJECT or SHRINK a
//! placement that reaches into material that is not there.
//!
//! Everything here is in millimetres in the rough frame (bounding box at the origin, see
//! [`import_mesh`](super::hull::import_mesh)) and deterministic: ordered maps and sorted
//! vectors only, the IEEE total order, fixed ray directions, triangles in file order.
//!
//! # The predicate
//!
//! A convex body P lies in the solid M exactly when no triangle of M meets the interior of
//! P and one interior point of P (its centroid) is inside M: M's boundary is a closed
//! surface, so P cannot be partly in and partly out without the surface crossing it. A
//! stone also keeps a clearance `inset` (skin plus allowance) from the surface, which the
//! queries apply by inflating the body, never the mesh. The inflation is exact for a box
//! and, for a polytope, shifts every plane outward, which contains the true offset body:
//! it can only reject too much, never accept a stone that touches the clearance.
//!
//! Touching is not meeting: the body is shrunk by [`RoughMesh::eps`] first, so a stone
//! that rests exactly on the clearance (the LP puts it there, to within its own
//! `1e-9 (1 + |m|)` tolerance) is clear.
//!
//! # Limitation
//!
//! A self-intersecting scan has no well-defined inside; the parity test then answers
//! wrongly near the intersection. [`RoughMesh::new`] checks that the mesh is a closed,
//! consistently oriented manifold, which rules out the common defects but not
//! self-intersection.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use glam::DVec3;

/// The most triangles a mesh may have. The planner asks the mesh for every candidate
/// piece, so a very fine scan has to be decimated first.
pub const MAX_MESH_TRIANGLES: usize = 50_000;

/// Vertices closer than this fraction of the mesh's size are one vertex.
const WELD_FRACTION: f64 = 1e-7;
/// A triangle whose doubled area is below this fraction of the squared size is dropped.
const MIN_AREA_FRACTION: f64 = 1e-14;
/// Smallest volume, as a fraction of the cubed size, a mesh may enclose.
const MIN_VOLUME_FRACTION: f64 = 1e-12;
/// The shrink applied to a query body, as a fraction of the mesh's size.
const EPS_FRACTION: f64 = 1e-7;
/// How far a cut plane is moved outward before volumes are taken, as a fraction of the
/// mesh's size. A vertex exactly on a cut then counts as inside, which is the volume of a
/// plane `1e-10` of the size further out: the volume is continuous in the plane, and the
/// shift makes every coincidence (a cut along a face of the mesh) generic.
const CUT_SHIFT_FRACTION: f64 = 1e-10;
/// Triangles per BVH leaf.
const LEAF_SIZE: usize = 4;
/// Blocker planes whose normals are within this angle (5 degrees, as a cosine) of the chosen
/// one are one wall of a scanned surface; see [`RoughMesh::blocker_rows`].
const SAME_WALL_COS: f64 = 0.996_194_698_091_745_5;
/// The most rows [`RoughMesh::blocker_rows`] returns in one round.
const MAX_ROWS_PER_ROUND: usize = 32;
/// Capacity of the traversal stack: the tree is balanced, so 128 is far beyond its depth.
const STACK: usize = 128;

/// The fixed ray directions of the inside test. Irrational ratios keep a ray from running
/// along the edges and through the vertices of a mesh with round coordinates.
const RAY_DIRECTIONS: [DVec3; 3] = [
    DVec3::new(1.0, 1.414_7, 1.732_050_807_568_877),
    DVec3::new(-0.618_033_988_749_895, 1.0, 2.236_067_977_499_79),
    DVec3::new(2.645_751_311_064_591, -1.0, 0.577_350_269_189_626),
];

/// Why a triangle mesh is not a usable rough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshError {
    /// There are no faces.
    NoFaces,
    /// This face (0-based, in the order given) names a vertex that does not exist, or one
    /// that is not finite.
    BadFace(usize),
    /// Some edge belongs to one triangle only: the surface has a hole.
    Open,
    /// Some edge belongs to more than two triangles.
    NonManifold,
    /// Two triangles that share an edge run along it in the same direction: the faces are
    /// not consistently wound.
    Inconsistent,
    /// The mesh has this many triangles, more than [`MAX_MESH_TRIANGLES`].
    TooManyTriangles(usize),
    /// Every triangle has no area, or the mesh encloses no volume.
    Degenerate,
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NoFaces => write!(f, "the mesh has no faces"),
            Self::BadFace(i) => write!(f, "face {} names a missing or invalid vertex", i + 1),
            Self::Open => write!(f, "the mesh surface has a hole (an edge with one face)"),
            Self::NonManifold => write!(f, "the mesh has an edge shared by more than two faces"),
            Self::Inconsistent => write!(f, "the mesh faces are not consistently wound"),
            Self::TooManyTriangles(n) => write!(
                f,
                "the mesh has {n} triangles; at most {MAX_MESH_TRIANGLES} are supported, so \
                 simplify the mesh first"
            ),
            Self::Degenerate => write!(f, "the mesh encloses no volume"),
        }
    }
}

impl std::error::Error for MeshError {}

/// How the solid meets a box (see [`RoughMesh::box_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxState {
    /// The box, with its clearance, lies in the material.
    Clear,
    /// The box lies in air: the surface does not meet it and its centre is outside.
    Air,
    /// The surface meets the box: some of it may be material.
    Crossing,
}

/// A node of the bounding-volume hierarchy. A leaf (`count > 0`) holds the triangles
/// `order[first..first + count]`; an inner node has children `first` and `first + 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BvhNode {
    min: DVec3,
    max: DVec3,
    first: u32,
    count: u32,
}

impl BvhNode {
    const EMPTY: Self = Self {
        min: DVec3::ZERO,
        max: DVec3::ZERO,
        first: 0,
        count: 0,
    };
}

/// A closed, consistently wound triangle mesh with outward normals, and the queries the
/// planner needs. See the module documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct RoughMesh {
    verts: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
    /// The outward unit normal and offset `n . p == d` of every triangle's plane.
    planes: Vec<(DVec3, f64)>,
    volume: f64,
    lo: DVec3,
    hi: DVec3,
    bvh: Vec<BvhNode>,
    /// Triangle indices in BVH order.
    order: Vec<u32>,
}

/// The surface of a mesh cut by planes, for display.
#[derive(Debug, Clone, PartialEq)]
pub struct ClippedSurface {
    /// The triangles of the mesh's own surface that remain, outward wound.
    pub triangles: Vec<[DVec3; 3]>,
    /// One cap per cut that cuts into the material.
    pub caps: Vec<SurfaceCap>,
}

/// The flat face a cut leaves on the mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceCap {
    /// Index of the cut in the list given to [`RoughMesh::clipped_surface`].
    pub cut: usize,
    /// The cut's outward unit normal.
    pub normal: DVec3,
    /// The face as triangles, wound counter-clockwise seen from outside.
    pub triangles: Vec<[DVec3; 3]>,
}

/// What a cut plane is in the frame of a volume computation.
#[derive(Debug, Clone, Copy)]
struct Cut {
    /// Unit outward normal.
    n: DVec3,
    /// Offset in the frame centred on the mesh, already moved by the cut shift.
    m: f64,
    /// Position in the list the caller gave.
    index: usize,
}

impl RoughMesh {
    /// Builds a mesh from `points` and triangles `tris` indexing them.
    ///
    /// Vertices closer than `1e-7` of the mesh's size are welded (in first-use order, so
    /// the result does not depend on anything but the input), triangles with no area are
    /// dropped, and every undirected edge must then belong to exactly two triangles that
    /// run along it in opposite directions. The orientation is NOT unified per component:
    /// a cavity keeps the inward-pointing normals it was written with. If the enclosed
    /// volume is negative the whole mesh is flipped.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError`] for no faces, too many triangles, a bad vertex index, an open
    /// or non-manifold or inconsistently wound surface, or a mesh with no volume.
    pub fn new(points: &[DVec3], tris: &[[u32; 3]]) -> Result<Self, MeshError> {
        if tris.is_empty() {
            return Err(MeshError::NoFaces);
        }
        if tris.len() > MAX_MESH_TRIANGLES {
            return Err(MeshError::TooManyTriangles(tris.len()));
        }
        for (i, tri) in tris.iter().enumerate() {
            let usable = |&v: &u32| points.get(v as usize).is_some_and(|p| p.is_finite());
            if !tri.iter().all(usable) {
                return Err(MeshError::BadFace(i));
            }
        }
        let (lo, hi) = bounds_of(tris.iter().flatten().map(|&v| points[v as usize]));
        let scale = (hi - lo).max_element();
        if scale.is_nan() || scale <= 0.0 {
            return Err(MeshError::Degenerate);
        }

        let (verts, mut welded) = weld(points, tris, scale);
        welded.retain(|tri| {
            let [a, b, c] = tri.map(|v| verts[v as usize]);
            let doubled_area = (b - a).cross(c - a).length();
            tri[0] != tri[1]
                && tri[1] != tri[2]
                && tri[0] != tri[2]
                && doubled_area > MIN_AREA_FRACTION * scale * scale
        });
        if welded.is_empty() {
            return Err(MeshError::Degenerate);
        }
        let (verts, mut tris) = compact(&verts, &welded);
        check_closed(&tris)?;
        if signed_volume(&verts, &tris, lo) < 0.0 {
            for tri in &mut tris {
                tri.swap(1, 2);
            }
        }
        Self::from_parts(verts, tris).ok_or(MeshError::Degenerate)
    }

    /// The mesh of already checked vertices and triangles; `None` when it encloses no
    /// volume.
    fn from_parts(verts: Vec<DVec3>, tris: Vec<[u32; 3]>) -> Option<Self> {
        let (lo, hi) = bounds_of(verts.iter().copied());
        let scale = (hi - lo).max_element();
        let volume = signed_volume(&verts, &tris, lo);
        if !(volume.is_finite() && volume > MIN_VOLUME_FRACTION * scale * scale * scale) {
            return None;
        }
        let planes = tris
            .iter()
            .map(|tri| {
                let [a, b, c] = tri.map(|v| verts[v as usize]);
                let n = (b - a).cross(c - a).normalize_or_zero();
                (n, n.dot(a))
            })
            .collect();
        let (bvh, order) = build_bvh(&verts, &tris);
        Some(Self {
            verts,
            tris,
            planes,
            volume,
            lo,
            hi,
            bvh,
            order,
        })
    }

    /// The mesh moved by `by`; `None` when the moved mesh would enclose no volume (the
    /// caller must not pair moved planes with an unmoved mesh).
    #[must_use]
    pub fn translated(&self, by: DVec3) -> Option<Self> {
        let verts = self.verts.iter().map(|&v| v + by).collect();
        Self::from_parts(verts, self.tris.clone())
    }

    /// The mesh with every length multiplied by `factor`; `None` when `factor` is not a
    /// usable scale (finite and positive) or the scaled mesh would enclose no volume.
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Option<Self> {
        if !(factor.is_finite() && factor > 0.0) {
            return None;
        }
        let verts = self.verts.iter().map(|&v| v * factor).collect();
        Self::from_parts(verts, self.tris.clone())
    }

    /// The welded vertices, in the rough frame in mm. With [`triangles`](Self::triangles)
    /// these are what a saved plan stores.
    #[must_use]
    pub fn vertices(&self) -> &[DVec3] {
        &self.verts
    }

    /// The triangles as indices into [`vertices`](Self::vertices), wound counter-clockwise
    /// seen from outside the material.
    #[must_use]
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.tris
    }

    /// The enclosed volume in mm^3.
    #[must_use]
    pub const fn volume(&self) -> f64 {
        self.volume
    }

    /// The minimum and maximum corner of the mesh's bounding box.
    #[must_use]
    pub const fn bounds(&self) -> (DVec3, DVec3) {
        (self.lo, self.hi)
    }

    /// The outward unit normal and offset of triangle `index`'s plane (`n . p <= d` on the
    /// material side).
    ///
    /// # Panics
    ///
    /// Panics if `index` is not a triangle of the mesh.
    #[must_use]
    pub fn triangle_plane(&self, index: u32) -> (DVec3, f64) {
        self.planes[index as usize]
    }

    /// The shrink applied to a query body: touching the clearance is not meeting it.
    #[must_use]
    pub fn eps(&self) -> f64 {
        EPS_FRACTION * (self.hi - self.lo).max_element()
    }

    /// The corners of triangle `t`.
    fn corners(&self, t: u32) -> [DVec3; 3] {
        self.tris[t as usize].map(|v| self.verts[v as usize])
    }

    /// Visits the leaves under every node `enter` accepts; `visit` returns `false` to stop.
    fn walk(&self, mut enter: impl FnMut(&BvhNode) -> bool, mut visit: impl FnMut(u32) -> bool) {
        let mut stack = [0_u32; STACK];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let node = &self.bvh[stack[top] as usize];
            if !enter(node) {
                continue;
            }
            if node.count > 0 {
                let leaf = &self.order[node.first as usize..(node.first + node.count) as usize];
                for &t in leaf {
                    if !visit(t) {
                        return;
                    }
                }
            } else if top + 2 <= STACK {
                // The left child is popped first.
                stack[top] = node.first + 1;
                stack[top + 1] = node.first;
                top += 2;
            }
        }
    }

    /// Whether `p` is inside the material: the majority of three ray parities.
    ///
    /// A point on the surface may answer either way.
    #[must_use]
    pub fn contains_point(&self, p: DVec3) -> bool {
        if p.cmplt(self.lo).any() || p.cmpgt(self.hi).any() {
            return false;
        }
        let odd = RAY_DIRECTIONS
            .iter()
            .filter(|&&dir| self.crossings(p, dir) % 2 == 1)
            .count();
        odd >= 2
    }

    /// The number of triangles the ray `origin + t dir`, `t > 0`, meets.
    fn crossings(&self, origin: DVec3, dir: DVec3) -> usize {
        let inv = DVec3::ONE / dir;
        let mut count = 0;
        self.walk(
            |node| ray_meets_box(origin, inv, node.min, node.max),
            |t| {
                count += usize::from(ray_meets_triangle(origin, dir, self.corners(t)));
                true
            },
        );
        count
    }

    /// The box `[min, max]` grown by `inset` and shrunk by [`eps`](Self::eps), as corners.
    fn inflated(&self, min: [f64; 3], max: [f64; 3], inset: f64) -> (DVec3, DVec3) {
        let grow = inset - self.eps();
        let mut lo = DVec3::from(min) - DVec3::splat(grow);
        let mut hi = DVec3::from(max) + DVec3::splat(grow);
        // A box thinner than the shrink collapses to its middle.
        let mid = (lo + hi) * 0.5;
        lo = lo.min(mid);
        hi = hi.max(mid);
        (lo, hi)
    }

    /// How the material meets the box `[min, max]` kept `inset` clear of the surface.
    ///
    /// [`Crossing`](BoxState::Crossing) when a triangle meets the box grown by `inset`;
    /// otherwise the surface is entirely outside it and its centre decides:
    /// [`Clear`](BoxState::Clear) in the material, [`Air`](BoxState::Air) outside.
    #[must_use]
    pub fn box_state(&self, min: [f64; 3], max: [f64; 3], inset: f64) -> BoxState {
        let (lo, hi) = self.inflated(min, max, inset);
        let (centre, half) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
        let mut hit = false;
        self.walk(
            |node| node.min.cmple(hi).all() && node.max.cmpge(lo).all(),
            |t| {
                hit = triangle_meets_box(centre, half, self.corners(t));
                !hit
            },
        );
        if hit {
            BoxState::Crossing
        } else if self.contains_point(DVec3::from(min).midpoint(DVec3::from(max))) {
            BoxState::Clear
        } else {
            BoxState::Air
        }
    }

    /// Writes into `out` (cleared first, ascending) the triangles that meet the box
    /// `[min, max]` grown by `inset`.
    pub fn box_blockers(&self, min: [f64; 3], max: [f64; 3], inset: f64, out: &mut Vec<u32>) {
        out.clear();
        let (lo, hi) = self.inflated(min, max, inset);
        let (centre, half) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
        self.walk(
            |node| node.min.cmple(hi).all() && node.max.cmpge(lo).all(),
            |t| {
                if triangle_meets_box(centre, half, self.corners(t)) {
                    out.push(t);
                }
                true
            },
        );
        out.sort_unstable();
    }

    /// Writes into `out` (cleared first, ascending) the triangles that meet the interior of
    /// the convex polytope `n . p <= d` for `planes_world`, grown by `inset` (every plane
    /// moved outward by it) and shrunk by [`eps`](Self::eps).
    ///
    /// The BVH discards what lies wholly beyond a plane; each surviving triangle is
    /// clipped by every plane (Sutherland-Hodgman) and is a blocker when anything of it
    /// survives.
    pub fn polytope_blockers(&self, planes_world: &[(DVec3, f64)], inset: f64, out: &mut Vec<u32>) {
        out.clear();
        let grow = inset - self.eps();
        let beyond = |lo: DVec3, hi: DVec3| {
            planes_world.iter().any(|&(n, d)| {
                let low = DVec3::select(n.cmpge(DVec3::ZERO), lo, hi);
                n.dot(low) > d + grow
            })
        };
        let mut current: Vec<DVec3> = Vec::with_capacity(12);
        let mut next: Vec<DVec3> = Vec::with_capacity(12);
        self.walk(
            |node| !beyond(node.min, node.max),
            |t| {
                current.clear();
                current.extend(self.corners(t));
                for &(n, d) in planes_world {
                    clip_halfspace(&current, n, d + grow, &mut next);
                    std::mem::swap(&mut current, &mut next);
                    if current.len() < 3 {
                        return true;
                    }
                }
                out.push(t);
                true
            },
        );
        out.sort_unstable();
    }

    /// The plane of the blocking triangle (of `blockers`) that a stone reaches least far
    /// past, among those no plane of `known` already equals; `violation(n, d)` is how far
    /// the stone passes `n . p <= d`. `None` when every blocker's plane is known.
    ///
    /// This is the row the cutting-plane fit adds next: the stone is kept on the material
    /// side of the triangle's plane. One row per round, the cheapest to satisfy, because
    /// the planes of the walls of a notch face each other: all of them together leave no
    /// stone at all, while the cheapest first moves the stone out of the notch the short
    /// way. Ties go to the lower triangle index.
    pub(crate) fn least_violated_blocker(
        &self,
        blockers: &[u32],
        known: &[(DVec3, f64)],
        violation: impl Fn(DVec3, f64) -> f64,
    ) -> Option<(DVec3, f64)> {
        let tolerance = 1e-9 * (self.hi - self.lo).max_element();
        let mut best: Option<(f64, (DVec3, f64))> = None;
        for &t in blockers {
            let (n, d) = self.planes[t as usize];
            let seen = known
                .iter()
                .any(|&(kn, kd)| kn.dot(n) > 1.0 - 1e-12 && (kd - d).abs() <= tolerance);
            if seen {
                continue;
            }
            let cost = violation(n, d);
            if best.is_none_or(|(least, _)| cost < least) {
                best = Some((cost, (n, d)));
            }
        }
        best.map(|(_, plane)| plane)
    }

    /// The rows the cutting-plane fit adds next: the plane
    /// [`least_violated_blocker`](Self::least_violated_blocker) chooses, then every other unknown blocker plane that
    /// faces the same way within 5 degrees of it (at most 32 rows in all, the chosen one
    /// first, the rest in triangle order).
    ///
    /// A noisy scanned wall is a thousand slightly different planes, and one row per round
    /// only ever removes the stone's contact with one of them. Planes that face the same
    /// way are the same wall, so they are added together; planes that face each other (the
    /// two walls of a notch) never are, which is what keeps the rows feasible. Empty when
    /// every blocker's plane is known.
    pub(crate) fn blocker_rows(
        &self,
        blockers: &[u32],
        known: &[(DVec3, f64)],
        violation: impl Fn(DVec3, f64) -> f64,
    ) -> Vec<(DVec3, f64)> {
        let Some((n0, d0)) = self.least_violated_blocker(blockers, known, violation) else {
            return Vec::new();
        };
        let tolerance = 1e-9 * (self.hi - self.lo).max_element();
        let same = |(an, ad): (DVec3, f64), (bn, bd): (DVec3, f64)| {
            an.dot(bn) > 1.0 - 1e-12 && (ad - bd).abs() <= tolerance
        };
        let mut rows = vec![(n0, d0)];
        for &t in blockers {
            if rows.len() >= MAX_ROWS_PER_ROUND {
                break;
            }
            let plane = self.planes[t as usize];
            if plane.0.dot(n0) < SAME_WALL_COS
                || known.iter().any(|&k| same(k, plane))
                || rows.iter().any(|&r| same(r, plane))
            {
                continue;
            }
            rows.push(plane);
        }
        rows
    }

    /// The cuts as normalised planes in the frame centred on the mesh, deduplicated: of
    /// equal normals only the tighter one is kept.
    fn effective_cuts(&self, cuts: &[(DVec3, f64)], origin: DVec3) -> Vec<Cut> {
        let shift = CUT_SHIFT_FRACTION * (self.hi - self.lo).max_element();
        let mut kept: Vec<Cut> = Vec::new();
        for (index, &(n, m)) in cuts.iter().enumerate() {
            let len = n.length();
            if !(len > 0.0 && len.is_finite() && m.is_finite()) {
                continue;
            }
            let n = n / len;
            let m = m / len - n.dot(origin) + shift;
            match kept.iter_mut().find(|c| c.n.dot(n) > 1.0 - 1e-12) {
                Some(prev) if m < prev.m => {
                    prev.m = m;
                    prev.index = index;
                }
                Some(_) => {}
                None => kept.push(Cut { n, m, index }),
            }
        }
        kept
    }

    /// The triangles (ascending) that can contribute to the material inside `cuts`, in the
    /// frame centred on `origin`, each with whether it still has to be clipped.
    ///
    /// The BVH is walked once: a node wholly outside some cut is dropped, and one wholly
    /// inside every cut is kept as it is (no clipping, which would copy its triangles
    /// unchanged). A node within [`CUT_SHIFT_FRACTION`]-sized slack of a plane counts as
    /// neither, so rounding can never change an outcome; the surviving triangles are clipped
    /// exactly as the full scan clips them and in the same order, so the volume is
    /// bit-identical. With `prune` false every triangle is returned to be clipped.
    fn clip_candidates(&self, origin: DVec3, cuts: &[Cut], prune: bool) -> Vec<(u32, bool)> {
        if !prune || self.bvh.is_empty() {
            return (0..self.tris.len() as u32).map(|t| (t, true)).collect();
        }
        let slack = 1e-9 * (self.hi - self.lo).max_element();
        let mut found: Vec<(u32, bool)> = Vec::new();
        let mut stack: Vec<(u32, bool)> = vec![(0, false)];
        while let Some((index, inside)) = stack.pop() {
            let node = &self.bvh[index as usize];
            let mut inside = inside;
            if !inside {
                let (lo, hi) = (node.min - origin, node.max - origin);
                let mut beyond = false;
                inside = true;
                for cut in cuts {
                    let positive = cut.n.cmpge(DVec3::ZERO);
                    let least = cut.n.dot(DVec3::select(positive, lo, hi));
                    let most = cut.n.dot(DVec3::select(positive, hi, lo));
                    if least > cut.m + slack {
                        beyond = true;
                        break;
                    }
                    inside &= most <= cut.m - slack;
                }
                if beyond {
                    continue;
                }
            }
            if node.count > 0 {
                let leaf = &self.order[node.first as usize..(node.first + node.count) as usize];
                found.extend(leaf.iter().map(|&t| (t, !inside)));
            } else {
                stack.push((node.first, inside));
                stack.push((node.first + 1, inside));
            }
        }
        found.sort_unstable();
        found
    }

    /// The triangles of the mesh clipped by `cuts`, in the frame centred on `origin`.
    fn clipped_polygons(&self, origin: DVec3, cuts: &[Cut], prune: bool) -> Vec<Vec<DVec3>> {
        let mut polygons = Vec::new();
        let mut current: Vec<DVec3> = Vec::with_capacity(8);
        let mut next: Vec<DVec3> = Vec::with_capacity(8);
        for (t, clip) in self.clip_candidates(origin, cuts, prune) {
            current.clear();
            current.extend(self.corners(t).map(|p| p - origin));
            if clip {
                for cut in cuts {
                    clip_halfspace(&current, cut.n, cut.m, &mut next);
                    std::mem::swap(&mut current, &mut next);
                    if current.len() < 3 {
                        break;
                    }
                }
            }
            if current.len() >= 3 {
                polygons.push(current.clone());
            }
        }
        polygons
    }

    /// The triangles (ascending) that can have vertices on both sides of `cut`'s plane: the
    /// BVH discards every node wholly on one side, with the slack of
    /// [`clip_candidates`](Self::clip_candidates). With `prune` false, all of them.
    fn crossing_triangles(&self, origin: DVec3, cut: &Cut, prune: bool) -> Vec<u32> {
        if !prune || self.bvh.is_empty() {
            return (0..self.tris.len() as u32).collect();
        }
        let slack = 1e-9 * (self.hi - self.lo).max_element();
        let positive = cut.n.cmpge(DVec3::ZERO);
        let mut found = Vec::new();
        self.walk(
            |node| {
                let (lo, hi) = (node.min - origin, node.max - origin);
                let least = cut.n.dot(DVec3::select(positive, lo, hi));
                let most = cut.n.dot(DVec3::select(positive, hi, lo));
                least <= cut.m + slack && most > cut.m - slack
            },
            |t| {
                found.push(t);
                true
            },
        );
        found.sort_unstable();
        found
    }

    /// The closed outlines the mesh's surface makes on the plane of `cuts[which]`, as
    /// polygons wound counter-clockwise about the cut's normal (holes clockwise), clipped
    /// by every other cut.
    ///
    /// A triangle with vertices on both sides of the plane contributes one segment between
    /// the points where its edges cross; those points are keyed by the edge, so the two
    /// triangles on an edge agree on them exactly and the segments chain into closed
    /// loops whatever the rounding. A vertex exactly on the plane counts as inside (the
    /// cut shift makes that the generic case).
    #[expect(
        clippy::many_single_char_names,
        reason = "entry and exit crossings, loop and vertex names are short by convention"
    )]
    fn cap_polygons(
        &self,
        origin: DVec3,
        cuts: &[Cut],
        which: usize,
        prune: bool,
    ) -> Vec<Vec<DVec3>> {
        let cut = cuts[which];
        let mut next: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
        let mut points: BTreeMap<(u32, u32), DVec3> = BTreeMap::new();
        for t in self.crossing_triangles(origin, &cut, prune) {
            let tri = &self.tris[t as usize];
            let v = tri.map(|k| self.verts[k as usize] - origin);
            let s = v.map(|p| cut.n.dot(p) - cut.m);
            let inside = s.map(|x| x <= 0.0);
            let mut entry = None;
            let mut exit = None;
            for k in 0..3 {
                let l = (k + 1) % 3;
                if inside[k] == inside[l] {
                    continue;
                }
                let (a, b) = if tri[k] < tri[l] { (k, l) } else { (l, k) };
                let key = (tri[a], tri[b]);
                let point = v[a] + (v[b] - v[a]) * (s[a] / (s[a] - s[b]));
                points.entry(key).or_insert(point);
                if inside[k] {
                    exit = Some(key);
                } else {
                    entry = Some(key);
                }
            }
            // The clipped surface leaves the plane's inside at `exit` and returns at
            // `entry`; the cap, which closes it, runs the other way.
            if let (Some(entry), Some(exit)) = (entry, exit) {
                next.insert(entry, exit);
            }
        }

        let mut loops = Vec::new();
        let mut done: BTreeSet<(u32, u32)> = BTreeSet::new();
        for &start in next.keys() {
            if done.contains(&start) {
                continue;
            }
            let mut outline = Vec::new();
            let mut at = start;
            while done.insert(at) {
                outline.push(points[&at]);
                match next.get(&at) {
                    Some(&following) => at = following,
                    None => break,
                }
            }
            loops.push(outline);
        }

        let mut scratch = Vec::new();
        for (j, other) in cuts.iter().enumerate() {
            if j == which {
                continue;
            }
            for outline in &mut loops {
                clip_halfspace(outline, other.n, other.m, &mut scratch);
                std::mem::swap(outline, &mut scratch);
            }
            loops.retain(|outline| outline.len() >= 3);
        }
        loops
    }

    /// The volume of the material that satisfies `n . p <= m` for every `(n, m)` of `cuts`,
    /// in mm^3 (exact up to rounding).
    ///
    /// The material's boundary after the cuts is the mesh's own triangles, clipped, and one
    /// flat cap per cut; the divergence theorem gives `V = sum (1/3) p . n dA` over it. A
    /// cap is the cross-section of the mesh with the cut plane (closed loops chained from
    /// the triangles that cross it) clipped by the other cuts, and its term is
    /// `m A / 3`. Cuts with equal normals are reduced to the tighter one.
    ///
    /// With no cuts this is [`volume`](Self::volume). The cuts of a rough model are its
    /// own planes, which a face cut may position anywhere: a cut through a notch wall or
    /// along a face of the mesh gives the correct volume too.
    #[must_use]
    pub fn volume_within(&self, cuts: &[(DVec3, f64)]) -> f64 {
        self.volume_within_pruned(cuts, true)
    }

    /// [`volume_within`](Self::volume_within), with the BVH pruning of the triangles
    /// switched by `prune`: the result is bit-identical either way, only the work differs.
    fn volume_within_pruned(&self, cuts: &[(DVec3, f64)], prune: bool) -> f64 {
        if cuts.is_empty() {
            return self.volume;
        }
        let origin = (self.lo + self.hi) * 0.5;
        let cuts = self.effective_cuts(cuts, origin);
        let mut total = 0.0;
        for polygon in self.clipped_polygons(origin, &cuts, prune) {
            let first = polygon[0];
            for pair in polygon[1..].windows(2) {
                total += first.dot(pair[0].cross(pair[1])) / 6.0;
            }
        }
        for (which, cut) in cuts.iter().enumerate() {
            let area: f64 = self
                .cap_polygons(origin, &cuts, which, prune)
                .iter()
                .map(|outline| cut.n.dot(area_vector(outline)))
                .sum();
            total += cut.m * area / 3.0;
        }
        total.max(0.0)
    }

    /// The mesh's surface inside `cuts` and the caps the cuts leave, for display.
    ///
    /// The triangles keep the mesh's own winding. Cuts with equal normals are reduced to
    /// the tighter one, so a cap names the index of the cut that is kept.
    #[must_use]
    pub fn clipped_surface(&self, cuts: &[(DVec3, f64)]) -> ClippedSurface {
        let origin = (self.lo + self.hi) * 0.5;
        let cuts = self.effective_cuts(cuts, origin);
        let min_area = 1e-12 * (self.hi - self.lo).length_squared();
        let mut triangles = Vec::new();
        for polygon in self.clipped_polygons(origin, &cuts, true) {
            let first = polygon[0];
            for pair in polygon[1..].windows(2) {
                let area2 = (pair[0] - first).cross(pair[1] - first).length();
                if area2 > min_area {
                    triangles.push([first + origin, pair[0] + origin, pair[1] + origin]);
                }
            }
        }
        let mut caps = Vec::new();
        for (which, cut) in cuts.iter().enumerate() {
            let loops = self.cap_polygons(origin, &cuts, which, true);
            let cap_triangles = triangulate(&loops, cut.n, min_area);
            if !cap_triangles.is_empty() {
                caps.push(SurfaceCap {
                    cut: cut.index,
                    normal: cut.n,
                    triangles: cap_triangles
                        .into_iter()
                        .map(|tri| tri.map(|p| p + origin))
                        .collect(),
                });
            }
        }
        ClippedSurface { triangles, caps }
    }
}

/// The minimum and maximum corner of `points`.
fn bounds_of(points: impl Iterator<Item = DVec3>) -> (DVec3, DVec3) {
    points.fold(
        (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    )
}

/// The vertices of `tris` welded by quantised position, and the triangles over them.
///
/// Vertices are numbered in order of first use by a triangle, and a welded vertex keeps
/// the position of the first of its members.
fn weld(points: &[DVec3], tris: &[[u32; 3]], scale: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let quantum = WELD_FRACTION * scale;
    let mut ids: BTreeMap<[i64; 3], u32> = BTreeMap::new();
    let mut verts: Vec<DVec3> = Vec::new();
    let mut welded = Vec::with_capacity(tris.len());
    for tri in tris {
        welded.push(tri.map(|v| {
            let p = points[v as usize];
            let key = [p.x, p.y, p.z].map(|c| (c / quantum).round() as i64);
            *ids.entry(key).or_insert_with(|| {
                verts.push(p);
                (verts.len() - 1) as u32
            })
        }));
    }
    (verts, welded)
}

/// `tris` renumbered over only the vertices they use, in order of first use.
fn compact(verts: &[DVec3], tris: &[[u32; 3]]) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut renumber: BTreeMap<u32, u32> = BTreeMap::new();
    let mut kept = Vec::new();
    let mut out = Vec::with_capacity(tris.len());
    for tri in tris {
        out.push(tri.map(|v| {
            *renumber.entry(v).or_insert_with(|| {
                kept.push(verts[v as usize]);
                (kept.len() - 1) as u32
            })
        }));
    }
    (kept, out)
}

/// Checks that the triangles form a closed, consistently wound manifold: every undirected
/// edge in exactly two triangles, once per direction.
fn check_closed(tris: &[[u32; 3]]) -> Result<(), MeshError> {
    // Per undirected edge (low, high): uses low -> high and uses high -> low.
    let mut edges: BTreeMap<(u32, u32), [u32; 2]> = BTreeMap::new();
    for tri in tris {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let uses = edges.entry((a.min(b), a.max(b))).or_default();
            uses[usize::from(a > b)] += 1;
        }
    }
    let mut open = false;
    let mut inconsistent = false;
    for uses in edges.values() {
        match *uses {
            [1, 1] => {}
            [a, b] if a + b > 2 => return Err(MeshError::NonManifold),
            [a, b] if a + b == 1 => open = true,
            _ => inconsistent = true,
        }
    }
    if open {
        Err(MeshError::Open)
    } else if inconsistent {
        Err(MeshError::Inconsistent)
    } else {
        Ok(())
    }
}

/// The signed volume the triangles enclose, measured from `origin` (a corner of the mesh,
/// for accuracy).
fn signed_volume(verts: &[DVec3], tris: &[[u32; 3]], origin: DVec3) -> f64 {
    tris.iter()
        .map(|tri| {
            let [a, b, c] = tri.map(|v| verts[v as usize] - origin);
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

/// The BVH over `tris`: a median split on the longest axis of the centroids, ties by
/// triangle index.
fn build_bvh(verts: &[DVec3], tris: &[[u32; 3]]) -> (Vec<BvhNode>, Vec<u32>) {
    let boxes: Vec<(DVec3, DVec3)> = tris
        .iter()
        .map(|tri| bounds_of(tri.iter().map(|&v| verts[v as usize])))
        .collect();
    let centroids: Vec<DVec3> = boxes.iter().map(|&(lo, hi)| (lo + hi) * 0.5).collect();
    let mut order: Vec<u32> = (0..tris.len() as u32).collect();
    let mut nodes = vec![BvhNode::EMPTY];
    let mut work = vec![(0_usize, 0_usize, tris.len())];
    while let Some((slot, start, end)) = work.pop() {
        let (min, max) = order[start..end].iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), &t| (lo.min(boxes[t as usize].0), hi.max(boxes[t as usize].1)),
        );
        if end - start <= LEAF_SIZE {
            nodes[slot] = BvhNode {
                min,
                max,
                first: start as u32,
                count: (end - start) as u32,
            };
            continue;
        }
        let (c_lo, c_hi) = bounds_of(order[start..end].iter().map(|&t| centroids[t as usize]));
        let spread = c_hi - c_lo;
        let axis = if spread.x >= spread.y && spread.x >= spread.z {
            0
        } else if spread.y >= spread.z {
            1
        } else {
            2
        };
        order[start..end].sort_by(|&a, &b| {
            centroids[a as usize][axis]
                .total_cmp(&centroids[b as usize][axis])
                .then(a.cmp(&b))
        });
        let mid = start + (end - start) / 2;
        let left = nodes.len();
        nodes.push(BvhNode::EMPTY);
        nodes.push(BvhNode::EMPTY);
        nodes[slot] = BvhNode {
            min,
            max,
            first: left as u32,
            count: 0,
        };
        work.push((left + 1, mid, end));
        work.push((left, start, mid));
    }
    (nodes, order)
}

/// Whether the ray `origin + t dir`, `t >= 0`, meets the box; `inv` is `1 / dir`.
fn ray_meets_box(origin: DVec3, inv: DVec3, min: DVec3, max: DVec3) -> bool {
    let t1 = (min - origin) * inv;
    let t2 = (max - origin) * inv;
    let t_enter = t1.min(t2).max_element();
    let t_exit = t1.max(t2).min_element();
    t_exit >= t_enter.max(0.0)
}

/// Whether the ray `origin + t dir`, `t > 0`, meets the triangle (Moller-Trumbore, both
/// sides). A hit exactly on an edge may count for both triangles of it; the callers vote
/// over three rays instead of trusting one.
#[expect(
    clippy::many_single_char_names,
    reason = "Moller-Trumbore's own variable names"
)]
fn ray_meets_triangle(origin: DVec3, dir: DVec3, [a, b, c]: [DVec3; 3]) -> bool {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() <= 1e-18 * e1.length() * e2.length() * dir.length() {
        return false;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    e2.dot(q) * inv > 0.0
}

/// Whether the triangle meets the closed box with the given centre and half extents
/// (Akenine-Moller: the box's three axes, the triangle's plane and the nine cross
/// products of edges and axes).
fn triangle_meets_box(centre: DVec3, half: DVec3, tri: [DVec3; 3]) -> bool {
    let v = tri.map(|p| p - centre);
    for axis in 0..3 {
        let lo = v[0][axis].min(v[1][axis]).min(v[2][axis]);
        let hi = v[0][axis].max(v[1][axis]).max(v[2][axis]);
        if lo > half[axis] || hi < -half[axis] {
            return false;
        }
    }
    let edges = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    let normal = edges[0].cross(edges[1]);
    if normal.dot(v[0]).abs() > half.dot(normal.abs()) {
        return false;
    }
    for edge in edges {
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            let sep = axis.cross(edge);
            if sep == DVec3::ZERO {
                continue;
            }
            let p = v.map(|q| sep.dot(q));
            let lo = p[0].min(p[1]).min(p[2]);
            let hi = p[0].max(p[1]).max(p[2]);
            let reach = half.dot(sep.abs());
            if lo > reach || hi < -reach {
                return false;
            }
        }
    }
    true
}

/// Writes into `out` the part of the polygon `poly` with `n . p <= limit`
/// (Sutherland-Hodgman). For a non-convex polygon the result may carry zero-width bridges
/// along the clipping line, which change neither its area nor its fan volume.
fn clip_halfspace(poly: &[DVec3], n: DVec3, limit: f64, out: &mut Vec<DVec3>) {
    out.clear();
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let (sa, sb) = (n.dot(a) - limit, n.dot(b) - limit);
        if sa <= 0.0 {
            out.push(a);
        }
        if (sa <= 0.0) != (sb <= 0.0) {
            out.push(a + (b - a) * (sa / (sa - sb)));
        }
    }
}

/// Half the sum of `a x b` over the edges of the closed polygon: its area times its
/// normal.
fn area_vector(poly: &[DVec3]) -> DVec3 {
    let mut sum = DVec3::ZERO;
    for (i, &a) in poly.iter().enumerate() {
        sum += a.cross(poly[(i + 1) % poly.len()]);
    }
    sum * 0.5
}

/// Triangulates the face bounded by `loops` (counter-clockwise outlines about `normal`,
/// clockwise holes) by ear clipping, after joining each hole to its outline by a bridge.
/// Triangles smaller than `min_area` (doubled) are dropped; a loop that cannot be
/// triangulated is skipped, which only thins a display.
fn triangulate(loops: &[Vec<DVec3>], normal: DVec3, min_area: f64) -> Vec<[DVec3; 3]> {
    let helper = if normal.x.abs() < 0.9 {
        DVec3::X
    } else {
        DVec3::Y
    };
    let u = normal.cross(helper).normalize_or_zero();
    let v = normal.cross(u);
    let flat = |p: DVec3| (p.dot(u), p.dot(v));
    let signed = |poly: &[DVec3]| normal.dot(area_vector(poly));

    let mut outlines: Vec<Vec<DVec3>> = Vec::new();
    let mut holes: Vec<Vec<DVec3>> = Vec::new();
    for outline in loops {
        if signed(outline) > 0.0 {
            outlines.push(outline.clone());
        } else if signed(outline) < 0.0 {
            holes.push(outline.clone());
        }
    }
    // A hole goes into the first outline that contains it, rightmost hole first.
    holes.sort_by(|a, b| {
        let right = |h: &[DVec3]| {
            h.iter()
                .map(|&p| flat(p).0)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        right(b).total_cmp(&right(a))
    });
    for hole in holes {
        let probe = flat(hole[0]);
        let home = outlines.iter().position(|outline| {
            let ring: Vec<(f64, f64)> = outline.iter().map(|&p| flat(p)).collect();
            point_in_ring(probe, &ring)
        });
        if let Some(home) = home {
            outlines[home] = bridge(&outlines[home], &hole, &flat);
        }
    }

    let mut out = Vec::new();
    for outline in &outlines {
        ear_clip(outline, &flat, min_area, normal, &mut out);
    }
    out
}

/// Whether `p` is inside the ring (even-odd).
fn point_in_ring(p: (f64, f64), ring: &[(f64, f64)]) -> bool {
    let mut inside = false;
    for (i, &(x1, y1)) in ring.iter().enumerate() {
        let (x2, y2) = ring[(i + 1) % ring.len()];
        if (y1 > p.1) != (y2 > p.1) && p.0 < (x2 - x1) * (p.1 - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
    }
    inside
}

/// Whether the open segments `a1 a2` and `b1 b2` cross.
fn segments_cross(a1: (f64, f64), a2: (f64, f64), b1: (f64, f64), b2: (f64, f64)) -> bool {
    let side = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
        (q.0 - p.0).mul_add(r.1 - p.1, -((q.1 - p.1) * (r.0 - p.0)))
    };
    let (d1, d2) = (side(a1, a2, b1), side(a1, a2, b2));
    let (d3, d4) = (side(b1, b2, a1), side(b1, b2, a2));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// `outline` with `hole` joined in along the shortest bridge from the hole's rightmost
/// vertex to an outline vertex that sees it.
fn bridge(outline: &[DVec3], hole: &[DVec3], flat: &impl Fn(DVec3) -> (f64, f64)) -> Vec<DVec3> {
    let hi = (0..hole.len())
        .max_by(|&a, &b| flat(hole[a]).0.total_cmp(&flat(hole[b]).0))
        .unwrap_or(0);
    let from = flat(hole[hi]);
    let dist = |p: DVec3| {
        let q = flat(p);
        (q.0 - from.0).hypot(q.1 - from.1)
    };
    let mut candidates: Vec<usize> = (0..outline.len()).collect();
    candidates.sort_by(|&a, &b| {
        dist(outline[a])
            .total_cmp(&dist(outline[b]))
            .then(a.cmp(&b))
    });
    let sees = |at: usize| {
        let to = flat(outline[at]);
        let clear_of = |ring: &[DVec3]| {
            (0..ring.len())
                .all(|i| !segments_cross(from, to, flat(ring[i]), flat(ring[(i + 1) % ring.len()])))
        };
        clear_of(outline) && clear_of(hole)
    };
    let at = candidates
        .iter()
        .copied()
        .find(|&c| sees(c))
        .or_else(|| candidates.first().copied())
        .unwrap_or(0);
    let mut joined = Vec::with_capacity(outline.len() + hole.len() + 2);
    joined.extend_from_slice(&outline[..=at]);
    for k in 0..=hole.len() {
        joined.push(hole[(hi + k) % hole.len()]);
    }
    joined.push(outline[at]);
    joined.extend_from_slice(&outline[at + 1..]);
    joined
}

/// Ear-clips the counter-clockwise ring `ring` into `out`.
///
/// The ring is a doubly linked list, so clipping an ear costs nothing, and only reflex
/// vertices are tested for lying inside a candidate ear (a convex vertex cannot be inside
/// the ear of a simple polygon). A clip can only turn a reflex neighbour convex, so the
/// reflex list shrinks as the ring does. The work is O(n r), `r` the reflex count, which is
/// O(n) for the outline of a cut through a smooth scan; the loop gives up after a whole
/// lap without a clip, which only thins a display.
fn ear_clip(
    ring: &[DVec3],
    flat: &impl Fn(DVec3) -> (f64, f64),
    min_area: f64,
    normal: DVec3,
    out: &mut Vec<[DVec3; 3]>,
) {
    let n = ring.len();
    if n < 3 {
        return;
    }
    let plan: Vec<(f64, f64)> = ring.iter().map(|&p| flat(p)).collect();
    let cross = |a: usize, b: usize, c: usize| {
        let (pa, pb, pc) = (plan[a], plan[b], plan[c]);
        (pb.0 - pa.0).mul_add(pc.1 - pa.1, -((pb.1 - pa.1) * (pc.0 - pa.0)))
    };
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = vec![true; n];
    let mut is_reflex: Vec<bool> = (0..n).map(|i| cross(prev[i], i, next[i]) < 0.0).collect();
    let mut reflex: Vec<usize> = (0..n).filter(|&i| is_reflex[i]).collect();
    let mut live = n;
    let mut at = 0;
    let mut idle = 0;
    while live >= 3 && idle <= live {
        let (a, c) = (prev[at], next[at]);
        let turn = cross(a, at, c);
        // A zero-width spike or a collinear vertex has no area: drop it, emitting nothing.
        let spike = turn.abs() <= min_area;
        let ear = !spike
            && turn > 0.0
            && !reflex.iter().any(|&k| {
                if !(alive[k] && is_reflex[k]) {
                    return false;
                }
                let same = |q: usize| (ring[k] - ring[q]).length_squared() <= 1e-24;
                if same(a) || same(at) || same(c) {
                    return false;
                }
                cross(a, at, k) >= 0.0 && cross(at, c, k) >= 0.0 && cross(c, a, k) >= 0.0
            });
        if !(spike || ear) {
            at = c;
            idle += 1;
            continue;
        }
        if ear {
            let tri = [ring[a], ring[at], ring[c]];
            // Counter-clockwise about the normal means the triangle's own normal agrees.
            if normal.dot((tri[1] - tri[0]).cross(tri[2] - tri[0])) > 0.0 {
                out.push(tri);
            }
        }
        alive[at] = false;
        is_reflex[at] = false;
        next[a] = c;
        prev[c] = a;
        live -= 1;
        idle = 0;
        for k in [a, c] {
            let now = live >= 3 && cross(prev[k], k, next[k]) < 0.0;
            if now && !is_reflex[k] {
                reflex.push(k);
            }
            is_reflex[k] = now;
        }
        if reflex.len() > 2 * live + 8 {
            reflex.retain(|&k| alive[k] && is_reflex[k]);
        }
        at = c;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rough_plan::shape::mesh_fixture::{C_SHAPE_OBJ, CUBE_OBJ, parse_obj};

    fn mesh_of(text: &str) -> RoughMesh {
        let (points, tris) = parse_obj(text);
        RoughMesh::new(&points, &tris).expect("a closed mesh")
    }

    /// Whether `p` is in the C-shaped fixture: the 20 mm cube minus the notch
    /// `x > 10, 5 < y < 15`.
    fn in_c_shape(p: DVec3) -> bool {
        (0.0..=20.0).contains(&p.x)
            && (0.0..=20.0).contains(&p.y)
            && (0.0..=20.0).contains(&p.z)
            && !(p.x > 10.0 && p.y > 5.0 && p.y < 15.0)
    }

    #[test]
    fn the_fixtures_have_their_hand_volumes() {
        // 20 x 20 x 20 minus the 10 x 10 x 20 notch.
        assert!((mesh_of(C_SHAPE_OBJ).volume() - 6000.0).abs() < 1e-9);
        assert!((mesh_of(CUBE_OBJ).volume() - 8000.0).abs() < 1e-9);
    }

    #[test]
    fn a_flipped_mesh_is_turned_outward() {
        let (points, mut tris) = parse_obj(C_SHAPE_OBJ);
        for tri in &mut tris {
            tri.swap(1, 2);
        }
        let mesh = RoughMesh::new(&points, &tris).expect("flipped meshes are accepted");
        assert!((mesh.volume() - 6000.0).abs() < 1e-9);
        assert!(mesh.contains_point(DVec3::new(2.0, 2.0, 2.0)));
    }

    #[test]
    fn an_open_mesh_is_rejected() {
        let (points, mut tris) = parse_obj(C_SHAPE_OBJ);
        tris.pop();
        assert_eq!(RoughMesh::new(&points, &tris), Err(MeshError::Open));
    }

    #[test]
    fn an_inconsistently_wound_mesh_is_rejected() {
        let (points, mut tris) = parse_obj(C_SHAPE_OBJ);
        tris[3].swap(1, 2);
        assert_eq!(RoughMesh::new(&points, &tris), Err(MeshError::Inconsistent));
    }

    #[test]
    fn an_edge_of_three_faces_is_rejected() {
        let (points, mut tris) = parse_obj(CUBE_OBJ);
        // A fin on the edge of vertices 0 and 1.
        let mut points = points;
        points.push(DVec3::new(-5.0, -5.0, -5.0));
        tris.push([0, 1, (points.len() - 1) as u32]);
        assert_eq!(RoughMesh::new(&points, &tris), Err(MeshError::NonManifold));
    }

    #[test]
    fn bad_input_is_named() {
        let (points, tris) = parse_obj(CUBE_OBJ);
        assert_eq!(RoughMesh::new(&points, &[]), Err(MeshError::NoFaces));
        let mut bad = tris;
        bad[2][1] = 99;
        assert_eq!(RoughMesh::new(&points, &bad), Err(MeshError::BadFace(2)));
        let flat = [points[0], points[1], points[2]];
        assert_eq!(RoughMesh::new(&flat, &[[0, 1, 2]]), Err(MeshError::Open));
        let line = [DVec3::ZERO, DVec3::X, DVec3::X * 2.0];
        assert_eq!(
            RoughMesh::new(&line, &[[0, 1, 2]]),
            Err(MeshError::Degenerate)
        );
        let many = vec![[0, 1, 2]; MAX_MESH_TRIANGLES + 1];
        assert_eq!(
            RoughMesh::new(&points, &many),
            Err(MeshError::TooManyTriangles(MAX_MESH_TRIANGLES + 1))
        );
    }

    #[test]
    fn duplicate_vertices_are_welded() {
        // Every triangle with its own copies of its corners.
        let (points, tris) = parse_obj(C_SHAPE_OBJ);
        let split: Vec<DVec3> = tris
            .iter()
            .flat_map(|tri| tri.map(|v| points[v as usize]))
            .collect();
        let split_tris: Vec<[u32; 3]> = (0..tris.len() as u32)
            .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
            .collect();
        let mesh = RoughMesh::new(&split, &split_tris).expect("welds shut");
        assert_eq!(mesh.vertices().len(), 16);
        assert!((mesh.volume() - 6000.0).abs() < 1e-9);
    }

    #[test]
    fn points_inside_and_outside_are_told_apart_even_on_a_lattice_of_edges() {
        // Coordinates on the planes of the notch walls and the faces, so axis-aligned
        // rays would run along edges and through vertices; the fixed irrational rays
        // must not care.
        let mesh = mesh_of(C_SHAPE_OBJ);
        let at = [2.5, 5.0, 7.5, 10.0, 12.5, 15.0, 17.5];
        let mut checked = 0;
        for &x in &at {
            for &y in &at {
                for &z in &at {
                    let p = DVec3::new(x, y, z);
                    // Points on a wall may go either way.
                    let on_wall = x == 10.0 || y == 5.0 || y == 15.0;
                    if on_wall {
                        continue;
                    }
                    assert_eq!(mesh.contains_point(p), in_c_shape(p), "{p}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 100);
        assert!(!mesh.contains_point(DVec3::new(-1.0, 10.0, 10.0)));
        assert!(!mesh.contains_point(DVec3::new(15.0, 10.0, 25.0)));
    }

    #[test]
    fn box_states_follow_the_surface() {
        let mesh = mesh_of(C_SHAPE_OBJ);
        let state = |min: [f64; 3], max: [f64; 3], inset: f64| mesh.box_state(min, max, inset);
        // Well inside the left arm, the lower arm and the back.
        assert_eq!(state([1.0; 3], [4.0; 3], 0.0), BoxState::Clear);
        assert_eq!(
            state([12.0, 0.5, 1.0], [18.0, 4.0, 19.0], 0.0),
            BoxState::Clear
        );
        // Wholly in the notch.
        assert_eq!(
            state([12.0, 7.0, 1.0], [18.0, 13.0, 19.0], 0.0),
            BoxState::Air
        );
        // Straddling the notch wall at x = 10.
        assert_eq!(
            state([8.0, 6.0, 1.0], [12.0, 9.0, 4.0], 0.0),
            BoxState::Crossing
        );
        // Resting on a wall is clear without a clearance and crossing with one.
        assert_eq!(
            state([2.0, 2.0, 2.0], [10.0, 5.0, 8.0], 0.0),
            BoxState::Clear
        );
        assert_eq!(
            state([2.0, 2.0, 2.0], [10.0, 5.0, 8.0], 0.1),
            BoxState::Crossing
        );
        // A box that flush to the outer face with no clearance is clear.
        assert_eq!(state([0.0; 3], [4.0; 3], 0.0), BoxState::Clear);
    }

    #[test]
    fn polytope_in_the_notch_is_blocked_and_a_stone_in_the_arm_is_not() {
        let mesh = mesh_of(C_SHAPE_OBJ);
        // The cube 12..18 x 7..13 x 1..19 as planes: in the notch.
        let cube = |lo: [f64; 3], hi: [f64; 3]| -> Vec<(DVec3, f64)> {
            let mut planes = Vec::new();
            for axis in 0..3 {
                let mut n = DVec3::ZERO;
                n[axis] = 1.0;
                planes.push((n, hi[axis]));
                planes.push((-n, -lo[axis]));
            }
            planes
        };
        let mut blockers = Vec::new();
        // Entirely in air: nothing of the surface is inside it, the centre is outside.
        mesh.polytope_blockers(
            &cube([12.0, 7.0, 1.0], [18.0, 13.0, 19.0]),
            0.0,
            &mut blockers,
        );
        assert!(blockers.is_empty());
        assert!(!mesh.contains_point(DVec3::new(15.0, 10.0, 10.0)));
        // Reaching into the wall at x = 10 (two triangles of the wall and the notch faces).
        mesh.polytope_blockers(&cube([8.0, 6.0, 1.0], [12.0, 9.0, 4.0]), 0.0, &mut blockers);
        assert!(!blockers.is_empty());
        // Inside the lower arm: clear, and with a clearance as well.
        mesh.polytope_blockers(
            &cube([11.0, 1.0, 1.0], [19.0, 4.0, 19.0]),
            0.5,
            &mut blockers,
        );
        assert!(blockers.is_empty());
        // The clearance alone can block: 0.5 from the notch floor at y = 5 with a 1 mm margin.
        mesh.polytope_blockers(
            &cube([11.0, 1.0, 1.0], [19.0, 4.5, 19.0]),
            1.0,
            &mut blockers,
        );
        assert!(!blockers.is_empty());
        // The row to add is the one the stone passes least far: here only the notch floor
        // (y = 5, outward normal +y) blocks; a plane already known is never chosen again.
        let reach = |n: DVec3, d: f64| n.dot(DVec3::new(15.0, 4.5, 10.0)) - d;
        let (n, d) = mesh
            .least_violated_blocker(&blockers, &[], reach)
            .expect("a blocker");
        assert!((n - DVec3::Y).length() < 1e-12 && (d - 5.0).abs() < 1e-12);
        assert_eq!(
            mesh.least_violated_blocker(&blockers, &[(n, d)], reach),
            None
        );
    }

    #[test]
    fn volume_within_cuts_matches_hand_value() {
        let mesh = mesh_of(C_SHAPE_OBJ);
        assert!((mesh.volume_within(&[]) - 6000.0).abs() < 1e-9);
        let plane = |n: DVec3, m: f64| (n, m);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6 * b.max(1.0);

        // x <= 5: the left slab, 5 x 20 x 20, no notch in it.
        assert!(close(mesh.volume_within(&[plane(DVec3::X, 5.0)]), 2000.0));
        // x <= 15: the full left half (10 x 20 x 20) plus the two arms' 5 mm: 2 x (5 x 5 x 20).
        let expect = 5000.0;
        assert!(close(mesh.volume_within(&[plane(DVec3::X, 15.0)]), expect));
        // A cut through the notch's floor plane y <= 5: 20 x 5 x 20.
        assert!(close(mesh.volume_within(&[plane(DVec3::Y, 5.0)]), 2000.0));
        // z <= 10: half of everything.
        assert!(close(mesh.volume_within(&[plane(DVec3::Z, 10.0)]), 3000.0));
        // Along the notch wall x <= 10 (coincident with a face), and beyond everything.
        assert!(close(mesh.volume_within(&[plane(DVec3::X, 10.0)]), 4000.0));
        assert!(close(mesh.volume_within(&[plane(DVec3::X, 25.0)]), 6000.0));
        // Two cuts: x <= 15 and y <= 10 keep the lower half's arm: x <= 10 part is
        // 10 x 10 x 20 = 2000, and x in 10..15 with y <= 5 is 5 x 5 x 20 = 500.
        let two = [plane(DVec3::X, 15.0), plane(DVec3::Y, 10.0)];
        assert!(close(mesh.volume_within(&two), 2500.0));
        // A slanted cut x + y <= 10: the triangle below it, 50 x 20 (the notch starts at
        // x > 10 so it does not interfere).
        let slant = DVec3::new(1.0, 1.0, 0.0) / 2.0_f64.sqrt();
        assert!(close(
            mesh.volume_within(&[plane(slant, 10.0 / 2.0_f64.sqrt())]),
            1000.0
        ));
        // A cut that leaves nothing.
        assert_eq!(mesh.volume_within(&[plane(DVec3::X, -1.0)]), 0.0);
    }

    #[test]
    fn the_clipped_surface_encloses_the_same_volume() {
        let mesh = mesh_of(C_SHAPE_OBJ);
        let cuts = [
            (DVec3::X, 15.0),
            (
                DVec3::new(0.0, 1.0, 1.0) / 2.0_f64.sqrt(),
                20.0 / 2.0_f64.sqrt(),
            ),
        ];
        let surface = mesh.clipped_surface(&cuts);
        assert_eq!(surface.caps.len(), 2);
        let tetra = |tri: &[DVec3; 3]| tri[0].dot(tri[1].cross(tri[2])) / 6.0;
        let from_surface: f64 = surface
            .triangles
            .iter()
            .chain(surface.caps.iter().flat_map(|cap| &cap.triangles))
            .map(tetra)
            .sum();
        let exact = mesh.volume_within(&cuts);
        assert!(
            (from_surface - exact).abs() < 1e-6 * exact,
            "{from_surface} vs {exact}"
        );
        // With no cuts the surface is the mesh.
        let whole = mesh.clipped_surface(&[]);
        assert_eq!(whole.triangles.len(), mesh.triangles().len());
        assert!(whole.caps.is_empty());
    }

    #[test]
    fn pruned_volumes_are_bit_identical_to_the_full_scan() {
        use crate::rough_plan::shape::mesh_fixture::noisy_c_shape;
        let (points, tris) = noisy_c_shape(3, 0.2);
        let mesh = RoughMesh::new(&points, &tris).expect("a closed mesh");
        let slant = DVec3::new(1.0, 2.0, 0.5).normalize();
        let boxed = |lo: [f64; 3], hi: [f64; 3]| {
            let mut cuts = Vec::new();
            for axis in 0..3 {
                let unit = [DVec3::X, DVec3::Y, DVec3::Z][axis];
                cuts.push((unit, hi[axis]));
                cuts.push((-unit, -lo[axis]));
            }
            cuts
        };
        let cases = [
            boxed([0.0; 3], [20.0; 3]),
            boxed([2.0, 3.0, 1.0], [9.0, 14.0, 17.0]),
            boxed([8.0, 4.0, 0.0], [18.0, 16.0, 20.0]),
            boxed([12.0, 6.0, 1.0], [16.0, 10.0, 4.0]),
            boxed([-5.0, -5.0, -5.0], [30.0, 30.0, 30.0]),
            vec![(DVec3::X, 15.0), (slant, 14.0)],
            vec![(DVec3::X, 10.0)],
            vec![(DVec3::new(0.0, 1.0, 0.0), 5.0)],
        ];
        for cuts in &cases {
            let pruned = mesh.volume_within_pruned(cuts, true);
            let full = mesh.volume_within_pruned(cuts, false);
            assert_eq!(
                pruned.to_bits(),
                full.to_bits(),
                "{cuts:?}: {pruned} {full}"
            );
        }
    }

    #[test]
    fn a_large_cap_outline_is_triangulated_quickly() {
        // A 3000-pointed star: half its vertices are reflex, so a cubic ear clip would
        // never finish.
        let count = 3000;
        let ring: Vec<DVec3> = (0..count)
            .map(|i| {
                let angle = std::f64::consts::TAU * f64::from(i) / f64::from(count);
                let radius = if i % 2 == 0 { 10.0 } else { 8.0 };
                DVec3::new(radius * angle.cos(), radius * angle.sin(), 0.0)
            })
            .collect();
        let triangles = triangulate(std::slice::from_ref(&ring), DVec3::Z, 0.0);
        assert_eq!(triangles.len(), count as usize - 2);
        let area: f64 = triangles
            .iter()
            .map(|t| (t[1] - t[0]).cross(t[2] - t[0]).z * 0.5)
            .sum();
        let outline = DVec3::Z.dot(area_vector(&ring));
        assert!(
            (area - outline).abs() < 1e-6 * outline,
            "{area} vs {outline}"
        );
    }

    #[test]
    fn moving_a_mesh_that_collapses_gives_none() {
        let mesh = mesh_of(C_SHAPE_OBJ);
        assert!(mesh.translated(DVec3::splat(f64::NAN)).is_none());
        assert!(mesh.scaled(0.0).is_none() && mesh.scaled(f64::NAN).is_none());
        let moved = mesh.translated(DVec3::new(1.0, 2.0, 3.0)).expect("moves");
        assert!((moved.volume() - 6000.0).abs() < 1e-9);
        let doubled = mesh.scaled(2.0).expect("scales");
        assert!((doubled.volume() - 48000.0).abs() < 1e-6);
    }
}
