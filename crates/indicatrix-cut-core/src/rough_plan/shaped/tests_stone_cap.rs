//! The stone cap of the shaped planner: the positional count DP against a brute-force
//! enumeration of every staged layout, a cap that does not bind changing nothing, the progress
//! and cancel points of the second pass, the uniform pass, and the whole plan under one cap and
//! under both.

use super::{
    clip::{CLASS_PARTIAL, ClippedTable},
    ctx::ShapedCtx,
    dp::{plan_shaped_for_order, solve_order},
    dp_capped::solve_order_capped,
    grid::ShapedGrid,
    tests::corner_cut_model,
    tests_dp::grid_with_cells,
    tests_plan::clipped_for,
    tree::layout_from_tree_shaped,
    uniform::shaped_uniform_layouts,
};
use crate::rough_plan::{
    Axis, BoxFace, CandidateDesign, CutOrder, DesignHull, NEG, PlanInput, PlanProgress,
    PlanSettings, RoughBase, RoughCut, RoughLayout, RoughModel, pareto_front, plan,
    shape::mesh_fixture::box_hull,
    tests::{Lcg, box_design, close, random_designs, settings_with},
    tests_brute::{ByCount, at_most, compositions, fold_parts, merge_counts},
};

/// A 30 x 6 x 6 mm bar with its long top-front edge chamfered by 2 mm: a cube design fits about
/// five stones along it, and the chamfer leaves partial pieces beside the cut.
fn chamfered_bar() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 30.0,
            y_mm: 6.0,
            z_mm: 6.0,
        },
        vec![RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [2.0, 2.0],
        }],
    )
}

/// A 30 x 10 x 10 mm block with its long top-front edge chamfered by 0.1 mm: a rough that takes
/// the shaped path and answers like the plain block to within 1.5 %, because the chamfer only
/// shrinks a stone that spans the cross-section from 10 to 9.95 mm (its far corner must stay
/// under the plane `y + z = 19.9`).
fn chamfered_block() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 30.0,
            y_mm: 10.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [0.1, 0.1],
        }],
    )
}

/// A cube design and a bar of twice its length that holds 1.8 times its volume: neither
/// dominates the other, so both are on the Pareto front.
fn cube_and_bar() -> [CandidateDesign; 2] {
    let bar = CandidateDesign {
        entry_id: 2,
        width: 1.0,
        length: 2.0,
        height: 1.0,
        volume: 1.8,
    };
    [box_design(1), bar]
}

/// The cells `start..end` of each run of `runs`, in position order.
fn spans(runs: &[usize]) -> Vec<(usize, usize)> {
    let mut start = 0;
    runs.iter()
        .map(|&len| {
            let span = (start, start + len);
            start += len;
            span
        })
        .collect()
}

/// Explicit enumeration of every positional staged layout of one cut order, by exact piece
/// count: every composition of every axis, every piece read from the clipped table (see the
/// block planner's `tests_brute` for the sums).
struct Brute<'a> {
    grid: &'a ShapedGrid,
    table: &'a ClippedTable,
    ord: [usize; 3],
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

    /// The bar over the stage-A range `a` and stage-B range `b`, by piece count.
    fn bar(&self, a: (usize, usize), b: (usize, usize)) -> ByCount {
        let mut best = ByCount::new();
        for runs in compositions(self.grid.cells[self.ord[2]]) {
            let pieces: Vec<ByCount> = spans(&runs)
                .into_iter()
                .map(|c| vec![NEG, self.piece(a, b, c)])
                .collect();
            merge_counts(&mut best, &fold_parts(&pieces));
        }
        best
    }

    /// The slab over the stage-A range `a`, by piece count.
    fn slab(&self, a: (usize, usize)) -> ByCount {
        let mut best = ByCount::new();
        for runs in compositions(self.grid.cells[self.ord[1]]) {
            let bars: Vec<ByCount> = spans(&runs).into_iter().map(|b| self.bar(a, b)).collect();
            merge_counts(&mut best, &fold_parts(&bars));
        }
        best
    }

    /// The whole layout, by piece count.
    fn root(&self) -> ByCount {
        let mut best = ByCount::new();
        for runs in compositions(self.grid.cells[self.ord[0]]) {
            let slabs: Vec<ByCount> = spans(&runs).into_iter().map(|a| self.slab(a)).collect();
            merge_counts(&mut best, &fold_parts(&slabs));
        }
        best
    }
}

/// One grid of one rough: what the capped DP and the layout built from it are checked with.
struct Case<'a> {
    ctx: &'a ShapedCtx,
    grid: &'a ShapedGrid,
    table: &'a ClippedTable,
    front: &'a [CandidateDesign],
    settings: &'a PlanSettings,
}

impl Case<'_> {
    /// Checks the count DP of `order` under `cap` against the enumeration `by_count` of that
    /// order: the value bit for bit, the tree's piece count, and the layout built from it.
    /// `None` when no layout of at most `cap` pieces exists, otherwise whether the cap bound
    /// (the best of at most `cap` pieces is worth less than the best of any number).
    fn check(&self, order: CutOrder, by_count: &[f64], cap: usize, what: &str) -> Option<bool> {
        let (grid, table, front, settings) = (self.grid, self.table, self.front, self.settings);
        let dp = solve_order_capped(grid, table, order, cap, &mut || true).expect("dp");
        let candidates = dp.candidates(grid, table, self.ctx, settings, front);
        let expected = at_most(by_count, cap);
        let value = dp.root_value();
        if expected == NEG {
            assert!(value == NEG, "{what}: dp {value}, brute -inf");
            assert!(candidates.is_empty(), "{what}");
            return None;
        }
        // The DP and the enumeration add the same pieces in the same order and floating-point
        // addition is monotone, so the maxima agree bit for bit.
        assert_eq!(
            value.to_bits(),
            expected.to_bits(),
            "{what}: dp {value}, brute {expected}"
        );
        let tree = dp.reconstruct(grid, table);
        let leaves = tree.leaf_count();
        assert!((1..=cap).contains(&leaves), "{what}: {leaves} pieces");
        // Of the layouts worth the best value, the tree has the fewest pieces.
        let fewest = by_count
            .iter()
            .position(|v| v.to_bits() == expected.to_bits())
            .expect("the best value is reached by some count");
        assert_eq!(leaves, fewest, "{what}");
        // Every piece holds a stone, so nothing is merged and the layout weighs what the DP
        // valued the tree at.
        let layout = layout_from_tree_shaped(self.ctx, order, &tree, front, settings);
        assert_eq!(layout.stone_count(), leaves, "{what}");
        assert!(close(layout.total_volume_mm3, value, 1e-9), "{what}");
        assert_eq!(candidates, vec![layout], "{what}");
        Some(expected < at_most(by_count, usize::MAX))
    }
}

#[test]
fn the_capped_shaped_dp_equals_brute_force_on_cut_roughs() {
    let settings = settings_with(3, 0.3, 0.2, 0.5);
    let cases = [
        (
            corner_cut_model(),
            random_designs(&mut Lcg(21), 3),
            [[3, 2, 3], [2, 3, 3]],
        ),
        (chamfered_bar(), vec![box_design(1)], [[3, 2, 3], [3, 3, 2]]),
    ];
    let (mut checked, mut binding, mut partial) = (0_usize, 0_usize, 0_usize);
    for (model, designs, cell_sets) in &cases {
        let front = pareto_front(designs);
        let ctx = ShapedCtx::new(model, &settings).expect("ctx");
        for cells in cell_sets {
            let grid = grid_with_cells(&ctx, &settings, *cells);
            let table = clipped_for(&ctx, &grid, &front, &settings);
            partial += usize::from(table.classes.contains(&CLASS_PARTIAL));
            let case = Case {
                ctx: &ctx,
                grid: &grid,
                table: &table,
                front: &front,
                settings: &settings,
            };
            for order in CutOrder::ALL {
                let by_count = Brute {
                    grid: &grid,
                    table: &table,
                    ord: order.axes(),
                }
                .root();
                for cap in 1..=5 {
                    let what = format!("{cells:?} {order} cap {cap}");
                    if let Some(bound) = case.check(order, &by_count, cap, &what) {
                        checked += 1;
                        binding += usize::from(bound);
                    }
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
    assert!(partial > 0, "the cuts must leave partial pieces");
}

#[test]
fn the_capped_shaped_dp_ticks_and_cancels() {
    let settings = settings_with(1, 0.3, 0.2, 1.0);
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 30.0,
            y_mm: 6.0,
            z_mm: 6.0,
        },
        Vec::new(),
    );
    let front = [box_design(1)];
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let grid = grid_with_cells(&ctx, &settings, [6, 3, 3]);
    let table = clipped_for(&ctx, &grid, &front, &settings);
    let mut binding_orders = 0;
    for order in CutOrder::ALL {
        let planes = grid.cells[order.axes()[0]];
        let free = solve_order(&grid, &table, order, &mut || true).expect("dp");
        let pieces = free.reconstruct(&grid, &table).leaf_count();
        if pieces < 2 {
            continue;
        }
        let run = |cap: usize| {
            let settings = PlanSettings {
                max_stones: u8::try_from(cap).expect("a small cap"),
                ..settings
            };
            let mut events = 0_usize;
            let layouts = plan_shaped_for_order(
                &grid,
                &table,
                &ctx,
                &front,
                order,
                &settings,
                &mut |event| {
                    assert!(matches!(event, PlanProgress::Dp { .. }));
                    events += 1;
                    true
                },
            )
            .expect("not cancelled");
            (events, layouts)
        };
        // The free DP ticks twice per plane (the bar fill and the slab fill). A cap that its
        // optimum respects adds nothing and returns it; one it exceeds runs the count DP as a
        // second pass.
        let (free_events, free_layouts) = run(0);
        assert_eq!(free_events, 2 * planes, "{order}");
        let (loose_events, loose_layouts) = run(pieces);
        assert_eq!(loose_events, 2 * planes, "{order}");
        assert_eq!(loose_layouts, free_layouts, "{order}");
        let (binding_events, binding_layouts) = run(pieces - 1);
        assert_eq!(binding_events, 4 * planes, "{order}");
        assert!(
            binding_layouts.iter().all(|l| l.stone_count() < pieces),
            "{order}"
        );

        // A cancel is seen in either pass, including at the first event of the second.
        let tight = PlanSettings {
            max_stones: u8::try_from(pieces - 1).expect("a small cap"),
            ..settings
        };
        for stop in [1, 2 * planes, 2 * planes + 1, 4 * planes] {
            let mut seen = 0;
            let cancelled =
                plan_shaped_for_order(&grid, &table, &ctx, &front, order, &tight, &mut |_| {
                    seen += 1;
                    seen < stop
                });
            assert!(cancelled.is_none(), "{order}: stop {stop}");
            assert_eq!(seen, stop, "{order}");
        }
        binding_orders += 1;
    }
    assert!(binding_orders > 0, "no order held more than one piece");
}

#[test]
fn the_shaped_uniform_pass_keeps_the_stone_cap() {
    let free = settings_with(1, 0.3, 0.2, 1.0);
    // A 40 x 6 x 6 bar of cubes: the best uncapped counts are six, seven and five stones.
    let bar = RoughModel::new(
        RoughBase::Block {
            x_mm: 40.0,
            y_mm: 6.0,
            z_mm: 6.0,
        },
        Vec::new(),
    );
    let designs = [box_design(1)];
    let ctx = ShapedCtx::new(&bar, &free).expect("ctx");
    let uniform = |max_stones: u8, lanes: usize| {
        let settings = PlanSettings { max_stones, ..free };
        shaped_uniform_layouts(&ctx, &designs, &settings, lanes, &mut |_| true).expect("uniform")
    };
    assert!(uniform(0, 1).iter().any(|l| l.stone_count() > 3));
    for cap in [1_u8, 3] {
        let layouts = uniform(cap, 1);
        assert_ne!(layouts.len(), 0, "cap {cap}");
        assert!(
            layouts.iter().all(|l| l.stone_count() <= usize::from(cap)),
            "cap {cap}"
        );
        // The best capped grid fills the cap: one cube per cell.
        assert_eq!(
            layouts.iter().map(RoughLayout::stone_count).max(),
            Some(usize::from(cap)),
            "cap {cap}"
        );
        assert_eq!(uniform(cap, 3), layouts, "cap {cap}, three lanes");
    }

    // A cylinder: cells at its corners are outside it, so a grid has fewer stones than cells.
    let cylinder = RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 12.0,
            length_mm: 20.0,
            axis: Axis::Z,
        },
        Vec::new(),
    );
    let designs = random_designs(&mut Lcg(5), 6);
    let settings = PlanSettings {
        max_stones: 3,
        ..settings_with(8, 0.3, 0.2, 1.0)
    };
    let ctx = ShapedCtx::new(&cylinder, &settings).expect("ctx");
    let layouts =
        shaped_uniform_layouts(&ctx, &designs, &settings, 1, &mut |_| true).expect("uniform");
    assert_ne!(layouts.len(), 0);
    assert!(layouts.iter().all(|l| l.stone_count() <= 3));
}

#[test]
fn a_stone_cap_bounds_every_shaped_layout_and_a_loose_cap_changes_nothing() {
    let model = chamfered_bar();
    let designs = [box_design(1)];
    let hulls: Vec<DesignHull> = Vec::new();
    let run = |max_stones: u8| {
        let settings = PlanSettings {
            max_stones,
            ..settings_with(1, 0.3, 0.2, 1.0)
        };
        let input = PlanInput {
            model: &model,
            settings: &settings,
            designs: &designs,
            hulls: &hulls,
        };
        plan(&input, &mut |_| true).expect("not cancelled")
    };

    // The free plan holds about five cubes in its best layout.
    let free = run(0);
    assert_ne!(free.len(), 0);
    assert!(
        free[0].stone_count() > 2,
        "{} stones",
        free[0].stone_count()
    );

    // A cap of two: every layout of the list fits. That the list is the best capped one and not
    // the free list cut down is checked in `the_best_capped_shaped_layout_beats_a_truncated_...`.
    let capped = run(2);
    assert_ne!(capped.len(), 0);
    assert!(capped.iter().all(|l| l.stone_count() <= 2));
    assert!(capped[0].total_volume_mm3 <= free[0].total_volume_mm3 * (1.0 + 1e-9));

    // The best layouts hold far fewer than 99 stones, so a cap of 99 never binds: the plan is
    // the uncapped one, bit for bit.
    assert_eq!(run(99), free);
}

#[test]
fn the_capped_shaped_dp_beats_a_truncated_uncapped_layout_on_every_order() {
    // 20 x 10 x 10 mm, no losses, on a grid of 5 mm cells: two cubes of 10 mm fill the block
    // (2000 mm^3). With one stone the answer is NOT one of those cubes (1000 mm^3): the bar
    // fills the whole block (1.8 x 10^3 = 1800 mm^3).
    let free = settings_with(2, 0.0, 0.0, 0.01);
    let one_stone = PlanSettings {
        max_stones: 1,
        ..free
    };
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 20.0,
            y_mm: 10.0,
            z_mm: 10.0,
        },
        Vec::new(),
    );
    let front = pareto_front(&cube_and_bar());
    let ctx = ShapedCtx::new(&model, &free).expect("ctx");
    let grid = grid_with_cells(&ctx, &free, [4, 2, 2]);
    let table = clipped_for(&ctx, &grid, &front, &free);
    for order in CutOrder::ALL {
        let solve = |settings: &PlanSettings| {
            plan_shaped_for_order(&grid, &table, &ctx, &front, order, settings, &mut |_| true)
                .expect("not cancelled")
        };
        let uncapped = solve(&free);
        assert_eq!(uncapped.len(), 1, "{order}");
        assert_eq!(uncapped[0].composition(), vec![(1, 2)], "{order}");
        assert!(close(uncapped[0].total_volume_mm3, 2000.0, 1e-9), "{order}");
        let truncated = uncapped[0]
            .stones
            .iter()
            .map(|stone| stone.volume_mm3)
            .fold(0.0, f64::max);
        assert!(close(truncated, 1000.0, 1e-9), "{order}");

        let capped = solve(&one_stone);
        assert_eq!(capped.len(), 1, "{order}");
        assert_eq!(capped[0].composition(), vec![(2, 1)], "{order}");
        assert!(
            close(capped[0].total_volume_mm3, 1800.0, 1e-9),
            "{order}: {}",
            capped[0].total_volume_mm3
        );
        assert!(capped[0].total_volume_mm3 > truncated, "{order}");
    }
}

#[test]
fn the_best_capped_shaped_layout_beats_a_truncated_uncapped_one() {
    // The chamfer leaves a cross-section whose stones must keep under y + z = 10 at the far
    // corner, so a cube holds at most 5 mm (125 mm^3) and six of them fill the 30 mm bar: no
    // layout of any design weighs more than 25 mm^3 per mm of bar (750 mm^3 in all). With one
    // stone the answer is NOT one of those cubes: the bar design along the whole rough holds
    // 5 x 10 x 5 mm, 1.8 x 125 = 225 mm^3.
    let model = chamfered_bar();
    let designs = cube_and_bar();
    let hulls: Vec<DesignHull> = Vec::new();
    let run = |max_stones: u8| {
        let settings = PlanSettings {
            max_stones,
            ..settings_with(2, 0.0, 0.0, 0.01)
        };
        let input = PlanInput {
            model: &model,
            settings: &settings,
            designs: &designs,
            hulls: &hulls,
        };
        plan(&input, &mut |_| true).expect("not cancelled")
    };

    let free = run(0);
    assert_ne!(free.len(), 0);
    assert!(
        free[0].stone_count() > 1,
        "{} stones",
        free[0].stone_count()
    );
    let truncated = free[0]
        .stones
        .iter()
        .map(|stone| stone.volume_mm3)
        .fold(0.0, f64::max);

    let capped = run(1);
    assert_ne!(capped.len(), 0);
    assert!(capped.iter().all(|l| l.stone_count() == 1));
    let best = &capped[0];
    assert_eq!(best.composition(), vec![(2, 1)]);
    assert!(
        close(best.total_volume_mm3, 225.0, 1e-6),
        "{} mm^3",
        best.total_volume_mm3
    );
    assert!(
        best.total_volume_mm3 > truncated,
        "{} mm^3 against the largest stone of the free best, {truncated}",
        best.total_volume_mm3
    );
}

#[test]
fn both_caps_on_a_shaped_rough() {
    // Without a cap the best layout is three cubes (about 2955 mm^3). With two stones at most it
    // is a cube and a bar, two designs (about 2758 mm^3): the stone cap is what makes the design
    // cap bind, and the design cap alone leaves the three cubes alone.
    let model = chamfered_block();
    let designs = cube_and_bar();
    let hulls = [box_hull(1, 1.0, 1.0, 1.0), box_hull(2, 1.0, 1.0, 2.0)];
    let run = |max_designs: u8, max_stones: u8| {
        let settings = PlanSettings {
            max_stones,
            ..settings_with(max_designs, 0.0, 0.0, 0.01)
        };
        let input = PlanInput {
            model: &model,
            settings: &settings,
            designs: &designs,
            hulls: &hulls,
        };
        plan(&input, &mut |_| true).expect("not cancelled")
    };

    // Each cap alone is a real limit: the best layout under the stone cap mixes two designs,
    // and the best under the design cap holds three stones.
    let only_stones = run(2, 2);
    assert_ne!(only_stones.len(), 0);
    assert!(
        only_stones[0].design_count() > 1,
        "{} designs",
        only_stones[0].design_count()
    );
    let only_designs = run(1, 0);
    assert_ne!(only_designs.len(), 0);
    assert!(
        only_designs[0].stone_count() > 2,
        "{} stones",
        only_designs[0].stone_count()
    );

    // Together every layout of the list keeps both.
    let planned = run(1, 2);
    assert_ne!(planned.len(), 0);
    for layout in &planned {
        assert!(
            layout.design_count() <= 1,
            "{} designs",
            layout.design_count()
        );
        assert!(layout.stone_count() <= 2, "{} stones", layout.stone_count());
    }
}

#[test]
#[ignore = "slow rough-planner test (over 60 s); run with --ignored"]
fn a_stone_cap_bounds_every_layout_of_a_cylinder() {
    let model = RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 12.0,
            length_mm: 20.0,
            axis: Axis::Y,
        },
        Vec::new(),
    );
    let designs = random_designs(&mut Lcg(101), 4);
    let settings = PlanSettings {
        max_stones: 2,
        ..settings_with(2, 0.3, 0.2, 0.5)
    };
    let hulls: Vec<DesignHull> = Vec::new();
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };
    let planned = plan(&input, &mut |_| true).expect("not cancelled");
    assert_ne!(planned.len(), 0);
    assert!(planned.iter().all(|l| l.stone_count() <= 2));
}
