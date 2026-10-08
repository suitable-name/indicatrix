//! Single-design layouts: `n` equal cells of one design in an `a x b x c` grid.
//!
//! Every grid with `1 <= a*b*c <= K` is enumerated on all three axis
//! assignments (each ordered triple of counts), fully filled: `n = a*b*c`
//! stones and no waste cells. Continuous, no unit-grid error. The best layout
//! per `(design, n)` is kept, then only the best three per design, so no
//! single design can flood the ranking.

use super::{
    pareto::sanitize,
    piece::{ASSIGNMENTS, Norm, stone_value, usable_rough},
    shaped::parallel::{even_chunks, run_lanes_dynamic},
    tree::{Bar, Leaf, Slab, Tree, layout_from_tree},
    types::{CandidateDesign, CutOrder, PlanProgress, PlanSettings, RoughBlock, RoughLayout},
};

/// Layouts kept per design.
pub const PER_DESIGN: usize = 3;
/// Designs scanned between two progress reports (and cancel checks).
const POLL_EVERY: usize = 256;
/// Chunks of designs per lane in the threaded scan; more chunks than lanes let a lane that
/// draws the cheap designs take more of them.
const CHUNKS_PER_LANE: usize = 4;
/// Layouts returned in all (a pool for the refinement, well beyond the top 10).
const KEEP: usize = 200;

/// One candidate: a design filled into a grid.
#[derive(Debug, Clone, Copy)]
struct Cand {
    /// Total finished volume (`n` times one stone).
    total: f64,
    /// Stones in the layout.
    n: usize,
    /// Index into the sanitised design list.
    design: usize,
    /// Cells per rough axis.
    counts: [usize; 3],
    /// Assignment of the design to the cell.
    orient: usize,
}

/// Every ordered triple of counts with product at most `k`, in a fixed order.
pub fn enumerate_grids(k: usize) -> Vec<[usize; 3]> {
    let mut grids = Vec::new();
    for cx in 1..=k {
        for cy in 1..=k / cx {
            for cz in 1..=k / (cx * cy) {
                grids.push([cx, cy, cz]);
            }
        }
    }
    grids
}

/// The cell size (piece) per axis for `counts` cells, kerf included between.
pub fn cell_size(usable: &[f64; 3], kerf: f64, counts: [usize; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| (usable[i] + kerf) / counts[i] as f64 - kerf)
}

/// The best assignment of a design in a cell box (strictly greater replaces).
fn best_orient(norm: &Norm, p: [f64; 3], min_width: f64) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for orient in 0..ASSIGNMENTS.len() {
        let value = stone_value(norm, orient, p, min_width);
        if value > f64::NEG_INFINITY && best.is_none_or(|(_, b)| value > b) {
            best = Some((orient, value));
        }
    }
    best
}

/// The best layouts of one design: per stone count the best grid, then the top
/// [`PER_DESIGN`] by total volume (fewer stones first on a tie).
fn best_for_design(
    design: usize,
    norm: &Norm,
    grids: &[[usize; 3]],
    boxes: &[[f64; 3]],
    settings: &PlanSettings,
) -> Vec<Cand> {
    let k = settings.count_usize();
    let mut slots: Vec<Option<Cand>> = vec![None; k + 1];
    for (counts, p) in grids.iter().zip(boxes) {
        let Some((orient, value)) = best_orient(norm, *p, settings.min_width_mm) else {
            continue;
        };
        let n = counts[0] * counts[1] * counts[2];
        let total = n as f64 * value;
        if slots[n].is_none_or(|c| total > c.total) {
            slots[n] = Some(Cand {
                total,
                n,
                design,
                counts: *counts,
                orient,
            });
        }
    }
    let mut found: Vec<Cand> = slots.into_iter().flatten().collect();
    found.sort_by(|a, b| b.total.total_cmp(&a.total).then(a.n.cmp(&b.n)));
    found.truncate(PER_DESIGN);
    found
}

/// The equal-cell layout of `cand`. Axes with more than one cell are cut
/// first (in x, y, z order), so the saw makes no needless stage.
fn build_layout(
    rough: &RoughBlock,
    settings: &PlanSettings,
    design: &CandidateDesign,
    cand: &Cand,
) -> RoughLayout {
    let usable = usable_rough(rough, settings.skin_mm);
    let size = cell_size(&usable, settings.kerf_mm, cand.counts);
    let mut axes = [0usize, 1, 2];
    axes.sort_by_key(|&i| (cand.counts[i] == 1, i));
    let order = CutOrder::from_axes(axes);
    let leaf = Leaf {
        len: size[axes[2]],
        design: 0,
        orient: cand.orient,
    };
    let bar = Bar {
        width: size[axes[1]],
        leaves: vec![leaf; cand.counts[axes[2]]],
    };
    let slab = Slab {
        thickness: size[axes[0]],
        bars: vec![bar; cand.counts[axes[1]]],
    };
    let tree = Tree {
        slabs: vec![slab; cand.counts[axes[0]]],
    };
    layout_from_tree(rough, settings, order, &tree, std::slice::from_ref(design))
}

/// The best equal-cell layout per `(design, n)`, at most three per design.
///
/// Considers EVERY valid candidate (not only the Pareto front) and every grid
/// with `1 <= a*b*c <= K`. The result is sorted by total volume descending,
/// then fewer stones, then `entry_id`, and cut to the best 200 layouts (a pool
/// for the refinement, well beyond the final top 10).
///
/// Each design is scanned over about a thousand grids and six assignments, so the scan
/// reports [`PlanProgress::Uniform`] (`done` of `total` designs) before every 256th design
/// and once more when it is complete. `None` when `on_progress` returns `false`.
pub fn uniform_layouts(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    uniform_layouts_lanes(rough, settings, designs, 1, on_progress)
}

/// [`uniform_layouts`] with the designs split over `lanes` scoped threads.
///
/// The designs are cut into consecutive chunks (four per lane) that the lanes pull in
/// ascending order; the candidates are concatenated in design order before the one sort, so
/// the layouts are bitwise those of the serial scan whatever the lane count. A lane reports
/// `Uniform { done, total }` with `done` the index of the design it is on, before every 256th
/// design and at the start of each chunk; the calling thread forwards the largest `done`
/// seen so far, so the progress never goes back. `None` when `on_progress` returns `false`,
/// which every lane notices at its next report. With one lane nothing is spawned and the
/// events are those of the serial scan.
pub fn uniform_layouts_lanes(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    let pool = sanitize(designs);
    let grids = enumerate_grids(settings.count_usize());
    let usable = usable_rough(rough, settings.skin_mm);
    let a2 = 2.0 * settings.allowance_mm;
    let boxes: Vec<[f64; 3]> = grids
        .iter()
        .map(|&c| cell_size(&usable, settings.kerf_mm, c).map(|s| s - a2))
        .collect();
    let total = pool.len();
    let chunks = even_chunks(
        total,
        if lanes <= 1 {
            1
        } else {
            lanes * CHUNKS_PER_LANE
        },
    );
    // Lanes name the design they are on; only the furthest one is forwarded.
    let mut reached = 0_usize;
    let mut forward = |event: PlanProgress| match event {
        PlanProgress::Uniform { done, total } => {
            reached = reached.max(done);
            on_progress(PlanProgress::Uniform {
                done: reached,
                total,
            })
        }
        other => on_progress(other),
    };
    let parts = run_lanes_dynamic(
        chunks,
        lanes,
        |chunk, report| {
            let mut found: Vec<Cand> = Vec::new();
            for i in chunk.clone() {
                if (i == chunk.start || i.is_multiple_of(POLL_EVERY))
                    && !report(PlanProgress::Uniform { done: i, total })
                {
                    return None;
                }
                found.extend(best_for_design(
                    i,
                    &Norm::of(&pool[i]),
                    &grids,
                    &boxes,
                    settings,
                ));
            }
            Some(found)
        },
        &mut forward,
    )?;
    let mut cands: Vec<Cand> = parts.into_iter().flatten().collect();
    if !on_progress(PlanProgress::Uniform { done: total, total }) {
        return None;
    }
    cands.sort_by(|a, b| {
        b.total
            .total_cmp(&a.total)
            .then(a.n.cmp(&b.n))
            .then(pool[a.design].entry_id.cmp(&pool[b.design].entry_id))
    });
    cands.truncate(KEEP);
    Some(
        cands
            .iter()
            .map(|c| build_layout(rough, settings, &pool[c.design], c))
            .collect(),
    )
}
