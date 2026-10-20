//! The positional count DP of one cut order under a cap on the number of pieces (stones).
//!
//! The twin of [`solve_order`](super::dp::solve_order) for the best layout of AT MOST `K`
//! pieces, as the block planner's `dp_capped` is of its free DP: the same recurrences over
//! positional ranges (a piece is the run of cells `start..end` of its axis, valued by the
//! clipped table at the ranges of its three axes), with a stone budget in every table, so a cap
//! the free optimum exceeds is solved on its own and not by cutting a larger layout down. See
//! that module for the recurrences, the "at most `n` pieces" semantics, the tie rule and the
//! pruning; the tables and their kernels are shared (`CountTables`).
//!
//! A piece the table values `-inf` (outside the rough, or too small for a stone) is unusable,
//! as in the free DP. Reconstruction builds the tree with the same leaves; the layout is then
//! built by `layout_from_tree_shaped`, whose compaction can only lower the stone count.
//!
//! The budget counts the pieces of a tree, not the stones built from it. They differ only when
//! the fitter leaves a piece without a stone that the table valued (a mesh rough's solver is a
//! heuristic), and the compaction merges it away. Such a tree of more pieces than the cap is
//! outside this search, so its search space is slightly smaller than "at most `K` stones". The
//! one tree that matters most, the free optimum's own, is covered: `plan_shaped_for_order`
//! tests the stones its layout holds before it runs this DP.
//!
//! # Ticks and memory
//!
//! `tick` is called once per starting plane of the a range in the bar fill and once per
//! starting plane in the slab fill, exactly where the free DP calls it (two calls per cell of
//! the a axis). The argmax tables hold `p_a * p_b * (kb + 1) * (G_C + 1)` bytes for the bars
//! (`p_x = G_x (G_x + 1) / 2`, `kb = min(K, G_C)`): about 5 MB for a grid of 16 cells an axis,
//! the largest one the piece cap allows for a cube.

use super::{
    clip::ClippedTable,
    ctx::ShapedCtx,
    dp::{Tick, candidate_of_tree},
    grid::ShapedGrid,
};
use crate::rough_plan::{
    CandidateDesign, CutOrder, NEG, PlanSettings, RoughLayout,
    dp_capped::CountTables,
    tree::{Bar, Leaf, Slab, Tree},
};

/// A solved positional order under a stone cap.
pub(super) struct ShapedCappedDp {
    /// The cut order.
    order: CutOrder,
    /// The tables: slab entries are slab ranges, bar entries bar ranges.
    tables: CountTables,
}

impl ShapedCappedDp {
    /// The best total value of at most the capped number of stones (`-inf` when nothing fits).
    pub(super) fn root_value(&self) -> f64 {
        self.tables.value()
    }

    /// Reconstructs the guillotine tree of the best layout: the one of the capped value with
    /// the fewest pieces.
    pub(super) fn reconstruct(&self, grid: &ShapedGrid, table: &ClippedTable) -> Tree {
        let ax = self.order.axes()[0];
        let slabs = self
            .tables
            .root_parts()
            .into_iter()
            .map(|(start, end, stones)| {
                let slab_rng = grid.range_index(ax, start, end);
                self.build_slab(grid, table, slab_rng, (end - start, stones))
            })
            .collect();
        Tree { slabs }
    }

    /// The slab of the range `slab_rng` (`size` units along stage A), holding at most `stones`
    /// stones.
    fn build_slab(
        &self,
        grid: &ShapedGrid,
        table: &ClippedTable,
        slab_rng: usize,
        (size, stones): (usize, usize),
    ) -> Slab {
        let [ax, bx, _] = self.order.axes();
        let bars = self
            .tables
            .slab_parts(slab_rng, stones)
            .into_iter()
            .map(|(start, end, stones)| {
                let bar_rng = grid.range_index(bx, start, end);
                self.build_bar(grid, table, [slab_rng, bar_rng], (end - start, stones))
            })
            .collect();
        Slab {
            thickness: grid.run_mm(ax, size),
            bars,
        }
    }

    /// The bar of the ranges `[slab_rng, bar_rng]` (`size` units along stage B), holding at
    /// most `stones` stones.
    fn build_bar(
        &self,
        grid: &ShapedGrid,
        table: &ClippedTable,
        ranges: [usize; 2],
        (size, stones): (usize, usize),
    ) -> Bar {
        let [ax, bx, cx] = self.order.axes();
        let pair = self.tables.pair(ranges[0], ranges[1]);
        let leaves = self
            .tables
            .bar_parts(pair, stones)
            .into_iter()
            .map(|(start, end)| {
                let mut canonical = [0; 3];
                canonical[ax] = ranges[0];
                canonical[bx] = ranges[1];
                canonical[cx] = grid.range_index(cx, start, end);
                let index = table.index(canonical[0], canonical[1], canonical[2]);
                Leaf {
                    len: grid.run_mm(cx, end - start),
                    design: table.design[index] as usize,
                    orient: usize::from(table.orient[index]),
                }
            })
            .collect();
        Bar {
            width: grid.run_mm(bx, size),
            leaves,
        }
    }

    /// This order's candidate: the best layout of at most the capped number of stones, built in
    /// full, or none when nothing fits.
    pub(super) fn candidates(
        &self,
        grid: &ShapedGrid,
        table: &ClippedTable,
        ctx: &ShapedCtx,
        settings: &PlanSettings,
        pool: &[CandidateDesign],
    ) -> Vec<RoughLayout> {
        if self.root_value() <= NEG {
            return Vec::new();
        }
        let tree = self.reconstruct(grid, table);
        candidate_of_tree(ctx, self.order, &tree, pool, settings)
    }
}

/// What the bar and slab fills read and write.
struct Fill<'a> {
    /// The unit grid.
    grid: &'a ShapedGrid,
    /// The clipped piece table.
    table: &'a ClippedTable,
    /// Canonical axes of stages A, B, C.
    axes: [usize; 3],
    /// Units along stages A, B, C.
    units: [usize; 3],
    /// The tables being filled.
    tables: CountTables,
    /// Scratch of the bar kernel.
    scratch: Vec<f64>,
    /// The value of the piece `start..end` of the current bar's stage-C axis, at
    /// `start * (G_C + 1) + end`.
    runs: Vec<f64>,
}

impl Fill<'_> {
    /// The bars of every slab range that starts at the plane `ra_start`.
    fn bars_from(&mut self, ra_start: usize) {
        let [ax, bx, _] = self.axes;
        let [ga, gb, _] = self.units;
        for ra_end in (ra_start + 1)..=ga {
            let slab_rng = self.grid.range_index(ax, ra_start, ra_end);
            for rb_start in 0..gb {
                for rb_end in (rb_start + 1)..=gb {
                    let bar_rng = self.grid.range_index(bx, rb_start, rb_end);
                    self.fill_bar(slab_rng, bar_rng);
                }
            }
        }
    }

    /// The bar of the slab range `slab_rng` and the bar range `bar_rng`: the value of every
    /// piece of its stage-C axis, then the kernel.
    fn fill_bar(&mut self, slab_rng: usize, bar_rng: usize) {
        let [ax, bx, cx] = self.axes;
        let gc = self.units[2];
        let width = gc + 1;
        for start in 0..gc {
            for end in (start + 1)..=gc {
                let mut canonical = [0; 3];
                canonical[ax] = slab_rng;
                canonical[bx] = bar_rng;
                canonical[cx] = self.grid.range_index(cx, start, end);
                self.runs[start * width + end] =
                    self.table.value(canonical[0], canonical[1], canonical[2]);
            }
        }
        let pair = self.tables.pair(slab_rng, bar_rng);
        let runs = &self.runs;
        self.tables.fill_bar(
            pair,
            |start, end| runs[start * width + end],
            &mut self.scratch,
        );
    }

    /// The slab of every range that starts at the plane `ra_start`.
    fn slabs_from(&mut self, ra_start: usize) {
        let [ax, bx, _] = self.axes;
        let grid = self.grid;
        for ra_end in (ra_start + 1)..=self.units[0] {
            let slab_rng = grid.range_index(ax, ra_start, ra_end);
            self.tables
                .fill_slab(slab_rng, |start, end| grid.range_index(bx, start, end));
        }
    }
}

/// Runs the three levels of the count DP of `order` for the best layout of at most `cap`
/// pieces; `None` when `tick` cancels. `tick` is called once per starting plane of the a range
/// in the bar fill and once per starting plane in the slab fill (two calls per cell of the a
/// axis, as [`solve_order`](super::dp::solve_order)).
pub(super) fn solve_order_capped(
    grid: &ShapedGrid,
    table: &ClippedTable,
    order: CutOrder,
    cap: usize,
    tick: &mut Tick<'_>,
) -> Option<ShapedCappedDp> {
    let axes = order.axes();
    let [ax, bx, _] = axes;
    let units = axes.map(|axis| grid.cells[axis]);
    let tables = CountTables::new(units, grid.range_count(ax), grid.range_count(bx), cap);
    let mut fill = Fill {
        grid,
        table,
        axes,
        units,
        scratch: tables.bar_scratch(),
        runs: vec![NEG; (units[2] + 1) * (units[2] + 1)],
        tables,
    };
    for ra_start in 0..units[0] {
        if !tick() {
            return None;
        }
        fill.bars_from(ra_start);
    }
    for ra_start in 0..units[0] {
        if !tick() {
            return None;
        }
        fill.slabs_from(ra_start);
    }
    fill.tables
        .fill_root(|start, end| grid.range_index(ax, start, end));
    Some(ShapedCappedDp {
        order,
        tables: fill.tables,
    })
}
