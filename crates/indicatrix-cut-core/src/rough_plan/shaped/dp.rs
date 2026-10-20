//! Positional dynamic programming for multi-stone planning in shaped roughs.
//!
//! Solves the 3-stage guillotine cut sequence (slabs across stage A, bars across stage B,
//! pieces across stage C) where each piece's value depends on its spatial bounds.
//!
//! # The number of pieces
//!
//! The number of pieces is free unless `PlanSettings::max_stones` caps it: every order's DP
//! places as many stones as the rough, the losses and the minimum width allow. A stone cap the
//! free optimum's layout already respects (counted in stones built, see
//! [`plan_shaped_for_order`]) changes nothing; one that binds is solved by the count DP of
//! `dp_capped`, the positional twin of this DP with a stone budget in every table. The design
//! cap is applied outside the DP (see [`plan_shaped_design_cap`]).
//!
//! # Memory
//!
//! Only the argmax run lengths are stored, one byte each. Per cut order the bar table
//! holds `p_a * p_b * (G_C + 1)` bytes (`p_x = G_x (G_x + 1) / 2` range counts), the slab
//! table `p_a * (G_B + 1)` and the root table `G_A + 1`. The value tables are `f64` and
//! hold one value per range.

use super::{
    clip::{BuildClipParams, ClippedTable, build_clipped_table_jobs},
    ctx::ShapedCtx,
    dp_capped::solve_order_capped,
    grid::ShapedGrid,
    tree::layout_from_tree_shaped,
};
use crate::rough_plan::{
    CandidateDesign, CutOrder, LayoutGroup, NEG, PlanProgress, PlanSettings, RoughLayout,
    dp::ALTERNATIVE_ROUNDS,
    pareto::pareto_front,
    rank::{best_layout, least_contributing_design, most_used_design, pool_without},
    tree::{Bar, Leaf, Slab, Tree},
};

/// Reports one unit of progress; returns `false` to cancel.
pub(super) type Tick<'a> = dyn FnMut() -> bool + 'a;

/// Solved positional DP for one cut order.
pub struct ShapedOrderDp {
    order: CutOrder,
    ga: usize,
    gb: usize,
    gc: usize,
    /// Bar argmax run length per `(slab range, bar range, c)`.
    bar_arg: Vec<u8>,
    /// Slab argmax run length per `(slab range, b)`.
    slab_arg: Vec<u8>,
    /// Root argmax run length per `a`.
    root_arg: Vec<u8>,
    /// The best total value (`-inf` when nothing fits).
    root_value: f64,
}

impl ShapedOrderDp {
    /// The best total value of this order (`-inf` when no layout fits).
    #[must_use]
    pub const fn root_value(&self) -> f64 {
        self.root_value
    }

    /// Reconstructs the guillotine tree of the best layout.
    #[must_use]
    pub fn reconstruct(&self, grid: &ShapedGrid, table: &ClippedTable) -> Tree {
        let [ax, bx, cx] = self.order.axes();
        let pb = grid.range_count(bx);
        let mut slabs = Vec::new();
        let mut a = self.ga;
        while a > 0 {
            let g_a = usize::from(self.root_arg[a]);
            if g_a == 0 || g_a > a {
                break;
            }
            let a_prime = a - g_a;
            let slab_rng = grid.range_index(ax, a_prime, a);

            let mut bars = Vec::new();
            let mut b = self.gb;
            while b > 0 {
                let g_b = usize::from(self.slab_arg[slab_rng * (self.gb + 1) + b]);
                if g_b == 0 || g_b > b {
                    break;
                }
                let b_prime = b - g_b;
                let bar_rng = grid.range_index(bx, b_prime, b);

                let mut leaves = Vec::new();
                let mut c = self.gc;
                let bar_base = (slab_rng * pb + bar_rng) * (self.gc + 1);
                while c > 0 {
                    let g_c = usize::from(self.bar_arg[bar_base + c]);
                    if g_c == 0 || g_c > c {
                        break;
                    }
                    let c_prime = c - g_c;
                    let mut r_canon = [0; 3];
                    r_canon[ax] = slab_rng;
                    r_canon[bx] = bar_rng;
                    r_canon[cx] = grid.range_index(cx, c_prime, c);
                    let t_idx = table.index(r_canon[0], r_canon[1], r_canon[2]);
                    leaves.push(Leaf {
                        len: grid.run_mm(cx, g_c),
                        design: table.design[t_idx] as usize,
                        orient: table.orient[t_idx] as usize,
                    });
                    c = c_prime;
                }
                leaves.reverse();
                bars.push(Bar {
                    width: grid.run_mm(bx, g_b),
                    leaves,
                });
                b = b_prime;
            }
            bars.reverse();
            slabs.push(Slab {
                thickness: grid.run_mm(ax, g_a),
                bars,
            });
            a = a_prime;
        }
        slabs.reverse();
        Tree { slabs }
    }

    /// This order's candidate: the best layout, built in full, or none when nothing fits.
    #[must_use]
    pub fn candidates(
        &self,
        grid: &ShapedGrid,
        table: &ClippedTable,
        ctx: &ShapedCtx,
        settings: &PlanSettings,
        pool: &[CandidateDesign],
    ) -> Vec<RoughLayout> {
        if self.root_value <= NEG {
            return Vec::new();
        }
        let tree = self.reconstruct(grid, table);
        candidate_of_tree(ctx, self.order, &tree, pool, settings)
    }
}

/// The candidate of a cut order built from its best `tree`: the one layout, built in full, or
/// none when no piece holds a stone.
pub(super) fn candidate_of_tree(
    ctx: &ShapedCtx,
    order: CutOrder,
    tree: &Tree,
    pool: &[CandidateDesign],
    settings: &PlanSettings,
) -> Vec<RoughLayout> {
    let layout = layout_from_tree_shaped(ctx, order, tree, pool, settings);
    if layout.stones.is_empty() {
        Vec::new()
    } else {
        vec![layout]
    }
}

/// A stored argmax (a run length) as one byte.
fn arg_byte(value: usize) -> u8 {
    u8::try_from(value).expect("a run is at most 20 cells")
}

/// The best split of a run of `units` cells into runs, given the value of the run between
/// `start` and `end` (`run_value(start, end)`, `-inf` when unusable): fills
/// `best[0..=units]` and `arg`. The longest run is tried first, so it is kept on a tie: of two
/// splits worth the same, the one with fewer pieces wins, as in the ranking.
fn split_runs(
    units: usize,
    best: &mut [f64],
    arg: &mut [u8],
    run_value: impl Fn(usize, usize) -> f64,
) {
    best[0] = 0.0;
    arg[0] = 0;
    for u in 1..=units {
        let (mut top, mut top_g) = (NEG, 0);
        for g in (1..=u).rev() {
            let prev = best[u - g];
            if prev > NEG {
                let v = run_value(u - g, u);
                if v > NEG && prev + v > top {
                    top = prev + v;
                    top_g = g;
                }
            }
        }
        best[u] = top;
        arg[u] = arg_byte(top_g);
    }
}

/// Runs the three DP levels of `order`; `None` when `tick` cancels. `tick` is called once
/// per starting plane of the a range in the bar fill and once per starting plane in the slab
/// fill (two calls per cell of the a axis).
pub(super) fn solve_order(
    grid: &ShapedGrid,
    table: &ClippedTable,
    order: CutOrder,
    tick: &mut Tick<'_>,
) -> Option<ShapedOrderDp> {
    let [ax, bx, cx] = order.axes();
    let (ga, gb, gc) = (grid.cells[ax], grid.cells[bx], grid.cells[cx]);
    let (pa, pb) = (grid.range_count(ax), grid.range_count(bx));

    // Bars: for every (slab range, bar range), the best split of the c axis.
    let mut bar_arg = vec![0u8; pa * pb * (gc + 1)];
    let mut bar_vals = vec![NEG; pa * pb];
    let mut t_buf = vec![NEG; gc + 1];
    for ra_start in 0..ga {
        if !tick() {
            return None;
        }
        for ra_end in (ra_start + 1)..=ga {
            let slab_rng = grid.range_index(ax, ra_start, ra_end);
            for rb_start in 0..gb {
                for rb_end in (rb_start + 1)..=gb {
                    let bar_rng = grid.range_index(bx, rb_start, rb_end);
                    let pair = slab_rng * pb + bar_rng;
                    let args = &mut bar_arg[pair * (gc + 1)..(pair + 1) * (gc + 1)];
                    split_runs(gc, &mut t_buf, args, |c0, c1| {
                        let mut r_canon = [0; 3];
                        r_canon[ax] = slab_rng;
                        r_canon[bx] = bar_rng;
                        r_canon[cx] = grid.range_index(cx, c0, c1);
                        table.value(r_canon[0], r_canon[1], r_canon[2])
                    });
                    bar_vals[pair] = t_buf[gc];
                }
            }
        }
    }

    // Slabs: for every slab range, the best split of the b axis into bars.
    let mut slab_arg = vec![0u8; pa * (gb + 1)];
    let mut slab_vals = vec![NEG; pa];
    let mut s_buf = vec![NEG; gb + 1];
    for ra_start in 0..ga {
        if !tick() {
            return None;
        }
        for ra_end in (ra_start + 1)..=ga {
            let slab_rng = grid.range_index(ax, ra_start, ra_end);
            let args = &mut slab_arg[slab_rng * (gb + 1)..(slab_rng + 1) * (gb + 1)];
            split_runs(gb, &mut s_buf, args, |b0, b1| {
                bar_vals[slab_rng * pb + grid.range_index(bx, b0, b1)]
            });
            slab_vals[slab_rng] = s_buf[gb];
        }
    }

    // Root: the best split of the a axis into slabs.
    let mut root_arg = vec![0u8; ga + 1];
    let mut root_buf = vec![NEG; ga + 1];
    split_runs(ga, &mut root_buf, &mut root_arg, |a0, a1| {
        slab_vals[grid.range_index(ax, a0, a1)]
    });
    Some(ShapedOrderDp {
        order,
        ga,
        gb,
        gc,
        bar_arg,
        slab_arg,
        root_arg,
        root_value: root_buf[ga],
    })
}

/// Solves positional DP for a given cut order.
///
/// Reports [`PlanProgress::Dp`] once per starting plane in the bar and slab
/// fills, so a cancel is seen within a fraction of the order's work.
///
/// The stone cap is `settings.max_stones`. Without one this is exactly the free DP. With one,
/// the free DP runs first and its layout is the answer whenever it already holds at most that
/// many stones (no tree of at most N pieces is worth more than the free optimum, so it IS the
/// capped optimum). The test is made on the built layout: a tree has at least as many pieces
/// as the layout has stones, because a piece the fitter leaves without a stone is merged into
/// a neighbour when the layout is built, so a tree of more pieces than the cap can still build
/// a layout within it (the table valued a piece the fitter rejects, which a mesh rough's
/// heuristic solver can do). Otherwise the count DP of the cap runs on the same table (a
/// second pass, which reports the same events again) and the answer is the best layout of at
/// most that many stones, not a larger one cut down. That DP limits the pieces of a tree, not
/// the stones built from it (see `dp_capped`).
///
/// # Returns
///
/// `None` if cancelled via `on_progress`.
pub fn plan_shaped_for_order(
    grid: &ShapedGrid,
    table: &ClippedTable,
    ctx: &ShapedCtx,
    pool: &[CandidateDesign],
    order: CutOrder,
    settings: &PlanSettings,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    let order_index = order.index();
    let mut tick = || {
        on_progress(PlanProgress::Dp {
            order: order_index,
            of: CutOrder::ALL.len(),
        })
    };
    let dp = solve_order(grid, table, order, &mut tick)?;
    let Some(cap) = settings.stone_cap() else {
        return Some(dp.candidates(grid, table, ctx, settings, pool));
    };
    if dp.root_value <= NEG {
        return Some(Vec::new());
    }
    let free = dp.reconstruct(grid, table);
    let free_layouts = candidate_of_tree(ctx, order, &free, pool, settings);
    // A tree of at most `cap` pieces builds at most `cap` stones. A larger tree builds fewer
    // stones than it has pieces when the fitter leaves a piece without one, and may still be
    // within the cap.
    if free.leaf_count() <= cap
        || free_layouts
            .first()
            .is_some_and(|layout| layout.stone_count() <= cap)
    {
        return Some(free_layouts);
    }
    let capped = solve_order_capped(grid, table, order, cap, &mut tick)?;
    Some(capped.candidates(grid, table, ctx, settings, pool))
}

/// Parameters for running shaped leave-one-out alternative rounds.
#[derive(Debug, Clone)]
pub struct ShapedAltParams<'a> {
    /// The unit grid.
    pub grid: &'a ShapedGrid,
    /// Baseline clipped table (its classes are reused by every round).
    pub table: &'a ClippedTable,
    /// The modelled rough's geometry.
    pub ctx: &'a ShapedCtx,
    /// Full sanitised design catalogue.
    pub all_designs: &'a [CandidateDesign],
    /// The baseline best layout.
    pub best: &'a RoughLayout,
    /// Planning settings.
    pub settings: &'a PlanSettings,
    /// Scoped threads each round's size table (per `a0` plane) and clipped table (per
    /// a-range job, see `build_clipped_table_jobs`) are built on (`1` builds on the
    /// calling thread and spawns nothing). The result never depends on it.
    pub lanes: usize,
}

/// The clipped table of one alternatives round over the round's `pool`.
///
/// The size-only table of interior pieces is rebuilt for `pool`: its design
/// indices point into the pool it was built from, so the baseline's table would
/// score the round with designs that may no longer exist. The classification is
/// reused from the baseline table. Both tables are built on `params.lanes` threads, the
/// clipped one in one job per a-range.
pub(super) fn alternative_table(
    params: &ShapedAltParams<'_>,
    pool: &[CandidateDesign],
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<ClippedTable> {
    let size_table =
        params
            .grid
            .size_table_lanes(params.settings, pool, params.lanes, on_progress)?;
    let clip_params = BuildClipParams {
        grid: params.grid,
        front: pool,
        non_box_planes: &params.ctx.non_box,
        mesh: params.ctx.mesh.as_deref(),
        size_table: &size_table,
        settings: params.settings,
        slice: 0..params.grid.cells[0],
        cached_classes: Some(&params.table.classes),
    };
    build_clipped_table_jobs(&clip_params, params.lanes, on_progress)
}

/// Runs leave-one-out alternative rounds for shaped planning.
///
/// Mirrors the plain planner's alternatives: the best layout's most-used design
/// is removed from the full catalogue, the Pareto front of the rest is
/// recomputed and the DP of the best layout's cut order re-run, at most three
/// times, each removal cumulative. Reports [`PlanProgress::Alternatives`] at the start
/// of every round and at the end; between them the round's own [`PlanProgress::Grid`]
/// (size table, clipped table) and [`PlanProgress::Dp`] events are forwarded as they
/// are, so the caller sees the round move and can cancel inside it. The rounds' tables
/// are built on `params.lanes` threads and the result never depends on that count.
/// `None` when `on_progress` cancels.
pub fn plan_shaped_alternatives(
    params: &ShapedAltParams<'_>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<LayoutGroup>> {
    let mut groups = Vec::new();
    let mut removed = Vec::new();
    let mut current_best = params.best.clone();

    for round in 0..ALTERNATIVE_ROUNDS {
        let label = PlanProgress::Alternatives {
            done: round,
            total: ALTERNATIVE_ROUNDS,
        };
        if !on_progress(label) {
            return None;
        }
        let Some(drop_id) = most_used_design(&current_best) else {
            break;
        };
        if removed.contains(&drop_id) {
            break;
        }
        removed.push(drop_id);

        let remaining: Vec<CandidateDesign> = params
            .all_designs
            .iter()
            .filter(|d| !removed.contains(&d.entry_id))
            .copied()
            .collect();
        let pool = pareto_front(&remaining);
        if pool.is_empty() {
            break;
        }

        let alt_table = alternative_table(params, &pool, on_progress)?;
        let layouts = plan_shaped_for_order(
            params.grid,
            &alt_table,
            params.ctx,
            &pool,
            params.best.cut_order,
            params.settings,
            on_progress,
        )?;
        let Some(next_best) = best_layout(&layouts).cloned() else {
            break;
        };
        current_best = next_best;
        groups.push(LayoutGroup { pool, layouts });
    }

    let done = PlanProgress::Alternatives {
        done: ALTERNATIVE_ROUNDS,
        total: ALTERNATIVE_ROUNDS,
    };
    on_progress(done).then_some(groups)
}

/// The design-cap chain of the shaped planner: the twin of the block planner's
/// `plan_design_cap` over the clipped tables.
///
/// While the best layout (`params.best` at the start) uses more than
/// [`PlanSettings::max_designs`] designs, the one contributing the least volume is dropped,
/// the clipped table is rebuilt over the designs the layout still uses (the classification
/// is reused, see `alternative_table`) and the six orders are solved again. One
/// [`LayoutGroup`] per round; nothing runs when the best layout is already within the cap.
/// Reports [`PlanProgress::Alternatives`] at the start of every round, with the round's own
/// `Grid` and `Dp` events between; `None` when `on_progress` cancels.
pub fn plan_shaped_design_cap(
    params: &ShapedAltParams<'_>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<LayoutGroup>> {
    let max_designs = params.settings.max_designs_usize();
    let total = params.best.design_count().saturating_sub(max_designs);
    let mut current = params.best.clone();
    let mut groups = Vec::new();
    let mut round = 0;
    while current.design_count() > max_designs {
        if !on_progress(PlanProgress::Alternatives {
            done: round.min(total),
            total,
        }) {
            return None;
        }
        round += 1;
        let Some(drop) = least_contributing_design(&current) else {
            break;
        };
        let pool = pool_without(params.all_designs, &current, drop);
        if pool.is_empty() {
            break;
        }
        let table = alternative_table(params, &pool, on_progress)?;
        let mut layouts = Vec::new();
        for order in CutOrder::ALL {
            layouts.extend(plan_shaped_for_order(
                params.grid,
                &table,
                params.ctx,
                &pool,
                order,
                params.settings,
                on_progress,
            )?);
        }
        let Some(next) = best_layout(&layouts).cloned() else {
            break;
        };
        current = next;
        groups.push(LayoutGroup { pool, layouts });
    }
    Some(groups)
}
