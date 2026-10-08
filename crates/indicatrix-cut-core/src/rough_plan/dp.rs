//! The mixed staged-guillotine DP, one cut order at a time.
//!
//! For an order `(A, B, C)` over the shared [`PieceTable`] `P`:
//!
//! - bar along C for cross-section `(gA, gB)`:
//!   `T[gc][n] = max over g of T[gc - g][n - 1] + P[gA][gB][g]`, `T[0][0] = 0`,
//!   every other `T[0][*]` and `T[*][0]` `-inf` (no waste run);
//! - slab along B: `S[gb][n] = max over g, m of S[gb - g][n - m] + Bar[gA][g][m]`;
//! - root along A: `Rt[ga][n] = max over g, m of Rt[ga - g][n - m] + Slab[g][m]`.
//!
//! Every `n` in `1..=K` with a finite `Rt[G_A][n]` is a candidate layout (the
//! count is "up to K"). All argmax choices are stored, so reconstruction is a
//! plain walk. Iteration order is fixed (`g` ascending, then `m` ascending)
//! and a later choice replaces only on a strictly greater value, which makes
//! the result deterministic.

use super::{
    NEG,
    pareto::pareto_front,
    piece::{Grid, PieceTable},
    rank::{best_layout, most_used_design, rank_indices, take_indices},
    tree::{Bar, Leaf, Slab, Tree, layout_from_tree},
    types::{CandidateDesign, CutOrder, LayoutGroup, PlanProgress, RoughLayout, clamp_count},
};

/// Candidates kept per cut order (after dedup and the same-design-set cap).
pub const MAX_PER_ORDER: usize = 24;
/// Leave-one-out rounds.
pub const ALTERNATIVE_ROUNDS: usize = 3;

/// The read-only shape of one order's DP.
struct Shape<'a> {
    /// The piece table's values, canonical layout.
    vals: &'a [f64],
    /// Table strides for stages A, B, C.
    strides: [usize; 3],
    /// Units for stages A, B, C.
    cells: [usize; 3],
    /// Most pieces in a bar.
    kb: usize,
    /// Most pieces in a slab.
    ks: usize,
    /// Most pieces overall.
    kr: usize,
}

/// A solved order: the root values and every argmax.
pub struct OrderDp {
    /// Canonical axes of stages A, B, C.
    order: [usize; 3],
    /// Units for stages A, B, C.
    cells: [usize; 3],
    /// Most pieces in a bar.
    kb: usize,
    /// Most pieces in a slab.
    ks: usize,
    /// Most pieces overall.
    kr: usize,
    /// Bar argmax per `(gA, gB, gc, n)`.
    bar_arg: Vec<u8>,
    /// Slab argmax run length per `(gA, gb, n)`.
    slab_arg_g: Vec<u8>,
    /// Slab argmax piece count per `(gA, gb, n)`.
    slab_arg_m: Vec<u8>,
    /// Root value per `(ga, n)`.
    root_val: Vec<f64>,
    /// Root argmax run length per `(ga, n)`.
    root_arg_g: Vec<u8>,
    /// Root argmax piece count per `(ga, n)`.
    root_arg_m: Vec<u8>,
}

/// Fill `t` (`(G_C + 1) x (kb + 1)`) and `arg` for the bar at `base`.
fn bar_dp(shape: &Shape<'_>, base: usize, t: &mut [f64], arg: &mut [u8]) {
    let w = shape.kb + 1;
    t.fill(NEG);
    arg.fill(0);
    t[0] = 0.0;
    for gc in 1..=shape.cells[2] {
        for n in 1..=gc.min(shape.kb) {
            let (mut best, mut best_g) = (NEG, 0);
            for g in 1..=(gc + 1 - n) {
                let cand = t[(gc - g) * w + n - 1] + shape.vals[base + (g - 1) * shape.strides[2]];
                if cand > best {
                    best = cand;
                    best_g = g;
                }
            }
            t[gc * w + n] = best;
            arg[gc * w + n] = best_g as u8;
        }
    }
}

/// The slab DP of one `gA`: fill `s` (`(G_B + 1) x (ks + 1)`) and the argmax.
fn slab_dp(shape: &Shape<'_>, bar_vals: &[f64], s: &mut [f64], arg_g: &mut [u8], arg_m: &mut [u8]) {
    let (w_s, w_b) = (shape.ks + 1, shape.kb + 1);
    s.fill(NEG);
    s[0] = 0.0;
    for gb in 1..=shape.cells[1] {
        for n in 1..=(gb * shape.kb).min(shape.ks) {
            let (mut best, mut best_g, mut best_m) = (NEG, 0, 0);
            for g in 1..=gb {
                for m in 1..=n.min(shape.kb) {
                    let cand = s[(gb - g) * w_s + n - m] + bar_vals[(g - 1) * w_b + m];
                    if cand > best {
                        (best, best_g, best_m) = (cand, g, m);
                    }
                }
            }
            s[gb * w_s + n] = best;
            arg_g[gb * w_s + n] = best_g as u8;
            arg_m[gb * w_s + n] = best_m as u8;
        }
    }
}

/// Solve one cut order. `progress` is called once per `gA` iteration and must
/// return `false` to cancel.
pub fn run_order_dp(
    grid: &Grid,
    table: &PieceTable,
    order: [usize; 3],
    kmax: usize,
    progress: &mut dyn FnMut() -> bool,
) -> Option<OrderDp> {
    let cells = order.map(|i| grid.cells[i]);
    let table_strides = table.strides();
    let kb = kmax.min(cells[2]);
    let ks = kmax.min(cells[1] * kb);
    let kr = kmax.min(cells[0] * ks);
    let shape = Shape {
        vals: &table.values,
        strides: order.map(|i| table_strides[i]),
        cells,
        kb,
        ks,
        kr,
    };
    let (w_b, w_s) = (kb + 1, ks + 1);
    let bar_block = (cells[2] + 1) * w_b;
    let slab_block = (cells[1] + 1) * w_s;
    let mut dp = OrderDp {
        order,
        cells,
        kb,
        ks,
        kr,
        bar_arg: vec![0; cells[0] * cells[1] * bar_block],
        slab_arg_g: vec![0; cells[0] * slab_block],
        slab_arg_m: vec![0; cells[0] * slab_block],
        root_val: vec![NEG; (cells[0] + 1) * (kr + 1)],
        root_arg_g: vec![0; (cells[0] + 1) * (kr + 1)],
        root_arg_m: vec![0; (cells[0] + 1) * (kr + 1)],
    };
    let mut slab_val = vec![NEG; cells[0] * w_s];
    let (mut t, mut bar_vals, mut s) = (
        vec![NEG; bar_block],
        vec![NEG; cells[1] * w_b],
        vec![NEG; slab_block],
    );
    for ga in 1..=cells[0] {
        for gb in 1..=cells[1] {
            let base = (ga - 1) * shape.strides[0] + (gb - 1) * shape.strides[1];
            let off = ((ga - 1) * cells[1] + gb - 1) * bar_block;
            bar_dp(&shape, base, &mut t, &mut dp.bar_arg[off..off + bar_block]);
            bar_vals[(gb - 1) * w_b..gb * w_b].copy_from_slice(&t[cells[2] * w_b..]);
        }
        let range = (ga - 1) * slab_block..ga * slab_block;
        let (arg_g, arg_m) = (&mut dp.slab_arg_g[range.clone()], &mut dp.slab_arg_m[range]);
        slab_dp(&shape, &bar_vals, &mut s, arg_g, arg_m);
        slab_val[(ga - 1) * w_s..ga * w_s].copy_from_slice(&s[cells[1] * w_s..]);
        if !progress() {
            return None;
        }
    }
    root_dp(&shape, &slab_val, &mut dp, progress)?;
    Some(dp)
}

/// The root DP along stage A, filling `dp`'s root arrays.
fn root_dp(
    shape: &Shape<'_>,
    slab_val: &[f64],
    dp: &mut OrderDp,
    progress: &mut dyn FnMut() -> bool,
) -> Option<()> {
    let (w_r, w_s) = (shape.kr + 1, shape.ks + 1);
    dp.root_val[0] = 0.0;
    for ga in 1..=shape.cells[0] {
        for n in 1..=(ga * shape.ks).min(shape.kr) {
            let (mut best, mut best_g, mut best_m) = (NEG, 0, 0);
            for g in 1..=ga {
                for m in 1..=n.min(shape.ks) {
                    let cand = dp.root_val[(ga - g) * w_r + n - m] + slab_val[(g - 1) * w_s + m];
                    if cand > best {
                        (best, best_g, best_m) = (cand, g, m);
                    }
                }
            }
            dp.root_val[ga * w_r + n] = best;
            dp.root_arg_g[ga * w_r + n] = best_g as u8;
            dp.root_arg_m[ga * w_r + n] = best_m as u8;
        }
        if !progress() {
            return None;
        }
    }
    Some(())
}

impl OrderDp {
    /// The best total value with exactly `n` pieces (`-inf` when none).
    pub(crate) fn value(&self, n: usize) -> f64 {
        if n == 0 || n > self.kr {
            return NEG;
        }
        self.root_val[self.cells[0] * (self.kr + 1) + n]
    }

    /// Walk the stored argmax chain: `(run length, piece count)` pairs in
    /// position order. `next` maps `(remaining units, remaining pieces)` to
    /// the argmax `(g, m)` of that cell.
    fn chain(
        units: usize,
        count: usize,
        next: impl Fn(usize, usize) -> (usize, usize),
    ) -> Vec<(usize, usize)> {
        let (mut left, mut n) = (units, count);
        let mut runs = Vec::new();
        while left > 0 && n > 0 {
            let (g, m) = next(left, n);
            if g == 0 || m == 0 || g > left || m > n {
                break;
            }
            runs.push((g, m));
            left -= g;
            n -= m;
        }
        runs.reverse();
        runs
    }

    /// Reconstruct the layout tree with exactly `n` pieces.
    pub(crate) fn reconstruct(&self, grid: &Grid, table: &PieceTable, n: usize) -> Tree {
        let w_r = self.kr + 1;
        let runs = Self::chain(self.cells[0], n, |ga, nn| {
            (
                usize::from(self.root_arg_g[ga * w_r + nn]),
                usize::from(self.root_arg_m[ga * w_r + nn]),
            )
        });
        Tree {
            slabs: runs
                .into_iter()
                .map(|(g_a, m)| self.build_slab(grid, table, g_a, m))
                .collect(),
        }
    }

    /// The slab of `g_a` units along stage A holding `m` pieces.
    fn build_slab(&self, grid: &Grid, table: &PieceTable, g_a: usize, m: usize) -> Slab {
        let w_s = self.ks + 1;
        let block = (g_a - 1) * (self.cells[1] + 1) * w_s;
        let runs = Self::chain(self.cells[1], m, |gb, nn| {
            (
                usize::from(self.slab_arg_g[block + gb * w_s + nn]),
                usize::from(self.slab_arg_m[block + gb * w_s + nn]),
            )
        });
        Slab {
            thickness: grid.run_mm(self.order[0], g_a),
            bars: runs
                .into_iter()
                .map(|(g_b, count)| self.build_bar(grid, table, [g_a, g_b], count))
                .collect(),
        }
    }

    /// The bar of cross-section `[g_a, g_b]` holding `count` pieces.
    fn build_bar(&self, grid: &Grid, table: &PieceTable, cross: [usize; 2], count: usize) -> Bar {
        let w_b = self.kb + 1;
        let block = ((cross[0] - 1) * self.cells[1] + cross[1] - 1) * (self.cells[2] + 1) * w_b;
        let runs = Self::chain(self.cells[2], count, |gc, nn| {
            (usize::from(self.bar_arg[block + gc * w_b + nn]), 1)
        });
        let leaves = runs
            .into_iter()
            .map(|(g_c, _)| {
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

    /// The per-`n` candidates of this order: reconstructed, ranked, deduplicated
    /// by composition and capped.
    pub(crate) fn candidates(
        &self,
        grid: &Grid,
        table: &PieceTable,
        front: &[CandidateDesign],
        order: CutOrder,
    ) -> Vec<RoughLayout> {
        let all: Vec<RoughLayout> = (1..=self.kr)
            .filter(|&n| self.value(n) > NEG)
            .map(|n| {
                let tree = self.reconstruct(grid, table, n);
                layout_from_tree(&grid.rough, &grid.settings, order, &tree, front)
            })
            .collect();
        let keep = rank_indices(&all, MAX_PER_ORDER);
        take_indices(all, &keep)
    }
}

/// Run one cut order of the mixed DP over the shared piece table.
///
/// `front` must be the (entry-id-sorted) slice the table was built from and
/// `count` is the most stones a layout may hold. Reports
/// [`PlanProgress::Dp`] once per `gA` iteration.
///
/// Returns `None` when `on_progress` cancels; otherwise the order's candidate
/// layouts, best first: one per stone count `n` in `1..=count` whose
/// composition is new, at most [`SAME_SET_CAP`](super::SAME_SET_CAP) per design set and
/// 24 in all.
/// An empty vector means nothing is feasible for this order.
pub fn plan_rough_for_order(
    grid: &Grid,
    table: &PieceTable,
    front: &[CandidateDesign],
    order: CutOrder,
    count: u8,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    debug_assert!(
        table
            .design
            .iter()
            .all(|&d| (d as usize) < front.len().max(1)),
        "the piece table picks a design outside the front passed alongside it"
    );
    let kmax = clamp_count(count);
    let index = order.index();
    let dp = run_order_dp(grid, table, order.axes(), kmax, &mut || {
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
    count: u8,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<LayoutGroup>> {
    plan_alternatives_lanes(grid, designs, best, count, 1, on_progress)
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
    count: u8,
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
        let layouts =
            plan_rough_for_order(grid, &table, &pool, best.cut_order, count, &mut |_| {
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
fn build_piece_table_quiet(
    grid: &Grid,
    pool: &[CandidateDesign],
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
    label: PlanProgress,
) -> Option<PieceTable> {
    super::piece::build_piece_table_lanes(grid, pool, lanes, &mut |_| on_progress(label))
}
