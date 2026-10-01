//! The LP of one stone clipped by planes, and the buffers it reuses.
//!
//! Every place that fits a box-model stone into a clipped piece (the piece
//! table, the layout builder, the refinement and the uniform pass) builds the
//! same rows: the piece's six box faces plus one row per violated cut plane.
//! [`PartialSolver`] builds them into a reusable buffer, so a solve allocates
//! nothing after the first call.

use glam::DVec3;

use crate::rough_plan::{
    lp::{LpScratch, ScaleRow, max_scale_with_scratch},
    piece::{ASSIGNMENTS, Norm},
};

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
}
