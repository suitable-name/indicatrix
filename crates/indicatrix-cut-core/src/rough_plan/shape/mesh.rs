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
//! The module is split by job: `build` turns raw points and triangles into a checked
//! solid, `geometry` holds the BVH and the ray, box and triangle predicates, `volume` the
//! material inside cut planes, and `triangulate` the display of a cut face.
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
//! # Hollow roughs
//!
//! A rough with a cavity is the outer surface plus one more closed surface for the
//! cavity. [`RoughMesh::new`] groups the triangles into closed shells (sets that share
//! edges) and turns each by its nesting: a body's normals point out of the material, a
//! cavity's point into the void, whichever way the file wound them. The volume is then the
//! bodies less the cavities.
//!
//! # Small defects and self-intersection
//!
//! [`RoughMesh::new`] checks that the mesh is a closed, consistently oriented manifold. A
//! mesh that is not gets one chance (the `repair` module): a coarser weld for hair-wide gaps,
//! turning the faces that run the wrong way, and filling a small hole. What was done is in
//! [`RoughMesh::repair_note`]. A closed, consistently wound mesh is never touched.
//!
//! A self-intersecting scan has no well-defined inside; the parity test would answer wrongly
//! near the crossing. [`RoughMesh::new`] therefore looks for triangles that cross (the
//! `intersect` module) and returns [`MeshError::SelfIntersecting`], naming where, so the
//! import falls back to the convex outline. Shells that cross one another are the same
//! defect. Surfaces that only touch (a corner, or a vertex resting on a face) are not
//! crossing, and two surfaces that overlap in one plane are.
//!
//! # Inclusions
//!
//! An inclusion (a crack, a feather, a crystal of another mineral) is material you hold and
//! pay for, where no stone may go. For placement it is a cavity: a second closed shell inside
//! the outer one. [`RoughMesh::with_inclusions`] adds such shells (the `inclusion` module),
//! and the mesh then remembers which shells they are, so the weight and the yield can count
//! them as material ([`RoughMesh::gross_volume`]) while [`RoughMesh::volume`] stays the
//! volume stones can use.

use std::fmt;

use glam::DVec3;

mod build;
mod geometry;
mod inclusion;
mod intersect;
mod ray;
mod repair;
#[cfg(test)]
mod tests;
mod triangulate;
mod volume;

use build::{check_closed, compact, drop_degenerate, orient_shells, signed_volume, weld};
use geometry::{
    bounds_of, build_bvh, clip_halfspace, ray_meets_box, ray_meets_triangle, triangle_meets_box,
};
use intersect::first_self_intersections;
pub use ray::RayHit;
use repair::repair;

/// The most triangles a mesh may have. The planner asks the mesh for every candidate
/// piece, so a very fine scan has to be decimated first.
///
/// Set from the release timing (`tests_mesh_perf::mesh_scaling_timings`, 2026-10-07, 16
/// lanes): importing stays under 2 s up to 330k triangles, but planning grows with the
/// triangle count. A smooth pebble scan at K = 10 projects to 23 s at 82k triangles and 75 s
/// at 328k on 16 lanes; 200k is about 50 s there, inside the default scan time limit on
/// eight cores and more.
pub const MAX_MESH_TRIANGLES: usize = 200_000;

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
/// How many crossing triangle pairs the self-intersection check looks for before it stops.
const INTERSECTION_LIMIT: usize = 16;
/// Triangles per BVH leaf.
const LEAF_SIZE: usize = 4;
/// Blocker planes whose normals are within this angle (5 degrees, as a cosine) of the chosen
/// one are one wall of a scanned surface; see [`RoughMesh::blocker_rows`].
const SAME_WALL_COS: f64 = 0.996_194_698_091_745_5;

/// Centre-satisfied blocker planes within this angle of each other (about 32 degrees) are
/// one contact direction for [`RoughMesh::blocker_rows`], which adds only the least-violated
/// plane of each. A noisy wall's tilted planes stay one direction; the corner contacts of a
/// cube (70 degrees apart and more) stay separate.
const CONTACT_DIRECTION_COS: f64 = 0.85;
/// The most rows [`RoughMesh::blocker_rows`] returns in one round.
const MAX_ROWS_PER_ROUND: usize = 64;
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
    /// The surface passes through itself: `count` pairs of triangles cross (the search stops
    /// at 16), the first near `first_at`, the centre of one of them in hundredths of a
    /// millimetre in the frame of the points given to [`RoughMesh::new`].
    SelfIntersecting {
        /// The crossing pairs found, at most 16.
        count: usize,
        /// Where the first one is, `x`, `y`, `z` in 0.01 mm.
        first_at: [i64; 3],
    },
    /// Inclusion `n` (0-based, in the order given) is not inside the rough's material: it
    /// lies outside the rough, in a hollow, or inside another inclusion.
    InclusionOutside(usize),
    /// The surface of inclusion `n` (0-based, in the order given) crosses the rough's
    /// surface.
    InclusionReachesSurface(usize),
    /// Two inclusions cross each other, or one lies inside the other.
    InclusionsOverlap,
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
            Self::SelfIntersecting { count, first_at } => {
                let [x, y, z] = first_at.map(|v| v as f64 / 100.0);
                let more = if count >= INTERSECTION_LIMIT {
                    "at least "
                } else {
                    ""
                };
                write!(
                    f,
                    "the mesh crosses itself in {more}{count} places, the first near \
                     x {x:.2}, y {y:.2}, z {z:.2} mm"
                )
            }
            Self::InclusionOutside(_) => write!(
                f,
                "the inclusion is not inside the rough's material (it lies outside the rough, \
                 in a hollow, or inside another inclusion)"
            ),
            Self::InclusionReachesSurface(_) => write!(
                f,
                "an inclusion that reaches the surface must be cut away: model it as a notch in \
                 the rough's own mesh"
            ),
            Self::InclusionsOverlap => write!(
                f,
                "the inclusions cross each other, or one lies inside another"
            ),
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
    /// What [`new`](Self::new) repaired, for the user; `None` for a mesh that was closed.
    repair_note: Option<String>,
    /// The index of the first triangle of every inclusion's shells, ascending. The
    /// inclusions come after the rough's own triangles, and their vertices after its
    /// vertices. Empty for a mesh without inclusions.
    inclusion_shells: Vec<u32>,
    /// The volume of the inclusions together, in mm^3 (what [`volume`](Self::volume) lacks of
    /// the material you hold).
    inclusion_volume_mm3: f64,
    /// Every inclusion as a solid of its own, wound outward. Empty without inclusions.
    inclusions: Vec<Self>,
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

impl RoughMesh {
    /// Builds a mesh from `points` and triangles `tris` indexing them.
    ///
    /// Vertices within `1e-7` of the mesh's size of an earlier vertex are welded (in
    /// first-use order, so the result does not depend on anything but the input), triangles
    /// with no area are dropped, and every undirected edge must then belong to exactly two
    /// triangles that run along it in opposite directions.
    ///
    /// The triangles are then grouped into closed shells (sets that share edges) and every
    /// shell is turned to face out of the material: a lone shell is flipped when its signed
    /// volume is negative; with several shells (an outer surface and the surface of a
    /// cavity) a shell inside an even number of the others is a body with outward normals,
    /// one inside an odd number is a cavity with normals into the void, whichever way the
    /// file wound it. The volume is the bodies less the cavities.
    ///
    /// A mesh that is open or inconsistently wound is repaired first when the defect is
    /// small (see the module documentation).
    ///
    /// # Errors
    ///
    /// Returns [`MeshError`] for no faces, too many triangles, a bad vertex index, an open
    /// or non-manifold or inconsistently wound surface that the repair cannot fix, a surface
    /// that crosses itself, or a mesh with no volume.
    pub fn new(points: &[DVec3], tris: &[[u32; 3]]) -> Result<Self, MeshError> {
        Self::new_phased(points, tris, &mut |_| {})
    }

    /// [`new`](Self::new) with the time of each phase, for the owner's timing run: the
    /// weld, compaction and closed check (with any repair), the self-intersection check, and
    /// the orientation plus the BVH. A refused mesh reports the phases it got through.
    #[cfg(test)]
    pub(crate) fn new_timed(
        points: &[DVec3],
        tris: &[[u32; 3]],
    ) -> (
        Result<Self, MeshError>,
        Vec<(&'static str, std::time::Duration)>,
    ) {
        let mut phases = Vec::new();
        let mut last = std::time::Instant::now();
        let built = Self::new_phased(points, tris, &mut |name| {
            let now = std::time::Instant::now();
            phases.push((name, now - last));
            last = now;
        });
        (built, phases)
    }

    /// The body of [`new`](Self::new); `phase` is called with a phase's name as it ends.
    fn new_phased(
        points: &[DVec3],
        tris: &[[u32; 3]],
        phase: &mut dyn FnMut(&'static str),
    ) -> Result<Self, MeshError> {
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
        drop_degenerate(&verts, &mut welded, scale);
        if welded.is_empty() {
            return Err(MeshError::Degenerate);
        }
        let (verts, tris) = compact(&verts, &welded);
        // A mesh that is closed and consistently wound is never touched by the repair.
        let (verts, mut tris, repair_note) = match check_closed(&tris) {
            Ok(()) => (verts, tris, None),
            Err(error) => {
                let fixed = repair(&verts, &tris, scale, error).ok_or(error)?;
                (fixed.verts, fixed.tris, Some(fixed.note))
            }
        };
        phase("weld + closed check");
        let crossings = first_self_intersections(&verts, &tris, scale, INTERSECTION_LIMIT);
        phase("self-intersection check");
        if let Some(&(first, _)) = crossings.first() {
            let [a, b, c] = tris[first as usize].map(|v| verts[v as usize]);
            let at = (a + b + c) / 3.0;
            return Err(MeshError::SelfIntersecting {
                count: crossings.len(),
                first_at: at.to_array().map(|x| (x * 100.0).round() as i64),
            });
        }
        orient_shells(&verts, &mut tris, lo);
        let mut mesh = Self::from_parts(verts, tris).ok_or(MeshError::Degenerate)?;
        mesh.repair_note = repair_note;
        phase("orientation + BVH");
        Ok(mesh)
    }

    /// What the import repaired to make the mesh closed, as one sentence for the user, or
    /// `None` when it was closed to begin with. Only the mesh [`new`](Self::new) built has
    /// it: a mesh read back from a saved plan is already repaired and says nothing.
    #[must_use]
    pub fn repair_note(&self) -> Option<&str> {
        self.repair_note.as_deref()
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
            repair_note: None,
            inclusion_shells: Vec::new(),
            inclusion_volume_mm3: 0.0,
            inclusions: Vec::new(),
        })
    }

    /// The mesh moved by `by`; `None` when the moved mesh would enclose no volume (the
    /// caller must not pair moved planes with an unmoved mesh). The repair note and the
    /// inclusions come along.
    #[must_use]
    pub fn translated(&self, by: DVec3) -> Option<Self> {
        let verts = self.verts.iter().map(|&v| v + by).collect();
        let mut moved = Self::from_parts(verts, self.tris.clone())?;
        moved.repair_note.clone_from(&self.repair_note);
        self.carry_inclusions(moved, |body| body.translated(by))
    }

    /// The mesh with every length multiplied by `factor`; `None` when `factor` is not a
    /// usable scale (finite and positive) or the scaled mesh would enclose no volume.
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Option<Self> {
        if !(factor.is_finite() && factor > 0.0) {
            return None;
        }
        let verts = self.verts.iter().map(|&v| v * factor).collect();
        let scaled = Self::from_parts(verts, self.tris.clone())?;
        self.carry_inclusions(scaled, |body| body.scaled(factor))
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

    /// Removes from `blockers` the triangles whose plane the stone already clears by the
    /// inset: `violation(n, d) <= eps` says the stone's support along `n`, plus the inset,
    /// stays on the material side of the plane `n . p = d`.
    ///
    /// The blocker queries grow the stone by the inset in every axis (a cube, or the planes
    /// moved outward), which reaches up to `sqrt(3)` times the inset along an oblique normal,
    /// while a row keeps the stone the inset clear of the plane. A stone resting on its rows
    /// at a corner of a smooth cap would then be blocked for ever by a triangle whose row it
    /// satisfies. Clear of the triangle's plane by the inset is clear of the triangle by the
    /// inset, which is the clearance the outline planes promise, so such a triangle is no
    /// blocker.
    pub(crate) fn drop_cleared(
        &self,
        blockers: &mut Vec<u32>,
        violation: impl Fn(DVec3, f64) -> f64,
    ) {
        let eps = self.eps();
        blockers.retain(|&t| {
            let (n, d) = self.planes[t as usize];
            violation(n, d) > eps
        });
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
    /// [`least_violated_blocker`](Self::least_violated_blocker) chooses, then every other
    /// unknown blocker plane that faces the same way within 5 degrees of the chosen one,
    /// then, for each other contact direction (within about 32 degrees), the least-violated unknown blocker plane that
    /// the stone's centre is on the material side of (`centre_inside(n, d)`: the centre
    /// satisfies `n . c <= d - inset`). At most 64 rows in all, the chosen one first, the
    /// same-wall ones in triangle order, then one per direction in order of first sight.
    ///
    /// A plane the centre satisfies cannot make the LP infeasible together with the others
    /// the centre satisfies (the centre at scale 0 is feasible for all of them), so a
    /// smooth cap's contact directions can be added in one round. Only one plane per
    /// direction: a noisy wall the centre sits on gives dozens of tilted planes, and
    /// taking all of them carves the block far inside the wall. A facing wall of a notch
    /// has the centre on its air side, so it is never added except as the least-violated
    /// choice, which is the one-wall-per-round behaviour that keeps notches feasible. A
    /// noisy scanned wall is a thousand slightly different planes; the 5 degree rule adds
    /// the ones the centre does not satisfy yet. Empty when every blocker's plane is known.
    pub(crate) fn blocker_rows(
        &self,
        blockers: &[u32],
        known: &[(DVec3, f64)],
        centre_inside: impl Fn(DVec3, f64) -> bool,
        violation: impl Fn(DVec3, f64) -> f64,
    ) -> Vec<(DVec3, f64)> {
        let Some((n0, d0)) = self.least_violated_blocker(blockers, known, &violation) else {
            return Vec::new();
        };
        let tolerance = 1e-9 * (self.hi - self.lo).max_element();
        let same = |(an, ad): (DVec3, f64), (bn, bd): (DVec3, f64)| {
            an.dot(bn) > 1.0 - 1e-12 && (ad - bd).abs() <= tolerance
        };
        let mut rows = vec![(n0, d0)];
        // One (plane, violation) per further wall direction, least violated wins.
        let mut directions: Vec<((DVec3, f64), f64)> = Vec::new();
        for &t in blockers {
            let plane = self.planes[t as usize];
            if known.iter().any(|&k| same(k, plane)) || rows.iter().any(|&r| same(r, plane)) {
                continue;
            }
            if plane.0.dot(n0) >= SAME_WALL_COS {
                if rows.len() < MAX_ROWS_PER_ROUND {
                    rows.push(plane);
                }
                continue;
            }
            if !centre_inside(plane.0, plane.1) {
                continue;
            }
            let cost = violation(plane.0, plane.1);
            match directions
                .iter_mut()
                .find(|(p, _)| p.0.dot(plane.0) >= CONTACT_DIRECTION_COS)
            {
                Some(slot) if cost < slot.1 => *slot = (plane, cost),
                Some(_) => {}
                None => directions.push((plane, cost)),
            }
        }
        for (plane, _) in directions {
            if rows.len() >= MAX_ROWS_PER_ROUND {
                break;
            }
            rows.push(plane);
        }
        rows
    }
}
