//! The value of one stone in one piece, the unit grid, and the shared piece
//! table (`P[gx][gy][gz]`).
//!
//! A design is normalised to `(1, l = L/W, h = H/W, f = V/W^3)`. In a piece
//! with usable box `p` and an axis assignment `pi` (which piece axis takes the
//! design's W, L and H), the finished width in mm is
//! `s = min(p[pi(W)], p[pi(L)] / l, p[pi(H)] / h)` and the finished volume is
//! `f * s^3`. Every consumer (the table, the uniform pass, the refinement, the
//! layout builder) goes through [`stone_scale`]/[`stone_value`], so their
//! numbers agree bit for bit.

use super::{
    OP_CAP,
    types::{CandidateDesign, PlanProgress, PlanSettings, RoughBlock},
};

/// The six ways a design's `(W, L, H)` map onto a piece's axes: entry `j` of
/// an assignment is the piece axis that design dimension `j` runs along.
pub const ASSIGNMENTS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [1, 0, 2],
    [0, 2, 1],
    [2, 0, 1],
    [1, 2, 0],
    [2, 1, 0],
];

/// Relative slack of the minimum-width test: a stone counts as wide enough when its width is
/// at least `min_width * (1 - MIN_WIDTH_SLACK)`.
///
/// The table, the uniform pass and the refinement reach the same physical width through
/// different expressions (`g * unit - kerf - 2a`, `(r + kerf) / n - kerf - 2a`,
/// `x - 2a`), which can differ in the last bits. Accepting that much noise keeps a stone
/// exactly at the minimum from being feasible on one path and infeasible on another.
pub const MIN_WIDTH_SLACK: f64 = 1e-12;

/// The smallest finished width [`stone_value`] accepts for the minimum width `min_width`.
pub const fn min_width_floor(min_width: f64) -> f64 {
    min_width * (1.0 - MIN_WIDTH_SLACK)
}

/// A design normalised by its width.
#[derive(Debug, Clone, Copy)]
pub struct Norm {
    /// `L / W`.
    pub l: f64,
    /// `H / W`.
    pub h: f64,
    /// `V / W^3`.
    pub f: f64,
}

impl Norm {
    /// Normalise `design` (its `width` must be positive).
    pub(crate) fn of(design: &CandidateDesign) -> Self {
        let w = design.width;
        Self {
            l: design.length / w,
            h: design.height / w,
            f: design.volume / (w * w * w),
        }
    }

    /// The design's dimensions in units of its width: `[1, l, h]`.
    pub(crate) const fn dims(&self) -> [f64; 3] {
        [1.0, self.l, self.h]
    }
}

/// The finished width in mm of a stone in the usable box `p` (canonical axes)
/// under assignment `orient`.
///
/// # Panics
///
/// When `orient` is not an index into [`ASSIGNMENTS`] (`0..6`).
pub fn stone_scale(norm: &Norm, orient: usize, p: [f64; 3]) -> f64 {
    let a = &ASSIGNMENTS[orient];
    p[a[0]].min(p[a[1]] / norm.l).min(p[a[2]] / norm.h)
}

/// The finished volume in mm^3, or `-inf` when the stone is infeasible
/// (non-positive box or narrower than `min_width`, to within
/// [`MIN_WIDTH_SLACK`] relative).
///
/// # Panics
///
/// When `orient` is not an index into [`ASSIGNMENTS`] (`0..6`).
pub fn stone_value(norm: &Norm, orient: usize, p: [f64; 3], min_width: f64) -> f64 {
    let s = stone_scale(norm, orient, p);
    if s > 0.0 && s >= min_width_floor(min_width) {
        norm.f * (s * s * s)
    } else {
        f64::NEG_INFINITY
    }
}

/// The best `(design, orientation, value)` over `norms` (ascending index) and
/// the six assignments; a later candidate replaces only on a strictly greater
/// value. `None` when nothing is feasible.
pub fn best_pick(norms: &[Norm], p: [f64; 3], min_width: f64) -> Option<(usize, usize, f64)> {
    let mut best: Option<(usize, usize, f64)> = None;
    for (index, norm) in norms.iter().enumerate() {
        for orient in 0..ASSIGNMENTS.len() {
            let value = stone_value(norm, orient, p, min_width);
            if value > f64::NEG_INFINITY && best.is_none_or(|(_, _, b)| value > b) {
                best = Some((index, orient, value));
            }
        }
    }
    best
}

/// The unit grid the DP cuts on: per axis `G_i` units of `u_i` mm.
///
/// A run of `g` units has usable length `g * u_i - kerf`; `n` runs covering
/// all `G_i` units total `R'_i - (n - 1) * kerf`, the exact kerf accounting,
/// whichever saw stage cuts the axis.
#[derive(Debug, Clone)]
pub struct Grid {
    /// The rough.
    pub(crate) rough: RoughBlock,
    /// The planner settings.
    pub(crate) settings: PlanSettings,
    /// Units per axis.
    pub(crate) cells: [usize; 3],
    /// Unit length per axis, in mm.
    pub(crate) unit: [f64; 3],
}

impl Grid {
    /// A grid with explicit `cells` per axis (at most 255 each).
    pub(crate) fn with_cells(
        rough: &RoughBlock,
        settings: &PlanSettings,
        cells: [usize; 3],
    ) -> Self {
        debug_assert!(
            cells.iter().all(|&c| (1..=255).contains(&c)),
            "grid cells must be in 1..=255 (the DP stores them as u8): {cells:?}"
        );
        let usable = usable_rough(rough, settings.skin_mm);
        let unit = [0, 1, 2].map(|i| (usable[i] + settings.kerf_mm) / cells[i] as f64);
        Self {
            rough: *rough,
            settings: *settings,
            cells,
            unit,
        }
    }

    /// Units per axis `[x, y, z]`.
    #[must_use]
    pub const fn cells(&self) -> [usize; 3] {
        self.cells
    }

    /// The stone-count cap the grid was sized for.
    #[must_use]
    pub fn count(&self) -> usize {
        self.settings.count_usize()
    }

    /// Length in mm of a run of `g` units along `axis`.
    pub(crate) fn run_mm(&self, axis: usize, g: usize) -> f64 {
        (g as f64).mul_add(self.unit[axis], -self.settings.kerf_mm)
    }

    /// The usable stone box of a piece `(gx, gy, gz)` units in size.
    pub(crate) fn stone_box(&self, g: [usize; 3]) -> [f64; 3] {
        let a2 = 2.0 * self.settings.allowance_mm;
        [0, 1, 2].map(|i| self.run_mm(i, g[i]) - a2)
    }
}

/// The rough minus the skin on both faces of every axis (never negative).
pub fn usable_rough(rough: &RoughBlock, skin_mm: f64) -> [f64; 3] {
    rough.sizes().map(|r| 2.0f64.mul_add(-skin_mm, r).max(0.0))
}

/// The smallest integer `c` with `c^3 >= k`.
const fn ceil_cube_root(k: usize) -> usize {
    let mut c = 1;
    while c * c * c < k {
        c += 1;
    }
    c
}

/// A deterministic cube root by bisection (no libm).
fn cube_root(x: f64) -> f64 {
    let (mut lo, mut hi) = (0.0f64, x.max(1.0));
    for _ in 0..200 {
        let mid = f64::midpoint(lo, hi);
        if mid * mid * mid <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The DP operation estimate summed over the six cut orders.
fn op_estimate(cells: [usize; 3], k: usize) -> f64 {
    let kf = k as f64;
    let mut total = 0.0;
    for order in super::types::CutOrder::ALL {
        let [a, b, c] = order.axes().map(|i| cells[i] as f64);
        total += (a * b * c * c).mul_add(kf, (a * b * b).mul_add(kf * kf, a * a * kf * kf));
    }
    total
}

/// Choose the unit grid: about six units per expected piece, `8..=48` per
/// axis, shrunk until the DP operation estimate is at most `3e9`.
///
/// The shrink works in 5 % steps on all axes together and may take an axis
/// below the 8-unit floor (never below 2); e.g. a 100 x 100 x 2 mm rough at
/// K = 99 goes from `[48, 48, 8]` to `[45, 45, 7]`.
#[must_use]
pub fn choose_grid(rough: &RoughBlock, settings: &PlanSettings) -> Grid {
    let k = settings.count_usize();
    let usable = usable_rough(rough, settings.skin_mm);
    let geomean = cube_root(usable[0] * usable[1] * usable[2]);
    let per_axis = (6 * ceil_cube_root(k)) as f64;
    let mut cells = if geomean > 0.0 {
        usable.map(|r| ((per_axis * r / geomean).round() as usize).clamp(8, 48))
    } else {
        [8usize; 3]
    };
    loop {
        let next = cells.map(|g| (g * 19 / 20).max(2));
        if op_estimate(cells, k) <= OP_CAP || next == cells {
            break;
        }
        cells = next;
    }
    Grid::with_cells(rough, settings, cells)
}

/// The best stone value per piece size, shared read-only by all cut orders.
///
/// Holds, for every `(gx, gy, gz)` in `1..=G`, the best value over the front
/// and the six assignments (`-inf` = infeasible) and its argmax: the front
/// index and the assignment.
#[derive(Debug, Clone)]
pub struct PieceTable {
    /// Units per axis.
    pub(crate) cells: [usize; 3],
    /// Best value per cell.
    pub(crate) values: Vec<f64>,
    /// Front index of the best design per cell.
    pub(crate) design: Vec<u32>,
    /// Best assignment per cell.
    pub(crate) orient: Vec<u8>,
}

impl PieceTable {
    /// Flat-index strides of the three axes.
    pub(crate) const fn strides(&self) -> [usize; 3] {
        [self.cells[1] * self.cells[2], self.cells[2], 1]
    }

    /// Flat index of the 1-based cell `g`.
    pub(crate) const fn index(&self, g: [usize; 3]) -> usize {
        let s = self.strides();
        (g[0] - 1) * s[0] + (g[1] - 1) * s[1] + (g[2] - 1) * s[2]
    }
}

/// Fill the piece table over `front` (which must be sorted by `entry_id`, as
/// [`super::pareto_front`] returns it). Reports [`PlanProgress::Grid`] once per
/// x-plane; `None` when `on_progress` returns `false`.
pub fn build_piece_table(
    grid: &Grid,
    front: &[CandidateDesign],
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<PieceTable> {
    let norms: Vec<Norm> = front.iter().map(Norm::of).collect();
    let [nx, ny, nz] = grid.cells;
    let total = nx * ny * nz;
    let mut table = PieceTable {
        cells: grid.cells,
        values: Vec::with_capacity(total),
        design: Vec::with_capacity(total),
        orient: Vec::with_capacity(total),
    };
    for gx in 1..=nx {
        for gy in 1..=ny {
            for gz in 1..=nz {
                let p = grid.stone_box([gx, gy, gz]);
                let (d, o, v) = best_pick(&norms, p, grid.settings.min_width_mm).unwrap_or((
                    0,
                    0,
                    f64::NEG_INFINITY,
                ));
                table.values.push(v);
                table.design.push(d as u32);
                table.orient.push(o as u8);
            }
        }
        if !on_progress(PlanProgress::Grid {
            done: gx,
            total: nx,
        }) {
            return None;
        }
    }
    Some(table)
}
