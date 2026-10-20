//! Brute-force cross-checks: the Pareto front against all designs, and the DP
//! against explicit enumeration of every staged guillotine layout.

use super::{
    dp::run_order_dp,
    dp_capped::run_order_dp_capped,
    piece::{ASSIGNMENTS, Norm, best_pick, stone_value},
    tests::{Lcg, box_design, close, random_designs, settings_with},
    tree::layout_from_tree,
    *,
};

/// Every ordered composition of `total` into positive parts.
pub(super) fn compositions(total: usize) -> Vec<Vec<usize>> {
    if total == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for first in 1..=total {
        for mut rest in compositions(total - first) {
            rest.insert(0, first);
            out.push(rest);
        }
    }
    out
}

/// Explicit enumeration of every staged layout of one cut order: the best total over every
/// split of the rough into slabs, bars and pieces (the number of pieces is free).
struct Brute<'a> {
    table: &'a PieceTable,
    order: [usize; 3],
    cells: [usize; 3],
}

impl Brute<'_> {
    fn piece(&self, g: [usize; 3]) -> f64 {
        let mut cell = [0; 3];
        for (stage, axis) in self.order.iter().enumerate() {
            cell[*axis] = g[stage];
        }
        self.table.values[self.table.index(cell)]
    }

    fn bar(&self, ga: usize, gb: usize) -> f64 {
        compositions(self.cells[2])
            .iter()
            .map(|runs| runs.iter().map(|&g| self.piece([ga, gb, g])).sum::<f64>())
            .fold(NEG, f64::max)
    }

    fn slab(&self, ga: usize) -> f64 {
        compositions(self.cells[1])
            .iter()
            .map(|runs| runs.iter().map(|&g| self.bar(ga, g)).sum::<f64>())
            .fold(NEG, f64::max)
    }

    fn root(&self) -> f64 {
        compositions(self.cells[0])
            .iter()
            .map(|runs| runs.iter().map(|&g| self.slab(g)).sum::<f64>())
            .fold(NEG, f64::max)
    }
}

/// Re-derives every cell of `table` with explicit loops (the first maximum wins over designs
/// in front order, then over the six assignments) and compares value bits, picked design and
/// picked assignment.
fn assert_table_picks(grid: &Grid, table: &PieceTable, norms: &[Norm], min_width: f64) {
    let [nx, ny, nz] = grid.cells();
    for gx in 1..=nx {
        for gy in 1..=ny {
            for gz in 1..=nz {
                let p = grid.stone_box([gx, gy, gz]);
                let mut best: Option<(usize, usize, f64)> = None;
                for (design, norm) in norms.iter().enumerate() {
                    for orient in 0..ASSIGNMENTS.len() {
                        let value = stone_value(norm, orient, p, min_width);
                        if value > NEG && best.is_none_or(|(_, _, b)| value > b) {
                            best = Some((design, orient, value));
                        }
                    }
                }
                let index = table.index([gx, gy, gz]);
                match best {
                    Some((design, orient, value)) => {
                        let cell = format!("cell {:?}", [gx, gy, gz]);
                        assert_eq!(table.values[index].to_bits(), value.to_bits(), "{cell}");
                        assert_eq!(table.design[index] as usize, design, "{cell}");
                        assert_eq!(usize::from(table.orient[index]), orient, "{cell}");
                    }
                    None => assert!(table.values[index] == NEG, "{gx} {gy} {gz}"),
                }
            }
        }
    }
}

#[test]
fn the_free_count_dp_equals_brute_force_over_all_staged_layouts() {
    let mut rng = Lcg(77);
    let designs = random_designs(&mut rng, 6);
    let front = pareto_front(&designs);
    let rough = RoughBlock {
        x_mm: 9.0,
        y_mm: 7.0,
        z_mm: 8.0,
    };
    let settings = settings_with(4, 0.3, 0.2, 0.5);
    let norms: Vec<Norm> = front.iter().map(Norm::of).collect();
    let a2 = 2.0 * settings.allowance_mm;
    let mut finite = 0;
    for cells in [[3, 2, 3], [2, 3, 4], [4, 3, 2], [1, 3, 3]] {
        let grid = Grid::with_cells(&rough, &settings, cells);
        let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
        assert_table_picks(&grid, &table, &norms, settings.min_width_mm);
        for order in CutOrder::ALL {
            let axes = order.axes();
            let dp = run_order_dp(&grid, &table, axes, &mut || true).expect("dp");
            let expected = Brute {
                table: &table,
                order: axes,
                cells: axes.map(|i| cells[i]),
            }
            .root();
            let value = dp.value();
            if expected == NEG {
                assert!(value == NEG, "{cells:?} {order}: dp {value}, brute -inf");
                continue;
            }
            // Both sum the same pieces in the same left-to-right association and
            // floating-point addition is monotone, so the maxima agree bit for bit.
            assert_eq!(
                value.to_bits(),
                expected.to_bits(),
                "{cells:?} {order}: dp {value}, brute {expected}"
            );
            // The reconstructed tree is worth exactly the DP value, whatever its stone count.
            let tree = dp.reconstruct(&grid, &table);
            let layout = layout_from_tree(&rough, &settings, order, &tree, &front);
            assert_ne!(layout.stone_count(), 0);
            assert!(close(layout.total_volume_mm3, value, 1e-9));
            for stone in &layout.stones {
                // Each piece holds the design and assignment an independent pick
                // over the front makes for its usable box.
                let p = stone.piece_size_mm.map(|s| s - a2);
                let (design, orient, v) = best_pick(&norms, p, settings.min_width_mm)
                    .expect("a placed piece is feasible");
                assert_eq!(stone.entry_id, front[design].entry_id);
                assert_eq!(stone.table_axis, Axis::from_index(ASSIGNMENTS[orient][2]));
                assert_eq!(stone.volume_mm3.to_bits(), v.to_bits());
            }
            // The order's one candidate is that layout.
            let candidates = dp.candidates(&grid, &table, &front, order);
            assert_eq!(candidates, vec![layout]);
            finite += 1;
        }
    }
    assert!(
        finite > 12,
        "the cases should exercise feasible layouts ({finite})"
    );
}

/// The best total per EXACT number of pieces: entry `c` is the best total of the layouts with
/// `c` pieces, `-inf` where there is none.
pub(super) type ByCount = Vec<f64>;

/// `into` and `other` combined entry by entry, keeping the larger.
pub(super) fn merge_counts(into: &mut ByCount, other: &[f64]) {
    if into.len() < other.len() {
        into.resize(other.len(), NEG);
    }
    for (slot, &value) in into.iter_mut().zip(other) {
        if value > *slot {
            *slot = value;
        }
    }
}

/// Every way to give the consecutive `parts` a piece count each, summed left to right in
/// position order (as the DP adds a part to what lies before it): entry `c` of the result is
/// the best total of exactly `c` pieces.
///
/// Reducing every part to its best total per count first is exact: a sum is monotone in each
/// of its terms, so the best of the sums is the sum of the bests.
pub(super) fn fold_parts(parts: &[ByCount]) -> ByCount {
    let mut done: ByCount = vec![0.0];
    for part in parts {
        let mut next = vec![NEG; done.len() + part.len() - 1];
        for (before, &sum) in done.iter().enumerate() {
            for (own, &value) in part.iter().enumerate() {
                let total = sum + value;
                if total > next[before + own] {
                    next[before + own] = total;
                }
            }
        }
        done = next;
    }
    done
}

/// The best total of at most `cap` pieces.
pub(super) fn at_most(by_count: &[f64], cap: usize) -> f64 {
    by_count
        .iter()
        .take(cap.saturating_add(1))
        .copied()
        .fold(NEG, f64::max)
}

impl Brute<'_> {
    /// The bar `(ga, gb)` by exact piece count: every composition of the bar's length, one
    /// piece per run.
    fn bar_counts(&self, ga: usize, gb: usize) -> ByCount {
        let mut best = ByCount::new();
        for runs in compositions(self.cells[2]) {
            let pieces: Vec<ByCount> = runs
                .iter()
                .map(|&g| vec![NEG, self.piece([ga, gb, g])])
                .collect();
            merge_counts(&mut best, &fold_parts(&pieces));
        }
        best
    }

    /// The slab `ga` by exact piece count: every composition into bars.
    fn slab_counts(&self, ga: usize) -> ByCount {
        let mut best = ByCount::new();
        for runs in compositions(self.cells[1]) {
            let bars: Vec<ByCount> = runs.iter().map(|&g| self.bar_counts(ga, g)).collect();
            merge_counts(&mut best, &fold_parts(&bars));
        }
        best
    }

    /// The whole layout by exact piece count: every composition into slabs.
    fn root_counts(&self) -> ByCount {
        let mut best = ByCount::new();
        for runs in compositions(self.cells[0]) {
            let slabs: Vec<ByCount> = runs.iter().map(|&g| self.slab_counts(g)).collect();
            merge_counts(&mut best, &fold_parts(&slabs));
        }
        best
    }
}

#[test]
fn the_capped_count_dp_equals_brute_force_over_all_staged_layouts() {
    let rough_designs = random_designs(&mut Lcg(77), 6);
    // A cube and a bar of twice its length: along a long thin rough the best layout holds
    // several stones, so a small cap binds.
    let bar = CandidateDesign {
        entry_id: 2,
        width: 1.0,
        length: 2.0,
        height: 1.0,
        volume: 1.8,
    };
    let cases = [
        (
            RoughBlock {
                x_mm: 9.0,
                y_mm: 7.0,
                z_mm: 8.0,
            },
            rough_designs,
        ),
        (
            RoughBlock {
                x_mm: 30.0,
                y_mm: 6.0,
                z_mm: 7.0,
            },
            vec![box_design(1), bar],
        ),
    ];
    let settings = settings_with(4, 0.3, 0.2, 0.5);
    let (mut checked, mut binding) = (0, 0);
    for (rough, designs) in &cases {
        let front = pareto_front(designs);
        for cells in [[3, 2, 3], [2, 3, 3], [3, 3, 2], [1, 3, 3]] {
            let grid = Grid::with_cells(rough, &settings, cells);
            let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
            for order in CutOrder::ALL {
                let axes = order.axes();
                let by_count = Brute {
                    table: &table,
                    order: axes,
                    cells: axes.map(|i| cells[i]),
                }
                .root_counts();
                let free = at_most(&by_count, usize::MAX);
                for cap in 1..=6 {
                    let what = format!("{rough:?} {cells:?} {order} cap {cap}");
                    let dp =
                        run_order_dp_capped(&grid, &table, axes, cap, &mut || true).expect("dp");
                    let expected = at_most(&by_count, cap);
                    let value = dp.value();
                    if expected == NEG {
                        assert!(value == NEG, "{what}: dp {value}, brute -inf");
                        assert_eq!(dp.candidates(&grid, &table, &front, order), Vec::new());
                        continue;
                    }
                    // Both sum the same pieces in the same left-to-right association and
                    // floating-point addition is monotone, so the maxima agree bit for bit.
                    assert_eq!(
                        value.to_bits(),
                        expected.to_bits(),
                        "{what}: dp {value}, brute {expected}"
                    );
                    let tree = dp.reconstruct(&grid, &table);
                    let leaves = tree.leaf_count();
                    assert!((1..=cap).contains(&leaves), "{what}: {leaves} pieces");
                    // Of the layouts worth the best value, the tree has the fewest pieces.
                    let fewest = by_count
                        .iter()
                        .position(|v| v.to_bits() == expected.to_bits())
                        .expect("the best value is reached by some count");
                    assert_eq!(leaves, fewest, "{what}");
                    let layout = layout_from_tree(rough, &settings, order, &tree, &front);
                    assert_eq!(layout.stone_count(), leaves, "{what}");
                    assert!(close(layout.total_volume_mm3, value, 1e-9), "{what}");
                    assert_eq!(
                        dp.candidates(&grid, &table, &front, order),
                        vec![layout],
                        "{what}"
                    );
                    checked += 1;
                    binding += usize::from(expected < free);
                }
            }
        }
    }
    assert!(
        checked > 30,
        "the cases should exercise feasible layouts ({checked})"
    );
    assert!(
        binding > 0,
        "no cap bound: the cases exercise nothing capped"
    );
}

#[test]
fn front_matches_brute_force_over_all_designs() {
    let mut rng = Lcg(2024);
    let designs = random_designs(&mut rng, 200);
    let front = pareto_front(&designs);
    assert!(!front.is_empty() && front.len() < designs.len());
    assert!(front.windows(2).all(|w| w[0].entry_id < w[1].entry_id));
    let all: Vec<Norm> = designs.iter().map(Norm::of).collect();
    let kept: Vec<Norm> = front.iter().map(Norm::of).collect();
    for _ in 0..400 {
        let p = [
            rng.range(0.2, 6.0),
            rng.range(0.2, 6.0),
            rng.range(0.2, 6.0),
        ];
        let w_min = rng.range(0.0, 2.5);
        let brute = best_pick(&all, p, w_min).map(|t| t.2);
        let pruned = best_pick(&kept, p, w_min).map(|t| t.2);
        match (brute, pruned) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                // A dominated design never strictly beats its dominator on any piece, bit
                // for bit, so the best value over the front is the best over all designs.
                assert_eq!(a.to_bits(), b.to_bits(), "front lost value: {a} vs {b}");
            }
            other => panic!("feasibility differs: {other:?}"),
        }
    }
}
