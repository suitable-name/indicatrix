//! Unit grid sizing and coordinate mapping for multi-stone planning in shaped roughs.
//!
//! Provides [`ShapedGrid`] over the bounding box of usable rough, sized with caps on
//! total piece table entries and positional DP operations.
//!
//! For example, K = 99 on a 20 mm cube yields a resolution of 16 x 16 x 16 cells,
//! fitting within the 2,600,000 piece table entry cap.

use super::parallel::{even_chunks, run_lanes};
use crate::rough_plan::{
    CandidateDesign, CutOrder, Grid, OP_CAP, PieceTable, PlanProgress, PlanSettings, RoughBlock,
    build_piece_table, choose_grid,
    piece::{Norm, best_pick, usable_rough},
};

/// Maximum grid cells along any single axis in shaped rough planning.
pub const SHAPED_MAX_CELLS: usize = 20;

/// Maximum total piece table entries across all axis ranges (`2,600,000`).
pub const SHAPED_PIECE_CAP: usize = 2_600_000;

/// Maximum total piece table entries for a mesh rough: `325,000`.
///
/// An eighth of [`SHAPED_PIECE_CAP`]. Every entry of a mesh rough's clipped table costs
/// a classification against the whole mesh, so the table is the stage that takes minutes;
/// an eighth of the entries is 11 cells per axis for a cube at K = 10 instead of 16.
pub const SHAPED_MESH_PIECE_CAP: usize = SHAPED_PIECE_CAP / 8;

/// Maximum estimated DP operations summed over all 6 cut orders (`3.0e9`): the plain
/// planner's cap.
pub const SHAPED_OP_CAP: f64 = OP_CAP;

/// The unit grid over the bounding box of usable shaped rough.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedGrid {
    /// Number of grid cells along each canonical axis `[x, y, z]`.
    pub cells: [usize; 3],
    /// Unit cell width along each axis in mm (kerf included).
    pub unit: [f64; 3],
    /// Minimum corner of the usable bounding box in the rough frame in mm: the
    /// corner of the solid's bounding box plus the skin.
    pub origin_mm: [f64; 3],
    /// Usable bounding box extents in mm (the bounding box minus the skin on
    /// both sides).
    pub usable_mm: [f64; 3],
    /// Extents of the solid's bounding box in mm, before the skin.
    pub extents_mm: [f64; 3],
    /// Rough skin trimmed off every outer face, in mm.
    pub skin_mm: f64,
    /// Saw kerf in mm.
    pub kerf_mm: f64,
    /// Preforming and polishing allowance per side in mm.
    pub allowance_mm: f64,
}

impl ShapedGrid {
    /// The rough block that has this grid's bounding-box extents. Its plain
    /// [`Grid`] with the same cells is the size-only table's frame.
    #[must_use]
    pub const fn block(&self) -> RoughBlock {
        RoughBlock {
            x_mm: self.extents_mm[0],
            y_mm: self.extents_mm[1],
            z_mm: self.extents_mm[2],
        }
    }

    /// The size-only piece table of `front` over this grid's cell counts: the
    /// value of an interior piece depends on its size alone, never its position.
    ///
    /// The table is specific to `front`: its design indices point into `front`,
    /// so a caller that changes the design pool must build a new one.
    /// Reports [`PlanProgress::Grid`] once per x-plane; `None` when cancelled.
    pub fn size_table(
        &self,
        settings: &PlanSettings,
        front: &[CandidateDesign],
        on_progress: &mut dyn FnMut(PlanProgress) -> bool,
    ) -> Option<PieceTable> {
        let grid = Grid::with_cells(&self.block(), settings, self.cells);
        build_piece_table(&grid, front, on_progress)
    }

    /// [`Self::size_table`] on `lanes` scoped threads, one run of x-planes per thread.
    ///
    /// Every cell is computed by the same call as in the serial build, and the runs are
    /// concatenated in plane order, so the table is bitwise the serial one whatever the
    /// lane count. Reports [`PlanProgress::Grid`] once per x-plane (`done` is the plane,
    /// 1-based; the events of different lanes interleave) on the calling thread; `None`
    /// when cancelled. With one lane nothing is spawned.
    pub fn size_table_lanes(
        &self,
        settings: &PlanSettings,
        front: &[CandidateDesign],
        lanes: usize,
        on_progress: &mut dyn FnMut(PlanProgress) -> bool,
    ) -> Option<PieceTable> {
        let grid = Grid::with_cells(&self.block(), settings, self.cells);
        let norms: Vec<Norm> = front.iter().map(Norm::of).collect();
        let [nx, ny, nz] = self.cells;
        let parts = run_lanes(
            even_chunks(nx, lanes),
            |planes, report| {
                let cells = planes.len() * ny * nz;
                let mut part = PieceTable {
                    cells: self.cells,
                    values: Vec::with_capacity(cells),
                    design: Vec::with_capacity(cells),
                    orient: Vec::with_capacity(cells),
                };
                for plane in planes {
                    let gx = plane + 1;
                    for gy in 1..=ny {
                        for gz in 1..=nz {
                            let p = grid.stone_box([gx, gy, gz]);
                            let (d, o, v) = best_pick(&norms, p, settings.min_width_mm)
                                .unwrap_or((0, 0, f64::NEG_INFINITY));
                            part.values.push(v);
                            part.design.push(d as u32);
                            part.orient.push(o as u8);
                        }
                    }
                    if !report(PlanProgress::Grid {
                        done: gx,
                        total: nx,
                    }) {
                        return None;
                    }
                }
                Some(part)
            },
            on_progress,
        )?;
        let mut table = PieceTable {
            cells: self.cells,
            values: Vec::with_capacity(nx * ny * nz),
            design: Vec::with_capacity(nx * ny * nz),
            orient: Vec::with_capacity(nx * ny * nz),
        };
        for part in parts {
            table.values.extend(part.values);
            table.design.extend(part.design);
            table.orient.extend(part.orient);
        }
        Some(table)
    }

    /// Number of cell ranges `0 <= start < end <= G` along `axis`.
    #[must_use]
    pub const fn range_count(&self, axis: usize) -> usize {
        let g = self.cells[axis];
        g * (g + 1) / 2
    }

    /// Flat index of range `[start, end)` along `axis` (`0 <= start < end <= G`).
    #[must_use]
    pub const fn range_index(&self, axis: usize, start: usize, end: usize) -> usize {
        let g = self.cells[axis];
        let prev = start * g - start * start.saturating_sub(1) / 2;
        prev + (end - start - 1)
    }

    /// Length in mm of a run of `g` units along `axis`.
    #[must_use]
    pub fn run_mm(&self, axis: usize, g: usize) -> f64 {
        (g as f64).mul_add(self.unit[axis], -self.kerf_mm)
    }

    /// Computes the piece box `(origin_mm, size_mm)` for ranges along axes 0, 1, 2.
    #[must_use]
    pub fn piece_box(&self, ranges: [(usize, usize); 3]) -> ([f64; 3], [f64; 3]) {
        let mut origin = [0.0; 3];
        let mut size = [0.0; 3];
        for i in 0..3 {
            let (start, end) = ranges[i];
            let len = end - start;
            origin[i] = (start as f64).mul_add(self.unit[i], self.origin_mm[i]);
            size[i] = self.run_mm(i, len);
        }
        (origin, size)
    }

    /// Computes the usable stone box `(min_corner, max_corner)` inside piece box.
    #[must_use]
    pub fn stone_box(
        origin_mm: [f64; 3],
        size_mm: [f64; 3],
        allowance_mm: f64,
    ) -> ([f64; 3], [f64; 3]) {
        let mut min_corner = [0.0; 3];
        let mut max_corner = [0.0; 3];
        for i in 0..3 {
            min_corner[i] = origin_mm[i] + allowance_mm;
            max_corner[i] = origin_mm[i] + size_mm[i] - allowance_mm;
        }
        (min_corner, max_corner)
    }
}

/// Computes the total number of piece table entries for given grid cells.
#[must_use]
pub const fn piece_count(cells: [usize; 3]) -> usize {
    let px = cells[0] * (cells[0] + 1) / 2;
    let py = cells[1] * (cells[1] + 1) / 2;
    let pz = cells[2] * (cells[2] + 1) / 2;
    px * py * pz
}

/// Evaluates the positional DP operation estimate summed over all 6 cut orders.
#[must_use]
pub fn shaped_op_estimate(cells: [usize; 3], k: usize) -> f64 {
    let mut total = 0.0;
    for order in CutOrder::ALL {
        let [a, b, c] = order.axes();
        let ga = cells[a];
        let gb = cells[b];
        let gc = cells[c];
        let pa = (ga * (ga + 1) / 2) as f64;
        let pb = (gb * (gb + 1) / 2) as f64;
        let pc = (gc * (gc + 1) / 2) as f64;
        let kb = (k.min(gc)) as f64;
        let ks = (k.min(gb * k.min(gc))) as f64;
        let kr = (k.min(ga * gb * gc)) as f64;
        let bar_cost = pa * pb * pc * kb;
        let slab_cost = (pa * pb * ks).mul_add(kb, bar_cost);
        total += (pa * kr).mul_add(ks, slab_cost);
    }
    total
}

/// Chooses the unit grid for a rough whose bounding box starts at the origin
/// and has `extents`; see [`choose_shaped_grid_at`].
#[must_use]
pub fn choose_shaped_grid(extents: [f64; 3], settings: &PlanSettings) -> ShapedGrid {
    choose_shaped_grid_at([0.0; 3], extents, settings)
}

/// Chooses the unit grid over the bounding box `[min_corner, min_corner + extents]`
/// of the solid rough (the box of the cut solid, not of the uncut base).
///
/// Starts from the cells computed by [`choose_grid`] clamped to [`SHAPED_MAX_CELLS`],
/// then shrinks each axis by 5% until both [`SHAPED_PIECE_CAP`] and [`SHAPED_OP_CAP`] hold.
/// The op estimate uses `kr = min(K, cells)` for the root table, since the root
/// cannot hold more stones than there are cells.
#[must_use]
pub fn choose_shaped_grid_at(
    min_corner: [f64; 3],
    extents: [f64; 3],
    settings: &PlanSettings,
) -> ShapedGrid {
    choose_shaped_grid_capped(min_corner, extents, settings, SHAPED_PIECE_CAP)
}

/// [`choose_shaped_grid_at`] with another piece-table entry cap.
///
/// `piece_cap` replaces [`SHAPED_PIECE_CAP`] (the op cap [`SHAPED_OP_CAP`] is unchanged). With
/// `piece_cap == SHAPED_PIECE_CAP` the result is exactly that of the plain chooser.
#[must_use]
pub fn choose_shaped_grid_capped(
    min_corner: [f64; 3],
    extents: [f64; 3],
    settings: &PlanSettings,
    piece_cap: usize,
) -> ShapedGrid {
    let k = settings.count_usize();
    let block = RoughBlock {
        x_mm: extents[0],
        y_mm: extents[1],
        z_mm: extents[2],
    };
    let base_grid = choose_grid(&block, settings);
    let mut cells = base_grid.cells().map(|g| g.min(SHAPED_MAX_CELLS));

    loop {
        let pieces = piece_count(cells);
        let ops = shaped_op_estimate(cells, k);
        if pieces <= piece_cap && ops <= SHAPED_OP_CAP {
            break;
        }
        let next = cells.map(|g| (g * 19 / 20).max(2));
        if next == cells {
            break;
        }
        cells = next;
    }

    let usable = usable_rough(&block, settings.skin_mm);
    let unit = [0, 1, 2].map(|i| (usable[i] + settings.kerf_mm) / cells[i] as f64);
    let origin_mm = min_corner.map(|c| c + settings.skin_mm);

    ShapedGrid {
        cells,
        unit,
        origin_mm,
        usable_mm: usable,
        extents_mm: extents,
        skin_mm: settings.skin_mm,
        kerf_mm: settings.kerf_mm,
        allowance_mm: settings.allowance_mm,
    }
}
