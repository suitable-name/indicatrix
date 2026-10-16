//! The mixed staged-guillotine DP, one cut order at a time.
//!
//! For an order `(A, B, C)` over the shared [`PieceTable`] `P`:
//!
//! - bar along C for cross-section `(gA, gB)`:
//!   `T[gc] = max over g of T[gc - g] + P[gA][gB][g]`, `T[0] = 0` (no waste run: every
//!   piece holds a stone, a piece too small for one is `-inf`);
//! - slab along B: `S[gb] = max over g of S[gb - g] + Bar[gA][g]`;
//! - root along A: `Rt[ga] = max over g of Rt[ga - g] + Slab[g]`.
//!
//! The number of pieces is free: the DP places as many stones as the rough, the losses and
//! the minimum width allow, and `Rt[G_A]` is the best total. The design cap is applied
//! outside the DP (see [`plan_design_cap`]). All argmax choices are stored, so
//! reconstruction is a plain walk. Iteration order is fixed (`g` descending) and a later
//! choice replaces only on a strictly greater value, which makes the result deterministic and
//! keeps the longest run on a tie: of two splits worth the same, the one with fewer pieces
//! (fewer stones, fewer cuts) wins, as in the ranking.

use super::{
    NEG,
    pareto::pareto_front,
    piece::{Grid, PieceTable},
    rank::{best_layout, least_contributing_design, most_used_design, pool_without},
    tree::{Bar, Leaf, Slab, Tree, layout_from_tree},
    types::{CandidateDesign, CutOrder, LayoutGroup, PlanProgress, RoughLayout},
};

/// A stored argmax (a run length) as one byte.
fn arg_byte(value: usize) -> u8 {
    u8::try_from(value).expect("a run is at most 48 cells")
}

/// Leave-one-out rounds.
pub const ALTERNATIVE_ROUNDS: usize = 3;

/// A solved order: the root values and every argmax.
pub struct OrderDp {
    /// Canonical axes of stages A, B, C.
    order: [usize; 3],
    /// Units for stages A, B, C.
    cells: [usize; 3],
    /// Bar argmax run length per `(gA, gB, gc)`.
    bar_arg: Vec<u8>,
    /// Slab argmax run length per `(gA, gb)`.
    slab_arg: Vec<u8>,
    /// Root value per `ga`.
    root_val: Vec<f64>,
    /// Root argmax run length per `ga`.
    root_arg: Vec<u8>,
}

/// The best split of a run of `units` cells into runs, given the value of a run of `g`
/// cells (`run_value(g)`, `-inf` when unusable): fills `best[0..=units]` and `arg`. The
/// longest run is tried first, so it is kept on a tie.
fn split_runs(units: usize, best: &mut [f64], arg: &mut [u8], run_value: impl Fn(usize) -> f64) {
    best[0] = 0.0;
    arg[0] = 0;
    for u in 1..=units {
        let (mut top, mut top_g) = (NEG, 0);
        for g in (1..=u).rev() {
            let cand = best[u - g] + run_value(g);
            if cand > top {
                top = cand;
                top_g = g;
            }
        }
        best[u] = top;
        arg[u] = arg_byte(top_g);
    }
}

/// Solve one cut order. `progress` is called once per `gA` iteration and must
/// return `false` to cancel.
pub fn run_order_dp(
    grid: &Grid,
    table: &PieceTable,
    order: [usize; 3],
    progress: &mut dyn FnMut() -> bool,
) -> Option<OrderDp> {
    let cells = order.map(|i| grid.cells[i]);
    let table_strides = table.strides();
    let strides = order.map(|i| table_strides[i]);
    let vals = &table.values;
    let bar_block = cells[2] + 1;
    let slab_block = cells[1] + 1;
    let mut bar_arg = vec![0; cells[0] * cells[1] * bar_block];
    let mut slab_arg = vec![0; cells[0] * slab_block];
    let mut slab_val = vec![NEG; cells[0]];
    let (mut t, mut bar_vals, mut s) = (
        vec![NEG; bar_block],
        vec![NEG; cells[1]],
        vec![NEG; slab_block],
    );
    for ga in 1..=cells[0] {
        for gb in 1..=cells[1] {
            let base = (ga - 1) * strides[0] + (gb - 1) * strides[1];
            let off = ((ga - 1) * cells[1] + gb - 1) * bar_block;
            split_runs(cells[2], &mut t, &mut bar_arg[off..off + bar_block], |g| {
                vals[base + (g - 1) * strides[2]]
            });
            bar_vals[gb - 1] = t[cells[2]];
        }
        let range = (ga - 1) * slab_block..ga * slab_block;
        split_runs(cells[1], &mut s, &mut slab_arg[range], |g| bar_vals[g - 1]);
        slab_val[ga - 1] = s[cells[1]];
        if !progress() {
            return None;
        }
    }
    let mut root_val = vec![NEG; cells[0] + 1];
    let mut root_arg = vec![0; cells[0] + 1];
    split_runs(cells[0], &mut root_val, &mut root_arg, |g| slab_val[g - 1]);
    Some(OrderDp {
        order,
        cells,
        bar_arg,
        slab_arg,
        root_val,
        root_arg,
    })
}

impl OrderDp {
    /// The best total value of this order (`-inf` when nothing fits).
    pub(crate) fn value(&self) -> f64 {
        self.root_val[self.cells[0]]
    }

    /// Walk the stored argmax chain from `units`: the run lengths in position order.
    fn chain(units: usize, next: impl Fn(usize) -> usize) -> Vec<usize> {
        let mut left = units;
        let mut runs = Vec::new();
        while left > 0 {
            let g = next(left);
            if g == 0 || g > left {
                break;
            }
            runs.push(g);
            left -= g;
        }
        runs.reverse();
        runs
    }

    /// Reconstruct the best layout tree.
    pub(crate) fn reconstruct(&self, grid: &Grid, table: &PieceTable) -> Tree {
        let runs = Self::chain(self.cells[0], |ga| usize::from(self.root_arg[ga]));
        Tree {
            slabs: runs
                .into_iter()
                .map(|g_a| self.build_slab(grid, table, g_a))
                .collect(),
        }
    }

    /// The slab of `g_a` units along stage A.
    fn build_slab(&self, grid: &Grid, table: &PieceTable, g_a: usize) -> Slab {
        let block = (g_a - 1) * (self.cells[1] + 1);
        let runs = Self::chain(self.cells[1], |gb| usize::from(self.slab_arg[block + gb]));
        Slab {
            thickness: grid.run_mm(self.order[0], g_a),
            bars: runs
                .into_iter()
                .map(|g_b| self.build_bar(grid, table, [g_a, g_b]))
                .collect(),
        }
    }

    /// The bar of cross-section `[g_a, g_b]`.
    fn build_bar(&self, grid: &Grid, table: &PieceTable, cross: [usize; 2]) -> Bar {
        let block = ((cross[0] - 1) * self.cells[1] + cross[1] - 1) * (self.cells[2] + 1);
        let runs = Self::chain(self.cells[2], |gc| usize::from(self.bar_arg[block + gc]));
        let leaves = runs
            .into_iter()
            .map(|g_c| {
                let mut cell = [0; 3];
                cell[self.order[0]] = cross[0];
                cell[self.order[1]] = cross[1];
                cell[self.order[2]] = g_c;
                let index = table.index(cell);
                Leaf {
                    len: grid.run_mm(self.order[2], g_c),
                    design: table.design[index] as usize,
                    orient: usize::from(table.orient[index]),
                }
            })
            .collect();
        Bar {
            width: grid.run_mm(self.order[1], cross[1]),
            leaves,
        }
    }

    /// This order's candidate: the best layout, or none when nothing fits.
    pub(crate) fn candidates(
        &self,
        grid: &Grid,
        table: &PieceTable,
        front: &[CandidateDesign],
        order: CutOrder,
    ) -> Vec<RoughLayout> {
        if self.value() <= NEG {
            return Vec::new();
        }
        let tree = self.reconstruct(grid, table);
        let layout = layout_from_tree(&grid.rough, &grid.settings, order, &tree, front);
        if layout.stones.is_empty() {
            Vec::new()
        } else {
            vec![layout]
        }
    }
}

/// Run one cut order of the mixed DP over the shared piece table.
///
/// `front` must be the (entry-id-sorted) slice the table was built from. Reports
/// [`PlanProgress::Dp`] once per `gA` iteration.
///
/// Returns `None` when `on_progress` cancels; otherwise the order's best layout (as many
/// stones as fit), or an empty vector when nothing is feasible for this order.
pub fn plan_rough_for_order(
    grid: &Grid,
    table: &PieceTable,
    front: &[CandidateDesign],
    order: CutOrder,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    debug_assert!(
        table
            .design
            .iter()
            .all(|&d| (d as usize) < front.len().max(1)),
        "the piece table picks a design outside the front passed alongside it"
    );
    let index = order.index();
    let dp = run_order_dp(grid, table, order.axes(), &mut || {
        on_progress(PlanProgress::Dp {
            order: index,
            of: CutOrder::ALL.len(),
        })
    })?;
    Some(dp.candidates(grid, table, front, order))
}

/// The leave-one-out mixed alternatives.
///
/// Starting from `best` (the best mixed layout of the six orders), remove its
/// most-used design from `designs`, recompute the Pareto front of what is
/// left (a design dominated only by a removed one may now be needed), rebuild
/// the piece table, re-run the DP of `best`'s cut order, and repeat up to
/// three times; each removal is cumulative. Each round yields one
/// [`LayoutGroup`] whose `pool` is that round's front, so the caller can
/// refine it against that same pool.
///
/// `designs` is the FULL candidate list (not the Pareto front); it is
/// sanitised internally, so any input order gives the same result.
///
/// Reports [`PlanProgress::Alternatives`] (`done` of 3 rounds, and once per
/// piece-table plane). `None` when `on_progress` cancels.
pub fn plan_alternatives(
    grid: &Grid,
    designs: &[CandidateDesign],
    best: &RoughLayout,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<LayoutGroup>> {
    plan_alternatives_lanes(grid, designs, best, 1, on_progress)
}

/// [`plan_alternatives`] with each round's piece table built on `lanes` scoped threads
/// (the round's DP is one cut order and stays on the calling thread).
///
/// The tables are bitwise the serial ones whatever the lane count, so the groups are too.
/// The per-plane events are relabelled as the round's [`PlanProgress::Alternatives`] on the
/// calling thread, as in the serial run. With one lane nothing is spawned.
pub fn plan_alternatives_lanes(
    grid: &Grid,
    designs: &[CandidateDesign],
    best: &RoughLayout,
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<LayoutGroup>> {
    let mut removed_ids: Vec<i64> = Vec::new();
    let mut current = best.clone();
    let mut groups = Vec::new();
    for round in 0..ALTERNATIVE_ROUNDS {
        let progress = PlanProgress::Alternatives {
            done: round,
            total: ALTERNATIVE_ROUNDS,
        };
        if !on_progress(progress) {
            return None;
        }
        let Some(removed) = most_used_design(&current) else {
            break;
        };
        removed_ids.push(removed);
        let remaining: Vec<CandidateDesign> = designs
            .iter()
            .filter(|d| !removed_ids.contains(&d.entry_id))
            .copied()
            .collect();
        let pool = pareto_front(&remaining);
        if pool.is_empty() {
            break;
        }
        let table = build_piece_table_quiet(grid, &pool, lanes, on_progress, progress)?;
        let layouts = plan_rough_for_order(grid, &table, &pool, best.cut_order, &mut |_| {
            on_progress(progress)
        })?;
        let Some(next) = best_layout(&layouts).cloned() else {
            break;
        };
        current = next;
        groups.push(LayoutGroup { pool, layouts });
    }
    let done = PlanProgress::Alternatives {
        done: ALTERNATIVE_ROUNDS,
        total: ALTERNATIVE_ROUNDS,
    };
    on_progress(done).then_some(groups)
}

/// Build a piece table, re-labelling its per-plane progress as `label`.
pub(super) fn build_piece_table_quiet(
    grid: &Grid,
    pool: &[CandidateDesign],
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
    label: PlanProgress,
) -> Option<PieceTable> {
    super::piece::build_piece_table_lanes(grid, pool, lanes, &mut |_| on_progress(label))
}

/// The design-cap chain of the block planner: layouts of `best`'s cut quality with at most
/// `max_designs` different designs.
///
/// The DP places stones freely and may use any number of designs. While the best layout
/// uses more than `max_designs`, the design contributing the least volume to it is dropped,
/// the piece table is rebuilt over the designs it still uses, and the six orders are solved
/// again; each round yields one [`LayoutGroup`] (its pool is the round's designs, so the
/// refinement cannot bring a dropped design back). The pool shrinks by at least one design
/// per round, so the chain ends after at most `best.design_count() - max_designs` rounds.
/// Nothing runs when `best` is already within the cap.
///
/// `designs` is the sanitised catalogue (sorted by `entry_id`). Each round's tables are built
/// on `lanes` scoped threads and the result does not depend on that count. Reports
/// [`PlanProgress::Alternatives`] (`done` of the rounds needed at the start) and relabels the
/// round's own events as that; `None` when `on_progress` cancels.
pub fn plan_design_cap(
    grid: &Grid,
    designs: &[CandidateDesign],
    best: &RoughLayout,
    max_designs: usize,
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<LayoutGroup>> {
    let total = best.design_count().saturating_sub(max_designs);
    let mut current = best.clone();
    let mut groups = Vec::new();
    let mut round = 0;
    while current.design_count() > max_designs {
        let label = PlanProgress::Alternatives {
            done: round.min(total),
            total,
        };
        if !on_progress(label) {
            return None;
        }
        round += 1;
        let Some(drop) = least_contributing_design(&current) else {
            break;
        };
        let pool = pool_without(designs, &current, drop);
        if pool.is_empty() {
            break;
        }
        let table = build_piece_table_quiet(grid, &pool, lanes, on_progress, label)?;
        let mut layouts = Vec::new();
        for order in CutOrder::ALL {
            layouts.extend(plan_rough_for_order(
                grid,
                &table,
                &pool,
                order,
                &mut |_| on_progress(label),
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
