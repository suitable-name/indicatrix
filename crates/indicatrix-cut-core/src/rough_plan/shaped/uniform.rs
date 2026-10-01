//! Shaped uniform grid layouts for single designs.
//!
//! Evaluates equal-sized cell grids `a * b * c <= K` across the rough's bounding box,
//! clipping individual cells against non-box planes. Every cell stays in the layout's
//! tree: a cell that cannot hold a stone (an exterior corner of a cylinder, say) is
//! merged into its neighbour when the layout is built, never deleted, so the remaining
//! pieces keep the positions their stones were valued at.
//!
//! The geometry of a grid (each cell's stone box and how the planes cut it) does not
//! depend on the design, so it is classified once and shared by every design. A grid
//! is scored against the design's unclipped box value first: a clipped cell never
//! beats it, so grids whose bound cannot reach a design's best three are skipped, and
//! so is a grid whose running bound (the exact value of the cells scored so far plus
//! the box value of each cell still to come) falls to that level. A cell's
//! orientations are tried best bound first, stopping once none can win.
//!
//! Designs are independent, so they can be split over threads; the layouts come back
//! in design order whatever the thread count.

use std::ops::Range;

use glam::DVec3;

use super::{
    clip::{CLASS_EXTERIOR, CLASS_INTERIOR, classify_box_into},
    ctx::ShapedCtx,
    parallel::{Report, even_chunks, run_lanes},
    rows::{ClipRegion, PartialSolver, caliper_extents},
    tree::layout_from_tree_shaped,
};
use crate::rough_plan::{
    CandidateDesign, CutOrder, PlanProgress, PlanSettings, RoughBlock, RoughLayout,
    SHAPED_UNIFORM_DESIGNS,
    piece::{ASSIGNMENTS, Norm, stone_value, usable_rough},
    tree::{Bar, Leaf, Slab, Tree},
    uniform::{PER_DESIGN, cell_size, enumerate_grids},
};

/// The unclipped box value and assignment of each of the six orientations,
/// best first (ties by assignment index).
type BoxBounds = [(f64, usize); ASSIGNMENTS.len()];

/// One uniform cell: its stone box and how the rough's planes cut it.
struct Cell {
    b_min: [f64; 3],
    b_max: [f64; 3],
    class: u8,
    violated: Vec<usize>,
}

/// A grid of equal cells, classified once for all designs.
struct GridGeom {
    counts: [usize; 3],
    size: [f64; 3],
    usable: [f64; 3],
    open_cells: usize,
    /// Cells in `(ix, iy, iz)` order, `iz` fastest.
    cells: Vec<Cell>,
}

/// A design's score on one grid.
struct GridScore {
    feasible: usize,
    total: f64,
    orients: Vec<usize>,
}

/// Classifies the cells of the `counts` grid over the usable box `usable_box`.
fn grid_geometry(
    ctx: &ShapedCtx,
    settings: &PlanSettings,
    usable_box: &[f64; 3],
    counts: [usize; 3],
) -> Option<GridGeom> {
    let kerf = settings.kerf_mm;
    let allowance = settings.allowance_mm;
    let size = cell_size(usable_box, kerf, counts);
    let usable = size.map(|s| 2.0f64.mul_add(-allowance, s).max(0.0));
    if usable.iter().any(|&s| s <= 0.0) {
        return None;
    }

    let mut violated = Vec::new();
    let mut cells = Vec::with_capacity(counts[0] * counts[1] * counts[2]);
    let mut open_cells = 0;
    for ix in 0..counts[0] {
        for iy in 0..counts[1] {
            for iz in 0..counts[2] {
                let index = [ix, iy, iz];
                let origin =
                    [0, 1, 2].map(|i| (index[i] as f64).mul_add(size[i] + kerf, ctx.origin_mm[i]));
                let b_min = origin.map(|o| o + allowance);
                let b_max = [0, 1, 2].map(|i| origin[i] + size[i] - allowance);
                let class = classify_box_into(b_min, b_max, &ctx.non_box, &mut violated);
                open_cells += usize::from(class != CLASS_EXTERIOR);
                cells.push(Cell {
                    b_min,
                    b_max,
                    class,
                    violated: violated.clone(),
                });
            }
        }
    }
    Some(GridGeom {
        counts,
        size,
        usable,
        open_cells,
        cells,
    })
}

/// Evaluates one design's cells.
struct DesignEval<'a> {
    norm: Norm,
    min_width: f64,
    non_box: &'a [(DVec3, f64)],
    solver: PartialSolver,
}

impl DesignEval<'_> {
    /// The box bounds of this design in a cell with usable extents `usable`.
    fn box_bounds(&self, usable: [f64; 3]) -> BoxBounds {
        let mut bounds =
            [0, 1, 2, 3, 4, 5].map(|o| (stone_value(&self.norm, o, usable, self.min_width), o));
        bounds.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        bounds
    }

    /// The best `(value, assignment)` of the design in `cell`; `None` when no
    /// stone of at least the minimum width fits.
    fn cell_value(&mut self, bounds: &BoxBounds, cell: &Cell) -> Option<(f64, usize)> {
        match cell.class {
            CLASS_EXTERIOR => None,
            CLASS_INTERIOR => {
                let (value, orient) = bounds[0];
                (value > 0.0).then_some((value, orient))
            }
            _ => self.clipped_value(bounds, cell),
        }
    }

    /// The best value of a clipped cell: assignments are tried best bound
    /// first, and the search stops once no remaining bound can beat the best.
    fn clipped_value(&mut self, bounds: &BoxBounds, cell: &Cell) -> Option<(f64, usize)> {
        let mut best: Option<(f64, usize)> = None;
        for &(bound, orient) in bounds {
            if bound <= 0.0 || best.is_some_and(|(value, _)| bound < value) {
                break;
            }
            let region = ClipRegion {
                min: cell.b_min,
                max: cell.b_max,
                planes: self.non_box,
                violated: &cell.violated,
            };
            let Some((k, _)) = self
                .solver
                .solve(&region, caliper_extents(&self.norm, orient))
            else {
                continue;
            };
            if k < self.min_width {
                continue;
            }
            let value = self.norm.f * (k * k * k);
            let wins = best.is_none_or(|(v, o)| value > v || (value == v && orient < o));
            if value > 0.0 && wins {
                best = Some((value, orient));
            }
        }
        best
    }

    /// The design's score on `geom`; `None` when no cell holds a stone, or when the
    /// grid cannot score above `floor`.
    ///
    /// After every cell the running bound is the exact value of the cells scored so
    /// far plus the unclipped box value of each open cell still to come (an interior
    /// cell reaches that value exactly, a clipped one at most). Once it is at most
    /// `floor` the grid is dropped without scoring the rest. The total itself is
    /// summed in cell order.
    fn score_grid(&mut self, bounds: &BoxBounds, geom: &GridGeom, floor: f64) -> Option<GridScore> {
        let ceiling = bounds[0].0;
        let mut feasible = 0;
        let mut total = 0.0;
        let mut open_left = geom.open_cells;
        let mut orients = Vec::with_capacity(geom.cells.len());
        for cell in &geom.cells {
            open_left -= usize::from(cell.class != CLASS_EXTERIOR);
            if let Some((value, orient)) = self.cell_value(bounds, cell) {
                feasible += 1;
                total += value;
                orients.push(orient);
            } else {
                orients.push(0);
            }
            if (open_left as f64).mul_add(ceiling, total) <= floor {
                return None;
            }
        }
        (feasible >= 1 && total > 0.0).then_some(GridScore {
            feasible,
            total,
            orients,
        })
    }
}

/// The third largest total among `best_per_n`, or `-inf` with fewer than three.
fn third_best(best_per_n: &[Option<(GridScore, usize)>]) -> f64 {
    let mut top = [f64::NEG_INFINITY; 3];
    for (score, _) in best_per_n.iter().flatten() {
        if score.total > top[2] {
            top[2] = score.total;
            top.sort_by(|a, b| b.total_cmp(a));
        }
    }
    top[2]
}

/// The best grid per feasible-cell count for one design, most valuable first,
/// at most [`PER_DESIGN`] of them: `(score, index into geoms)`.
///
/// `poll` is called before every grid is scored; `None` when it returns `false`.
fn best_grids(
    eval: &mut DesignEval<'_>,
    geoms: &[GridGeom],
    k: usize,
    poll: &mut dyn FnMut() -> bool,
) -> Option<Vec<(GridScore, usize)>> {
    let mut order: Vec<(f64, usize, BoxBounds)> = Vec::with_capacity(geoms.len());
    for (index, geom) in geoms.iter().enumerate() {
        let bounds = eval.box_bounds(geom.usable);
        if bounds[0].0 > 0.0 && geom.open_cells > 0 {
            order.push((geom.open_cells as f64 * bounds[0].0, index, bounds));
        }
    }
    order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut best_per_n: Vec<Option<(GridScore, usize)>> = (0..=k).map(|_| None).collect();
    for (bound_total, index, bounds) in &order {
        let floor = third_best(&best_per_n);
        if *bound_total <= floor {
            break;
        }
        if !poll() {
            return None;
        }
        let Some(score) = eval.score_grid(bounds, &geoms[*index], floor) else {
            continue;
        };
        let slot = &mut best_per_n[score.feasible.min(k)];
        if slot
            .as_ref()
            .is_none_or(|(kept, _)| score.total > kept.total)
        {
            *slot = Some((score, *index));
        }
    }

    let mut found: Vec<(GridScore, usize)> = best_per_n.into_iter().flatten().collect();
    found.sort_by(|a, b| b.0.total.total_cmp(&a.0.total));
    found.truncate(PER_DESIGN);
    Some(found)
}

/// The tree of an equal-cell grid of design `d_idx`, one leaf per cell.
fn grid_tree(geom: &GridGeom, d_idx: usize, orients: &[usize]) -> Tree {
    let [cx, cy, cz] = geom.counts;
    let mut cell_orients = orients.iter().copied();
    let mut slabs = Vec::with_capacity(cx);
    for _ in 0..cx {
        let mut bars = Vec::with_capacity(cy);
        for _ in 0..cy {
            let leaves = (0..cz)
                .map(|_| Leaf {
                    len: geom.size[2],
                    design: d_idx,
                    orient: cell_orients.next().unwrap_or(0),
                })
                .collect();
            bars.push(Bar {
                width: geom.size[1],
                leaves,
            });
        }
        slabs.push(Slab {
            thickness: geom.size[0],
            bars,
        });
    }
    Tree { slabs }
}

/// What every worker of the uniform pass reads.
struct UniformPass<'a> {
    ctx: &'a ShapedCtx,
    settings: &'a PlanSettings,
    geoms: &'a [GridGeom],
    /// The shortlisted designs; a leaf's design index points into it.
    pool: &'a [CandidateDesign],
}

impl UniformPass<'_> {
    /// The layouts of the designs `chunk` of the pool, in design order.
    ///
    /// Every design gets its own evaluator and solver. `report` hears
    /// `Uniform { done: d_idx, .. }` before each grid and at each design's start; `None`
    /// when it returns `false`.
    fn layouts_of(&self, chunk: Range<usize>, report: &mut Report<'_>) -> Option<Vec<RoughLayout>> {
        let k = self.settings.count_usize();
        let total = self.pool.len();
        let mut layouts = Vec::new();
        for d_idx in chunk {
            let mut eval = DesignEval {
                norm: Norm::of(&self.pool[d_idx]),
                min_width: self.settings.min_width_mm,
                non_box: &self.ctx.non_box,
                solver: PartialSolver::new(),
            };
            let mut poll = || report(PlanProgress::Uniform { done: d_idx, total });
            if !poll() {
                return None;
            }
            for (score, index) in best_grids(&mut eval, self.geoms, k, &mut poll)? {
                let tree = grid_tree(&self.geoms[index], d_idx, &score.orients);
                let layout = layout_from_tree_shaped(
                    self.ctx,
                    CutOrder::Xyz,
                    &tree,
                    self.pool,
                    self.settings,
                );
                if layout.stone_count() > 0 {
                    layouts.push(layout);
                }
            }
        }
        Some(layouts)
    }
}

/// Generates shaped uniform layouts across shortlisted designs.
///
/// The shortlist is `designs` deduplicated by `entry_id` and capped at
/// [`SHAPED_UNIFORM_DESIGNS`] (the caller puts the Pareto front and the best
/// single-stone fits there). For each design and each stone count the best `a x b x c`
/// grid is kept, then the best three counts per design. A layout may hold fewer stones
/// than its grid has cells: a cell without a stone is merged into a neighbour.
///
/// The designs are split over `lanes` scoped threads (`1` spawns nothing); the layouts
/// come back in design order and are bitwise the same for any lane count. Reports
/// `Uniform { done, total }` on the calling thread, `done` counting the designs reached
/// so far; `None` when `on_progress` cancels, which every lane notices before its next
/// grid.
#[must_use]
pub fn shaped_uniform_layouts(
    ctx: &ShapedCtx,
    designs: &[CandidateDesign],
    settings: &PlanSettings,
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    let k = settings.count_usize();
    let block = RoughBlock {
        x_mm: ctx.bbox_extents[0],
        y_mm: ctx.bbox_extents[1],
        z_mm: ctx.bbox_extents[2],
    };
    let usable_box = usable_rough(&block, settings.skin_mm);
    let geoms: Vec<GridGeom> = enumerate_grids(k)
        .into_iter()
        .filter_map(|counts| grid_geometry(ctx, settings, &usable_box, counts))
        .collect();

    let mut shortlisted: Vec<CandidateDesign> = Vec::new();
    for d in designs {
        if !shortlisted.iter().any(|s| s.entry_id == d.entry_id) {
            shortlisted.push(*d);
            if shortlisted.len() >= SHAPED_UNIFORM_DESIGNS {
                break;
            }
        }
    }

    let pass = UniformPass {
        ctx,
        settings,
        geoms: &geoms,
        pool: &shortlisted,
    };
    // A lane names the design it is on; the count of distinct designs named is the
    // progress, and it only ever grows.
    let mut reached = vec![false; shortlisted.len()];
    let mut count = 0_usize;
    let mut forward = |event: PlanProgress| match event {
        PlanProgress::Uniform { done, total } => {
            if let Some(seen) = reached.get_mut(done)
                && !*seen
            {
                *seen = true;
                count += 1;
            }
            on_progress(PlanProgress::Uniform {
                done: count.saturating_sub(1),
                total,
            })
        }
        other => on_progress(other),
    };
    let parts = run_lanes(
        even_chunks(shortlisted.len(), lanes),
        |chunk, report| pass.layouts_of(chunk, report),
        &mut forward,
    )?;
    Some(parts.into_iter().flatten().collect())
}
