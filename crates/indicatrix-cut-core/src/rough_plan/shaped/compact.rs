//! Merging of pieces that hold no stone into their neighbours.
//!
//! A guillotine tree over a shaped rough can contain pieces that lie outside
//! the rough or are too small for any stone (the corner cells of a uniform grid
//! on a cylinder, or a piece a refinement step shrank below the minimum width).
//! Deleting such a piece would slide every later piece toward the origin, away
//! from the material its stone was valued in; keeping it would make the saw
//! cut air and leave the layout with fewer stones than pieces.
//!
//! [`compact_tree`] instead merges an empty piece into its neighbour in the
//! same bar (the previous one, or the next one when it is the first): the new
//! piece is `len_a + kerf + len_b` long and spans the same material, so every
//! other piece keeps its exact position, and the neighbour's stone is fitted
//! again in the larger piece (its value can only grow). A bar without a stone
//! merges into its neighbour bar of the slab the same way, and a slab without
//! one into its neighbour slab. When nothing is left the tree is empty.

use crate::rough_plan::{
    CutOrder, PlacedStone,
    tree::{Bar, Leaf, Slab, Tree, to_canonical},
};

/// Fits a stone into the piece at `origin` with `size` (canonical `[x, y, z]`
/// in mm) holding `leaf`'s design; `None` when no stone fits.
pub type FitFn<'a> = dyn FnMut([f64; 3], [f64; 3], &Leaf) -> Option<PlacedStone> + 'a;

/// Where the pieces start and how far apart they sit.
struct Frame {
    ord: [usize; 3],
    origin: [f64; 3],
    kerf: f64,
}

/// The position and cross-section of one bar.
struct BarFrame<'a> {
    frame: &'a Frame,
    t_off: f64,
    w_off: f64,
    thickness: f64,
    width: f64,
}

impl BarFrame<'_> {
    /// Canonical origin and size of the piece of length `len` starting at `start`.
    const fn piece(&self, start: f64, len: f64) -> ([f64; 3], [f64; 3]) {
        let ord = self.frame.ord;
        (
            to_canonical(ord, [self.t_off, self.w_off, start]),
            to_canonical(ord, [self.thickness, self.width, len]),
        )
    }
}

/// A piece that holds a stone.
struct Slot {
    start: f64,
    leaf: Leaf,
    stone: PlacedStone,
}

/// Merges the pieces of `tree` that hold no stone into their neighbours.
///
/// Returns the stones of the remaining pieces in traversal order (one per
/// piece, so `stones.len()` equals the number of pieces left in `tree`).
/// `origin_mm` is the corner of the first piece and `fit` the stone fitter, see
/// [`FitFn`]. The tree's total extent along every stage axis is preserved.
pub fn compact_tree(
    order: CutOrder,
    tree: &mut Tree,
    origin_mm: [f64; 3],
    kerf_mm: f64,
    fit: &mut FitFn<'_>,
) -> Vec<PlacedStone> {
    let frame = Frame {
        ord: order.axes(),
        origin: origin_mm,
        kerf: kerf_mm,
    };
    loop {
        let mut stones = Vec::new();
        let mut changed = false;
        let mut has_stone = Vec::with_capacity(tree.slabs.len());
        let mut t_off = frame.origin[frame.ord[0]];
        for slab in &mut tree.slabs {
            let (slab_stones, merged) = compact_slab(&frame, t_off, slab, fit);
            changed |= merged;
            has_stone.push(!slab_stones.is_empty());
            stones.extend(slab_stones);
            t_off += slab.thickness + frame.kerf;
        }
        changed |= merge_empty(&mut tree.slabs, &has_stone, frame.kerf, slab_thickness);
        if !changed {
            return stones;
        }
    }
}

const fn slab_thickness(slab: &mut Slab) -> &mut f64 {
    &mut slab.thickness
}

const fn bar_width(bar: &mut Bar) -> &mut f64 {
    &mut bar.width
}

/// Sweeps the bars of `slab` at `t_off`; returns the slab's stones and whether
/// bars were merged (which invalidates the stones of the whole pass).
fn compact_slab(
    frame: &Frame,
    t_off: f64,
    slab: &mut Slab,
    fit: &mut FitFn<'_>,
) -> (Vec<PlacedStone>, bool) {
    let mut stones = Vec::new();
    let mut has_stone = Vec::with_capacity(slab.bars.len());
    let mut w_off = frame.origin[frame.ord[1]];
    let thickness = slab.thickness;
    for bar in &mut slab.bars {
        let bar_frame = BarFrame {
            frame,
            t_off,
            w_off,
            thickness,
            width: bar.width,
        };
        let bar_stones = sweep_bar(&bar_frame, bar, fit);
        has_stone.push(!bar_stones.is_empty());
        stones.extend(bar_stones);
        w_off += bar.width + frame.kerf;
    }
    let merged = merge_empty(&mut slab.bars, &has_stone, frame.kerf, bar_width);
    (stones, merged)
}

/// Fits every piece of `bar`, merging pieces without a stone into a neighbour.
/// Returns the stones of the remaining pieces; `bar.leaves` is rewritten to
/// match, and left empty when no piece holds a stone.
fn sweep_bar(bar_frame: &BarFrame<'_>, bar: &mut Bar, fit: &mut FitFn<'_>) -> Vec<PlacedStone> {
    let kerf = bar_frame.frame.kerf;
    let mut slots: Vec<Slot> = Vec::with_capacity(bar.leaves.len());
    let mut pending: Option<(f64, Leaf)> = None;
    let mut next_start = bar_frame.frame.origin[bar_frame.frame.ord[2]];
    for mut leaf in std::mem::take(&mut bar.leaves) {
        let mut start = next_start;
        next_start += leaf.len + kerf;
        if let Some((pending_start, pending_leaf)) = pending.take() {
            start = pending_start;
            leaf.len += pending_leaf.len + kerf;
        }
        let (origin, size) = bar_frame.piece(start, leaf.len);
        if let Some(stone) = fit(origin, size, &leaf) {
            slots.push(Slot { start, leaf, stone });
            continue;
        }
        if slots.is_empty() {
            pending = Some((start, leaf));
            continue;
        }
        let last = slots.len() - 1;
        absorb(bar_frame, &mut slots[last], leaf.len, fit);
    }
    let mut stones = Vec::with_capacity(slots.len());
    for slot in slots {
        bar.leaves.push(slot.leaf);
        stones.push(slot.stone);
    }
    stones
}

/// Merges the empty piece of length `other_len` that follows `slot` into it and
/// fits the slot's stone again. The old stone (moved into the larger piece) is
/// kept if the new fit is not larger, so a stone is never lost or shrunk.
fn absorb(bar_frame: &BarFrame<'_>, slot: &mut Slot, other_len: f64, fit: &mut FitFn<'_>) {
    slot.leaf.len += bar_frame.frame.kerf + other_len;
    let (origin, size) = bar_frame.piece(slot.start, slot.leaf.len);
    match fit(origin, size, &slot.leaf) {
        Some(stone) if stone.volume_mm3 >= slot.stone.volume_mm3 => slot.stone = stone,
        _ => {
            slot.stone.piece_origin_mm = origin;
            slot.stone.piece_size_mm = size;
        }
    }
}

/// Removes every item with `keep == false`, adding its extent (and one kerf) to
/// the previous kept item or, when there is none yet, to the next kept item.
/// Returns whether anything was merged; if no item is kept the list ends empty.
fn merge_empty<T>(
    items: &mut Vec<T>,
    keep: &[bool],
    kerf: f64,
    extent: fn(&mut T) -> &mut f64,
) -> bool {
    if keep.iter().all(|&kept| kept) {
        return false;
    }
    let mut out: Vec<T> = Vec::with_capacity(items.len());
    let mut carry: Option<f64> = None;
    for (mut item, &kept) in items.drain(..).zip(keep) {
        let own = *extent(&mut item);
        if kept {
            if let Some(carried) = carry.take() {
                *extent(&mut item) += carried + kerf;
            }
            out.push(item);
            continue;
        }
        if let Some(prev) = out.last_mut() {
            *extent(prev) += kerf + own;
            continue;
        }
        carry = Some(carry.map_or(own, |carried| carried + kerf + own));
    }
    *items = out;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rough_plan::{Axis, StonePose};

    /// Leaves of design `0` hold a stone; leaves of any other design do not.
    fn stone_for(origin: [f64; 3], size: [f64; 3], leaf: &Leaf) -> Option<PlacedStone> {
        (leaf.design == 0).then_some(PlacedStone {
            entry_id: 1,
            piece_origin_mm: origin,
            piece_size_mm: size,
            stone_size_mm: size,
            table_axis: Axis::Z,
            carat: 0.0,
            volume_mm3: size[0] * size[1] * size[2],
            pose: StonePose {
                center_mm: origin,
                axes: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]],
                mm_per_unit: 1.0,
            },
        })
    }

    fn leaf(len: f64, design: usize) -> Leaf {
        Leaf {
            len,
            design,
            orient: 0,
        }
    }

    fn bar(width: f64, leaves: Vec<Leaf>) -> Bar {
        Bar { width, leaves }
    }

    fn extent_along_leaves(bar: &Bar, kerf: f64) -> f64 {
        kerf.mul_add(
            (bar.leaves.len() - 1) as f64,
            bar.leaves.iter().map(|l| l.len).sum::<f64>(),
        )
    }

    fn compact(tree: &mut Tree, kerf: f64) -> Vec<PlacedStone> {
        compact_tree(CutOrder::Xyz, tree, [0.0; 3], kerf, &mut stone_for)
    }

    fn piece_count(tree: &Tree) -> usize {
        tree.slabs
            .iter()
            .flat_map(|s| &s.bars)
            .map(|b| b.leaves.len())
            .sum()
    }

    #[test]
    fn empty_pieces_merge_into_the_previous_or_else_the_next_piece() {
        let kerf = 0.5;
        // E F E F E: the first E has no previous piece and joins the first F.
        let leaves = vec![
            leaf(1.0, 1),
            leaf(2.0, 0),
            leaf(3.0, 1),
            leaf(4.0, 0),
            leaf(5.0, 1),
        ];
        let total = 4.0f64.mul_add(kerf, 15.0);
        let mut tree = Tree {
            slabs: vec![Slab {
                thickness: 2.0,
                bars: vec![bar(3.0, leaves)],
            }],
        };
        let stones = compact(&mut tree, kerf);
        let merged = &tree.slabs[0].bars[0];
        assert_eq!(merged.leaves.len(), 2);
        assert_eq!(stones.len(), 2);
        assert!((merged.leaves[0].len - (1.0 + kerf + 2.0 + kerf + 3.0)).abs() < 1e-12);
        assert!((merged.leaves[1].len - (4.0 + kerf + 5.0)).abs() < 1e-12);
        assert!((extent_along_leaves(merged, kerf) - total).abs() < 1e-12);
        // The merged pieces keep their exact positions.
        assert!((stones[0].piece_origin_mm[2]).abs() < 1e-12);
        let second_start = 1.0 + kerf + 2.0 + kerf + 3.0 + kerf;
        assert!((stones[1].piece_origin_mm[2] - second_start).abs() < 1e-12);
        assert!((stones[0].piece_size_mm[2] - merged.leaves[0].len).abs() < 1e-12);
    }

    #[test]
    fn a_bar_without_a_stone_joins_its_neighbour_bar() {
        let kerf = 0.25;
        let mut tree = Tree {
            slabs: vec![Slab {
                thickness: 2.0,
                bars: vec![
                    bar(1.0, vec![leaf(3.0, 1)]),
                    bar(2.0, vec![leaf(3.0, 0)]),
                    bar(4.0, vec![leaf(3.0, 1)]),
                ],
            }],
        };
        let stones = compact(&mut tree, kerf);
        let bars = &tree.slabs[0].bars;
        assert_eq!(bars.len(), 1);
        assert_eq!(stones.len(), 1);
        assert!((bars[0].width - (1.0 + kerf + 2.0 + kerf + 4.0)).abs() < 1e-12);
        assert!((stones[0].piece_size_mm[1] - bars[0].width).abs() < 1e-12);
        assert!(stones[0].piece_origin_mm[1].abs() < 1e-12);
    }

    #[test]
    fn a_slab_without_a_stone_joins_its_neighbour_slab() {
        let kerf = 0.5;
        let empty_slab = |t: f64| Slab {
            thickness: t,
            bars: vec![bar(2.0, vec![leaf(2.0, 1)])],
        };
        let mut tree = Tree {
            slabs: vec![
                empty_slab(1.0),
                empty_slab(2.0),
                Slab {
                    thickness: 3.0,
                    bars: vec![bar(2.0, vec![leaf(2.0, 0)])],
                },
                empty_slab(4.0),
            ],
        };
        let stones = compact(&mut tree, kerf);
        assert_eq!(tree.slabs.len(), 1);
        assert_eq!(stones.len(), 1);
        let expected = 1.0 + kerf + 2.0 + kerf + 3.0 + kerf + 4.0;
        assert!((tree.slabs[0].thickness - expected).abs() < 1e-12);
        assert!(stones[0].piece_origin_mm[0].abs() < 1e-12);
        assert_eq!(piece_count(&tree), stones.len());
    }

    #[test]
    fn a_tree_without_any_stone_ends_empty() {
        let mut tree = Tree {
            slabs: vec![Slab {
                thickness: 2.0,
                bars: vec![
                    bar(2.0, vec![leaf(1.0, 1), leaf(1.0, 1)]),
                    bar(2.0, vec![leaf(1.0, 1)]),
                ],
            }],
        };
        let stones = compact(&mut tree, 0.3);
        assert_eq!(stones.len(), 0);
        assert_eq!(tree.slabs.len(), 0);
    }

    /// Like [`stone_for`], but a piece longer than 3.0 mm along the stage-3 axis of
    /// `order` only gets half the volume of the stone that fills it.
    fn shrinking_fitter(
        order: CutOrder,
    ) -> impl FnMut([f64; 3], [f64; 3], &Leaf) -> Option<PlacedStone> {
        let long_axis = order.axes()[2];
        move |origin, size, leaf| {
            let mut stone = stone_for(origin, size, leaf)?;
            if size[long_axis] > 3.0 {
                stone.volume_mm3 *= 0.5;
            }
            Some(stone)
        }
    }

    fn assert_close3(actual: [f64; 3], expected: [f64; 3], what: &str) {
        for (a, e) in actual.iter().zip(&expected) {
            assert!((a - e).abs() < 1e-12, "{what}: {actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn merging_follows_every_cut_order_from_a_non_uniform_origin() {
        // Expected values are derived by hand: pieces start at the origin and follow one
        // kerf apart along each stage axis, whichever canonical axis a stage cuts.
        let kerf = 0.5;
        let origin = [1.0, 2.0, 3.0];
        for order in CutOrder::ALL {
            let ord = order.axes();
            // The second slab's bar is E F E: both empty pieces join the stone between
            // them, which ends up 1.0 + kerf + 2.0 + kerf + 1.0 = 5.0 long.
            let mut tree = Tree {
                slabs: vec![
                    Slab {
                        thickness: 1.0,
                        bars: vec![bar(2.0, vec![leaf(1.5, 0)])],
                    },
                    Slab {
                        thickness: 2.0,
                        bars: vec![bar(3.0, vec![leaf(1.0, 1), leaf(2.0, 0), leaf(1.0, 1)])],
                    },
                ],
            };
            let stones = compact_tree(order, &mut tree, origin, kerf, &mut stone_for);
            assert_eq!(stones.len(), 2, "{order:?}");
            assert_eq!(piece_count(&tree), 2, "{order:?}");
            let merged = &tree.slabs[1].bars[0].leaves;
            assert_eq!(merged.len(), 1);
            assert!((merged[0].len - 5.0).abs() < 1e-12);

            assert_close3(stones[0].piece_origin_mm, origin, "first origin");
            assert_close3(
                stones[0].piece_size_mm,
                to_canonical(ord, [1.0, 2.0, 1.5]),
                "first size",
            );
            let second_origin = to_canonical(
                ord,
                [origin[ord[0]] + 1.0 + kerf, origin[ord[1]], origin[ord[2]]],
            );
            assert_close3(stones[1].piece_origin_mm, second_origin, "second origin");
            assert_close3(
                stones[1].piece_size_mm,
                to_canonical(ord, [2.0, 3.0, 5.0]),
                "second size",
            );
        }
    }

    #[test]
    fn a_merge_that_fits_a_smaller_stone_keeps_the_old_stone_in_the_larger_piece() {
        // The stone of the 2.0 long piece has volume 2.0 * 3.0 * 2.0 = 12.0. After the
        // empty 1.0 piece joins it the piece is 3.5 long, where the fitter gives only
        // 0.5 * 2.0 * 3.0 * 3.5 = 10.5, so the old stone stays (never shrunk or lost)
        // and only its piece grows.
        let kerf = 0.5;
        let origin = [1.0, 2.0, 3.0];
        for order in CutOrder::ALL {
            let mut tree = Tree {
                slabs: vec![Slab {
                    thickness: 2.0,
                    bars: vec![bar(3.0, vec![leaf(2.0, 0), leaf(1.0, 1)])],
                }],
            };
            let mut fitter = shrinking_fitter(order);
            let stones = compact_tree(order, &mut tree, origin, kerf, &mut fitter);
            assert_eq!(stones.len(), 1, "{order:?}");
            assert!((stones[0].volume_mm3 - 12.0).abs() < 1e-12, "{order:?}");
            assert_close3(stones[0].piece_origin_mm, origin, "origin");
            assert_close3(
                stones[0].piece_size_mm,
                to_canonical(order.axes(), [2.0, 3.0, 3.5]),
                "size",
            );
            assert!((tree.slabs[0].bars[0].leaves[0].len - 3.5).abs() < 1e-12);
        }
    }

    #[test]
    fn a_merge_that_fits_a_larger_stone_replaces_the_old_one() {
        // The same tree with a fitter that never shrinks: the merged piece holds the
        // stone of volume 2.0 * 3.0 * 3.5 = 21.0.
        let kerf = 0.5;
        for order in CutOrder::ALL {
            let mut tree = Tree {
                slabs: vec![Slab {
                    thickness: 2.0,
                    bars: vec![bar(3.0, vec![leaf(2.0, 0), leaf(1.0, 1)])],
                }],
            };
            let stones = compact_tree(order, &mut tree, [1.0, 2.0, 3.0], kerf, &mut stone_for);
            assert_eq!(stones.len(), 1, "{order:?}");
            assert!((stones[0].volume_mm3 - 21.0).abs() < 1e-12, "{order:?}");
        }
    }

    #[test]
    fn a_tree_of_full_pieces_is_left_alone() {
        let mut tree = Tree {
            slabs: vec![Slab {
                thickness: 2.0,
                bars: vec![bar(2.0, vec![leaf(1.0, 0), leaf(2.0, 0)])],
            }],
        };
        let before = tree.clone();
        let stones = compact(&mut tree, 0.3);
        assert_eq!(stones.len(), 2);
        assert_eq!(tree.slabs.len(), before.slabs.len());
        assert_eq!(tree.slabs[0].bars[0].leaves.len(), 2);
        assert!((tree.slabs[0].bars[0].leaves[1].len - 2.0).abs() < 1e-15);
    }
}
