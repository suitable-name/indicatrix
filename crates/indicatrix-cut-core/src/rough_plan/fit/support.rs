//! Support value evaluation, proxy hull reduction, and reusable LP workspaces.
//!
//! For an orientation with column axes `axes` and rough halfspace normal `n_j`,
//! the support is `h_j = max_i (Rot^T n_j) · v_i`. Normals are rotated into the stone
//! frame once per plane, and a dot-product scan over vertices computes the unit support.

use std::collections::BTreeSet;

use glam::DVec3;

use super::FitMesh;
use crate::rough_plan::{
    lp::{LpScratch, ScaleRow, max_scale_with_scratch},
    shape::hull::convex_planes,
    shaped::rows::MESH_ROUNDS,
};

/// A design's outline prepared for checking poses against a rough's mesh.
pub struct StoneGuard<'a> {
    mesh: FitMesh<'a>,
    /// The outward planes of the design's convex outline, caliper frame, unit scale.
    planes: Vec<(DVec3, f64)>,
    /// A point inside the outline (the mean of its vertices), caliper frame, unit scale.
    inside: DVec3,
}

impl<'a> StoneGuard<'a> {
    /// The guard for a design with outline `vertices`; `None` when they span no volume.
    pub(crate) fn new(mesh: FitMesh<'a>, vertices: &[[f64; 3]]) -> Option<Self> {
        let points: Vec<DVec3> = vertices.iter().map(|&v| DVec3::from(v)).collect();
        let planes = convex_planes(&points)?;
        let inside = points.iter().copied().sum::<DVec3>() / points.len() as f64;
        Some(Self {
            mesh,
            planes,
            inside,
        })
    }
}

/// The 26 directions for screening proxy vertex selection:
/// 6 coordinate axes, 12 edge diagonals, and 8 corner diagonals.
const PROXY_DIRECTIONS: [[f64; 3]; 26] = [
    // 6 axes
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, -1.0],
    // 12 edge diagonals
    [1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, 1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, 1.0],
    [0.0, -1.0, -1.0],
    // 8 corner diagonals
    [1.0, 1.0, 1.0],
    [1.0, 1.0, -1.0],
    [1.0, -1.0, 1.0],
    [1.0, -1.0, -1.0],
    [-1.0, 1.0, 1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [-1.0, -1.0, -1.0],
];

/// Extracts the extreme vertices of `vertices` along 26 fixed directions.
///
/// The resulting subset forms an inscribed proxy polyhedron of at most 26 vertices,
/// deduplicated and returned in stable index order.
#[must_use]
pub fn compute_proxy_vertices(vertices: &[[f64; 3]]) -> Vec<[f64; 3]> {
    if vertices.is_empty() {
        return Vec::new();
    }

    let mut selected_indices = BTreeSet::new();

    for dir in &PROXY_DIRECTIONS {
        let mut best_index = 0;
        let mut max_dot = f64::NEG_INFINITY;

        for (v_idx, vert) in vertices.iter().enumerate() {
            let dot = f64::mul_add(
                dir[2],
                vert[2],
                f64::mul_add(dir[1], vert[1], dir[0] * vert[0]),
            );
            if dot > max_dot {
                max_dot = dot;
                best_index = v_idx;
            }
        }

        selected_indices.insert(best_index);
    }

    selected_indices
        .into_iter()
        .map(|idx| vertices[idx])
        .collect()
}

/// The 26 proxy directions as unit vectors, in the order of [`PROXY_DIRECTIONS`].
///
/// Only `sqrt` is used, so the vectors are identical on every platform; the six axes are
/// exact.
#[must_use]
pub fn proxy_unit_directions() -> Vec<[f64; 3]> {
    PROXY_DIRECTIONS
        .iter()
        .map(|dir| {
            let len = dir[2]
                .mul_add(dir[2], dir[1].mul_add(dir[1], dir[0] * dir[0]))
                .sqrt();
            [dir[0] / len, dir[1] / len, dir[2] / len]
        })
        .collect()
}

/// Largest `|n_a + n_b|` for which two region normals count as opposite.
const OPPOSITE_TOLERANCE: f64 = 1e-10;

/// Index pairs `(a, b)`, `a < b`, of region planes whose normals are opposite, in ascending
/// order of `a` then `b`.
///
/// Two such planes bound a slab of width `m_a + m_b` along `n_a`, which no stone wider than
/// that can fit; [`SupportWorkspace::scale_bound`] turns that into a bound on the scale
/// without solving the LP. Only pairs actually present in `region` are returned: a cut
/// plane has no partner unless another plane faces the other way.
#[must_use]
pub fn opposite_pairs(region: &[(DVec3, f64)]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for (index_a, &(normal_a, _)) in region.iter().enumerate() {
        for (index_b, &(normal_b, _)) in region.iter().enumerate().skip(index_a + 1) {
            if (normal_a + normal_b).length() <= OPPOSITE_TOLERANCE {
                pairs.push((index_a, index_b));
            }
        }
    }
    pairs
}

/// The LP row of the plane `(norm_dvec, offset)` for a stone with column axes `axes` and
/// outline `vertices`: the normal rotated into the stone frame, the largest dot product
/// with a vertex (clamped to be non-negative, as the LP requires) as the support.
fn support_row(
    axes: &[[f64; 3]; 3],
    vertices: &[[f64; 3]],
    norm_dvec: DVec3,
    offset: f64,
) -> ScaleRow {
    let norm_x = norm_dvec.x;
    let norm_y = norm_dvec.y;
    let norm_z = norm_dvec.z;

    // Rot^T * norm_dvec (project normal onto stone axes)
    let local_nx = f64::mul_add(
        norm_z,
        axes[0][2],
        f64::mul_add(norm_y, axes[0][1], norm_x * axes[0][0]),
    );
    let local_ny = f64::mul_add(
        norm_z,
        axes[1][2],
        f64::mul_add(norm_y, axes[1][1], norm_x * axes[1][0]),
    );
    let local_nz = f64::mul_add(
        norm_z,
        axes[2][2],
        f64::mul_add(norm_y, axes[2][1], norm_x * axes[2][0]),
    );

    let mut max_support = f64::NEG_INFINITY;
    for vert in vertices {
        let dot = local_nz.mul_add(vert[2], local_ny.mul_add(vert[1], local_nx * vert[0]));
        if dot > max_support {
            max_support = dot;
        }
    }

    ScaleRow {
        normal: [norm_x, norm_y, norm_z],
        support: max_support.max(0.0),
        offset,
    }
}

/// Reusable buffer workspace for evaluating support values and solving scaling LPs.
pub struct SupportWorkspace {
    scale_rows: Vec<ScaleRow>,
    /// `scale_rows` with the offsets shifted to the frame of the current centre.
    shifted: Vec<ScaleRow>,
    lp_scratch: LpScratch,
    /// The stone's planes in the rough frame, rebuilt per mesh check.
    world: Vec<(DVec3, f64)>,
    blockers: Vec<u32>,
}

impl SupportWorkspace {
    /// Creates a new workspace with capacity reserved for `plane_capacity` planes.
    #[must_use]
    pub(crate) fn new(plane_capacity: usize) -> Self {
        Self {
            scale_rows: Vec::with_capacity(plane_capacity),
            shifted: Vec::new(),
            lp_scratch: LpScratch::new(),
            world: Vec::new(),
            blockers: Vec::new(),
        }
    }

    /// Evaluates the maximum scale factor `k` and stone center `t` for a stone inside `region`.
    ///
    /// This is [`Self::prepare`] followed by [`Self::solve`]; see those for the details.
    pub(crate) fn evaluate(
        &mut self,
        axes: &[[f64; 3]; 3],
        region: &[(DVec3, f64)],
        vertices: &[[f64; 3]],
    ) -> Option<(f64, [f64; 3])> {
        if !self.prepare(axes, region, vertices) {
            return None;
        }
        self.solve()
    }

    /// [`Self::evaluate`] for a stone that must also stay in the material of the mesh of
    /// `guard`; see [`Self::verify_in_mesh`].
    pub(crate) fn evaluate_in_mesh(
        &mut self,
        axes: &[[f64; 3]; 3],
        region: &[(DVec3, f64)],
        vertices: &[[f64; 3]],
        guard: &StoneGuard<'_>,
    ) -> Option<(f64, [f64; 3])> {
        if !self.prepare(axes, region, vertices) {
            return None;
        }
        let first = self.solve()?;
        self.verify_in_mesh(axes, vertices, guard, first)
    }

    /// Checks the pose `first` (the solution of the rows of the last [`Self::prepare`])
    /// against the mesh, and where it reaches into air shrinks it by a cutting-plane loop.
    ///
    /// The stone is its outline at scale `k`, rotated by `axes`, centred at `t`. It is
    /// valid when no triangle of the mesh meets it (kept `inset_mm` clear) and its centre
    /// lies in the material. Otherwise the plane of the blocking triangle the pose passes
    /// least far (see `RoughMesh::blocker_rows`, which adds the planes of the same wall
    /// along with it), its outward side kept `inset_mm` clear, is added as a row and the LP
    /// solved again, at most
    /// [`MESH_ROUNDS`] times. Rows only shrink the feasible set, so every pose returned is
    /// verified; the result is conservative near a notch, since a triangle's plane also
    /// forbids the air side far beyond the triangle. `None` when no valid pose is found
    /// (also for a pose with no scale).
    pub(crate) fn verify_in_mesh(
        &mut self,
        axes: &[[f64; 3]; 3],
        vertices: &[[f64; 3]],
        guard: &StoneGuard<'_>,
        first: (f64, [f64; 3]),
    ) -> Option<(f64, [f64; 3])> {
        let (mesh, inset) = (guard.mesh.mesh, guard.mesh.inset_mm);
        let frame = axes.map(DVec3::from);
        let mut known: Vec<(DVec3, f64)> = Vec::new();
        let mut solution = first;
        for round in 0..=MESH_ROUNDS {
            let (k, t) = solution;
            if k <= 0.0 {
                return None;
            }
            let centre = DVec3::from(t);
            self.world.clear();
            for &(local, d) in &guard.planes {
                let n = local.x * frame[0] + local.y * frame[1] + local.z * frame[2];
                self.world.push((n, k.mul_add(d, n.dot(centre))));
            }
            mesh.polytope_blockers(&self.world, inset, &mut self.blockers);
            let violation = |n: DVec3, d: f64| {
                let row = support_row(axes, vertices, n, d - inset);
                k.mul_add(row.support, n.dot(centre)) - row.offset
            };
            mesh.drop_cleared(&mut self.blockers, violation);
            if self.blockers.is_empty() {
                let inside = guard.inside;
                let probe =
                    centre + k * (inside.x * frame[0] + inside.y * frame[1] + inside.z * frame[2]);
                return mesh.contains_point(probe).then_some(solution);
            }
            if round == MESH_ROUNDS {
                return None;
            }
            let added = mesh.blocker_rows(
                &self.blockers,
                &known,
                |n, d| n.dot(centre) <= d - inset,
                violation,
            );
            if added.is_empty() {
                return None;
            }
            for (n, d) in added {
                known.push((n, d));
                self.scale_rows
                    .push(support_row(axes, vertices, n, d - inset));
            }
            solution = self.solve_about(centre)?;
        }
        None
    }

    /// [`Self::solve`] in the frame of `centre`: every offset of a copy of the rows becomes
    /// `offset - n . centre`, and the centre is added back to the pose.
    ///
    /// Every row the centre satisfies gets a non-negative offset, so phase 1 of the simplex
    /// needs no artificial variable for it. The post-check in `lp.rs` accepts
    /// `1e-9 (1 + |offset|)` per row; the shifted offsets are smaller, so it is a little
    /// stricter, never looser.
    fn solve_about(&mut self, centre: DVec3) -> Option<(f64, [f64; 3])> {
        let c = [centre.x, centre.y, centre.z];
        self.shifted.clear();
        self.shifted.extend(self.scale_rows.iter().map(|row| {
            let shift =
                row.normal[2].mul_add(c[2], row.normal[1].mul_add(c[1], row.normal[0] * c[0]));
            ScaleRow {
                normal: row.normal,
                support: row.support,
                offset: row.offset - shift,
            }
        }));
        let (k, t) = max_scale_with_scratch(&self.shifted, &mut self.lp_scratch)?;
        Some((k, [t[0] + c[0], t[1] + c[1], t[2] + c[2]]))
    }

    /// Upper bound on the scale [`Self::solve`] can return for the rows of the last
    /// [`Self::prepare`], from the `pairs` of opposite region planes (see [`opposite_pairs`]).
    ///
    /// Adding the two rows of a pair cancels the translation term and leaves
    /// `k (h_a + h_b) <= m_a + m_b`, so `k <= (m_a + m_b) / (h_a + h_b)` with the same
    /// clamped supports the LP uses. The bound is the smallest such ratio, `+infinity` when
    /// there is no pair with a positive support sum. The LP accepts a violation of
    /// `1e-9 (1 + |m|)` per row, so callers compare against it with a small relative slack.
    #[must_use]
    pub(crate) fn scale_bound(&self, pairs: &[(usize, usize)]) -> f64 {
        pairs
            .iter()
            .filter_map(|&(a, b)| {
                let (row_a, row_b) = (self.scale_rows.get(a)?, self.scale_rows.get(b)?);
                let support = row_a.support + row_b.support;
                (support > 0.0).then_some((row_a.offset + row_b.offset) / support)
            })
            .fold(f64::INFINITY, f64::min)
    }

    /// Solves the scaling LP for the rows of the last [`Self::prepare`].
    pub(crate) fn solve(&mut self) -> Option<(f64, [f64; 3])> {
        max_scale_with_scratch(&self.scale_rows, &mut self.lp_scratch)
    }

    /// Builds the LP rows of a stone inside `region`; `false` (and no rows) when there is
    /// nothing to fit.
    ///
    /// Rotates each plane normal into the stone frame using `axes` (where column 0 is stone x,
    /// column 1 is stone y, and column 2 is stone z) and scans over `vertices` for maximum
    /// support.
    ///
    /// Supports are clamped to be non-negative, as the LP requires. For a hull whose
    /// origin (its bounding-box centre) lies inside the hull every true support is already
    /// non-negative and the clamp changes nothing; for an outline whose centre lies outside
    /// it the clamp is conservative (the stone is treated as slightly larger than it is),
    /// so a returned fit is always valid but may leave a little volume unused.
    pub(crate) fn prepare(
        &mut self,
        axes: &[[f64; 3]; 3],
        region: &[(DVec3, f64)],
        vertices: &[[f64; 3]],
    ) -> bool {
        self.scale_rows.clear();
        if vertices.is_empty() || region.is_empty() {
            return false;
        }

        for &(norm_dvec, offset) in region {
            self.scale_rows
                .push(support_row(axes, vertices, norm_dvec, offset));
        }

        true
    }
}
