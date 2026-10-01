//! Tests of the invariants between the shaped stages: the shared plane predicate, the
//! threaded builds against the serial ones, the progress and cancel points, and the
//! positional DP against a brute-force enumeration and against the layouts built from it.

use std::ops::Range;

use glam::DVec3;

use super::{
    clip::{
        BuildClipParams, CLASS_EXTERIOR, CLASS_INTERIOR, CLASS_PARTIAL, ClippedTable, PLANE_EPS_MM,
        build_clipped_table, build_clipped_table_lanes, classify_box, grid_poll_events,
        slice_entry_range,
    },
    ctx::ShapedCtx,
    dp::{ShapedAltParams, plan_shaped_alternatives, plan_shaped_for_order, solve_order},
    grid::{ShapedGrid, choose_shaped_grid_at},
    parallel::balanced_slices,
    tests::corner_cut_model,
    tests_plan::{clipped_for, four_front_designs},
    tree::layout_from_tree_shaped,
    uniform::shaped_uniform_layouts,
};
use crate::rough_plan::{
    Axis, CandidateDesign, CutOrder, NEG, PieceTable, PlanProgress, PlanSettings, RoughBase,
    RoughCut, RoughModel, pareto_front,
    rank::best_layout,
    tests::{Lcg, close, random_designs, settings_with},
    tree::Tree,
};

#[test]
fn a_box_touching_a_plane_is_interior_and_the_epsilon_decides_beyond_it() {
    // The plane x + y <= 8 as a unit normal and its offset. The box [0, 4]^2 x [0, 1]
    // touches it along the edge through (4, 4); the two sides of the comparison come
    // from different roundings of 8 / sqrt(2), so only the epsilon keeps them apart.
    assert!((PLANE_EPS_MM - 1e-9).abs() < 1e-20);
    let root2 = 2.0_f64.sqrt();
    let planes = [(DVec3::new(1.0, 1.0, 0.0) / root2, 8.0 / root2)];
    let classify = |lo: [f64; 3], hi: [f64; 3]| classify_box(lo, hi, &planes);

    let (class, violated) = classify([0.0; 3], [4.0, 4.0, 1.0]);
    assert_eq!(class, CLASS_INTERIOR);
    assert_eq!(violated.len(), 0);

    // 1e-12 beyond on x and y is 1.4e-12 beyond the plane: inside the epsilon.
    let over = 4.0 + 1e-12;
    assert_eq!(classify([0.0; 3], [over, over, 1.0]).0, CLASS_INTERIOR);

    // 1e-6 beyond is 1.4e-6 beyond the plane: the box is clipped, not excluded.
    let over = 4.0 + 1e-6;
    let (class, violated) = classify([0.0; 3], [over, over, 1.0]);
    assert_eq!(class, CLASS_PARTIAL);
    assert_eq!(violated, vec![0]);

    // A box on the far side touching the plane at (4, 4) is not exterior either; its
    // far corner (5, 5) is beyond the plane.
    let (class, violated) = classify([4.0, 4.0, 0.0], [5.0, 5.0, 1.0]);
    assert_eq!(class, CLASS_PARTIAL);
    assert_eq!(violated, vec![0]);

    // Only a box whose nearest corner is beyond the epsilon is exterior.
    let near = 4.0 + 1e-6;
    let (class, violated) = classify([near, near, 0.0], [5.0, 5.0, 1.0]);
    assert_eq!(class, CLASS_EXTERIOR);
    assert_eq!(violated.len(), 0);
}

/// The table inputs of one model.
struct ClipFixture {
    ctx: ShapedCtx,
    grid: ShapedGrid,
    front: Vec<CandidateDesign>,
    size_table: PieceTable,
    settings: PlanSettings,
}

impl ClipFixture {
    fn new(model: &RoughModel, settings: PlanSettings, seed: u64) -> Self {
        let designs = random_designs(&mut Lcg(seed), 4);
        let front = pareto_front(&designs);
        let ctx = ShapedCtx::new(model, &settings).expect("ctx");
        let grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, &settings);
        let size_table = grid
            .size_table(&settings, &front, &mut |_| true)
            .expect("size table");
        Self {
            ctx,
            grid,
            front,
            size_table,
            settings,
        }
    }

    fn params<'a>(
        &'a self,
        slice: Range<usize>,
        cached_classes: Option<&'a [u8]>,
    ) -> BuildClipParams<'a> {
        BuildClipParams {
            grid: &self.grid,
            front: &self.front,
            non_box_planes: &self.ctx.non_box,
            size_table: &self.size_table,
            settings: &self.settings,
            slice,
            cached_classes,
        }
    }
}

/// A cylinder fixture with an 8 x 8 x 8 grid: 46,656 table entries, 46 poll intervals.
fn cylinder_fixture() -> ClipFixture {
    let model = RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 15.0,
            length_mm: 25.0,
            axis: Axis::Z,
        },
        vec![],
    );
    ClipFixture::new(&model, settings_with(1, 0.3, 0.2, 0.5), 99)
}

#[test]
fn the_size_table_on_lanes_is_the_serial_table_with_one_event_per_plane() {
    let fx = cylinder_fixture();
    let serial = fx
        .grid
        .size_table(&fx.settings, &fx.front, &mut |_| true)
        .expect("serial");
    let nx = fx.grid.cells[0];
    for lanes in [1, 2, 3, 7] {
        let mut events = Vec::new();
        let table = fx
            .grid
            .size_table_lanes(&fx.settings, &fx.front, lanes, &mut |event| {
                events.push(event);
                true
            })
            .expect("lanes");
        assert_eq!(table.cells, serial.cells);
        assert_eq!(table.values, serial.values, "lanes {lanes}");
        assert_eq!(table.design, serial.design, "lanes {lanes}");
        assert_eq!(table.orient, serial.orient, "lanes {lanes}");

        assert_eq!(events.len(), nx, "lanes {lanes}");
        let mut planes: Vec<usize> = events
            .iter()
            .map(|event| match event {
                PlanProgress::Grid { done, total } => {
                    assert_eq!(*total, nx);
                    *done
                }
                other => panic!("unexpected event {other:?}"),
            })
            .collect();
        if lanes == 1 {
            assert!(planes.is_sorted(), "one lane reports the planes in order");
        }
        planes.sort_unstable();
        assert_eq!(planes, (1..=nx).collect::<Vec<_>>());

        let mut seen = 0;
        let cancelled = fx
            .grid
            .size_table_lanes(&fx.settings, &fx.front, lanes, &mut |_| {
                seen += 1;
                seen < 2
            });
        assert!(cancelled.is_none(), "lanes {lanes}");
        assert_eq!(seen, 2, "lanes {lanes}");
    }
}

#[test]
fn the_clipped_build_reports_one_event_per_1024_pieces_and_cancels_on_any_of_them() {
    let fx = cylinder_fixture();
    let ga = fx.grid.cells[0];
    let entries = slice_entry_range(&fx.grid, &(0..ga)).len();
    let expected = grid_poll_events(entries);
    assert!(expected >= 3, "the fixture needs a few poll intervals");

    let mut events = Vec::new();
    let table = build_clipped_table(&fx.params(0..ga, None), &mut |event| {
        events.push(event);
        true
    })
    .expect("table");
    assert_eq!(table.values.len(), entries);
    let want: Vec<PlanProgress> = (1..=expected)
        .map(|done| PlanProgress::Grid {
            done,
            total: expected,
        })
        .collect();
    assert_eq!(events, want);

    // Any of the events can cancel, the first one before a single piece is built.
    for stop in [1, 2, expected] {
        let mut seen = 0;
        let cancelled = build_clipped_table(&fx.params(0..ga, None), &mut |_| {
            seen += 1;
            seen < stop
        });
        assert!(cancelled.is_none(), "stop {stop}");
        assert_eq!(seen, stop);
    }

    // A slice reports the count of its own entries.
    let slice = 2..ga;
    let own = grid_poll_events(slice_entry_range(&fx.grid, &slice).len());
    let mut count = 0;
    let _ = build_clipped_table(&fx.params(slice, None), &mut |event| {
        count += 1;
        assert!(matches!(event, PlanProgress::Grid { total, .. } if total == own));
        true
    })
    .expect("slice");
    assert_eq!(count, own);
}

#[test]
fn the_lane_build_reports_the_events_of_its_slices_and_cancels_across_lanes() {
    let fx = cylinder_fixture();
    let ga = fx.grid.cells[0];
    for lanes in [2, 3] {
        let expected: usize = balanced_slices(ga, lanes)
            .iter()
            .map(|slice| grid_poll_events(slice_entry_range(&fx.grid, slice).len()))
            .sum();
        let mut count = 0;
        let table = build_clipped_table_lanes(&fx.params(0..ga, None), lanes, &mut |event| {
            assert!(matches!(event, PlanProgress::Grid { .. }));
            count += 1;
            true
        })
        .expect("lanes");
        assert_eq!(count, expected, "lanes {lanes}");
        assert_eq!(
            table.values.len(),
            slice_entry_range(&fx.grid, &(0..ga)).len()
        );

        let mut seen = 0;
        let cancelled = build_clipped_table_lanes(&fx.params(0..ga, None), lanes, &mut |_| {
            seen += 1;
            seen < 2
        });
        assert!(cancelled.is_none(), "lanes {lanes}");
        assert_eq!(seen, 2, "lanes {lanes}");
    }
}

#[test]
fn alternatives_are_the_same_on_any_lane_count_and_report_their_own_progress() {
    let designs = four_front_designs();
    let front = pareto_front(&designs);
    let settings = settings_with(4, 0.3, 0.2, 0.5);
    let ctx = ShapedCtx::new(&corner_cut_model(), &settings).expect("ctx");
    let grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, &settings);
    let table = clipped_for(&ctx, &grid, &front, &settings);
    let mut mixed = Vec::new();
    for order in CutOrder::ALL {
        mixed.extend(
            plan_shaped_for_order(&grid, &table, &ctx, &front, order, &settings, &mut |_| true)
                .expect("order dp"),
        );
    }
    let best = best_layout(&mixed).expect("a layout").clone();
    let params = |lanes: usize| ShapedAltParams {
        grid: &grid,
        table: &table,
        ctx: &ctx,
        all_designs: &designs,
        best: &best,
        settings: &settings,
        lanes,
    };

    let serial = plan_shaped_alternatives(&params(1), &mut |_| true).expect("serial");
    assert_ne!(serial.len(), 0);
    for lanes in [2, 3, 5] {
        let threaded = plan_shaped_alternatives(&params(lanes), &mut |_| true).expect("lanes");
        assert_eq!(threaded, serial, "lanes {lanes}");
    }

    // A round reports its own size table, clipped table and DP between the
    // Alternatives events, so it moves and can be cancelled from inside.
    let mut events = Vec::new();
    let _ = plan_shaped_alternatives(&params(1), &mut |event| {
        events.push(event);
        true
    })
    .expect("events");
    assert!(matches!(
        events.first(),
        Some(PlanProgress::Alternatives { done: 0, .. })
    ));
    assert!(matches!(
        events.last(),
        Some(PlanProgress::Alternatives { done, total }) if done == total
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PlanProgress::Grid { .. }))
    );
    assert!(events.iter().any(|e| matches!(e, PlanProgress::Dp { .. })));
    for lanes in [1, 3] {
        for stop in [2, 5, events.len() / 2, events.len()] {
            let mut seen = 0;
            let cancelled = plan_shaped_alternatives(&params(lanes), &mut |_| {
                seen += 1;
                seen < stop
            });
            assert!(cancelled.is_none(), "lanes {lanes}, stop {stop}");
        }
    }
}

#[test]
fn uniform_layouts_are_the_same_on_any_lane_count_and_progress_counts_designs() {
    let designs = random_designs(&mut Lcg(5), 6);
    let settings = settings_with(8, 0.3, 0.2, 1.0);
    let model = RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 12.0,
            length_mm: 20.0,
            axis: Axis::Z,
        },
        vec![],
    );
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");

    let serial =
        shaped_uniform_layouts(&ctx, &designs, &settings, 1, &mut |_| true).expect("serial");
    assert_ne!(serial.len(), 0);
    for lanes in [2, 4, 9] {
        let threaded =
            shaped_uniform_layouts(&ctx, &designs, &settings, lanes, &mut |_| true).expect("lanes");
        assert_eq!(threaded, serial, "lanes {lanes}");
    }

    for lanes in [1, 3] {
        let mut events = Vec::new();
        let _ = shaped_uniform_layouts(&ctx, &designs, &settings, lanes, &mut |event| {
            events.push(event);
            true
        })
        .expect("events");
        let mut last = 0;
        for event in &events {
            let PlanProgress::Uniform { done, total } = event else {
                panic!("unexpected event {event:?}");
            };
            assert_eq!(*total, designs.len());
            assert!((last..*total).contains(done), "lanes {lanes}: {done}");
            last = *done;
        }
        assert_eq!(last, designs.len() - 1, "lanes {lanes}");

        let mut seen = 0;
        let cancelled = shaped_uniform_layouts(&ctx, &designs, &settings, lanes, &mut |_| {
            seen += 1;
            seen < 3
        });
        assert!(cancelled.is_none(), "lanes {lanes}");
    }
}

/// The grid of `ctx` with `cells` cells per axis instead of the chosen resolution.
fn grid_with_cells(ctx: &ShapedCtx, settings: &PlanSettings, cells: [usize; 3]) -> ShapedGrid {
    let mut grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, settings);
    grid.cells = cells;
    grid.unit = [0, 1, 2].map(|i| (grid.usable_mm[i] + grid.kerf_mm) / cells[i] as f64);
    grid
}

/// A 20 x 20 x 10 block whose corner around (20, 20) is cut away by the plane
/// `x + y <= 40 - 12 sqrt(2)` (about 23.03): some pieces are exterior, some clipped.
fn slanted_model() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 20.0,
            y_mm: 20.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Face {
            normal: [1.0, 1.0, 0.0],
            depth_mm: 12.0,
        }],
    )
}

fn plain_model() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 12.0,
            y_mm: 9.0,
            z_mm: 8.0,
        },
        vec![],
    )
}

/// The number of cells a run of `len_mm` covers along `axis`: `len = g * unit - kerf`.
fn cells_of(grid: &ShapedGrid, axis: usize, len_mm: f64) -> usize {
    ((len_mm + grid.kerf_mm) / grid.unit[axis]).round() as usize
}

/// The sum of the table values of the pieces of `tree`, each piece found from its
/// position in cells.
fn table_sum(grid: &ShapedGrid, table: &ClippedTable, order: CutOrder, tree: &Tree) -> f64 {
    let ord = order.axes();
    let (mut a, mut total) = (0, 0.0);
    for slab in &tree.slabs {
        let ga = cells_of(grid, ord[0], slab.thickness);
        let mut b = 0;
        for bar in &slab.bars {
            let gb = cells_of(grid, ord[1], bar.width);
            let mut c = 0;
            for leaf in &bar.leaves {
                let gc = cells_of(grid, ord[2], leaf.len);
                let mut range = [0; 3];
                range[ord[0]] = grid.range_index(ord[0], a, a + ga);
                range[ord[1]] = grid.range_index(ord[1], b, b + gb);
                range[ord[2]] = grid.range_index(ord[2], c, c + gc);
                total += table.value(range[0], range[1], range[2]);
                c += gc;
            }
            b += gb;
        }
        a += ga;
    }
    total
}

fn leaf_count(tree: &Tree) -> usize {
    tree.slabs
        .iter()
        .flat_map(|slab| &slab.bars)
        .map(|bar| bar.leaves.len())
        .sum()
}

#[test]
fn the_dp_value_is_the_table_sum_of_its_tree_and_the_volume_of_the_layout_built_from_it() {
    let settings = settings_with(4, 0.3, 0.2, 0.5);
    let front = pareto_front(&random_designs(&mut Lcg(17), 4));
    let models = [
        ("plain block", plain_model()),
        ("slanted face", slanted_model()),
        ("corner cut", corner_cut_model()),
    ];
    for (name, model) in &models {
        let ctx = ShapedCtx::new(model, &settings).expect("ctx");
        for cells in [[3, 3, 3], [5, 4, 4]] {
            let grid = grid_with_cells(&ctx, &settings, cells);
            let table = clipped_for(&ctx, &grid, &front, &settings);
            let mut finite = 0;
            for order in CutOrder::ALL {
                let dp = solve_order(&grid, &table, order, settings.count_usize(), &mut || true)
                    .expect("dp");
                for n in 1..=settings.count_usize() {
                    let value = dp.root_value(n);
                    if value == NEG {
                        continue;
                    }
                    finite += 1;
                    let what = format!("{name}, {cells:?}, {order:?}, n = {n}");
                    let tree = dp.reconstruct(&grid, &table, n);
                    assert_eq!(leaf_count(&tree), n, "{what}");
                    let sum = table_sum(&grid, &table, order, &tree);
                    assert!(
                        close(sum, value, 1e-9),
                        "{what}: table sum {sum} vs DP {value}"
                    );

                    // Every piece holds a stone, so nothing is merged and the layout's
                    // volume is what the DP valued the tree at.
                    let layout = layout_from_tree_shaped(&ctx, order, &tree, &front, &settings);
                    assert_eq!(layout.stones.len(), n, "{what}");
                    assert!(
                        close(layout.total_volume_mm3, value, 1e-9),
                        "{what}: layout {} vs DP {value}",
                        layout.total_volume_mm3
                    );
                }
            }
            assert!(finite > 0, "{name}, {cells:?}: the DP found nothing");
        }
    }
}

/// The compositions of `total`: the ordered ways to write it as a sum of positive parts.
fn compositions(total: usize) -> Vec<Vec<usize>> {
    if total == 0 {
        return vec![Vec::new()];
    }
    let mut all = Vec::new();
    for first in 1..=total {
        for rest in compositions(total - first) {
            let mut composition = vec![first];
            composition.extend(rest);
            all.push(composition);
        }
    }
    all
}

/// `(stones, value)` of every way to cut a region.
type Outcomes = Vec<(usize, f64)>;

/// Every way to take one outcome from each list, dropping those above `k` stones.
fn combine(lists: &[Outcomes], k: usize) -> Outcomes {
    let mut combined: Outcomes = vec![(0, 0.0)];
    for list in lists {
        combined = combined
            .iter()
            .flat_map(|&(n, v)| {
                list.iter()
                    .filter(move |&&(m, _)| n + m <= k)
                    .map(move |&(m, w)| (n + m, v + w))
            })
            .collect();
    }
    combined
}

/// The three-stage guillotine cuts of a grid, enumerated without any dynamic
/// programming: every composition of every axis, every piece read from the table.
struct Brute<'a> {
    grid: &'a ShapedGrid,
    table: &'a ClippedTable,
    ord: [usize; 3],
    k: usize,
}

impl Brute<'_> {
    /// The table value of the piece over the ranges `a`, `b`, `c` of the three stages.
    fn piece(&self, a: (usize, usize), b: (usize, usize), c: (usize, usize)) -> f64 {
        let ord = self.ord;
        let mut range = [0; 3];
        range[ord[0]] = self.grid.range_index(ord[0], a.0, a.1);
        range[ord[1]] = self.grid.range_index(ord[1], b.0, b.1);
        range[ord[2]] = self.grid.range_index(ord[2], c.0, c.1);
        self.table.value(range[0], range[1], range[2])
    }

    /// Every way to cut the bar `(a, b)` into pieces that all hold a stone.
    fn bar(&self, a: (usize, usize), b: (usize, usize)) -> Outcomes {
        let mut outcomes = Vec::new();
        'compositions: for composition in compositions(self.grid.cells[self.ord[2]]) {
            if composition.len() > self.k {
                continue;
            }
            let (mut start, mut total) = (0, 0.0);
            for len in &composition {
                let value = self.piece(a, b, (start, start + len));
                if value == NEG {
                    continue 'compositions;
                }
                total += value;
                start += len;
            }
            outcomes.push((composition.len(), total));
        }
        outcomes
    }

    /// Every way to cut the slab `a` into bars and the bars into pieces.
    fn slab(&self, a: (usize, usize)) -> Outcomes {
        let mut outcomes = Vec::new();
        for composition in compositions(self.grid.cells[self.ord[1]]) {
            let mut start = 0;
            let mut bars = Vec::new();
            for len in &composition {
                bars.push(self.bar(a, (start, start + len)));
                start += len;
            }
            outcomes.extend(combine(&bars, self.k));
        }
        outcomes
    }

    /// The best value per stone count `0..=k` (`-inf` where no cut holds that many).
    fn best_per_count(&self) -> Vec<f64> {
        let mut best = vec![NEG; self.k + 1];
        for composition in compositions(self.grid.cells[self.ord[0]]) {
            let mut start = 0;
            let mut slabs = Vec::new();
            for len in &composition {
                slabs.push(self.slab((start, start + len)));
                start += len;
            }
            for (n, value) in combine(&slabs, self.k) {
                if n >= 1 && value > best[n] {
                    best[n] = value;
                }
            }
        }
        best
    }
}

#[test]
fn the_positional_dp_matches_a_brute_force_enumeration_on_a_3x3x3_grid() {
    let settings = settings_with(3, 0.3, 0.2, 0.5);
    let k = settings.count_usize();
    let front = pareto_front(&random_designs(&mut Lcg(21), 3));
    let models = [
        ("plain block", plain_model()),
        ("slanted face", slanted_model()),
        ("corner cut", corner_cut_model()),
    ];
    for (name, model) in &models {
        let ctx = ShapedCtx::new(model, &settings).expect("ctx");
        let grid = grid_with_cells(&ctx, &settings, [3, 3, 3]);
        let table = clipped_for(&ctx, &grid, &front, &settings);
        for order in CutOrder::ALL {
            let what = format!("{name}, {order:?}");
            let brute = Brute {
                grid: &grid,
                table: &table,
                ord: order.axes(),
                k,
            }
            .best_per_count();
            let dp = solve_order(&grid, &table, order, k, &mut || true).expect("dp");
            assert_eq!(dp.root_value(0), NEG, "{what}");
            for (n, &expected) in brute.iter().enumerate().skip(1) {
                let value = dp.root_value(n);
                if expected == NEG {
                    assert_eq!(value, NEG, "{what}, n = {n}");
                } else {
                    assert!(
                        close(value, expected, 1e-9),
                        "{what}, n = {n}: DP {value} vs enumeration {expected}"
                    );
                }
            }

            // The layouts of the order hold the best cut: the same volume as the best
            // value, since every piece of the DP's tree holds a stone.
            let layouts =
                plan_shaped_for_order(&grid, &table, &ctx, &front, order, &settings, &mut |_| true)
                    .expect("layouts");
            let best_value = brute.iter().copied().fold(NEG, f64::max);
            let best_volume = layouts
                .iter()
                .map(|layout| layout.total_volume_mm3)
                .fold(NEG, f64::max);
            if best_value == NEG {
                assert!(layouts.is_empty(), "{what}");
            } else {
                assert!(
                    close(best_volume, best_value, 1e-9),
                    "{what}: best layout {best_volume} vs enumeration {best_value}"
                );
            }
        }
    }
}
