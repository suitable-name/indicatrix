use chull::ConvexHull;
use glam::{DMat3, DVec3, Vec3};
use std::{collections::HashMap, fmt};

use super::plane::GpuFacetPlane;

/// Everything that can go wrong reconstructing a [`GemPolyhedron`] from half-space
/// planes in [`GemPolyhedron::from_planes`].
///
/// Every variant's [`Display`](fmt::Display) reproduces, verbatim, the human-readable
/// message this module used to return as a plain `String`.
#[derive(Debug, Clone, PartialEq)]
pub enum BrepError {
    /// Fewer than 4 planes were supplied -- the minimum to bound a finite solid.
    TooFewPlanes { count: usize },
    /// A plane's offset `d` was non-negative (every facet plane must face inward,
    /// containing the origin, for the dual-space construction to be valid).
    NonNegativeOffset { index: usize, d: f32 },
    /// Two input planes are coincident (identical half-spaces after polar inversion).
    CoincidentPlanes { i: usize, j: usize },
    /// The dual-space convex hull computation itself (via the `chull` crate) failed.
    HullComputationFailed { message: String },
    /// The dual hull's index buffer was not a multiple of 3 (expected all-triangle
    /// facets).
    NonTriangularHullFacets { indices_len: usize },
    /// A dual hull vertex could not be matched to any input plane at all (only
    /// possible when there are no input planes to match against).
    NoPlanesToMatch,
    /// A dual hull vertex did not match any input plane within tolerance.
    HullVertexUnmatched { distance: f64, tolerance: f64 },
    /// The origin is not strictly inside the dual convex hull, i.e. the half-space
    /// schedule does not bound a finite region.
    UnboundedRegion { signed_distance: f64 },
    /// Three planes meeting at a dual hull facet are near-parallel enough that the
    /// 3x3 vertex solve is ill-conditioned.
    IllConditionedTriple {
        a: usize,
        b: usize,
        c: usize,
        det: f64,
    },
    /// A well-conditioned 3x3 solve nonetheless produced a non-finite vertex.
    NonFiniteVertex {
        a: usize,
        b: usize,
        c: usize,
        vertex: DVec3,
    },
    /// The reconstructed mesh is non-manifold: some edge is not shared by exactly two
    /// facets.
    NonManifoldEdge { edge: (u32, u32), count: u32 },
    /// The reconstructed polyhedron fails Euler's formula (`V - E + F = 2`).
    EulerFormulaFailed {
        vertices: usize,
        edges: usize,
        faces: usize,
        euler: i64,
    },
    /// The reconstructed solid has non-finite or ~zero volume.
    DegenerateVolume { volume: f32 },
}

impl fmt::Display for BrepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewPlanes { count } => write!(
                f,
                "at least 4 half-space planes are required to bound a finite 3D polyhedron (a tetrahedron is the minimum); got {count}"
            ),
            Self::NonNegativeOffset { index, d } => write!(
                f,
                "Plane {index} offset d ({d}) must be negative to contain origin"
            ),
            Self::CoincidentPlanes { i, j } => write!(
                f,
                "planes {i} and {j} are coincident (identical half-spaces after polar inversion); a bounded polyhedron cannot use two duplicate faces"
            ),
            Self::HullComputationFailed { message } => {
                write!(f, "Dual convex hull computation failed: {message}")
            }
            Self::NonTriangularHullFacets { indices_len } => write!(
                f,
                "internal error: dual hull returned {indices_len} indices, not a multiple of 3 (expected all-triangle facets)"
            ),
            Self::NoPlanesToMatch => write!(
                f,
                "internal error: a dual hull vertex did not match any input plane (no input planes to match against)"
            ),
            Self::HullVertexUnmatched {
                distance,
                tolerance,
            } => write!(
                f,
                "internal error: a dual hull vertex did not match any input plane within tolerance (nearest distance {distance:.3e}, tolerance {tolerance:.3e})"
            ),
            Self::UnboundedRegion { signed_distance } => write!(
                f,
                "planes do not bound a finite region: the origin is not strictly inside the dual convex hull (signed facet distance {signed_distance:.3e}); the half-space schedule is unbounded"
            ),
            Self::IllConditionedTriple { a, b, c, det } => write!(
                f,
                "planes {a}, {b}, {c} meet at a near-parallel triple (|det| = {det:.3e} < {MIN_TRIPLE_DETERMINANT:e}); the 3x3 facet-intersection solve is ill-conditioned"
            ),
            Self::NonFiniteVertex { a, b, c, vertex } => write!(
                f,
                "planes {a}, {b}, {c} produced a non-finite vertex despite a well-conditioned solve ({vertex:?})"
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

/// Minimum `|determinant|` of a facet-normal triple's 3x3 system for the vertex solve
/// to be considered numerically trustworthy. Facet normals are unit vectors, so
/// `|det|` ranges from 0 (coplanar/parallel normals) to 1 (mutually orthogonal); below
/// this threshold the three planes meet at a poorly-determined point and we refuse to
/// fabricate a vertex from it. The determinant itself is now computed in `f64` (see
/// [`reconstruct_vertices`]), but the threshold stays a fine absolute value because
/// facet normals are always unit-length, independent of the polyhedron's physical
/// scale.
const MIN_TRIPLE_DETERMINANT: f64 = 1e-6;

/// Relative tolerance (in dual space) for treating two input planes as the same
/// half-space, i.e. their dual points coincide.
const COINCIDENT_PLANE_EPS: f64 = 1e-7;

/// Distance-from-origin tolerance (in dual space) used by the primal boundedness
/// check: the origin must be strictly this far inside every dual hull facet.
const ORIGIN_INTERIOR_EPS: f64 = 1e-9;

/// Relative tolerance (in dual space) for matching a `chull`-returned hull vertex back
/// to the input dual point it came from. See [`map_hull_vertices_to_planes`].
const HULL_VERTEX_MATCH_EPS_REL: f64 = 1e-9;

/// Relative distance below which two reconstructed primal vertices are welded into
/// one, expressed as a fraction of the polyhedron's own scale (see
/// [`GemPolyhedron::from_planes`], where the absolute weld epsilon is derived as
/// `VERTEX_WELD_EPS_REL * scale`). A fixed absolute epsilon (the previous
/// `1e-4` in model units) is meaningless once the caller's model units aren't "close to
/// 1" -- a stone described in millimeters and one described in meters would want wildly
/// different absolute welding distances for the same relative precision. See
/// [`weld_vertices`] for why welding is necessary at all.
const VERTEX_WELD_EPS_REL: f32 = 1e-4;

#[derive(Debug, Clone)]
pub struct GemPolyhedron {
    pub vertices: Vec<Vec3>,
    pub facet_planes: Vec<GpuFacetPlane>,
    pub facet_polygons: Vec<Vec<u32>>, // Ordered vertex index loops
    pub triangle_indices: Vec<u32>,    // Triangulated index buffer
    pub bounding_radius: f32,
}

impl GemPolyhedron {
    /// Reconstructs exact 3D polyhedron from cutting schedule half-space planes.
    ///
    /// Implements the polar-duality construction described in the project's rendering
    /// blueprint (`GEMSTONE_RENDERING_BLUEPRINT.md` section 1.2): each half-space
    /// plane `n . x + d <= 0` maps to a dual point `n / -d`; the 3D convex hull of the
    /// dual points (via the `chull` crate) is computed; each triangular facet of that
    /// dual hull maps back to a primal vertex by solving the 3x3 system formed by the
    /// three corresponding planes.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - fewer than 4 planes are supplied (the minimum to bound a finite solid, a
    ///   tetrahedron);
    /// - any plane's offset `d` is non-negative (every facet plane must face inward,
    ///   containing the origin, for the dual-space construction to be valid);
    /// - two planes are coincident (identical half-spaces after polar inversion);
    /// - the dual-space convex hull computation itself fails (e.g. because the dual
    ///   points are degenerate -- coplanar or otherwise not in general position);
    /// - the planes do not bound a finite region: the origin is not strictly interior
    ///   to the dual convex hull (a necessary and sufficient condition for the primal
    ///   half-space intersection to be bounded -- see `check_origin_interior` below);
    /// - three planes that meet at a dual hull facet are near-parallel enough that the
    ///   3x3 vertex solve is ill-conditioned;
    /// - the reconstructed mesh fails Euler's formula (`V - E + F = 2`) or has
    ///   non-finite / ~zero volume, indicating a topological or numerical corruption
    ///   in the steps above.
    pub fn from_planes(planes: Vec<GpuFacetPlane>) -> Result<Self, BrepError> {
        if planes.len() < 4 {
            return Err(BrepError::TooFewPlanes {
                count: planes.len(),
            });
        }

        let dual_points = dual_points_from_planes(&planes)?;

        // 3D Convex Hull in Dual Space using `chull`
        let hull = ConvexHull::try_new(&dual_points, 1e-6f64, None).map_err(|e| {
            BrepError::HullComputationFailed {
                message: format!("{e:?}"),
            }
        })?;

        let (vertices_flat, indices_flat) = hull.vertices_indices();
        if indices_flat.len() % 3 != 0 {
            return Err(BrepError::NonTriangularHullFacets {
                indices_len: indices_flat.len(),
            });
        }

        let hull_vertex_plane_idx = map_hull_vertices_to_planes(&dual_points, &vertices_flat)?;
        check_origin_interior(&vertices_flat, &indices_flat)?;

        let (raw_vertices, mut unordered_facet_polygons) =
            reconstruct_vertices(&planes, &indices_flat, &hull_vertex_plane_idx)?;
        // Absolute weld epsilon, scaled to the polyhedron's own size: a schedule
        // expressed in millimeters and the same schedule expressed in meters should
        // weld with the same *relative* precision, not the same absolute distance.
        let scale = raw_vertices
            .iter()
            .map(|v| v.length())
            .fold(1e-6_f32, f32::max);
        let weld_eps = VERTEX_WELD_EPS_REL * scale;
        let vertices = weld_vertices(&raw_vertices, &mut unordered_facet_polygons, weld_eps);
        let facet_polygons = order_facet_polygons(&planes, &vertices, &unordered_facet_polygons);
        let triangle_indices = triangulate_polygons(&facet_polygons);

        check_euler(&vertices, &facet_polygons)?;

        let volume = mesh_volume(&vertices, &triangle_indices);
        if !volume.is_finite() || volume < 1e-9 {
            return Err(BrepError::DegenerateVolume { volume });
        }

        let max_r = vertices.iter().map(|v| v.length()).fold(0.0f32, f32::max);

        Ok(Self {
            vertices,
            facet_planes: planes,
            facet_polygons,
            triangle_indices,
            bounding_radius: max_r,
        })
    }

    /// Indices of input planes that contributed no facet to the reconstructed hull.
    ///
    /// Every input plane should normally be touched by at least one facet; a plane
    /// contributing none means it is redundant with respect to the others -- the
    /// schedule over-constrains the solid. This is real diagnostic information about a
    /// bad cutting schedule, not a hard geometric error (the returned polyhedron is
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
    /// `facet_polygons`). Returns `0.0` for a plane that contributed no facet (see
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
        let v0 = self.vertices[poly[0] as usize];
        let mut cross_sum = Vec3::ZERO;
        for w in 1..poly.len() - 1 {
            let a = self.vertices[poly[w] as usize] - v0;
            let b = self.vertices[poly[w + 1] as usize] - v0;
            cross_sum += a.cross(b);
        }
        cross_sum.length() * 0.5
    }

    /// Areas of every facet, indexed the same way as `facet_planes` / `facet_polygons`.
    #[must_use]
    pub fn facet_areas(&self) -> Vec<f32> {
        (0..self.facet_polygons.len())
            .map(|i| self.facet_area(i))
            .collect()
    }

    /// Volume of the reconstructed solid, via the divergence theorem over the
    /// triangulated mesh. Always non-negative (a physical volume is unsigned, so this
    /// does not depend on triangle winding).
    ///
    /// # Panics
    ///
    /// Panics only if internal invariants are violated (should not happen for a
    /// `GemPolyhedron` returned by [`Self::from_planes`]).
    #[must_use]
    pub fn volume(&self) -> f32 {
        mesh_volume(&self.vertices, &self.triangle_indices)
    }

    /// The girdle outline: the polyhedron's silhouette viewed from directly above
    /// (looking down the Y axis, per this crate's Y-up convention), as an ordered loop
    /// of the actual 3D vertices on that silhouette -- the 2D convex hull of the
    /// vertices' X-Z projection. This is the widest horizontal cross-section of the
    /// stone: for a well-formed faceted gem it coincides with the girdle facet
    /// vertices, making it the natural basis for comparing a reconstruction against a
    /// published cutting diagram (itself drawn as a top-down outline).
    ///
    /// # Panics
    ///
    /// Panics only if internal invariants are violated (should not happen for a
    /// `GemPolyhedron` returned by [`Self::from_planes`]).
    #[must_use]
    pub fn girdle_outline(&self) -> Vec<Vec3> {
        #[derive(Clone, Copy)]
        struct Point2 {
            idx: usize,
            x: f32,
            z: f32,
        }

        fn cross(o: Point2, a: Point2, b: Point2) -> f32 {
            (a.z - o.z).mul_add(-(b.x - o.x), (a.x - o.x) * (b.z - o.z))
        }

        let mut pts: Vec<Point2> = self
            .vertices
            .iter()
            .enumerate()
            .map(|(idx, v)| Point2 {
                idx,
                x: v.x,
                z: v.z,
            })
            .collect();
        pts.sort_by(|p, q| {
            p.x.partial_cmp(&q.x)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| p.z.partial_cmp(&q.z).unwrap_or(std::cmp::Ordering::Equal))
        });
        pts.dedup_by(|p, q| (p.x - q.x).abs() < 1e-6 && (p.z - q.z).abs() < 1e-6);

        if pts.len() < 3 {
            return pts.into_iter().map(|p| self.vertices[p.idx]).collect();
        }

        let mut lower: Vec<Point2> = Vec::new();
        for &p in &pts {
            while lower.len() >= 2
                && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0
            {
                lower.pop();
            }
            lower.push(p);
        }

        let mut upper: Vec<Point2> = Vec::new();
        for &p in pts.iter().rev() {
            while upper.len() >= 2
                && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0
            {
                upper.pop();
            }
            upper.push(p);
        }

        lower.pop();
        upper.pop();
        lower.extend(upper);

        lower.into_iter().map(|p| self.vertices[p.idx]).collect()
    }
}

/// Builds the dual points (`n_i / -d_i`) for each plane, after validating that every
/// `d` is negative (required for the polar-duality construction) and that no two
/// planes are coincident (identical half-spaces).
fn dual_points_from_planes(planes: &[GpuFacetPlane]) -> Result<Vec<Vec<f64>>, BrepError> {
    let mut dual_points: Vec<Vec<f64>> = Vec::with_capacity(planes.len());
    for (i, p) in planes.iter().enumerate() {
        if p.d >= 0.0 {
            return Err(BrepError::NonNegativeOffset { index: i, d: p.d });
        }
        let n = Vec3::from_array(p.normal);
        let dual_pt = n / (-p.d);
        dual_points.push(vec![
            f64::from(dual_pt.x),
            f64::from(dual_pt.y),
            f64::from(dual_pt.z),
        ]);
    }

    for i in 0..dual_points.len() {
        for j in (i + 1)..dual_points.len() {
            let dist_sq: f64 = (0..3)
                .map(|k| (dual_points[i][k] - dual_points[j][k]).powi(2))
                .sum();
            let scale = dual_points[i]
                .iter()
                .chain(dual_points[j].iter())
                .fold(1.0f64, |m, &v| m.max(v.abs()));
            if dist_sq.sqrt() < COINCIDENT_PLANE_EPS * scale {
                return Err(BrepError::CoincidentPlanes { i, j });
            }
        }
    }

    Ok(dual_points)
}

/// Recovers each dual hull vertex's original plane index.
///
/// `ConvexHull::try_new` internally calls `remove_unused_points`, which drops every
/// input point that never became a hull vertex before triangulating. The index buffer
/// returned by `vertices_indices()` indexes into its own compacted vertex list -- NOT
/// into `planes`/`dual_points` directly. Relying on that compacted list being an
/// order- and exact-value-preserving subsequence of `dual_points` (as an earlier
/// version of this function did, via a two-pointer scan with `!=` comparison) bakes in
/// an undocumented property of a third-party crate: any future `chull` version that
/// reorders vertices during triangulation, or rounds coordinates while copying them,
/// would turn every reconstruction into an opaque "internal error" for the user.
///
/// Instead, for each hull vertex we find the input dual point nearest to it by
/// Euclidean distance (`O(H * P)`, fine since `P` is at most a few hundred planes) and
/// accept the match only if it falls within [`HULL_VERTEX_MATCH_EPS_REL`] relative to
/// the point's own scale. Two input planes are already rejected as coincident above
/// `COINCIDENT_PLANE_EPS` (1e-7 relative) before the hull is even computed, so within
/// the tighter 1e-9 match tolerance every hull vertex has at most one plausible input
/// point -- ties are not a practical concern.
fn map_hull_vertices_to_planes(
    dual_points: &[Vec<f64>],
    vertices_flat: &[Vec<f64>],
) -> Result<Vec<usize>, BrepError> {
    let mut hull_vertex_plane_idx = Vec::with_capacity(vertices_flat.len());
    for hv in vertices_flat {
        let mut best_idx = 0usize;
        let mut best_dist_sq = f64::INFINITY;
        for (i, dp) in dual_points.iter().enumerate() {
            let dist_sq: f64 = (0..3).map(|k| (dp[k] - hv[k]).powi(2)).sum();
            if dist_sq < best_dist_sq {
                best_dist_sq = dist_sq;
                best_idx = i;
            }
        }

        if !best_dist_sq.is_finite() {
            return Err(BrepError::NoPlanesToMatch);
        }

        let hv_norm = hv.iter().map(|v| v * v).sum::<f64>().sqrt();
        let tolerance = HULL_VERTEX_MATCH_EPS_REL * hv_norm.max(1.0);
        let best_dist = best_dist_sq.sqrt();
        if best_dist > tolerance {
            return Err(BrepError::HullVertexUnmatched {
                distance: best_dist,
                tolerance,
            });
        }

        hull_vertex_plane_idx.push(best_idx);
    }
    Ok(hull_vertex_plane_idx)
}

/// Boundedness check: the primal half-space intersection is bounded iff the origin
/// lies strictly inside the dual convex hull. Every plane already has `d < 0`, so the
/// origin is a strict interior point of every *individual* half-space -- that alone
/// does not stop their intersection from being unbounded (e.g. a cube missing one face
/// is an infinite prism that still contains the origin, but is not a finite solid).
fn check_origin_interior(
    vertices_flat: &[Vec<f64>],
    indices_flat: &[usize],
) -> Result<(), BrepError> {
    let mut centroid = [0.0f64; 3];
    for p in vertices_flat {
        centroid[0] += p[0];
        centroid[1] += p[1];
        centroid[2] += p[2];
    }
    let count = vertices_flat.len() as f64;
    for c in &mut centroid {
        *c /= count;
    }

    for tri in indices_flat.as_chunks::<3>().0 {
        let pa = &vertices_flat[tri[0]];
        let pb = &vertices_flat[tri[1]];
        let pc = &vertices_flat[tri[2]];
        let edge1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let edge2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let mut face_normal = [
            edge1[2].mul_add(-edge2[1], edge1[1] * edge2[2]),
            edge1[0].mul_add(-edge2[2], edge1[2] * edge2[0]),
            edge1[1].mul_add(-edge2[0], edge1[0] * edge2[1]),
        ];
        let to_centroid = [
            centroid[0] - pa[0],
            centroid[1] - pa[1],
            centroid[2] - pa[2],
        ];
        let dot_centroid = face_normal[2].mul_add(
            to_centroid[2],
            face_normal[1].mul_add(to_centroid[1], face_normal[0] * to_centroid[0]),
        );
        if dot_centroid > 0.0 {
            face_normal = [-face_normal[0], -face_normal[1], -face_normal[2]];
        }
        let face_normal_len = face_normal[2]
            .mul_add(
                face_normal[2],
                face_normal[1].mul_add(face_normal[1], face_normal[0] * face_normal[0]),
            )
            .sqrt();
        if face_normal_len < 1e-12 {
            continue; // degenerate dual triangle; the determinant check on the primal side will catch it
        }
        let to_origin = [-pa[0], -pa[1], -pa[2]];
        let side = face_normal[2].mul_add(
            to_origin[2],
            face_normal[1].mul_add(to_origin[1], face_normal[0] * to_origin[0]),
        ) / face_normal_len;
        if side > -ORIGIN_INTERIOR_EPS {
            return Err(BrepError::UnboundedRegion {
                signed_distance: side,
            });
        }
    }
    Ok(())
}

/// Solves each dual hull triangle's 3x3 system for its primal vertex, returning the
/// vertex list and, for each input plane, the (unordered) set of vertex indices lying
/// on it.
///
/// The solve itself is done in `f64` (`DMat3`/`DVec3`, via
/// [`GpuFacetPlane::to_halfspace_f64`]) and only the final vertex is narrowed to `Vec3`
/// for storage. `f32` gives roughly 7 decimal digits of precision; for a schedule
/// described in small model units (or one scaled far from unit size) that is not
/// enough headroom between the precision the inputs carry and the precision the
/// determinant/inverse computation consumes internally, so a triple that is
/// well-conditioned in exact arithmetic could previously be misjudged as ill-conditioned
/// (or worse, solved into a visibly wrong vertex) purely from `f32` rounding in the
/// solve. `f64` pushes that failure mode far below anything a real cutting schedule
/// exercises.
fn reconstruct_vertices(
    planes: &[GpuFacetPlane],
    indices_flat: &[usize],
    hull_vertex_plane_idx: &[usize],
) -> Result<(Vec<Vec3>, Vec<Vec<u32>>), BrepError> {
    let mut vertices = Vec::new();
    let mut facet_polygons = vec![Vec::new(); planes.len()];

    for tri in indices_flat.as_chunks::<3>().0 {
        let a_idx = hull_vertex_plane_idx[tri[0]];
        let b_idx = hull_vertex_plane_idx[tri[1]];
        let c_idx = hull_vertex_plane_idx[tri[2]];

        let (na, ma) = planes[a_idx].to_halfspace_f64();
        let (nb, mb) = planes[b_idx].to_halfspace_f64();
        let (nc, mc) = planes[c_idx].to_halfspace_f64();

        let m = DMat3::from_cols(na, nb, nc).transpose();

        let det = m.determinant();
        if det.abs() < MIN_TRIPLE_DETERMINANT {
            return Err(BrepError::IllConditionedTriple {
                a: a_idx,
                b: b_idx,
                c: c_idx,
                det,
            });
        }

        let rhs = DVec3::new(ma, mb, mc);
        let vertex = m.inverse() * rhs;

        if !vertex.is_finite() {
            return Err(BrepError::NonFiniteVertex {
                a: a_idx,
                b: b_idx,
                c: c_idx,
                vertex,
            });
        }

        let v_idx = vertices.len() as u32;
        vertices.push(vertex.as_vec3());
        facet_polygons[a_idx].push(v_idx);
        facet_polygons[b_idx].push(v_idx);
        facet_polygons[c_idx].push(v_idx);
    }

    Ok((vertices, facet_polygons))
}

/// Welds together reconstructed primal vertices that fall within `eps` of each other,
/// remapping every `facet_polygons` index accordingly.
///
/// This matters whenever more than 3 input planes pass through exactly the same
/// primal point -- a common occurrence in symmetric cutting schedules (e.g. a round
/// brilliant's star, kite and girdle facets deliberately meeting at shared special
/// points). In dual space that shows up as more than 3 dual points lying exactly on a
/// common plane, i.e. a dual hull facet that is a coplanar N-gon rather than a
/// triangle. `chull` always triangulates its hull facets (see
/// `map_hull_vertices_to_planes`), so such an N-gon comes back as a fan of N-2
/// triangles -- and every one of those triangles, solved independently via its own
/// 3x3 system, re-derives the *same* primal point. Without welding, each of those
/// solves produces its own distinct entry in `vertices`, so facets that share that
/// physical vertex end up referencing different indices for it, which silently
/// corrupts edge/facet adjacency (the mesh becomes non-manifold even though the
/// geometry is perfectly valid).
fn weld_vertices(vertices: &[Vec3], facet_polygons: &mut [Vec<u32>], eps: f32) -> Vec<Vec3> {
    let mut welded: Vec<Vec3> = Vec::new();
    let mut remap: Vec<u32> = Vec::with_capacity(vertices.len());
    for v in vertices {
        let mut found = None;
        for (wi, w) in welded.iter().enumerate() {
            if (*v - *w).length() < eps {
                found = Some(wi as u32);
                break;
            }
        }
        if let Some(idx) = found {
            remap.push(idx);
        } else {
            remap.push(welded.len() as u32);
            welded.push(*v);
        }
    }

    for poly in facet_polygons.iter_mut() {
        for idx in poly.iter_mut() {
            *idx = remap[*idx as usize];
        }
        poly.sort_unstable();
        poly.dedup();
    }

    welded
}

/// Sorts each facet's vertex loop into angular (counter-clockwise around the facet
/// normal) order, so it forms a proper polygon boundary rather than an arbitrary set.
fn order_facet_polygons(
    planes: &[GpuFacetPlane],
    vertices: &[Vec3],
    unordered: &[Vec<u32>],
) -> Vec<Vec<u32>> {
    let mut ordered = Vec::with_capacity(planes.len());
    for (i, p) in planes.iter().enumerate() {
        let mut poly_verts = unordered[i].clone();
        if poly_verts.len() < 3 {
            ordered.push(Vec::new());
            continue;
        }

        let mut center = Vec3::ZERO;
        for &vi in &poly_verts {
            center += vertices[vi as usize];
        }
        center /= poly_verts.len() as f32;

        let normal = Vec3::from_array(p.normal);
        let tangent = if normal.z.abs() < 0.999 {
            normal.cross(Vec3::Z).normalize()
        } else {
            normal.cross(Vec3::X).normalize()
        };
        let bitangent = normal.cross(tangent).normalize();

        poly_verts.sort_by(|&a, &b| {
            let va = vertices[a as usize] - center;
            let vb = vertices[b as usize] - center;
            let angle_a = va.dot(bitangent).atan2(va.dot(tangent));
            let angle_b = vb.dot(bitangent).atan2(vb.dot(tangent));
            angle_a
                .partial_cmp(&angle_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ordered.push(poly_verts);
    }
    ordered
}

/// Fan-triangulates every facet polygon into the flat index buffer used for rendering.
fn triangulate_polygons(facet_polygons: &[Vec<u32>]) -> Vec<u32> {
    let mut triangle_indices = Vec::new();
    for poly in facet_polygons {
        if poly.len() < 3 {
            continue;
        }
        for i in 1..(poly.len() - 1) {
            triangle_indices.push(poly[0]);
            triangle_indices.push(poly[i]);
            triangle_indices.push(poly[i + 1]);
        }
    }
    triangle_indices
}

/// Validity gate: every edge of a closed 2-manifold polyhedron must be shared by
/// exactly two facets, and Euler's formula `V - E + F = 2` must hold. This is the
/// single most valuable check on the reconstruction above -- it catches nearly any
/// topological error in the facet/vertex bookkeeping.
fn check_euler(vertices: &[Vec3], facet_polygons: &[Vec<u32>]) -> Result<(), BrepError> {
    let mut edge_counts: HashMap<(u32, u32), u32> = HashMap::new();
    for poly in facet_polygons {
        if poly.len() < 3 {
            continue;
        }
        for w in 0..poly.len() {
            let a = poly[w];
            let b = poly[(w + 1) % poly.len()];
            let key = if a < b { (a, b) } else { (b, a) };
            *edge_counts.entry(key).or_insert(0) += 1;
        }
    }

    let mut bad_edge: Option<((u32, u32), u32)> = None;
    for (&edge, &count) in &edge_counts {
        if count != 2 {
            bad_edge = Some((edge, count));
            break;
        }
    }
    if let Some((edge, count)) = bad_edge {
        return Err(BrepError::NonManifoldEdge { edge, count });
    }

    let vertex_count = vertices.len();
    let face_count = facet_polygons.iter().filter(|p| p.len() >= 3).count();
    let edge_count = edge_counts.len();
    let euler = vertex_count as i64 - edge_count as i64 + face_count as i64;
    if euler != 2 {
        return Err(BrepError::EulerFormulaFailed {
            vertices: vertex_count,
            edges: edge_count,
            faces: face_count,
            euler,
        });
    }
    Ok(())
}

/// Divergence-theorem volume of a triangulated closed mesh, from the origin. Always
/// non-negative: a physical volume is unsigned, so this does not depend on triangle
/// winding.
fn mesh_volume(vertices: &[Vec3], triangle_indices: &[u32]) -> f32 {
    triangle_indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|tri| {
            let a = vertices[tri[0] as usize];
            let b = vertices[tri[1] as usize];
            let c = vertices[tri[2] as usize];
            a.dot(b.cross(c))
        })
        .sum::<f32>()
        .abs()
        / 6.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six axis-aligned half-space planes bounding a cube of the given half-extent
    /// centered on the origin. `GpuFacetPlane::new` requires `d < 0`, so `d =
    /// -half_extent`.
    fn cube_planes(half_extent: f32) -> Vec<GpuFacetPlane> {
        vec![
            GpuFacetPlane::new(Vec3::X, -half_extent),
            GpuFacetPlane::new(-Vec3::X, -half_extent),
            GpuFacetPlane::new(Vec3::Y, -half_extent),
            GpuFacetPlane::new(-Vec3::Y, -half_extent),
            GpuFacetPlane::new(Vec3::Z, -half_extent),
            GpuFacetPlane::new(-Vec3::Z, -half_extent),
        ]
    }

    // Tests for map_hull_vertices_to_planes's tolerance-based matching.

    #[test]
    fn map_hull_vertices_to_planes_matches_cube_dual_points() {
        // A unit cube's six planes (`n . x <= 1` for each axis direction) have dual
        // points `n / -d = n / 1 = n` -- i.e. the six unit axis vectors themselves.
        let dual_points: Vec<Vec<f64>> = vec![
            vec![1.0, 0.0, 0.0],
            vec![-1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, -1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![0.0, 0.0, -1.0],
        ];
        let hull_vertices = dual_points.clone();
        let idx = map_hull_vertices_to_planes(&dual_points, &hull_vertices)
            .expect("cube dual points should all match their own input plane");
        assert_eq!(idx, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn map_hull_vertices_to_planes_tolerates_permutation() {
        let dual_points: Vec<Vec<f64>> = vec![
            vec![1.0, 0.0, 0.0],
            vec![-1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, -1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![0.0, 0.0, -1.0],
        ];
        // `chull` gives no guarantee that its compacted hull-vertex list preserves the
        // input order or exact input values. Simulate both: feed the matcher a
        // reordered copy of the dual points, each nudged by a tiny rounding error
        // (well inside the 1e-9 relative match tolerance).
        let permuted_order = [3usize, 0, 5, 1, 4, 2];
        let hull_vertices: Vec<Vec<f64>> = permuted_order
            .iter()
            .map(|&i| dual_points[i].iter().map(|v| v + 1e-12).collect())
            .collect();

        let idx = map_hull_vertices_to_planes(&dual_points, &hull_vertices)
            .expect("permuted, slightly-perturbed dual points should still map within tolerance");
        assert_eq!(idx, permuted_order.to_vec());
    }

    #[test]
    fn map_hull_vertices_to_planes_rejects_vertex_with_no_close_input_point() {
        let dual_points: Vec<Vec<f64>> = vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
        // Far outside the relative tolerance of either input dual point.
        let hull_vertices: Vec<Vec<f64>> = vec![vec![5.0, 5.0, 5.0]];
        assert!(map_hull_vertices_to_planes(&dual_points, &hull_vertices).is_err());
    }

    // Tests for from_planes's f64 vertex solve and relative weld tolerance.

    #[test]
    fn from_planes_cube_has_expected_topology() {
        let hull =
            GemPolyhedron::from_planes(cube_planes(1.0)).expect("unit cube must reconstruct");
        assert_eq!(hull.vertices.len(), 8);
        assert_eq!(hull.facet_polygons.len(), 6);
        assert!(hull.facet_polygons.iter().all(|p| p.len() == 4));
        assert_eq!(hull.triangle_indices.len(), 36); // 6 faces * 2 triangles * 3 indices
    }

    #[test]
    fn from_planes_is_scale_invariant_for_cube_topology() {
        // A cube scaled 1,000,000x in linear size (1000x and 0.001x here) must produce
        // the same vertex count, facet count and triangle count as the unit-scale
        // cube: the f64 vertex solve and the scale-relative weld tolerance should make
        // topology reconstruction independent of the caller's choice of model units.
        let unit =
            GemPolyhedron::from_planes(cube_planes(1.0)).expect("unit-scale cube must reconstruct");

        for &scale in &[1000.0_f32, 0.001_f32] {
            let scaled = GemPolyhedron::from_planes(cube_planes(scale))
                .unwrap_or_else(|e| panic!("cube scaled by {scale} must reconstruct: {e}"));

            assert_eq!(
                scaled.vertices.len(),
                unit.vertices.len(),
                "vertex count must be scale-invariant (scale = {scale})"
            );
            assert_eq!(
                scaled.facet_polygons.len(),
                unit.facet_polygons.len(),
                "facet count must be scale-invariant (scale = {scale})"
            );
            assert_eq!(
                scaled.triangle_indices.len(),
                unit.triangle_indices.len(),
                "triangle count must be scale-invariant (scale = {scale})"
            );

            let expected_radius = unit.bounding_radius * scale;
            let rel_err = (scaled.bounding_radius - expected_radius).abs() / expected_radius;
            assert!(
                rel_err < 1e-3,
                "bounding radius should scale linearly (scale = {scale}): got {}, expected ~{expected_radius}",
                scaled.bounding_radius
            );
        }
    }

    #[test]
    fn from_planes_round_brilliant_still_reconstructs_with_f64_solve() {
        // Sanity check on the real, non-trivial cutting schedule (57 facets, many
        // special points where more than 3 planes meet at once and rely on welding)
        // rather than just the trivial cube, at its native unit scale -- guards
        // against the f64 vertex solve or the relative weld tolerance regressing
        // topology reconstruction for a schedule this crate actually ships.
        //
        // A wider scale sweep like the cube test above isn't used here: at very small
        // scales this schedule's absolute volume approaches the fixed ~1e-9 "degenerate
        // solid" floor in `from_planes` (a separate, pre-existing constant this task
        // does not touch), which would make such a test flaky for reasons unrelated to
        // the vertex-solve/weld-tolerance fix under test.
        let planes = crate::geometry::cuts::StandardGemCuts::standard_round_brilliant();
        let hull =
            GemPolyhedron::from_planes(planes).expect("standard round brilliant must reconstruct");
        assert_eq!(hull.untouched_planes(), Vec::<usize>::new());
        check_euler(&hull.vertices, &hull.facet_polygons)
            .expect("round brilliant topology must satisfy Euler's formula");
    }
}
