//! Support value evaluation, proxy hull reduction, and reusable LP workspaces.
//!
//! For an orientation with column axes `axes` and rough halfspace normal `n_j`,
//! the support is `h_j = max_i (Rot^T n_j) · v_i`. Normals are rotated into the stone
//! frame once per plane, and a dot-product scan over vertices computes the unit support.

use std::collections::BTreeSet;

use glam::DVec3;

use crate::rough_plan::lp::{LpScratch, ScaleRow, max_scale_with_scratch};

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

/// Reusable buffer workspace for evaluating support values and solving scaling LPs.
pub struct SupportWorkspace {
    scale_rows: Vec<ScaleRow>,
    lp_scratch: LpScratch,
}

impl SupportWorkspace {
    /// Creates a new workspace with capacity reserved for `plane_capacity` planes.
    #[must_use]
    pub(crate) fn new(plane_capacity: usize) -> Self {
        Self {
            scale_rows: Vec::with_capacity(plane_capacity),
            lp_scratch: LpScratch::new(),
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

            self.scale_rows.push(ScaleRow {
                normal: [norm_x, norm_y, norm_z],
                support: max_support.max(0.0),
                offset,
            });
        }

        true
    }
}
