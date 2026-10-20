//! The staged-guillotine DP of one cut order under a cap on the number of pieces (stones).
//!
//! [`run_order_dp`](super::dp::run_order_dp) places as many stones as fit best. When
//! `PlanSettings::max_stones` caps that number and the free optimum has more pieces, the
//! best layout of AT MOST `K` pieces is not the free one cut down (a bar of two cubes beats
//! one cube when the cap is one stone), so every table here gets a stone-budget dimension.
//!
//! For an order `(A, B, C)` with cells `[gA, gB, gC]` over the shared piece table `P`, and the
//! budgets `kb = min(K, gC)` (a bar), `ks = min(K, gB * kb)` (a slab) and
//! `kr = min(K, gA * ks)` (the layout):
//!
//! - bar along C for cross-section `(gA, gB)`:
//!   `T[u][n] = max over g of T[u - g][n - 1] + P[gA][gB][g]`;
//! - slab along B: `S[u][n] = max over (g, m) of S[u - g][n - m] + Bar[gA][g][m]`, where
//!   `Bar[gA][g][m] = T[gC][m]` is the bar of `g` units at its best with `m` pieces;
//! - root along A: `R[u][n] = max over (g, m) of R[u - g][n - m] + Slab[g][m]`.
//!
//! Every table reads "the best total of at most `n` pieces": `X[0][n] = 0` for every `n`,
//! `X[u][0] = -inf` for `u >= 1`, and `-inf` propagates. As in the free DP, every run is one
//! stone (no waste runs), so a bar of `m` runs is `m` stones and the pieces of a layout are
//! its stones. The answer is `R[gA][kr]`.
//!
//! The tables ([`CountTables`]) take the value of a run by its position, so the positional DP
//! of a shaped rough (`shaped::dp_capped`) runs the same code.
//!
//! # Ties
//!
//! A later candidate replaces the best only on a strictly greater value, with the run length
//! `g` descending (the longest run first, as the free DP) and the pieces `m` ascending (the
//! fewest stones first). Reconstruction starts at the fewest stones that reach the best value
//! (see [`least_budget`]), so of two layouts worth the same the one with fewer stones is the
//! answer, as in the ranking.
//!
//! # Pruning
//!
//! Every table is bitwise non-decreasing in the stone budget (the candidate set only grows
//! and IEEE addition is monotone), so a bar or slab vector stops growing at some budget, its
//! reach, and a candidate that gives that part more stones than its reach can never be
//! strictly greater than the candidate at the reach itself: it only takes budget away from
//! the rest. Skipping those candidates changes neither a value nor an argmax.
//!
//! # Cost
//!
//! The bars cost about `gA * gB * gC^2 / 2 * kb` operations, the slabs
//! `gA * gB^2 / 2 * ks * reach` and the root `gA^2 / 2 * kr * reach`, and the bar argmax
//! tables take `gA * gB * (kb + 1) * (gC + 1)` bytes.
//! [`choose_grid`](super::choose_grid) scales about 30 cells an axis to
//! the rough's proportions (a product near 27,000 cells, `8..=48` an axis), so the grids it
//! returns top out near `48 x 24 x 24` and `48 x 48 x 12`: with a cap of 99 the worst order of
//! those costs about `1e8` operations before the reach pruning and takes about 1.4 MB of bar
//! argmax. A grid of 48 cells an axis, which `choose_grid` never returns, would be the loose
//! upper bound: about `4e8` and 6 MB. Only a cap the free optimum exceeds pays any of it (see
//! [`plan_rough_for_order`](super::plan_rough_for_order)).

use super::{
    NEG,
    dp::{candidate_of_tree, leaf_of},
    piece::{Grid, PieceTable},
    tree::{Bar, Slab, Tree},
    types::{CandidateDesign, CutOrder, RoughLayout},
};

/// A stored run length or stone budget as one byte (a run is at most 48 cells, a budget at
/// most 99).
fn arg_byte(value: usize) -> u8 {
    u8::try_from(value).expect("a run is at most 48 cells and a stone budget at most 99")
}

/// A stored argmax of a slab or root cell: the run that ends the units, in units, and the
/// stones given to it.
#[derive(Debug, Clone, Copy, Default)]
struct Split {
    /// Units of the part (a bar of a slab, a slab of the root); `0` when nothing is feasible.
    run: u8,
    /// Stones given to the part.
    pieces: u8,
}

/// One part of a level: its value per stone budget and the budget from which that stops
/// growing.
struct Part<'a> {
    /// The best value of the part with at most `n` stones, entry `n` (`-inf` when none).
    values: &'a [f64],
    /// The smallest budget whose value is the value at the largest budget (see
    /// [`least_budget`]); `0` when the part is infeasible.
    reach: usize,
}

/// The smallest stone budget whose value equals the value at `budget`.
///
/// `values` is bitwise non-decreasing in the budget, so the budgets that reach one value form
/// a run ending at `budget`, and its first member is the fewest stones that still reach it.
/// `0` when the value is `-inf` (no stone count makes it feasible).
fn least_budget(values: &[f64], budget: usize) -> usize {
    let target = values[budget].to_bits();
    values[..budget]
        .iter()
        .position(|value| value.to_bits() == target)
        .unwrap_or(budget)
}

/// Fills `best` (one column of `units + 1` entries per stone budget `0..=bar_cap`) with the
/// best total of a bar of the first `u` units cut into at most `n` pieces (entry `u` of column
/// `n`), and `args` (the same layout) with the length of the last piece that achieves it.
///
/// `run_value(start, end)` is the value of the piece covering units `start..end` (`-inf` when
/// unusable). A later run replaces the best only when strictly greater, longest run first.
fn solve_bar(
    units: usize,
    bar_cap: usize,
    run_value: impl Fn(usize, usize) -> f64,
    best: &mut [f64],
    args: &mut [u8],
) {
    let width = units + 1;
    best[..width].fill(NEG);
    best[0] = 0.0;
    for budget in 1..=bar_cap {
        let (done, rest) = best.split_at_mut(budget * width);
        let prev = &done[(budget - 1) * width..];
        let cur = &mut rest[..width];
        cur[0] = 0.0;
        for len in 1..width {
            let (mut top, mut top_run) = (NEG, 0);
            for run in (1..=len).rev() {
                let cand = prev[len - run] + run_value(len - run, len);
                if cand > top {
                    top = cand;
                    top_run = run;
                }
            }
            cur[len] = top;
            args[budget * width + len] = arg_byte(top_run);
        }
    }
}

/// Fills `best` (one column of `units + 1` entries per stone budget `0..=budget_cap`, entry
/// `u` of column `n` the best total of the first `u` units with at most `n` stones) and `args`
/// (the same layout) over the parts of one level: the slab level over the bars of a slab, or
/// the root level over the slabs.
///
/// `part(start, end)` is the part covering units `start..end`. A candidate replaces the best
/// only when strictly greater, with the run descending and the stones ascending; the stones of
/// a part stop at its reach (see the module docs).
fn combine<'a>(
    units: usize,
    budget_cap: usize,
    part: impl Fn(usize, usize) -> Part<'a>,
    best: &mut [f64],
    args: &mut [Split],
) {
    let width = units + 1;
    best[..width].fill(NEG);
    best[0] = 0.0;
    for budget in 1..=budget_cap {
        best[budget * width] = 0.0;
        for len in 1..=units {
            let (mut top, mut split) = (NEG, Split::default());
            for run in (1..=len).rev() {
                let held = part(len - run, len);
                for pieces in 1..=budget.min(held.reach) {
                    let cand = best[(budget - pieces) * width + len - run] + held.values[pieces];
                    if cand > top {
                        top = cand;
                        split = Split {
                            run: arg_byte(run),
                            pieces: arg_byte(pieces),
                        };
                    }
                }
            }
            best[budget * width + len] = top;
            args[budget * width + len] = split;
        }
    }
}

/// Walks the argmax chain of one level from the end of `units` with `budget` stones: the parts
/// in position order, each as `(start, end, stones)` in units. `arg_at(budget, end)` is the
/// stored argmax of the state. The walk stops early where no argmax is stored (nothing is
/// feasible from there).
fn chain(
    units: usize,
    budget: usize,
    arg_at: impl Fn(usize, usize) -> Split,
) -> Vec<(usize, usize, usize)> {
    let mut parts = Vec::new();
    let (mut end, mut left) = (units, budget);
    while end > 0 {
        let split = arg_at(left, end);
        let run = usize::from(split.run);
        if run == 0 || run > end {
            break;
        }
        let given = usize::from(split.pieces);
        parts.push((end - run, end, given));
        end -= run;
        left -= given;
    }
    parts.reverse();
    parts
}

/// The tables of one cut order under a stone cap: values and argmaxes per stone budget for the
/// bars, the slabs and the root.
///
/// A bar is addressed by `pair = slab * bars + bar` (both counted from 0), where a slab is a
/// slab entry (the block DP has one per slab SIZE, the positional DP one per slab RANGE) and a
/// bar a bar entry of that slab (likewise).
pub(super) struct CountTables {
    /// Units along stages A, B and C.
    units: [usize; 3],
    /// Bar entries per slab entry.
    bars: usize,
    /// The most stones a bar can hold.
    bar_cap: usize,
    /// The most stones a slab can hold.
    slab_cap: usize,
    /// The most stones the layout can hold.
    root_cap: usize,
    /// Bar argmax run length per `(bar, budget, units)`.
    bar_arg: Vec<u8>,
    /// Best value of a bar per `(bar, budget)`.
    bar_vals: Vec<f64>,
    /// The reach of each bar's value vector.
    bar_reach: Vec<u8>,
    /// Slab argmax per `(slab, budget, units)`.
    slab_arg: Vec<Split>,
    /// Best value of a slab per `(slab, budget)`.
    slab_vals: Vec<f64>,
    /// The reach of each slab's value vector.
    slab_reach: Vec<u8>,
    /// Root argmax per `(budget, units)`.
    root_arg: Vec<Split>,
    /// Best total per stone budget.
    root_vals: Vec<f64>,
}

impl CountTables {
    /// Empty tables for an order with `units` cells per stage, `slabs` slab entries of `bars`
    /// bar entries each, and the stone cap `cap`.
    pub(super) fn new(units: [usize; 3], slabs: usize, bars: usize, cap: usize) -> Self {
        let [ga, gb, gc] = units;
        let bar_cap = cap.min(gc);
        let slab_cap = cap.min(gb * bar_cap);
        let root_cap = cap.min(ga * slab_cap);
        let pairs = slabs * bars;
        Self {
            units,
            bars,
            bar_cap,
            slab_cap,
            root_cap,
            bar_arg: vec![0; pairs * (bar_cap + 1) * (gc + 1)],
            bar_vals: vec![NEG; pairs * (bar_cap + 1)],
            bar_reach: vec![0; pairs],
            slab_arg: vec![Split::default(); slabs * (slab_cap + 1) * (gb + 1)],
            slab_vals: vec![NEG; slabs * (slab_cap + 1)],
            slab_reach: vec![0; slabs],
            root_arg: vec![Split::default(); (root_cap + 1) * (ga + 1)],
            root_vals: vec![NEG; root_cap + 1],
        }
    }

    /// The number of the bar `bar` of slab `slab`.
    pub(super) const fn pair(&self, slab: usize, bar: usize) -> usize {
        slab * self.bars + bar
    }

    /// A scratch buffer for [`Self::fill_bar`].
    pub(super) fn bar_scratch(&self) -> Vec<f64> {
        vec![NEG; (self.bar_cap + 1) * (self.units[2] + 1)]
    }

    /// Fills bar `pair` from `run_value(start, end)`, the value of the piece covering the units
    /// `start..end` of stage C (`-inf` when unusable), using `scratch` from
    /// [`Self::bar_scratch`].
    pub(super) fn fill_bar(
        &mut self,
        pair: usize,
        run_value: impl Fn(usize, usize) -> f64,
        scratch: &mut [f64],
    ) {
        let gc = self.units[2];
        let args = (self.bar_cap + 1) * (gc + 1);
        solve_bar(
            gc,
            self.bar_cap,
            run_value,
            scratch,
            &mut self.bar_arg[pair * args..(pair + 1) * args],
        );
        let values = &mut self.bar_vals[pair * (self.bar_cap + 1)..(pair + 1) * (self.bar_cap + 1)];
        for (budget, value) in values.iter_mut().enumerate() {
            *value = scratch[budget * (gc + 1) + gc];
        }
        self.bar_reach[pair] = arg_byte(least_budget(values, self.bar_cap));
    }

    /// Fills slab `slab` from its bars. `bar_of(start, end)` is the bar entry of the units
    /// `start..end` of stage B (its bar number within the slab).
    pub(super) fn fill_slab(&mut self, slab: usize, bar_of: impl Fn(usize, usize) -> usize) {
        let gb = self.units[1];
        let (bars, bar_cap, slab_cap) = (self.bars, self.bar_cap, self.slab_cap);
        let (bar_vals, bar_reach) = (&self.bar_vals, &self.bar_reach);
        let part = move |start: usize, end: usize| {
            let pair = slab * bars + bar_of(start, end);
            Part {
                values: &bar_vals[pair * (bar_cap + 1)..(pair + 1) * (bar_cap + 1)],
                reach: usize::from(bar_reach[pair]),
            }
        };
        let width = gb + 1;
        let table = (slab_cap + 1) * width;
        let mut best = vec![NEG; table];
        combine(
            gb,
            slab_cap,
            part,
            &mut best,
            &mut self.slab_arg[slab * table..(slab + 1) * table],
        );
        let values = &mut self.slab_vals[slab * (slab_cap + 1)..(slab + 1) * (slab_cap + 1)];
        for (budget, value) in values.iter_mut().enumerate() {
            *value = best[budget * width + gb];
        }
        self.slab_reach[slab] = arg_byte(least_budget(values, slab_cap));
    }

    /// Fills the root from the slabs. `slab_of(start, end)` is the slab entry of the units
    /// `start..end` of stage A.
    pub(super) fn fill_root(&mut self, slab_of: impl Fn(usize, usize) -> usize) {
        let ga = self.units[0];
        let (slab_cap, root_cap) = (self.slab_cap, self.root_cap);
        let (slab_vals, slab_reach) = (&self.slab_vals, &self.slab_reach);
        let part = move |start: usize, end: usize| {
            let slab = slab_of(start, end);
            Part {
                values: &slab_vals[slab * (slab_cap + 1)..(slab + 1) * (slab_cap + 1)],
                reach: usize::from(slab_reach[slab]),
            }
        };
        let width = ga + 1;
        let mut best = vec![NEG; (root_cap + 1) * width];
        combine(ga, root_cap, part, &mut best, &mut self.root_arg);
        for (budget, value) in self.root_vals.iter_mut().enumerate() {
            *value = best[budget * width + ga];
        }
    }

    /// The best total value of at most the capped number of stones (`-inf` when nothing fits).
    pub(super) fn value(&self) -> f64 {
        self.root_vals[self.root_cap]
    }

    /// The slabs of the best layout in position order, as `(start, end, stones)` in units of
    /// stage A.
    ///
    /// The walk starts at the fewest stones that reach the best value ([`least_budget`]), and a
    /// layout of that many stones at that value cannot have fewer. Each part then gets the
    /// stones its argmax gave it, which is already the fewest that reach the part's value (the
    /// pieces ascend and only a strictly greater candidate replaces).
    pub(super) fn root_parts(&self) -> Vec<(usize, usize, usize)> {
        let ga = self.units[0];
        chain(
            ga,
            least_budget(&self.root_vals, self.root_cap),
            |budget, end| self.root_arg[budget * (ga + 1) + end],
        )
    }

    /// The bars of slab `slab` holding `budget` stones, as `(start, end, stones)` in units of
    /// stage B.
    pub(super) fn slab_parts(&self, slab: usize, budget: usize) -> Vec<(usize, usize, usize)> {
        let gb = self.units[1];
        let table = (self.slab_cap + 1) * (gb + 1);
        chain(gb, budget, |left, end| {
            self.slab_arg[slab * table + left * (gb + 1) + end]
        })
    }

    /// The pieces of bar `pair` holding `budget` stones, as `(start, end)` in units of stage C.
    pub(super) fn bar_parts(&self, pair: usize, budget: usize) -> Vec<(usize, usize)> {
        let gc = self.units[2];
        let table = (self.bar_cap + 1) * (gc + 1);
        chain(gc, budget, |left, end| Split {
            run: self.bar_arg[pair * table + left * (gc + 1) + end],
            pieces: 1,
        })
        .into_iter()
        .map(|(start, end, _)| (start, end))
        .collect()
    }
}

/// A solved order under a stone cap: the values and every argmax, per stone budget.
pub struct CappedOrderDp {
    /// Canonical axes of stages A, B, C.
    order: [usize; 3],
    /// The tables.
    tables: CountTables,
}

impl CappedOrderDp {
    /// The best total value of at most the capped number of stones (`-inf` when nothing fits).
    pub(crate) fn value(&self) -> f64 {
        self.tables.value()
    }

    /// Reconstruct the best layout tree: the one of the capped value with the fewest pieces.
    pub(crate) fn reconstruct(&self, grid: &Grid, table: &PieceTable) -> Tree {
        let slabs = self
            .tables
            .root_parts()
            .into_iter()
            .map(|(start, end, stones)| self.build_slab(grid, table, end - start, stones))
            .collect();
        Tree { slabs }
    }

    /// The slab of `g_a` units along stage A, holding at most `stones` stones.
    fn build_slab(&self, grid: &Grid, table: &PieceTable, g_a: usize, stones: usize) -> Slab {
        let bars = self
            .tables
            .slab_parts(g_a - 1, stones)
            .into_iter()
            .map(|(start, end, stones)| self.build_bar(grid, table, [g_a, end - start], stones))
            .collect();
        Slab {
            thickness: grid.run_mm(self.order[0], g_a),
            bars,
        }
    }

    /// The bar of cross-section `cross` (`[gA, gB]`), holding at most `stones` stones.
    fn build_bar(&self, grid: &Grid, table: &PieceTable, cross: [usize; 2], stones: usize) -> Bar {
        let pair = self.tables.pair(cross[0] - 1, cross[1] - 1);
        let leaves = self
            .tables
            .bar_parts(pair, stones)
            .into_iter()
            .map(|(start, end)| leaf_of(grid, table, self.order, cross, end - start))
            .collect();
        Bar {
            width: grid.run_mm(self.order[1], cross[1]),
            leaves,
        }
    }

    /// This order's candidate: the best layout of at most the capped number of stones, or none
    /// when nothing fits.
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
        candidate_of_tree(grid, front, order, &self.reconstruct(grid, table))
    }
}

/// Solve one cut order for the best layout of at most `cap` pieces (`cap >= 1`; `0` leaves
/// nothing feasible). `progress` is called once per `gA` iteration, exactly where the free DP
/// calls it, and must return `false` to cancel.
pub fn run_order_dp_capped(
    grid: &Grid,
    table: &PieceTable,
    order: [usize; 3],
    cap: usize,
    progress: &mut dyn FnMut() -> bool,
) -> Option<CappedOrderDp> {
    let units = order.map(|axis| grid.cells[axis]);
    let mut tables = CountTables::new(units, units[0], units[1], cap);
    let mut scratch = tables.bar_scratch();
    let table_strides = table.strides();
    let strides = order.map(|axis| table_strides[axis]);
    let mut runs = vec![NEG; units[2] + 1];
    for ga in 1..=units[0] {
        for gb in 1..=units[1] {
            let base = (ga - 1) * strides[0] + (gb - 1) * strides[1];
            for (g, run) in runs.iter_mut().enumerate().skip(1) {
                *run = table.values[base + (g - 1) * strides[2]];
            }
            let pair = tables.pair(ga - 1, gb - 1);
            tables.fill_bar(pair, |start, end| runs[end - start], &mut scratch);
        }
        tables.fill_slab(ga - 1, |start, end| end - start - 1);
        if !progress() {
            return None;
        }
    }
    tables.fill_root(|start, end| end - start - 1);
    Some(CappedOrderDp { order, tables })
}
