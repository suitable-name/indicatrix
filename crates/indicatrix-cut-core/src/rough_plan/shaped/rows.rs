//! The LP of one stone clipped by planes, and the buffers it reuses.
//!
//! Every place that fits a box-model stone into a clipped piece (the piece
//! table, the layout builder, the refinement and the uniform pass) builds the
//! same rows: the piece's six box faces plus one row per violated cut plane.
//! [`PartialSolver`] builds them into a reusable buffer, so a solve allocates
//! nothing after the first call.
//!
//! A rough with a non-convex mesh adds one more kind of row, per blocking triangle, in
//! [`PartialSolver::solve_in_mesh`]: the cutting-plane loop described there.

use glam::DVec3;

use crate::rough_plan::{
    fit::FitMesh,
    lp::{LpScratch, ScaleRow, max_scale_with_scratch},
    piece::{ASSIGNMENTS, Norm},
};

/// Most times [`PartialSolver::solve_in_mesh`] adds the planes of the triangles that block
/// the solution so far and solves again, before it gives up.
pub const MESH_ROUNDS: usize = 6;

/// The caliper extents of a stone of `norm`, in units of its width, along the
/// three rough axes under assignment `orient`.
///
/// Design dimension `j` (width, length, height) runs along rough axis
/// `ASSIGNMENTS[orient][j]`, so the extent along axis `a` is the dimension whose
/// `j` maps to `a`. (For the three assignments that are not their own inverse
/// this differs from indexing the dimensions by the assignment.)
#[must_use]
pub fn caliper_extents(norm: &Norm, orient: usize) -> [f64; 3] {
    let dims = norm.dims();
    let mut extents = [0.0; 3];
    for (dim, &axis) in dims.iter().zip(&ASSIGNMENTS[orient]) {
        extents[axis] = *dim;
    }
    extents
}

/// The region a stone must stay inside: a piece's stone box, clipped by the
/// planes of `planes` named in `violated`.
#[derive(Debug, Clone, Copy)]
pub struct ClipRegion<'a> {
    /// Minimum corner of the stone box, in mm.
    pub min: [f64; 3],
    /// Maximum corner of the stone box, in mm.
    pub max: [f64; 3],
    /// The cut planes `(n, m)` with `n . p <= m`.
    pub planes: &'a [(DVec3, f64)],
    /// Indices into `planes` of the planes the stone box crosses.
    pub violated: &'a [usize],
}

/// Reusable row buffer and simplex workspace for partial-piece solves.
#[derive(Debug, Default)]
pub struct PartialSolver {
    rows: Vec<ScaleRow>,
    scratch: LpScratch,
}

impl PartialSolver {
    /// An empty solver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The largest scale `k` and the stone centre `t` for a stone of caliper
    /// `extents` (at unit scale) inside `region`; `None` when the region is
    /// empty.
    pub fn solve(&mut self, region: &ClipRegion<'_>, extents: [f64; 3]) -> Option<(f64, [f64; 3])> {
        self.rows.clear();
        for i in 0..3 {
            let mut n_pos = [0.0; 3];
            n_pos[i] = 1.0;
            self.rows.push(ScaleRow {
                normal: n_pos,
                support: extents[i] * 0.5,
                offset: region.max[i],
            });
            let mut n_neg = [0.0; 3];
            n_neg[i] = -1.0;
            self.rows.push(ScaleRow {
                normal: n_neg,
                support: extents[i] * 0.5,
                offset: -region.min[i],
            });
        }
        for &v_idx in region.violated {
            let (n, m) = region.planes[v_idx];
            let support = n.x.abs().mul_add(
                extents[0] * 0.5,
                n.y.abs()
                    .mul_add(extents[1] * 0.5, n.z.abs() * extents[2] * 0.5),
            );
            self.rows.push(ScaleRow {
                normal: [n.x, n.y, n.z],
                support,
                offset: m,
            });
        }
        max_scale_with_scratch(&self.rows, &mut self.scratch)
    }

    /// [`solve`](Self::solve) when `mesh` is `None`, else
    /// [`solve_in_mesh`](Self::solve_in_mesh).
    pub fn solve_with(
        &mut self,
        region: &ClipRegion<'_>,
        extents: [f64; 3],
        mesh: Option<FitMesh<'_>>,
    ) -> Option<(f64, [f64; 3])> {
        match mesh {
            Some(mesh) => self.solve_in_mesh(region, extents, mesh),
            None => self.solve(region, extents),
        }
    }

    /// [`solve`](Self::solve) for a stone that must also stay in the material of `mesh`,
    /// `mesh.inset_mm` clear of its surface; `None` when no such stone is found.
    ///
    /// The mesh is not convex, so it is not one LP. The LP of the region is solved; if the
    /// stone box it gives meets triangles of the mesh, the plane of the blocking triangle
    /// the box passes least far (its outward side kept, `inset_mm` clear) is added as a
    /// row and the LP is solved again, at most [`MESH_ROUNDS`] times. One row per round:
    /// the walls of a notch face each other, so all of them together would leave no stone. Rows only shrink the feasible set, so
    /// every stone returned has been checked against the mesh (no triangle meets its box
    /// and its centre is in the material): the answer is valid, if conservative near a
    /// notch, because a blocking triangle's plane also forbids the air side far beyond the
    /// triangle.
    pub fn solve_in_mesh(
        &mut self,
        region: &ClipRegion<'_>,
        extents: [f64; 3],
        mesh: FitMesh<'_>,
    ) -> Option<(f64, [f64; 3])> {
        let mut solution = self.solve(region, extents)?;
        let mut blockers = Vec::new();
        let mut known: Vec<(DVec3, f64)> = Vec::new();
        for round in 0..=MESH_ROUNDS {
            let (k, t) = solution;
            if k <= 0.0 {
                return None;
            }
            let half = extents.map(|e| 0.5 * k * e);
            let min = [0, 1, 2].map(|i| t[i] - half[i]);
            let max = [0, 1, 2].map(|i| t[i] + half[i]);
            mesh.mesh
                .box_blockers(min, max, mesh.inset_mm, &mut blockers);
            if blockers.is_empty() {
                return mesh.mesh.contains_point(DVec3::from(t)).then_some(solution);
            }
            if round == MESH_ROUNDS {
                return None;
            }
            let (reach, centre) = (
                |n: DVec3| n.abs().dot(DVec3::from(extents) * (0.5 * k)),
                DVec3::from(t),
            );
            let added = mesh.mesh.blocker_rows(&blockers, &known, |n, d| {
                n.dot(centre) + reach(n) - (d - mesh.inset_mm)
            });
            if added.is_empty() {
                return None;
            }
            for (n, d) in added {
                known.push((n, d));
                self.rows.push(ScaleRow {
                    normal: [n.x, n.y, n.z],
                    support: n.abs().dot(DVec3::from(extents) * 0.5),
                    offset: d - mesh.inset_mm,
                });
            }
            solution = max_scale_with_scratch(&self.rows, &mut self.scratch)?;
        }
        None
    }
}
