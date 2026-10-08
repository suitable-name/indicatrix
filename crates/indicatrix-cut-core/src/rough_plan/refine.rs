//! Continuous refinement of a layout's cut positions.
//!
//! The tree structure (slabs, bars, piece counts) stays; the boundary between
//! each pair of neighbours moves with the pair's total fixed. The objective
//! along one boundary is NOT unimodal (where both neighbours are bound by the
//! moving axis it is `c1 x^3 + c2 (L - x)^3`, convex, with plateaus elsewhere),
//! so a bare golden-section search is unsafe. Each boundary instead evaluates
//! 64 even samples, every kink position (where a stone becomes bound by
//! another axis, or becomes wide enough to be feasible) and the current
//! position; takes the best sample; then runs a golden-section search only
//! inside the bracket between its two neighbouring samples. The move is
//! accepted only if strictly better. While boundaries move, every leaf keeps
//! its design and assignment, so a leaf costs O(1); designs are re-picked after
//! each pass. The result is never worse than the input.

use std::borrow::Borrow;

use super::{
    FINAL_TOP, PASSES, REFINE_TOP, REL_TOL,
    pareto::sanitize,
    piece::{ASSIGNMENTS, Norm, best_pick, stone_value},
    rank::{as_layout, flatten_groups, rank_indices_min, take_indices},
    shaped::parallel::run_lanes_dynamic,
    tree::{Bar, Leaf, Slab, Tree, layout_from_tree, to_canonical},
    types::{CandidateDesign, LayoutGroup, PlanProgress, PlanSettings, RoughBlock, RoughLayout},
};

/// Even samples per boundary.
const SAMPLES: usize = 64;
/// Golden-section iterations inside the best bracket.
const GOLDEN_STEPS: usize = 40;
/// `1 / phi`.
const INV_PHI: f64 = 0.618_033_988_749_894_9;

/// What every leaf evaluation needs.
struct Ctx<'a> {
    /// Canonical axes of stages 1, 2, 3.
    ord: [usize; 3],
    /// Allowance per side, in mm.
    allowance: f64,
    /// Minimum finished stone width, in mm.
    min_width: f64,
    /// The designs a leaf may use.
    pool: &'a [CandidateDesign],
    /// The pool, normalised.
    norms: Vec<Norm>,
}

impl Ctx<'_> {
    /// The usable stone box of a piece with stage sizes `dims`.
    fn usable(&self, dims: [f64; 3]) -> [f64; 3] {
        to_canonical(self.ord, dims).map(|s| 2.0f64.mul_add(-self.allowance, s))
    }

    /// One leaf's finished volume in a piece of stage sizes `dims`.
    fn leaf_value(&self, dims: [f64; 3], leaf: &Leaf) -> f64 {
        stone_value(
            &self.norms[leaf.design],
            leaf.orient,
            self.usable(dims),
            self.min_width,
        )
    }

    /// The summed value of one bar of width `w` in a slab of thickness `t`.
    fn bar_value(&self, t: f64, w: f64, bar: &Bar) -> f64 {
        bar.leaves
            .iter()
            .map(|l| self.leaf_value([t, w, l.len], l))
            .sum()
    }

    /// The summed value of one slab of thickness `t`.
    fn slab_value(&self, t: f64, slab: &Slab) -> f64 {
        slab.bars
            .iter()
            .map(|b| self.bar_value(t, b.width, b))
            .sum()
    }

    /// The whole tree's value.
    fn tree_value(&self, tree: &Tree) -> f64 {
        tree.slabs
            .iter()
            .map(|s| self.slab_value(s.thickness, s))
            .sum()
    }

    /// Push the piece sizes along stage `q` at which `leaf` (in a piece of
    /// stage sizes `dims`) becomes bound by another axis, or wide enough to
    /// be feasible. With `flip = Some(total)` the positions are mirrored for
    /// the second neighbour of a pair.
    fn push_kinks(
        &self,
        dims: [f64; 3],
        q: usize,
        leaf: &Leaf,
        flip: Option<f64>,
        out: &mut Vec<f64>,
    ) {
        let frame = KinkFrame {
            ord: self.ord,
            allowance: self.allowance,
            min_width: self.min_width,
        };
        frame.push(&self.norms[leaf.design], leaf.orient, dims, q, flip, out);
    }
}

/// What the kink positions of a leaf depend on besides the leaf itself.
#[derive(Debug, Clone, Copy)]
pub struct KinkFrame {
    /// Canonical axes of stages 1, 2, 3.
    pub ord: [usize; 3],
    /// Allowance per side, in mm.
    pub allowance: f64,
    /// Minimum finished stone width, in mm.
    pub min_width: f64,
}

impl KinkFrame {
    /// Push the piece sizes along stage `q` at which a stone of `norm` under
    /// assignment `orient` (in a piece of stage sizes `dims`) becomes bound by
    /// another axis, or wide enough to be feasible. With `flip = Some(total)`
    /// the positions are mirrored for the second neighbour of a pair.
    ///
    /// The feasibility kink is placed at the NOMINAL minimum width. [`stone_value`] accepts
    /// anything down to `min_width * (1 - 1e-12)`, so the stone at that position is feasible
    /// whichever way the last bits of `x - 2a` round, and the kink sits within 1e-12 relative
    /// of the exact boundary.
    pub fn push(
        &self,
        norm: &Norm,
        orient: usize,
        dims: [f64; 3],
        q: usize,
        flip: Option<f64>,
        out: &mut Vec<f64>,
    ) {
        let a2 = 2.0 * self.allowance;
        let assign = &ASSIGNMENTS[orient];
        let d = norm.dims();
        let p = to_canonical(self.ord, dims).map(|s| 2.0f64.mul_add(-self.allowance, s));
        let axis = self.ord[q];
        let (mut others, mut moving) = (f64::INFINITY, 1.0);
        for (&piece_axis, &dim) in assign.iter().zip(&d) {
            if piece_axis == axis {
                moving = dim;
            } else {
                others = others.min(p[piece_axis] / dim);
            }
        }
        for scale in [others, self.min_width] {
            if scale.is_finite() && scale > 0.0 {
                let x = scale.mul_add(moving, a2);
                out.push(flip.map_or(x, |t| t - x));
            }
        }
    }
}

/// Remember `(x, value)` if strictly better than `best`.
pub fn consider(best: &mut (f64, f64), x: f64, value: f64) {
    if value > best.1 {
        *best = (x, value);
    }
}

/// Golden-section search for a maximum of `eval` inside `[lo, hi]`.
pub fn golden(lo: f64, hi: f64, eval: &dyn Fn(f64) -> f64, best: &mut (f64, f64)) {
    let (mut a, mut b) = (lo, hi);
    let mut c = (-INV_PHI).mul_add(b - a, b);
    let mut d = INV_PHI.mul_add(b - a, a);
    let (mut fc, mut fd) = (eval(c), eval(d));
    consider(best, c, fc);
    consider(best, d, fd);
    for _ in 0..GOLDEN_STEPS {
        if fc > fd {
            b = d;
            d = c;
            fd = fc;
            c = (-INV_PHI).mul_add(b - a, b);
            fc = eval(c);
            consider(best, c, fc);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = INV_PHI.mul_add(b - a, a);
            fd = eval(d);
            consider(best, d, fd);
        }
    }
}

/// The best position of a boundary between two neighbours whose sizes sum to
/// `total`, starting at `current`. Returns `current` unless strictly better.
pub fn optimise_boundary(
    total: f64,
    current: f64,
    kinks: &[f64],
    eval: &dyn Fn(f64) -> f64,
) -> f64 {
    optimise_boundary_samples(total, current, kinks, SAMPLES, eval)
}

/// Boundary optimisation with a configurable number of samples.
pub fn optimise_boundary_samples(
    total: f64,
    current: f64,
    kinks: &[f64],
    samples: usize,
    eval: &dyn Fn(f64) -> f64,
) -> f64 {
    let current_value = eval(current);
    let mut xs: Vec<f64> = (1..=samples)
        .map(|j| total * j as f64 / (samples + 1) as f64)
        .collect();
    xs.extend(kinks.iter().copied().filter(|x| *x > 0.0 && *x < total));
    xs.push(current);
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    let values: Vec<f64> = xs.iter().map(|&x| eval(x)).collect();
    let mut best_i = 0;
    for (i, v) in values.iter().enumerate() {
        if *v > values[best_i] {
            best_i = i;
        }
    }
    let mut best = (xs[best_i], values[best_i]);
    // At either end of the list the bracket extends to the segment's end
    // instead of collapsing onto the sample.
    let lo = if best_i == 0 { 0.0 } else { xs[best_i - 1] };
    let hi = xs.get(best_i + 1).copied().unwrap_or(total);
    golden(lo, hi, eval, &mut best);
    if best.1 > current_value {
        best.0
    } else {
        current
    }
}

/// Move the boundary between two adjacent slabs.
fn move_slab_boundary(ctx: &Ctx<'_>, pair: &mut [Slab]) {
    let total = pair[0].thickness + pair[1].thickness;
    let mut kinks = Vec::new();
    for (side, slab) in pair.iter().enumerate() {
        let flip = (side == 1).then_some(total);
        for bar in &slab.bars {
            for leaf in &bar.leaves {
                ctx.push_kinks([0.0, bar.width, leaf.len], 0, leaf, flip, &mut kinks);
            }
        }
    }
    let x = optimise_boundary(total, pair[0].thickness, &kinks, &|x| {
        ctx.slab_value(x, &pair[0]) + ctx.slab_value(total - x, &pair[1])
    });
    pair[0].thickness = x;
    pair[1].thickness = total - x;
}

/// Move the boundary between two adjacent bars of a slab of thickness `t`.
fn move_bar_boundary(ctx: &Ctx<'_>, t: f64, pair: &mut [Bar]) {
    let total = pair[0].width + pair[1].width;
    let mut kinks = Vec::new();
    for (side, bar) in pair.iter().enumerate() {
        let flip = (side == 1).then_some(total);
        for leaf in &bar.leaves {
            ctx.push_kinks([t, 0.0, leaf.len], 1, leaf, flip, &mut kinks);
        }
    }
    let x = optimise_boundary(total, pair[0].width, &kinks, &|x| {
        ctx.bar_value(t, x, &pair[0]) + ctx.bar_value(t, total - x, &pair[1])
    });
    pair[0].width = x;
    pair[1].width = total - x;
}

/// Move the boundary between two adjacent pieces of a bar (`t` by `w`).
fn move_leaf_boundary(ctx: &Ctx<'_>, t: f64, w: f64, pair: &mut [Leaf]) {
    let total = pair[0].len + pair[1].len;
    let mut kinks = Vec::new();
    for (side, leaf) in pair.iter().enumerate() {
        let flip = (side == 1).then_some(total);
        ctx.push_kinks([t, w, 0.0], 2, leaf, flip, &mut kinks);
    }
    let x = optimise_boundary(total, pair[0].len, &kinks, &|x| {
        ctx.leaf_value([t, w, x], &pair[0]) + ctx.leaf_value([t, w, total - x], &pair[1])
    });
    pair[0].len = x;
    pair[1].len = total - x;
}

/// One pass over every boundary: slabs, then each slab's bars, then pieces.
fn refine_pass(ctx: &Ctx<'_>, tree: &mut Tree) {
    for i in 0..tree.slabs.len().saturating_sub(1) {
        move_slab_boundary(ctx, &mut tree.slabs[i..=i + 1]);
    }
    for slab in &mut tree.slabs {
        let t = slab.thickness;
        for i in 0..slab.bars.len().saturating_sub(1) {
            move_bar_boundary(ctx, t, &mut slab.bars[i..=i + 1]);
        }
        for bar in &mut slab.bars {
            let w = bar.width;
            for i in 0..bar.leaves.len().saturating_sub(1) {
                move_leaf_boundary(ctx, t, w, &mut bar.leaves[i..=i + 1]);
            }
        }
    }
}

/// Re-pick every leaf's design and assignment over the pool.
fn repick(ctx: &Ctx<'_>, tree: &mut Tree) {
    for slab in &mut tree.slabs {
        for bar in &mut slab.bars {
            for leaf in &mut bar.leaves {
                let p = ctx.usable([slab.thickness, bar.width, leaf.len]);
                if let Some((design, orient, _)) = best_pick(&ctx.norms, p, ctx.min_width) {
                    leaf.design = design;
                    leaf.orient = orient;
                }
            }
        }
    }
}

/// The tree of `layout`, each leaf on its own design with its best assignment.
/// `None` when a design is missing from the pool or a piece is infeasible.
fn tree_from_layout(ctx: &Ctx<'_>, layout: &RoughLayout) -> Option<Tree> {
    let mut next = layout.stones.iter();
    let mut slabs = Vec::new();
    for s in &layout.cut_plan.slabs {
        let mut bars = Vec::new();
        for b in &s.bars {
            let mut leaves = Vec::new();
            for &len in &b.pieces_mm {
                let stone = next.next()?;
                let design = ctx.pool.iter().position(|d| d.entry_id == stone.entry_id)?;
                let p = ctx.usable([s.thickness_mm, b.width_mm, len]);
                let (_, orient, _) = best_pick(&ctx.norms[design..=design], p, ctx.min_width)?;
                leaves.push(Leaf {
                    len,
                    design,
                    orient,
                });
            }
            bars.push(Bar {
                width: b.width_mm,
                leaves,
            });
        }
        slabs.push(Slab {
            thickness: s.thickness_mm,
            bars,
        });
    }
    Some(Tree { slabs })
}

/// [`refine`] against an already-sanitised pool.
fn refine_with_pool(
    rough: &RoughBlock,
    settings: &PlanSettings,
    pool: &[CandidateDesign],
    layout: &RoughLayout,
) -> RoughLayout {
    let ctx = Ctx {
        ord: layout.cut_order.axes(),
        allowance: settings.allowance_mm,
        min_width: settings.min_width_mm,
        pool,
        norms: pool.iter().map(Norm::of).collect(),
    };
    let Some(mut tree) = tree_from_layout(&ctx, layout) else {
        return layout.clone();
    };
    let mut previous = ctx.tree_value(&tree);
    for _ in 0..PASSES {
        refine_pass(&ctx, &mut tree);
        repick(&ctx, &mut tree);
        let now = ctx.tree_value(&tree);
        let gain = now - previous;
        previous = now;
        if gain <= REL_TOL * now.abs() {
            break;
        }
    }
    let refined = layout_from_tree(rough, settings, layout.cut_order, &tree, pool);
    if refined.total_volume_mm3 > layout.total_volume_mm3 {
        refined
    } else {
        layout.clone()
    }
}

/// Continuously optimise the cut positions of `layout`, keeping its tree
/// structure and per-leaf stone counts.
///
/// After each pass every leaf is re-picked over `designs` (its best design and
/// assignment); pass a one-design slice to restrict a single-design layout to
/// re-picking its assignment. Stops after three passes or when the gain falls
/// below 1e-9 relative. The result is never worse than the input: when the
/// refinement finds nothing strictly better (or `designs` lacks one of the
/// layout's designs) the input is returned unchanged.
#[must_use]
pub fn refine(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
    layout: &RoughLayout,
) -> RoughLayout {
    refine_with_pool(rough, settings, &sanitize(designs), layout)
}

/// The designs of `pool` that `layout` uses (the pool of a group without one).
#[must_use]
pub fn own_pool(pool: &[CandidateDesign], layout: &RoughLayout) -> Vec<CandidateDesign> {
    pool.iter()
        .filter(|d| layout.stones.iter().any(|s| s.entry_id == d.entry_id))
        .copied()
        .collect()
}

/// The final ranked top 10 of `refined`.
///
/// Refinement can merge two compositions into one; when fewer than
/// [`FINAL_TOP`] remain, the list is topped up from every candidate of `flat`
/// whose index is not in `refined_indices` (the ones that were refined),
/// including the worse duplicates ranking dropped, so a composition a refined
/// layout abandoned can still return.
///
/// `flat` may hold layouts or references to them (see [`flatten_groups`]); only the layouts
/// that end up in the result are cloned.
#[must_use]
pub fn final_ranking<L: Borrow<RoughLayout>>(
    refined: Vec<RoughLayout>,
    flat: &[L],
    refined_indices: &[usize],
) -> Vec<RoughLayout> {
    final_ranking_min(refined, flat, refined_indices, 1)
}

/// [`final_ranking`] keeping only layouts of at least `min_stones` stones, in the refined
/// list and in the top-up pool alike (refinement can merge a layout below the floor).
#[must_use]
pub fn final_ranking_min<L: Borrow<RoughLayout>>(
    refined: Vec<RoughLayout>,
    flat: &[L],
    refined_indices: &[usize],
    min_stones: usize,
) -> Vec<RoughLayout> {
    let keep = rank_indices_min(&refined, FINAL_TOP, min_stones);
    if keep.len() >= FINAL_TOP || refined_indices.len() >= flat.len() {
        return take_indices(refined, &keep);
    }
    let mut is_refined = vec![false; flat.len()];
    for &i in refined_indices {
        if let Some(slot) = is_refined.get_mut(i) {
            *slot = true;
        }
    }
    let unrefined = flat
        .iter()
        .zip(&is_refined)
        .filter(|(_, done)| !**done)
        .map(|(layout, _)| as_layout(layout));
    let pool: Vec<&RoughLayout> = refined.iter().chain(unrefined).collect();
    rank_indices_min(&pool, FINAL_TOP, min_stones)
        .into_iter()
        .map(|i| pool[i].clone())
        .collect()
}

/// The last steps of a plan: rank every candidate, refine the best 20, and
/// return the final ranked top 10.
///
/// When refinement merges compositions and fewer than 10 remain, the list is
/// topped up with the unrefined candidates ranked 21 and below.
///
/// `designs` is the full candidate list; each [`LayoutGroup`] names the pool
/// its layouts may re-pick from (an empty pool restricts each layout to its
/// own designs). Reports [`PlanProgress::Refine`] before each layout; `None`
/// when `on_progress` cancels.
pub fn finish_plan(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
    groups: &[LayoutGroup],
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    finish_plan_lanes(rough, settings, designs, groups, 1, on_progress)
}

/// [`finish_plan`] with the refinements of the best 20 spread over `lanes` scoped threads.
///
/// The layouts to refine are fixed by the ranking before any thread starts, each refinement
/// reads only its own layout and pool, and the results are collected in rank order before
/// the one final ranking, so the output is bitwise the serial one whatever the lane count.
/// A lane reports [`PlanProgress::Refine`] before each layout it takes, on the calling
/// thread; `None` when `on_progress` cancels, which every lane notices before its next
/// layout. With one lane nothing is spawned.
pub fn finish_plan_lanes(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
    groups: &[LayoutGroup],
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    let all = sanitize(designs);
    let (flat, group_of) = flatten_groups(groups);
    let min_stones = settings.min_count_usize();
    let ranked = rank_indices_min(&flat, usize::MAX, min_stones);
    let split = ranked.len().min(REFINE_TOP);
    let refined = run_lanes_dynamic(
        (0..split).collect(),
        lanes,
        |slot, report| {
            if !report(PlanProgress::Refine) {
                return None;
            }
            let i = ranked[slot];
            let group = &groups[group_of[i]];
            let pool = if group.pool.is_empty() {
                own_pool(&all, flat[i])
            } else {
                sanitize(&group.pool)
            };
            Some(refine_with_pool(rough, settings, &pool, flat[i]))
        },
        on_progress,
    )?;
    Some(final_ranking_min(
        refined,
        &flat,
        &ranked[..split],
        min_stones,
    ))
}
