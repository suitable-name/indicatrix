//! Brute-force cross-checks: the Pareto front against all designs, and the DP
//! against explicit enumeration of every staged guillotine layout.

use super::{
    dp::run_order_dp,
    piece::{ASSIGNMENTS, Norm, best_pick, stone_value},
    tests::{Lcg, close, random_designs, settings_with},
    tree::layout_from_tree,
    *,
};

/// Every ordered composition of `total` into positive parts.
fn compositions(total: usize) -> Vec<Vec<usize>> {
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
