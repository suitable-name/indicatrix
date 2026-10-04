//! Positional dynamic programming for multi-stone planning in shaped roughs.
//!
//! Solves the 3-stage guillotine cut sequence (slabs across stage A, bars across stage B,
//! pieces across stage C) where each piece's value depends on its spatial bounds.
//!
//! # Memory
//!
//! Only the argmax choices are stored, one byte each. Per cut order the bar table
//! holds `p_a * p_b * (G_C + 1) * (k_b + 1)` bytes (`p_x = G_x (G_x + 1) / 2`
//! range counts; about 5 MB at `G = 16` and `K = 99`), the slab tables
//! `2 * p_a * (G_B + 1) * (k_s + 1)` and the root tables `2 * (G_A + 1) * (K + 1)`.
//! The bar, slab and root value tables are `f64` and far smaller.

use super::{
    clip::{BuildClipParams, ClippedTable, build_clipped_table_lanes},
    ctx::ShapedCtx,
    grid::ShapedGrid,
    tree::layout_from_tree_shaped,
};
use crate::rough_plan::{
    Axis, CandidateDesign, CutOrder, CutPlan, LayoutGroup, NEG, PlacedStone, PlanProgress,
    PlanSettings, RoughLayout, StonePose,
    dp::{ALTERNATIVE_ROUNDS, MAX_PER_ORDER},
    pareto::pareto_front,
    rank::{best_layout, most_used_design, rank_indices},
    tree::{Bar, Leaf, Slab, Tree},
};

/// Reports one unit of progress; returns `false` to cancel.
type Tick<'a> = dyn FnMut() -> bool + 'a;

/// Solved positional DP for one cut order.
pub struct ShapedOrderDp {
    order: CutOrder,
    ga: usize,
    gb: usize,
    gc: usize,
    kb: usize,
    ks: usize,
    kr: usize,
    bar_arg: Vec<u8>,
    slab_arg_g: Vec<u8>,
    slab_arg_m: Vec<u8>,
    root_arg_g: Vec<u8>,
    root_arg_m: Vec<u8>,
    root_vals: Vec<f64>,
}

/// A stone that carries only its design; enough for the ranking's composition.
const fn blank_stone(entry_id: i64) -> PlacedStone {
    PlacedStone {
        entry_id,
        piece_origin_mm: [0.0; 3],
        piece_size_mm: [0.0; 3],
        stone_size_mm: [0.0; 3],
        table_axis: Axis::X,
        carat: 0.0,
        volume_mm3: 0.0,
        pose: StonePose {
            center_mm: [0.0; 3],
            axes: [[0.0; 3]; 3],
            mm_per_unit: 0.0,
        },
    }
}

/// A stand-in for the layout of `tree` that carries what the ranking reads: the
/// composition, the stone count, the DP `value` and the cut order. Building the
/// real layout costs one clipped fit per piece; the ranking needs none of it.
fn ranking_stub(order: CutOrder, tree: &Tree, pool: &[CandidateDesign], value: f64) -> RoughLayout {
    let stones = tree
        .slabs
        .iter()
        .flat_map(|slab| &slab.bars)
        .flat_map(|bar| &bar.leaves)
        .filter_map(|leaf| pool.get(leaf.design))
        .map(|design| blank_stone(design.entry_id))
        .collect();
    RoughLayout {
        cut_order: order,
        stones,
        cut_plan: CutPlan { slabs: Vec::new() },
        total_carat: 0.0,
        total_volume_mm3: value,
        yield_fraction: 0.0,
        exact_fit: false,
    }
}

impl ShapedOrderDp {
    /// The best total value of exactly `n` stones for this order (`-inf` when no
    /// layout holds `n`, including `n = 0` and `n` beyond the stone cap).
    #[must_use]
    pub fn root_value(&self, n: usize) -> f64 {
        self.root_vals.get(n).copied().unwrap_or(NEG)
    }

    /// Reconstructs the guillotine tree for a specific stone count `n`.
    #[must_use]
    pub fn reconstruct(&self, grid: &ShapedGrid, table: &ClippedTable, n: usize) -> Tree {
        let ord = self.order.axes();
        let [ax, bx, cx] = ord;
        let mut slabs = Vec::new();
        let mut a = self.ga;
        let mut rem_n = n;

        while a > 0 && rem_n > 0 {
            let g_a = self.root_arg_g[a * (self.kr + 1) + rem_n] as usize;
            let m_a = self.root_arg_m[a * (self.kr + 1) + rem_n] as usize;
            if g_a == 0 || m_a == 0 || g_a > a || m_a > rem_n {
                break;
            }
            let a_prime = a - g_a;
            let ra_idx = grid.range_index(ax, a_prime, a);

            let mut bars = Vec::new();
            let mut b = self.gb;
            let mut rem_b = m_a;

            while b > 0 && rem_b > 0 {
                let slab_offset =
                    ra_idx * (self.gb + 1) * (self.ks + 1) + b * (self.ks + 1) + rem_b;
                let g_b = self.slab_arg_g[slab_offset] as usize;
                let m_b = self.slab_arg_m[slab_offset] as usize;
                if g_b == 0 || m_b == 0 || g_b > b || m_b > rem_b {
                    break;
                }
                let b_prime = b - g_b;
                let rb_idx = grid.range_index(bx, b_prime, b);

                let mut leaves = Vec::new();
                let mut c = self.gc;
                let mut rem_c = m_b;

                while c > 0 && rem_c > 0 {
                    let pb = grid.range_count(bx);
                    let bar_stride = (self.gc + 1) * (self.kb + 1);
                    let bar_offset =
                        (ra_idx * pb + rb_idx) * bar_stride + c * (self.kb + 1) + rem_c;
                    let g_c = self.bar_arg[bar_offset] as usize;
                    if g_c == 0 || g_c > c {
                        break;
                    }
                    let c_prime = c - g_c;
                    let rc_idx = grid.range_index(cx, c_prime, c);

                    let mut r_canon = [0; 3];
                    r_canon[ax] = ra_idx;
                    r_canon[bx] = rb_idx;
                    r_canon[cx] = rc_idx;

                    let t_idx = table.index(r_canon[0], r_canon[1], r_canon[2]);
                    leaves.push(Leaf {
                        len: grid.run_mm(cx, g_c),
                        design: table.design[t_idx] as usize,
                        orient: table.orient[t_idx] as usize,
                    });

                    c = c_prime;
                    rem_c -= 1;
                }

                leaves.reverse();
                bars.push(Bar {
                    width: grid.run_mm(bx, g_b),
                    leaves,
                });

                b = b_prime;
                rem_b -= m_b;
            }

            bars.reverse();
            slabs.push(Slab {
                thickness: grid.run_mm(ax, g_a),
                bars,
            });

            a = a_prime;
            rem_n -= m_a;
        }

        slabs.reverse();
        Tree { slabs }
    }

    /// Evaluates candidate layouts for this cut order.
    ///
    /// Every stone count with a finite DP value is reconstructed and ranked on
    /// its DP value and composition; only the retained candidates (at most
    /// [`MAX_PER_ORDER`]) are built into real layouts.
    #[must_use]
    pub fn candidates(
        &self,
        grid: &ShapedGrid,
        table: &ClippedTable,
        ctx: &ShapedCtx,
        settings: &PlanSettings,
        pool: &[CandidateDesign],
    ) -> Vec<RoughLayout> {
        let mut trees = Vec::new();
        let mut stubs = Vec::new();
        for (n, &value) in self.root_vals.iter().enumerate().skip(1) {
            if value > NEG {
                let tree = self.reconstruct(grid, table, n);
                stubs.push(ranking_stub(self.order, &tree, pool, value));
                trees.push(tree);
            }
        }
        rank_indices(&stubs, MAX_PER_ORDER)
            .into_iter()
            .map(|i| layout_from_tree_shaped(ctx, self.order, &trees[i], pool, settings))
            .filter(|layout| !layout.stones.is_empty())
            .collect()
    }
}

struct DpDims {
    ord: [usize; 3],
    cells: [usize; 3],
    kb: usize,
    ks: usize,
    kr: usize,
}

struct RootBuffers<'a> {
    arg_g: &'a mut [u8],
    arg_m: &'a mut [u8],
    table: &'a mut [f64],
}

/// Fills the bar tables; ticks once per starting plane of the a range.
fn fill_bar_table(
    grid: &ShapedGrid,
    table: &ClippedTable,
    dims: &DpDims,
    bar_arg: &mut [u8],
    bar_vals: &mut [f64],
    tick: &mut Tick<'_>,
) -> bool {
    let [ax, bx, cx] = dims.ord;
    let [ga, gb, gc] = dims.cells;
    let pb = grid.range_count(bx);
    let bar_stride = (gc + 1) * (dims.kb + 1);
    let mut t_buf = vec![NEG; bar_stride];

    for ra_start in 0..ga {
        if !tick() {
            return false;
        }
        for ra_end in (ra_start + 1)..=ga {
            let ra_idx = grid.range_index(ax, ra_start, ra_end);
            for rb_start in 0..gb {
                for rb_end in (rb_start + 1)..=gb {
                    let rb_idx = grid.range_index(bx, rb_start, rb_end);
                    let bar_pair_idx = ra_idx * pb + rb_idx;

                    t_buf.fill(NEG);
                    t_buf[0] = 0.0;

                    for c in 1..=gc {
                        for n in 1..=c.min(dims.kb) {
                            let mut best = NEG;
                            let mut best_g = 0usize;
                            for g in 1..=c {
                                let c_prime = c - g;
                                let prev = t_buf[c_prime * (dims.kb + 1) + n - 1];
                                if prev > NEG {
                                    let rc_idx = grid.range_index(cx, c_prime, c);
                                    let mut r_canon = [0; 3];
                                    r_canon[ax] = ra_idx;
                                    r_canon[bx] = rb_idx;
                                    r_canon[cx] = rc_idx;
                                    let v = table.value(r_canon[0], r_canon[1], r_canon[2]);
                                    if v > NEG {
                                        let cand = prev + v;
                                        if cand > best {
                                            best = cand;
                                            best_g = g;
                                        }
                                    }
                                }
                            }
                            let offset = c * (dims.kb + 1) + n;
                            t_buf[offset] = best;
                            bar_arg[bar_pair_idx * bar_stride + offset] = best_g as u8;
                        }
                    }

                    for m in 1..=dims.kb {
                        bar_vals[bar_pair_idx * (dims.kb + 1) + m] = t_buf[gc * (dims.kb + 1) + m];
                    }
                }
            }
        }
    }
    true
}

/// Fills the slab tables; ticks once per starting plane of the a range.
fn fill_slab_table(
    grid: &ShapedGrid,
    dims: &DpDims,
    bar_vals: &[f64],
    slab_args: (&mut [u8], &mut [u8]),
    slab_vals: &mut [f64],
    tick: &mut Tick<'_>,
) -> bool {
    let (slab_arg_g, slab_arg_m) = slab_args;
    let [ax, bx, _] = dims.ord;
    let [ga, gb, _] = dims.cells;
    let pb = grid.range_count(bx);
    let slab_stride = (gb + 1) * (dims.ks + 1);
    let mut s_buf = vec![NEG; slab_stride];

    for ra_start in 0..ga {
        if !tick() {
            return false;
        }
        for ra_end in (ra_start + 1)..=ga {
            let ra_idx = grid.range_index(ax, ra_start, ra_end);

            s_buf.fill(NEG);
            s_buf[0] = 0.0;

            for b in 1..=gb {
                for n in 1..=(b * dims.kb).min(dims.ks) {
                    let mut best = NEG;
                    let mut best_g = 0usize;
                    let mut best_m = 0usize;

                    for g in 1..=b {
                        let b_prime = b - g;
                        let rb_idx = grid.range_index(bx, b_prime, b);
                        let bar_pair_idx = ra_idx * pb + rb_idx;

                        for m in 1..=n.min(dims.kb) {
                            let prev = s_buf[b_prime * (dims.ks + 1) + n - m];
                            let b_val = bar_vals[bar_pair_idx * (dims.kb + 1) + m];
                            if prev > NEG && b_val > NEG {
                                let cand = prev + b_val;
                                if cand > best {
                                    best = cand;
                                    best_g = g;
                                    best_m = m;
                                }
                            }
                        }
                    }

                    let offset = b * (dims.ks + 1) + n;
                    s_buf[offset] = best;
                    slab_arg_g[ra_idx * slab_stride + offset] = best_g as u8;
                    slab_arg_m[ra_idx * slab_stride + offset] = best_m as u8;
                }
            }

            for m in 1..=dims.ks {
                slab_vals[ra_idx * (dims.ks + 1) + m] = s_buf[gb * (dims.ks + 1) + m];
            }
        }
    }
    true
}

/// Fills the root table; ticks once per plane of the a axis.
fn fill_root_table(
    grid: &ShapedGrid,
    dims: &DpDims,
    slab_vals: &[f64],
    bufs: &mut RootBuffers<'_>,
    tick: &mut Tick<'_>,
) -> bool {
    let [ax, _, _] = dims.ord;
    let [ga, _, _] = dims.cells;
    let root_stride = dims.kr + 1;
    for a in 1..=ga {
        if !tick() {
            return false;
        }
        for n in 1..=(a * dims.ks).min(dims.kr) {
            let mut best = NEG;
            let mut best_g = 0usize;
            let mut best_m = 0usize;

            for g in 1..=a {
                let a_prime = a - g;
                let ra_idx = grid.range_index(ax, a_prime, a);

                for m in 1..=n.min(dims.ks) {
                    let prev = bufs.table[a_prime * root_stride + n - m];
                    let s_val = slab_vals[ra_idx * (dims.ks + 1) + m];
                    if prev > NEG && s_val > NEG {
                        let cand = prev + s_val;
                        if cand > best {
                            best = cand;
                            best_g = g;
                            best_m = m;
                        }
                    }
                }
            }

            let offset = a * root_stride + n;
            bufs.table[offset] = best;
            bufs.arg_g[offset] = best_g as u8;
            bufs.arg_m[offset] = best_m as u8;
        }
    }
    true
}

/// Runs the three DP levels of `order` for at most `k` stones; `None` when `tick`
/// cancels.
pub(super) fn solve_order(
    grid: &ShapedGrid,
    table: &ClippedTable,
    order: CutOrder,
    k: usize,
    tick: &mut Tick<'_>,
) -> Option<ShapedOrderDp> {
    let [ax, bx, cx] = order.axes();
    let ga = grid.cells[ax];
    let gb = grid.cells[bx];
    let gc = grid.cells[cx];

    let kb = k.min(gc);
    let ks = k.min(gb * kb);
    let kr = k;
    let dims = DpDims {
        ord: [ax, bx, cx],
        cells: [ga, gb, gc],
        kb,
        ks,
        kr,
    };

    let pa = grid.range_count(ax);
    let pb = grid.range_count(bx);

    let mut bar_arg = vec![0u8; pa * pb * (gc + 1) * (kb + 1)];
    let mut bar_vals = vec![NEG; pa * pb * (kb + 1)];
    if !fill_bar_table(grid, table, &dims, &mut bar_arg, &mut bar_vals, tick) {
        return None;
    }

    let slab_stride = (gb + 1) * (ks + 1);
    let mut slab_arg_g = vec![0u8; pa * slab_stride];
    let mut slab_arg_m = vec![0u8; pa * slab_stride];
    let mut slab_vals = vec![NEG; pa * (ks + 1)];
    let slab_args = (&mut slab_arg_g[..], &mut slab_arg_m[..]);
    if !fill_slab_table(grid, &dims, &bar_vals, slab_args, &mut slab_vals, tick) {
        return None;
    }

    let root_stride = kr + 1;
    let mut root_arg_g = vec![0u8; (ga + 1) * root_stride];
    let mut root_arg_m = vec![0u8; (ga + 1) * root_stride];
    let mut root_table = vec![NEG; (ga + 1) * root_stride];
    root_table[0] = 0.0;
    let mut root_bufs = RootBuffers {
        arg_g: &mut root_arg_g,
        arg_m: &mut root_arg_m,
        table: &mut root_table,
    };
    if !fill_root_table(grid, &dims, &slab_vals, &mut root_bufs, tick) {
        return None;
    }

    let root_vals = (0..=kr)
        .map(|n| {
            if n == 0 {
                NEG
            } else {
                root_table[ga * root_stride + n]
            }
        })
        .collect();
    Some(ShapedOrderDp {
        order,
        ga,
        gb,
        gc,
        kb,
        ks,
        kr,
        bar_arg,
        slab_arg_g,
        slab_arg_m,
        root_arg_g,
        root_arg_m,
        root_vals,
    })
}

/// Solves positional DP for a given cut order.
///
/// Reports [`PlanProgress::Dp`] once per starting plane in the bar and slab
/// fills and once per plane in the root fill, so a cancel is seen within a
/// fraction of the order's work.
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
    let dp = solve_order(grid, table, order, settings.count_usize(), &mut tick)?;
    Some(dp.candidates(grid, table, ctx, settings, pool))
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
    /// Scoped threads each round's size table and clipped table are built on (`1` builds
    /// on the calling thread and spawns nothing). The result never depends on it.
    pub lanes: usize,
}

/// The clipped table of one alternatives round over the round's `pool`.
///
/// The size-only table of interior pieces is rebuilt for `pool`: its design
/// indices point into the pool it was built from, so the baseline's table would
/// score the round with designs that may no longer exist. The classification is
/// reused from the baseline table. Both tables are built on `params.lanes` threads.
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
    build_clipped_table_lanes(&clip_params, params.lanes, on_progress)
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
