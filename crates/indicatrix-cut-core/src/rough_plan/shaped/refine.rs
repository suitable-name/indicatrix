//! Continuous spatial refinement of guillotine cut positions in shaped roughs.
//!
//! Mirrors plain-block refinement with 32 boundary samples, evaluating pieces at their
//! absolute spatial coordinates and resolving LP bounds for clipped partial pieces.
//! Every boundary also tries the kink positions of its neighbours (where a stone becomes
//! bound by another axis or wide enough to be feasible): they are exact for interior
//! pieces and harmless extra samples for clipped ones.

use std::cell::RefCell;

use glam::DVec3;

use super::{
    clip::{CLASS_EXTERIOR, CLASS_INTERIOR, classify_box_in},
    ctx::ShapedCtx,
    rows::{ClipRegion, PartialSolver, caliper_extents},
    tree::layout_from_tree_shaped,
};
use crate::rough_plan::{
    CandidateDesign, FitMesh, PASSES, PlanSettings, REL_TOL, RoughLayout,
    piece::{ASSIGNMENTS, Norm, min_width_floor, stone_value},
    refine::{KinkFrame, optimise_boundary_samples},
    tree::{Bar, Leaf, Slab, Tree, orient_of_pose, to_canonical},
};

/// Even samples per boundary (half the plain planner's, to bound the LP count).
const SAMPLES: usize = 32;
/// Relative margin by which a refined layout must beat its input to replace it.
const ACCEPT_REL: f64 = 1e-12;

/// Reusable buffers of a leaf evaluation.
struct EvalWork {
    solver: PartialSolver,
    violated: Vec<usize>,
}

/// A piece's stone box: `(min corner, max corner, usable extents)`.
type StoneBox = ([f64; 3], [f64; 3], [f64; 3]);

/// Workspace context for evaluating leaf values at continuous positions.
struct LeafEval<'a> {
    ord: [usize; 3],
    /// Corner of the first piece, canonical mm.
    origin: [f64; 3],
    allowance: f64,
    kerf: f64,
    min_width: f64,
    norms: Vec<Norm>,
    non_box: &'a [(DVec3, f64)],
    /// The rough's mesh, which a leaf's stone must also stay inside.
    mesh: Option<FitMesh<'a>>,
    work: RefCell<EvalWork>,
}

impl LeafEval<'_> {
    /// The stone box of the piece at `origin` with `size`; its usable extents are
    /// computed from the size alone, as the layout builder's fitter does.
    fn stone_box(&self, origin: [f64; 3], size: [f64; 3]) -> StoneBox {
        let b_min = origin.map(|o| o + self.allowance);
        let b_max = [0, 1, 2].map(|i| origin[i] + size[i] - self.allowance);
        let usable = size.map(|s| 2.0f64.mul_add(-self.allowance, s));
        (b_min, b_max, usable)
    }

    /// The finished volume of `design_idx` under `orient` in the piece at
    /// `origin` with `size`; `0` when nothing fits.
    fn leaf_val_at(
        &self,
        origin: [f64; 3],
        size: [f64; 3],
        design_idx: usize,
        orient: usize,
    ) -> f64 {
        let (b_min, b_max, usable) = self.stone_box(origin, size);
        if usable.iter().any(|&s| s <= 0.0) {
            return 0.0;
        }

        let norm = &self.norms[design_idx];
        let mut guard = self.work.borrow_mut();
        let work = &mut *guard;
        match classify_box_in(b_min, b_max, self.non_box, self.mesh, &mut work.violated) {
            CLASS_INTERIOR => {
                let v = stone_value(norm, orient, usable, self.min_width);
                if v > 0.0 { v } else { 0.0 }
            }
            CLASS_EXTERIOR => 0.0,
            _ => {
                let region = ClipRegion {
                    min: b_min,
                    max: b_max,
                    planes: self.non_box,
                    violated: &work.violated,
                };
                match work
                    .solver
                    .solve_with(&region, caliper_extents(norm, orient), self.mesh)
                {
                    Some((k, _)) if k >= min_width_floor(self.min_width) => norm.f * (k * k * k),
                    _ => 0.0,
                }
            }
        }
    }

    fn bar_value_at(&self, t_off: f64, w_off: f64, t: f64, w: f64, bar: &Bar) -> f64 {
        let mut total = 0.0;
        let mut l_off = self.origin[self.ord[2]];
        for leaf in &bar.leaves {
            let origin = to_canonical(self.ord, [t_off, w_off, l_off]);
            let size = to_canonical(self.ord, [t, w, leaf.len]);
            total += self.leaf_val_at(origin, size, leaf.design, leaf.orient);
            l_off += leaf.len + self.kerf;
        }
        total
    }

    fn slab_value_at(&self, t_off: f64, t: f64, slab: &Slab) -> f64 {
        let mut total = 0.0;
        let mut w_off = self.origin[self.ord[1]];
        for bar in &slab.bars {
            total += self.bar_value_at(t_off, w_off, t, bar.width, bar);
            w_off += bar.width + self.kerf;
        }
        total
    }

    fn tree_value(&self, tree: &Tree) -> f64 {
        let mut total = 0.0;
        let mut t_off = self.origin[self.ord[0]];
        for slab in &tree.slabs {
            total += self.slab_value_at(t_off, slab.thickness, slab);
            t_off += slab.thickness + self.kerf;
        }
        total
    }

    /// Pushes the kink positions of `leaf` (in a piece of stage sizes `dims`)
    /// along stage `q`; see [`KinkFrame::push`].
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

    /// The best design and assignment of the piece at `origin` with `size`,
    /// starting from `(design, orient)`: a candidate replaces the incumbent only
    /// when strictly better than the incumbent's own value.
    ///
    /// The candidates are visited in descending order of their unclipped box value
    /// (ties by design, then assignment), and the search ends at the first one that
    /// cannot beat the best found: the clipped value never exceeds the box value.
    fn best_pick_at(
        &self,
        origin: [f64; 3],
        size: [f64; 3],
        incumbent: (usize, usize),
    ) -> (usize, usize) {
        let usable = self.stone_box(origin, size).2;
        let mut best_val = self.leaf_val_at(origin, size, incumbent.0, incumbent.1);
        let mut best = incumbent;
        let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
        for (d_idx, norm) in self.norms.iter().enumerate() {
            for o_idx in 0..ASSIGNMENTS.len() {
                let bound = stone_value(norm, o_idx, usable, self.min_width);
                if bound > best_val && (d_idx, o_idx) != incumbent {
                    candidates.push((bound, d_idx, o_idx));
                }
            }
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        for (bound, d_idx, o_idx) in candidates {
            if bound <= best_val {
                break;
            }
            let v = self.leaf_val_at(origin, size, d_idx, o_idx);
            if v > best_val {
                best_val = v;
                best = (d_idx, o_idx);
            }
        }
        best
    }

    /// Re-picks every leaf's design and assignment over the pool.
    fn repick_tree(&self, tree: &mut Tree) {
        let mut t_off = self.origin[self.ord[0]];
        for slab in &mut tree.slabs {
            let mut w_off = self.origin[self.ord[1]];
            for bar in &mut slab.bars {
                let mut l_off = self.origin[self.ord[2]];
                for leaf in &mut bar.leaves {
                    let origin = to_canonical(self.ord, [t_off, w_off, l_off]);
                    let size = to_canonical(self.ord, [slab.thickness, bar.width, leaf.len]);
                    (leaf.design, leaf.orient) =
                        self.best_pick_at(origin, size, (leaf.design, leaf.orient));
                    l_off += leaf.len + self.kerf;
                }
                w_off += bar.width + self.kerf;
            }
            t_off += slab.thickness + self.kerf;
        }
    }
}

/// Moves the boundary between two adjacent slabs starting at `t_off`.
fn move_slab_boundary(ev: &LeafEval<'_>, t_off: f64, pair: &mut [Slab]) {
    let total = pair[0].thickness + pair[1].thickness;
    let mut kinks = Vec::new();
    for (side, slab) in pair.iter().enumerate() {
        let flip = (side == 1).then_some(total);
        for bar in &slab.bars {
            for leaf in &bar.leaves {
                ev.push_kinks([0.0, bar.width, leaf.len], 0, leaf, flip, &mut kinks);
            }
        }
    }
    let x = optimise_boundary_samples(total, pair[0].thickness, &kinks, SAMPLES, &|x| {
        ev.slab_value_at(t_off, x, &pair[0])
            + ev.slab_value_at(t_off + x + ev.kerf, total - x, &pair[1])
    });
    // A boundary that stays put keeps both sizes bitwise: `total - x` would not
    // give the second size back.
    if x != pair[0].thickness {
        pair[0].thickness = x;
        pair[1].thickness = total - x;
    }
}

/// Moves the boundary between two adjacent bars of a slab of thickness `t` at
/// `(t_off, w_off)`.
fn move_bar_boundary(ev: &LeafEval<'_>, offsets: [f64; 2], t: f64, pair: &mut [Bar]) {
    let [t_off, w_off] = offsets;
    let total = pair[0].width + pair[1].width;
    let mut kinks = Vec::new();
    for (side, bar) in pair.iter().enumerate() {
        let flip = (side == 1).then_some(total);
        for leaf in &bar.leaves {
            ev.push_kinks([t, 0.0, leaf.len], 1, leaf, flip, &mut kinks);
        }
    }
    let x = optimise_boundary_samples(total, pair[0].width, &kinks, SAMPLES, &|x| {
        ev.bar_value_at(t_off, w_off, t, x, &pair[0])
            + ev.bar_value_at(t_off, w_off + x + ev.kerf, t, total - x, &pair[1])
    });
    if x != pair[0].width {
        pair[0].width = x;
        pair[1].width = total - x;
    }
}

/// Moves the boundary between two adjacent pieces of a bar (`t` by `w`) whose
/// first piece starts at `start`.
fn move_leaf_boundary(ev: &LeafEval<'_>, start: [f64; 3], cross: [f64; 2], pair: &mut [Leaf]) {
    let [t, w] = cross;
    let total = pair[0].len + pair[1].len;
    let mut kinks = Vec::new();
    for (side, leaf) in pair.iter().enumerate() {
        let flip = (side == 1).then_some(total);
        ev.push_kinks([t, w, 0.0], 2, leaf, flip, &mut kinks);
    }
    let (leaf_a, leaf_b) = (&pair[0], &pair[1]);
    let x = optimise_boundary_samples(total, leaf_a.len, &kinks, SAMPLES, &|l| {
        let second = [start[0], start[1], start[2] + l + ev.kerf];
        ev.leaf_val_at(
            to_canonical(ev.ord, start),
            to_canonical(ev.ord, [t, w, l]),
            leaf_a.design,
            leaf_a.orient,
        ) + ev.leaf_val_at(
            to_canonical(ev.ord, second),
            to_canonical(ev.ord, [t, w, total - l]),
            leaf_b.design,
            leaf_b.orient,
        )
    });
    if x != pair[0].len {
        pair[0].len = x;
        pair[1].len = total - x;
    }
}

/// One pass over the boundaries inside `slab` at `t_off`: bars, then pieces.
fn refine_slab(ev: &LeafEval<'_>, t_off: f64, slab: &mut Slab) {
    let t = slab.thickness;
    let w_start = ev.origin[ev.ord[1]];
    let mut w_off = w_start;
    for i in 0..slab.bars.len().saturating_sub(1) {
        move_bar_boundary(ev, [t_off, w_off], t, &mut slab.bars[i..=i + 1]);
        w_off += slab.bars[i].width + ev.kerf;
    }

    w_off = w_start;
    for bar in &mut slab.bars {
        let w = bar.width;
        let mut l_off = ev.origin[ev.ord[2]];
        for i in 0..bar.leaves.len().saturating_sub(1) {
            let start = [t_off, w_off, l_off];
            move_leaf_boundary(ev, start, [t, w], &mut bar.leaves[i..=i + 1]);
            l_off += bar.leaves[i].len + ev.kerf;
        }
        w_off += bar.width + ev.kerf;
    }
}

/// Refines tree boundaries over one pass: slabs, then each slab's bars and pieces.
fn refine_tree_pass(ev: &LeafEval<'_>, tree: &mut Tree) {
    let mut t_off = ev.origin[ev.ord[0]];
    for i in 0..tree.slabs.len().saturating_sub(1) {
        move_slab_boundary(ev, t_off, &mut tree.slabs[i..=i + 1]);
        t_off += tree.slabs[i].thickness + ev.kerf;
    }

    t_off = ev.origin[ev.ord[0]];
    for slab in &mut tree.slabs {
        refine_slab(ev, t_off, slab);
        t_off += slab.thickness + ev.kerf;
    }
}

/// Continuously refines the cut positions of a shaped layout.
///
/// Keeps the tree structure, moves every boundary over 32 samples plus the
/// neighbours' kink positions, and re-picks each piece's design and assignment
/// from `pool` after every pass (pass a layout's own designs to restrict it to
/// them). A move or a re-pick is taken only when strictly better, so a piece keeps
/// its design unless another one is worth more in it.
///
/// Returns the refined layout if it beats `layout` in volume by more than a relative
/// `1e-12`; otherwise returns `layout` untouched, as it also does when `pool` lacks one
/// of the layout's designs. The refined layout is built like any other: a piece that
/// ends up without a stone (moved below the minimum width, say) is merged into its
/// neighbour, so its piece and stone counts can be lower than the input's.
#[must_use]
pub fn refine_shaped(
    ctx: &ShapedCtx,
    layout: &RoughLayout,
    pool: &[CandidateDesign],
    settings: &PlanSettings,
) -> RoughLayout {
    let ev = LeafEval {
        ord: layout.cut_order.axes(),
        origin: ctx.origin_mm,
        allowance: settings.allowance_mm,
        kerf: settings.kerf_mm,
        min_width: settings.min_width_mm,
        norms: pool.iter().map(Norm::of).collect(),
        non_box: &ctx.non_box,
        mesh: ctx.fit_mesh(),
        work: RefCell::new(EvalWork {
            solver: PartialSolver::new(),
            violated: Vec::new(),
        }),
    };

    let Some(mut tree) = tree_from_layout(layout, pool) else {
        return layout.clone();
    };

    let mut previous = ev.tree_value(&tree);
    for _ in 0..PASSES {
        refine_tree_pass(&ev, &mut tree);
        ev.repick_tree(&mut tree);
        let now = ev.tree_value(&tree);
        let gain = now - previous;
        previous = now;
        if gain <= REL_TOL * now.abs() {
            break;
        }
    }

    let refined = layout_from_tree_shaped(ctx, layout.cut_order, &tree, pool, settings);
    let margin = ACCEPT_REL * layout.total_volume_mm3.abs();
    if refined.total_volume_mm3 > layout.total_volume_mm3 + margin {
        refined
    } else {
        layout.clone()
    }
}

/// The tree of `layout`, each piece on its own design and assignment (read back
/// from the stone's pose). `None` when a design is missing from `pool` or the
/// stones do not match the cut plan's pieces one to one.
fn tree_from_layout(layout: &RoughLayout, pool: &[CandidateDesign]) -> Option<Tree> {
    let mut stones = layout.stones.iter();
    let mut slabs = Vec::with_capacity(layout.cut_plan.slabs.len());

    for s in &layout.cut_plan.slabs {
        let mut bars = Vec::with_capacity(s.bars.len());
        for b in &s.bars {
            let mut leaves = Vec::with_capacity(b.pieces_mm.len());
            for &len in &b.pieces_mm {
                let st = stones.next()?;
                let design = pool.iter().position(|d| d.entry_id == st.entry_id)?;
                leaves.push(Leaf {
                    len,
                    design,
                    orient: orient_of_pose(&st.pose).unwrap_or(0),
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

    stones.next().is_none().then_some(Tree { slabs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rough_plan::tests::box_design;

    fn evaluator<'a>(designs: &[CandidateDesign], min_width: f64, allowance: f64) -> LeafEval<'a> {
        LeafEval {
            ord: [0, 1, 2],
            origin: [0.0; 3],
            allowance,
            kerf: 0.3,
            min_width,
            norms: designs.iter().map(Norm::of).collect(),
            non_box: &[],
            mesh: None,
            work: RefCell::new(EvalWork {
                solver: PartialSolver::new(),
                violated: Vec::new(),
            }),
        }
    }

    #[test]
    fn the_usable_extents_come_from_the_piece_size_like_the_fitters_do() {
        let ev = evaluator(&[], 0.5, 0.2);
        let size = [4.0, 5.0, 6.0];
        let (b_min, b_max, usable) = ev.stone_box([1.0, 2.0, 3.0], size);
        assert_eq!(usable, size.map(|s| 2.0f64.mul_add(-0.2, s)));
        for (axis, &u) in usable.iter().enumerate() {
            assert!((u - (size[axis] - 0.4)).abs() < 1e-12);
            assert!((b_min[axis] - [1.2, 2.2, 3.2][axis]).abs() < 1e-12);
        }
        assert!((b_max[0] - 4.8).abs() < 1e-12);
    }

    #[test]
    fn a_repick_keeps_its_incumbent_unless_a_candidate_is_strictly_better() {
        // A unit cube design (value 4^3 = 64 in a 4 mm piece in any assignment) and the
        // same shape with half the volume factor (32).
        let full = box_design(1);
        let half = CandidateDesign {
            entry_id: 2,
            volume: 0.5,
            ..full
        };
        let ev = evaluator(&[full, half], 0.5, 0.0);
        let (origin, size) = ([0.0; 3], [4.0; 3]);

        // Equal values in six assignments: the incumbent stays where it is.
        assert_eq!(ev.best_pick_at(origin, size, (0, 3)), (0, 3));
        // The half-volume design is worth 32 and loses to the first 64, which then
        // holds against the equal assignments that follow it.
        assert_eq!(ev.best_pick_at(origin, size, (1, 2)), (0, 0));

        // A piece too small for the minimum width holds nothing: the incumbent stays.
        assert_eq!(ev.best_pick_at(origin, [0.3; 3], (0, 5)), (0, 5));
    }

    #[test]
    fn a_boundary_that_does_not_move_leaves_both_piece_sizes_untouched() {
        // The 0.5 mm cross section is below the 1 mm minimum width, so every position
        // is worth nothing and the boundary stays. 0.1 + 0.2 is not 0.3 in floating
        // point, so writing `total - x` back would change the second length.
        let ev = evaluator(&[box_design(1)], 1.0, 0.0);
        let leaf = |len: f64| Leaf {
            len,
            design: 0,
            orient: 0,
        };
        let mut pair = [leaf(0.1), leaf(0.2)];
        move_leaf_boundary(&ev, [0.0; 3], [0.5, 0.5], &mut pair);
        assert_eq!(pair[0].len.to_bits(), 0.1f64.to_bits());
        assert_eq!(pair[1].len.to_bits(), 0.2f64.to_bits());
    }
}
