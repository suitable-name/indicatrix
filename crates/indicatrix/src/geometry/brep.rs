//! Boundary representation ([`GemPolyhedron`]) of the convex solid bounded by a
//! list of facet half-space planes.
//!
//! # Algorithm
//!
//! [`GemPolyhedron::from_planes`] works entirely in the primal plane arrangement,
//! on the same deterministic walk [`super::stone_metrics`] measures solids with:
//!
//! 1. **Validate** every plane: finite normal and offset
//!    ([`BrepError::NonFinitePlane`]), a unit normal (within `1e-4`,
//!    [`BrepError::NonUnitNormal`]), and `d < 0` so the origin lies strictly
//!    inside every half-space ([`BrepError::NonNegativeOffset`]).
//! 2. **Normalise** every offset by the power of two nearest the largest `|d|`
//!    (the same `pow2_scale_norm` `stone_metrics` uses), so every tolerance below
//!    is relative to the design's own size. Output positions are multiplied back
//!    by that exact power of two, which is a bit-exact operation.
//! 3. **Reject coincident planes** with a purely relative test on the polar dual
//!    points `n / -d` ([`BrepError::CoincidentPlanes`]).
//! 4. **Enumerate** every plane triple `a < b < c` whose intersection satisfies
//!    every half-space within `stone_metrics`' feasibility slack (`EPS_FEAS`,
//!    `1e-5` of the normalised scale), in lexicographic order. A feasible,
//!    well-conditioned vertex that reaches `stone_metrics`' blank box (64 times
//!    the largest offset) means the planes do not close a solid inside it:
//!    [`BrepError::UnboundedRegion`], naming the smallest escaping plane. Of the
//!    feasible triples, only those that are vertices of the exact polytope (within
//!    `1e-9`, far tighter than the slack; see `INCIDENCE_EPS`) are kept.
//! 5. **Weld** the triple solutions into vertices: union-find over every pair
//!    closer than `VERTEX_WELD_EPS_REL` (`1e-4`) times the largest solution radius.
//!    Connected components do not depend on visiting order. A vertex's incident
//!    planes are the sorted union of its triples; its position is the solution of
//!    its best-conditioned triple (largest `|det|`, ties to the lexicographically
//!    smallest triple), which must reach `MIN_TRIPLE_DET`
//!    ([`BrepError::IllConditionedTriple`]). Vertices are ordered by incident set.
//! 6. **Rings**: each plane's facet polygon is the vertices incident to it, sorted
//!    by angle about their centroid (counter-clockwise seen from outside,
//!    `total_cmp`, ties by vertex index) and rotated so the smallest vertex index
//!    comes first. Triangles are fans from that first vertex.
//! 7. **Validate** the result: every vertex satisfies every plane within twice the
//!    weld radius ([`BrepError::InfeasibleVertex`]), every edge is shared by
//!    exactly two facets (the smallest offending edge is reported,
//!    [`BrepError::NonManifoldEdge`]), Euler's formula holds, and the volume is at
//!    least `1e-9 * bounding_radius^3` ([`BrepError::DegenerateVolume`]).
//!
//! # Determinism
//!
//! The output is a pure function of the input planes. There is no hashing and no
//! randomised iteration (`BTreeMap` and sorted `Vec`s only); all geometry is `f64`
//! with `total_cmp` ordering; and the triple solves go through `crate::simd`'s
//! batch kernels, which are bit-identical to the scalar `glam` sequence at every
//! dispatch level. Two calls with the same planes produce byte-identical vertices,
//! polygons, triangles, volume and areas, within one process or across processes.
//!
//! # Why `chull` was removed (2026-09-28)
//!
//! This module used to build the convex hull of the dual points `n / -d` with the
//! `chull` crate (0.2.4). `chull` iterates randomly seeded `HashSet`s while it builds
//! the hull, so its facet order and its triangulation of coplanar dual faces changed
//! on every call: vertex order, the positions of vertices where more than three
//! planes meet (up to ~3e-6 apart), polygons, triangles, volume and areas all varied
//! between calls and between runs. It also rejected valid solids whose origin sat
//! close to one facet (its degeneracy thresholds are absolute), and for dense inputs
//! (about 500 planes or more) it returned non-convex hulls and flipped between `Ok`
//! and `Err`. The primal enumeration here has none of these failure modes. Its cost
//! is `O(P^3)` in the plane count in the worst case, cut down by a conservative
//! pair prune in the walk (release build: about 0.7 ms for the round brilliant,
//! 7 ms for the 205-plane crackotto fixture, 0.7 s for a 1000-plane sphere). That is
//! acceptable because nothing calls `from_planes` on a hot path: the tracer
//! intersects raw planes and the CAD preview uses `stone_metrics::build_solid_mesh`.

mod helpers;

use glam::{DVec3, Vec3};
use helpers::{
    check_conditioning, check_euler, check_feasible, facet_rings, mesh_volume, output_vertices,
    triangulate_polygons, weld_candidates,
};
use std::fmt;

use super::{
    plane::GpuFacetPlane,
    stone_metrics::{
        MIN_TRIPLE_DET, escaping_plane_indices, for_each_feasible_triple, max_abs_offset,
        pow2_scale_norm,
    },
};

/// Everything that can go wrong reconstructing a [`GemPolyhedron`] from half-space
/// planes in [`GemPolyhedron::from_planes`].
///
/// Every payload is deterministic: when several planes, vertices or edges fail the
/// same check, the one reported is the first in input (or vertex) order.
#[derive(Debug, Clone, PartialEq)]
pub enum BrepError {
    /// Fewer than 4 planes were supplied -- the minimum to bound a finite solid.
    TooFewPlanes {
        /// Number of planes supplied.
        count: usize,
    },
    /// A plane's normal or offset is NaN or infinite. `GpuFacetPlane`'s fields are
    /// public (and serde-deserialisable), so planes built without
    /// [`GpuFacetPlane::new`] are checked here rather than trusted.
    NonFinitePlane {
        /// Index of the first non-finite plane in the input.
        index: usize,
    },
    /// A plane's normal is not unit length (within `1e-4`). Every tolerance in the
    /// reconstruction, including the determinant threshold, assumes unit normals.
    NonUnitNormal {
        /// Index of the first such plane in the input.
        index: usize,
        /// The normal's length.
        length: f64,
    },
    /// A plane's offset `d` was non-negative (every facet plane must contain the
    /// origin strictly inside its half-space `n . x + d <= 0`).
    NonNegativeOffset {
        /// Index of the first such plane in the input.
        index: usize,
        /// The offending offset.
        d: f32,
    },
    /// Two input planes are coincident: their polar dual points `n / -d` agree to
    /// within a relative `1e-7`.
    CoincidentPlanes {
        /// Index of the earlier plane.
        i: usize,
        /// Index of the later plane.
        j: usize,
    },
    /// The planes do not close a solid: a feasible, well-conditioned vertex of the
    /// arrangement reaches the blank box of half-extent 64 times the largest plane
    /// offset. A missing closing plane (an infinite prism) is the usual cause; a
    /// bounded but extremely elongated solid is reported the same way.
    UnboundedRegion {
        /// The smallest input plane index taking part in an escaping vertex. `None`
        /// only if every escaping vertex is formed by blank-box planes alone, which
        /// no input has been observed to produce.
        plane: Option<usize>,
    },
    /// A vertex's best-conditioned plane triple is still too close to singular
    /// (`|det| < 1e-6` for unit normals) for its position to be trusted.
    IllConditionedTriple {
        /// Smallest plane index of the triple.
        a: usize,
        /// Middle plane index of the triple.
        b: usize,
        /// Largest plane index of the triple.
        c: usize,
        /// The triple's (signed) normal-matrix determinant.
        det: f64,
    },
    /// A reconstructed vertex lies outside some input plane by more than twice the
    /// weld radius. Internal consistency gate: every vertex position is a feasible
    /// triple solution, so this is not expected to fire.
    InfeasibleVertex {
        /// Index of the vertex (in the reconstructed vertex order).
        vertex: usize,
        /// Index of the violated input plane.
        plane: usize,
        /// How far outside the plane the vertex lies, in input units.
        excess: f64,
    },
    /// A vertex position is not finite once scaled back to input units and
    /// narrowed to `f32` (a solid beyond `f32` range).
    NonFiniteVertex {
        /// Smallest plane index of the vertex's position triple.
        a: usize,
        /// Middle plane index of the vertex's position triple.
        b: usize,
        /// Largest plane index of the vertex's position triple.
        c: usize,
        /// The vertex in input units, before narrowing.
        vertex: DVec3,
    },
    /// The reconstructed mesh is non-manifold: some edge is not shared by exactly two
    /// facets. The smallest such edge is reported.
    NonManifoldEdge {
        /// The edge as `(smaller vertex index, larger vertex index)`.
        edge: (u32, u32),
        /// How many facets use it.
        count: u32,
    },
    /// The reconstructed polyhedron fails Euler's formula (`V - E + F = 2`).
    EulerFormulaFailed {
        /// Vertex count `V`.
        vertices: usize,
        /// Edge count `E`.
        edges: usize,
        /// Facet count `F` (facets with at least 3 vertices).
        faces: usize,
        /// `V - E + F`.
        euler: i64,
    },
    /// The reconstructed solid has fewer than 4 vertices, a non-finite volume, or a
    /// volume below `1e-9 * bounding_radius^3`.
    DegenerateVolume {
        /// The volume in input units (`0.0` when there were too few vertices).
        volume: f64,
    },
}

impl fmt::Display for BrepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewPlanes { count } => write!(
                f,
                "at least 4 half-space planes are required to bound a finite 3D polyhedron (a tetrahedron is the minimum); got {count}"
            ),
            Self::NonFinitePlane { index } => {
                write!(f, "plane {index} has a NaN or infinite normal or offset")
            }
            Self::NonUnitNormal { index, length } => write!(
                f,
                "plane {index} has a normal of length {length} (a unit normal is required)"
            ),
            Self::NonNegativeOffset { index, d } => write!(
                f,
                "Plane {index} offset d ({d}) must be negative to contain origin"
            ),
            Self::CoincidentPlanes { i, j } => write!(
                f,
                "planes {i} and {j} are coincident (identical half-spaces); a bounded polyhedron cannot use two duplicate faces"
            ),
            Self::UnboundedRegion { plane } => write!(
                f,
                "planes do not bound a finite region: the solid reaches the blank box (smallest escaping plane {plane:?}); the half-space schedule is unbounded"
            ),
            Self::IllConditionedTriple { a, b, c, det } => write!(
                f,
                "planes {a}, {b}, {c} meet at a near-parallel triple (|det| = {:.3e} < {MIN_TRIPLE_DET:e}); the 3x3 facet-intersection solve is ill-conditioned",
                det.abs()
            ),
            Self::InfeasibleVertex {
                vertex,
                plane,
                excess,
            } => write!(
                f,
                "reconstructed vertex {vertex} lies {excess:.3e} outside plane {plane}"
            ),
            Self::NonFiniteVertex { a, b, c, vertex } => write!(
                f,
                "planes {a}, {b}, {c} produced a vertex outside f32 range ({vertex:?})"
            ),
            Self::NonManifoldEdge { edge, count } => write!(
                f,
                "reconstructed mesh is non-manifold: edge {edge:?} is shared by {count} facets (expected exactly 2)"
            ),
            Self::EulerFormulaFailed {
                vertices,
                edges,
                faces,
                euler,
            } => write!(
                f,
                "reconstructed polyhedron fails Euler's formula (V - E + F = 2): V={vertices}, E={edges}, F={faces}, V-E+F={euler}"
            ),
            Self::DegenerateVolume { volume } => write!(
                f,
                "reconstructed polyhedron has non-finite or ~zero volume ({volume}); the half-space schedule does not bound a proper 3D solid"
            ),
        }
    }
}

impl std::error::Error for BrepError {}

/// How far a plane normal's length may be from 1 before
/// [`BrepError::NonUnitNormal`]. `GpuFacetPlane::new` normalises in `f32`, which
/// leaves errors around `1e-7`; this only rejects normals that were never
/// normalised at all.
const UNIT_NORMAL_TOLERANCE: f64 = 1e-4;

/// Relative distance below which two planes' polar dual points `q = n / m` count as
/// the same half-space: `|q_i - q_j| < COINCIDENT_PLANE_EPS * max(|q_i|, |q_j|)`.
/// Purely relative, with no absolute floor, so the verdict does not depend on the
/// design's scale (the old `max(1.0)` floor called opposite faces of a
/// cube at scale `1e12` coincident).
///
/// A parallel plane farther out than this is well above [`INCIDENCE_EPS`], so it
/// never touches a vertex and is reported by [`GemPolyhedron::untouched_planes`].
const COINCIDENT_PLANE_EPS: f64 = 1e-7;

/// A feasible triple counts as a vertex of the exact polytope (and so contributes
/// its planes to a vertex's incident set) only if its point violates no plane by
/// more than this, in the normalised frame, plus its own solve-residual bound
/// (`SOLVE_RESIDUAL_BOUND / |det|`, see [`SOLVE_RESIDUAL_BOUND`]).
///
/// Much tighter than the arrangement walk's `1e-5` feasibility slack on purpose.
/// That slack is harmless for `stone_metrics`, which only needs positions, but as an
/// *incidence* test it smears: two nearly parallel planes (angle `a`) both lie within
/// `1e-5` of each other along a band `1e-5 / a` wide, so a vertex anywhere in that
/// band "touches" both. On the Shah replica fixture the crown pair 6/7 (0.0064 rad
/// apart) claimed a vertex 1.1e-3 away on plane 7's side, putting it into plane 6's
/// ring and breaking manifoldness. Planes stored as `f32` are exact in `f64`, so the
/// true polytope's vertices solve to about `1e-15`; `1e-9` keeps a wide margin over
/// that while staying far below any real facet size. Vertices a design means to be
/// one point but that rounding split apart are all genuine at this tolerance, and
/// the weld ([`VERTEX_WELD_EPS_REL`]) merges them.
const INCIDENCE_EPS: f64 = 1e-9;

/// Bound on `|residual| * |det|` for one triple solve (about `4e-13` for unit
/// normals and normalised offsets; `stone_metrics`' walk derives it), so an
/// ill-conditioned but genuine vertex is not rejected by [`INCIDENCE_EPS`] for its
/// own rounding.
const SOLVE_RESIDUAL_BOUND: f64 = 1e-12;

/// Smallest `|det|` a plane triple may have and still be solved and considered.
/// Lower than `MIN_TRIPLE_DET` on purpose: a vertex whose *best* triple lies in
/// between is reported as [`BrepError::IllConditionedTriple`] instead of silently
/// dropped (which would leave a hole in the mesh). Below this floor the solve's
/// residual (up to about `4e-13 / |det|`) approaches the `1e-5` feasibility slack,
/// so such triples are skipped exactly like `stone_metrics` skips every triple
/// below `MIN_TRIPLE_DET`. The floor also sets the arrangement walk's pair-prune
/// margin (`1e-11 / floor / sin`), so it cannot go much lower without making
/// the prune ineffective.
const ENUMERATION_DET_FLOOR: f64 = 1e-8;

/// Weld (meet) tolerance: two triple solutions closer than this fraction of the
/// largest solution radius are one vertex.
///
/// This is a *meet* tolerance, deliberately looser than `stone_metrics`' position
/// dedup (`VERTEX_DEDUP`, `1e-6` per axis): facets that a design means to meet at
/// one point but whose offsets were rounded (hand-authored `.asc` masts carry 4-8
/// digits) produce several solutions a few `1e-5` apart, and they must weld into one
/// vertex or the mesh grows sliver facets. The consequence is that a genuine edge
/// shorter than this collapses (the B-rep then has fewer vertices than
/// `stone_metrics` counts: 215 against 236 on the 205-plane crackotto fixture), and
/// a facet whose every vertex welds into one or two points is reported by
/// [`GemPolyhedron::untouched_planes`].
const VERTEX_WELD_EPS_REL: f64 = 1e-4;

/// Feasibility gate: a reconstructed vertex may violate a plane by at most this many
/// weld radii (see [`BrepError::InfeasibleVertex`]).
const FEASIBILITY_GATE_WELDS: f64 = 2.0;

/// Smallest accepted volume, relative to `bounding_radius^3`. Relative, so a cube of
/// half-extent `1e-6` reconstructs exactly like a unit cube.
const VOLUME_FLOOR_REL: f64 = 1e-9;

/// [`GemPolyhedron::girdle_outline`] merges projected points closer than this
/// fraction of the bounding radius (per axis).
const GIRDLE_DEDUP_REL: f64 = 1e-6;

/// A convex polyhedron reconstructed from half-space planes by
/// [`GemPolyhedron::from_planes`].
///
/// Indexing convention: `facet_planes[i]` and `facet_polygons[i]` describe the same
/// input plane; polygon and triangle entries index `vertices`. Every field is a pure,
/// byte-identical function of the input planes (see the module docs).
#[derive(Debug, Clone)]
pub struct GemPolyhedron {
    /// Welded vertices in input units, ordered by their sorted set of incident plane
    /// indices (lexicographically).
    pub vertices: Vec<Vec3>,
    /// The input planes, unchanged and in input order.
    pub facet_planes: Vec<GpuFacetPlane>,
    /// One ordered vertex loop per input plane: counter-clockwise seen from outside
    /// the solid, starting at its smallest vertex index. Empty for a plane that
    /// touches fewer than three vertices (see [`GemPolyhedron::untouched_planes`]).
    pub facet_polygons: Vec<Vec<u32>>,
    /// Flat triangle index buffer: every facet polygon fan-triangulated from its
    /// first vertex, facets in plane order. Outward-facing (counter-clockwise).
    pub triangle_indices: Vec<u32>,
    /// Largest distance of any vertex from the origin, in input units.
    pub bounding_radius: f32,
}

/// One feasible plane-triple intersection from the arrangement walk, in the
/// normalised frame.
struct MeetCandidate {
    /// The triple's plane indices, ascending.
    planes: [usize; 3],
    /// The triple's normal-matrix determinant.
    det: f64,
    /// The intersection point.
    v: DVec3,
}

/// A welded vertex: a connected component of [`MeetCandidate`]s.
struct WeldedVertex {
    /// Sorted, deduplicated union of the member triples' planes.
    incident: Vec<usize>,
    /// Index (into the candidate list) of the best-conditioned member triple, which
    /// supplies the vertex position.
    best: usize,
}

impl GemPolyhedron {
    /// Reconstructs the convex polyhedron bounded by the half-spaces
    /// `n . x + d <= 0` of `planes`.
    ///
    /// See the module docs for the algorithm and the determinism guarantee: the result
    /// is byte-identical for identical input, call to call and run to run.
    ///
    /// # Errors
    ///
    /// Returns an error if fewer than 4 planes are supplied; a plane is non-finite,
    /// has a non-unit normal or a non-negative offset; two planes are coincident; the
    /// planes do not close a solid inside the blank box; a vertex is ill-conditioned
    /// or (in input units) beyond `f32` range; or the reconstruction fails its own
    /// feasibility, manifoldness, Euler or volume checks. See [`BrepError`].
    pub fn from_planes(planes: Vec<GpuFacetPlane>) -> Result<Self, BrepError> {
        if planes.len() < 4 {
            return Err(BrepError::TooFewPlanes {
                count: planes.len(),
            });
        }
        validate_planes(&planes)?;
        let (halfspaces, scale) = normalised_halfspaces(&planes);
        check_coincident(&halfspaces)?;

        let candidates = enumerate_candidates(&halfspaces)?;
        let weld_radius = VERTEX_WELD_EPS_REL * max_length(candidates.iter().map(|c| c.v));
        let welded = weld_candidates(&candidates, weld_radius);
        if welded.len() < 4 {
            return Err(BrepError::DegenerateVolume { volume: 0.0 });
        }
        check_conditioning(&candidates, &welded)?;

        let positions: Vec<DVec3> = welded.iter().map(|w| candidates[w.best].v).collect();
        check_feasible(
            &halfspaces,
            &positions,
            FEASIBILITY_GATE_WELDS * weld_radius,
            scale,
        )?;
        let facet_polygons = facet_rings(&halfspaces, &positions, &welded);
        check_euler(positions.len(), &facet_polygons)?;
        let triangle_indices = triangulate_polygons(&facet_polygons);

        let radius = max_length(positions.iter().copied());
        let volume = mesh_volume(&positions, &triangle_indices);
        if !volume.is_finite() || volume < VOLUME_FLOOR_REL * radius.powi(3) {
            return Err(BrepError::DegenerateVolume {
                volume: volume * scale * scale * scale,
            });
        }
        let vertices = output_vertices(&candidates, &welded, scale)?;

        Ok(Self {
            vertices,
            facet_planes: planes,
            facet_polygons,
            triangle_indices,
            bounding_radius: (radius * scale) as f32,
        })
    }

    /// Indices of input planes that contributed no facet to the reconstructed hull.
    ///
    /// Every input plane should normally be touched by at least one facet; a plane
    /// contributing none means it is redundant with respect to the others -- the
    /// schedule over-constrains the solid. This is real diagnostic information about a
    /// bad cutting instructions, not a hard geometric error (the returned polyhedron is
    /// still perfectly valid), so callers reconstructing from an untrusted schedule
    /// should treat a non-empty result here as a signal to fall back to a known-good
    /// cut rather than trust the reconstruction.
    #[must_use]
    pub fn untouched_planes(&self) -> Vec<usize> {
        self.facet_polygons
            .iter()
            .enumerate()
            .filter(|(_, p)| p.len() < 3)
            .map(|(i, _)| i)
            .collect()
    }

    /// Area of a single facet polygon (indexed the same way as `facet_planes` /
    /// `facet_polygons`), summed in `f64` over the polygon's fan in ring order.
    /// Returns `0.0` for a plane that contributed no facet (see
    /// [`Self::untouched_planes`]).
    ///
    /// # Panics
    ///
    /// Panics if `facet_idx` is out of range.
    #[must_use]
    pub fn facet_area(&self, facet_idx: usize) -> f32 {
        let poly = &self.facet_polygons[facet_idx];
        if poly.len() < 3 {
            return 0.0;
        }
        let at = |i: u32| self.vertices[i as usize].as_dvec3();
        let origin = at(poly[0]);
        let cross_sum = poly.windows(2).skip(1).fold(DVec3::ZERO, |acc, pair| {
            acc + (at(pair[0]) - origin).cross(at(pair[1]) - origin)
        });
        (cross_sum.length() * 0.5) as f32
    }

    /// Areas of every facet, indexed the same way as `facet_planes` / `facet_polygons`.
    #[must_use]
    pub fn facet_areas(&self) -> Vec<f32> {
        (0..self.facet_polygons.len())
            .map(|i| self.facet_area(i))
            .collect()
    }

    /// Volume of the reconstructed solid, via the divergence theorem over the
    /// triangulated mesh, summed in `f64` in triangle order. Always non-negative (a
    /// physical volume is unsigned, so this does not depend on triangle winding).
    ///
    /// # Panics
    ///
    /// Panics only if internal invariants are violated (should not happen for a
    /// `GemPolyhedron` returned by [`Self::from_planes`]).
    #[must_use]
    pub fn volume(&self) -> f32 {
        let positions: Vec<DVec3> = self.vertices.iter().map(|v| v.as_dvec3()).collect();
        mesh_volume(&positions, &self.triangle_indices) as f32
    }

    /// The girdle outline: the polyhedron's silhouette viewed from directly above
    /// (looking down the Y axis, per this crate's Y-up convention), as an ordered loop
    /// of the actual 3D vertices on that silhouette -- the 2D convex hull of the
    /// vertices' X-Z projection, counter-clockwise in `(x, z)`. This is the widest
    /// horizontal cross-section of the stone: for a well-formed faceted gem it
    /// coincides with the girdle facet vertices, making it the natural basis for
    /// comparing a reconstruction against a published cutting diagram (itself drawn
    /// as a top-down outline).
    ///
    /// Projected points closer than `1e-6 * bounding_radius` on both axes count as one
    /// (the lowest-index vertex among them is kept), so the result does not depend on
    /// the design's absolute scale.
    ///
    /// # Panics
    ///
    /// Panics only if internal invariants are violated (should not happen for a
    /// `GemPolyhedron` returned by [`Self::from_planes`]).
    #[must_use]
    pub fn girdle_outline(&self) -> Vec<Vec3> {
        let eps = GIRDLE_DEDUP_REL * f64::from(self.bounding_radius);
        let mut pts: Vec<(f64, f64, usize)> = self
            .vertices
            .iter()
            .enumerate()
            .map(|(idx, v)| (f64::from(v.x), f64::from(v.z), idx))
            .collect();
        pts.sort_by(|p, q| {
            p.0.total_cmp(&q.0)
                .then_with(|| p.1.total_cmp(&q.1))
                .then(p.2.cmp(&q.2))
        });
        pts.dedup_by(|later, kept| {
            (later.0 - kept.0).abs() <= eps && (later.1 - kept.1).abs() <= eps
        });

        if pts.len() < 3 {
            return pts.into_iter().map(|p| self.vertices[p.2]).collect();
        }
        let mut lower = half_hull(pts.iter().copied());
        let mut upper = half_hull(pts.iter().rev().copied());
        lower.pop();
        upper.pop();
        lower.extend(upper);
        lower.into_iter().map(|p| self.vertices[p.2]).collect()
    }
}

/// One half of Andrew's monotone chain over `(x, z, vertex index)` points visited in
/// sorted (or reverse-sorted) order: keeps only strict left turns.
fn half_hull(points: impl Iterator<Item = (f64, f64, usize)>) -> Vec<(f64, f64, usize)> {
    let turn = |o: (f64, f64, usize), a: (f64, f64, usize), b: (f64, f64, usize)| {
        (a.1 - o.1).mul_add(-(b.0 - o.0), (a.0 - o.0) * (b.1 - o.1))
    };
    let mut chain: Vec<(f64, f64, usize)> = Vec::new();
    for p in points {
        while chain.len() >= 2 && turn(chain[chain.len() - 2], chain[chain.len() - 1], p) <= 0.0 {
            chain.pop();
        }
        chain.push(p);
    }
    chain
}

/// Rejects non-finite planes, non-unit normals and non-negative offsets, reporting
/// the first offending plane in input order (each plane is checked for all three
/// before the next plane is looked at).
fn validate_planes(planes: &[GpuFacetPlane]) -> Result<(), BrepError> {
    for (index, p) in planes.iter().enumerate() {
        if !(p.d.is_finite() && p.normal.iter().all(|c| c.is_finite())) {
            return Err(BrepError::NonFinitePlane { index });
        }
        let length = p.to_halfspace_f64().0.length();
        if (length - 1.0).abs() > UNIT_NORMAL_TOLERANCE {
            return Err(BrepError::NonUnitNormal { index, length });
        }
        if p.d >= 0.0 {
            return Err(BrepError::NonNegativeOffset { index, d: p.d });
        }
    }
    Ok(())
}

/// Converts to `n . x <= m` form and divides every offset by the power of two
/// nearest the largest one; returns the normalised half-spaces and that power of two.
fn normalised_halfspaces(planes: &[GpuFacetPlane]) -> (Vec<(DVec3, f64)>, f64) {
    let raw: Vec<(DVec3, f64)> = planes.iter().map(|p| p.to_halfspace_f64()).collect();
    let scale = pow2_scale_norm(max_abs_offset(&raw));
    let halfspaces = raw.iter().map(|&(n, m)| (n, m / scale)).collect();
    (halfspaces, scale)
}

/// Reports the lexicographically first coincident pair `(i, j)`; see
/// [`COINCIDENT_PLANE_EPS`].
fn check_coincident(halfspaces: &[(DVec3, f64)]) -> Result<(), BrepError> {
    let duals: Vec<DVec3> = halfspaces.iter().map(|&(n, m)| n / m).collect();
    for (i, &qi) in duals.iter().enumerate() {
        for (j, &qj) in duals.iter().enumerate().skip(i + 1) {
            if (qi - qj).length() < COINCIDENT_PLANE_EPS * qi.length().max(qj.length()) {
                return Err(BrepError::CoincidentPlanes { i, j });
            }
        }
    }
    Ok(())
}

/// Every plane triple of the normalised arrangement that is a vertex of the exact
/// polytope (see [`INCIDENCE_EPS`]), in lexicographic order, or
/// [`BrepError::UnboundedRegion`] if the arrangement escapes to the blank.
fn enumerate_candidates(halfspaces: &[(DVec3, f64)]) -> Result<Vec<MeetCandidate>, BrepError> {
    let real = halfspaces.len();
    let mut candidates = Vec::new();
    let closed =
        for_each_feasible_triple(halfspaces, ENUMERATION_DET_FLOOR, true, |planes, det, v| {
            // A triple using a blank-box plane is never a vertex of a closed solid (the
            // walk only lets ill-conditioned ones through, and drops those at the blank).
            let tolerance = INCIDENCE_EPS + SOLVE_RESIDUAL_BOUND / det.abs();
            if planes[2] < real && halfspaces.iter().all(|&(n, m)| n.dot(v) - m <= tolerance) {
                candidates.push(MeetCandidate { planes, det, v });
            }
        });
    if closed.is_none() {
        return Err(BrepError::UnboundedRegion {
            plane: escaping_plane_indices(halfspaces).first().copied(),
        });
    }
    Ok(candidates)
}

/// Largest length among `points` (`0.0` for none).
fn max_length(points: impl Iterator<Item = DVec3>) -> f64 {
    points.map(DVec3::length).fold(0.0, f64::max)
}

#[cfg(test)]
mod tests;
