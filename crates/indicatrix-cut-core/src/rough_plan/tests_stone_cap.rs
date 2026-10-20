//! The stone cap of the block planner: `PlanSettings::max_stones` is part of the search (the
//! best layout of at most that many stones, never a larger layout cut down), a cap that does
//! not bind changes nothing, and every stage and ranking keeps it.

use super::{
    Axis, CandidateDesign, CutOrder, CutPlan, FINAL_LAYOUTS, Grid, LayoutCaps, LayoutGroup,
    PlacedStone, PlanInput, PlanProgress, PlanSettings, RoughBase, RoughBlock, RoughLayout,
    RoughModel, StonePose, best_layout, build_piece_table, choose_grid,
    dp::run_order_dp,
    finish_plan, finish_plan_lanes, merge_and_rank, merge_and_rank_capped, merge_and_rank_within,
    pareto::sanitize,
    pareto_front,
    piece::{Norm, stone_value, usable_rough},
    plan_alternatives, plan_alternatives_lanes, plan_design_cap, plan_rough_for_order,
    rank_indices, rank_indices_within, refine, refine_capped,
    shape::mesh_fixture::box_hull,
    tests::{
        Lcg, assert_layout_geometry, box_design, close, cube, plan, random_designs, settings_with,
    },
    tests_design_cap::mixed_best,
    uniform::{cell_size, enumerate_grids},
    uniform_layouts, uniform_layouts_lanes,
};

/// Lane counts below, at and above the work available in the small fixtures.
const LANES: [usize; 3] = [1, 3, 7];

/// Settings with the design cap `max_designs` and the stone cap `max_stones` (`0` is none).
fn capped(
    max_designs: u8,
    max_stones: u8,
    kerf: f64,
    allowance: f64,
    min_width: f64,
) -> PlanSettings {
    PlanSettings {
        max_stones,
        ..settings_with(max_designs, kerf, allowance, min_width)
    }
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

/// A 100 x 6 x 6 mm bar of rough: one cube design fills it with about sixteen stones.
fn long_bar() -> RoughBlock {
    RoughBlock {
        x_mm: 100.0,
        y_mm: 6.0,
        z_mm: 6.0,
    }
}

/// The best layout of every cut order over the Pareto front of `designs` (the mixed DPs).
fn order_layouts(grid: &Grid, designs: &[CandidateDesign]) -> Vec<RoughLayout> {
    let front = pareto_front(designs);
    let table = build_piece_table(grid, &front, &mut |_| true).expect("table");
    let mut mixed = Vec::new();
    for order in CutOrder::ALL {
        mixed.extend(
            plan_rough_for_order(grid, &table, &front, order, &mut |_| true).expect("order"),
        );
    }
    mixed
}

#[test]
fn every_layout_holds_at_most_the_stone_cap() {
    let rough = long_bar();
    let designs = [box_design(1)];
    let mut previous = 0.0;
    for cap in [1_u8, 2, 3, 5] {
        let settings = capped(1, cap, 0.3, 0.2, 1.0);
        let results = plan(&rough, &settings, &designs);
        assert!(!results.is_empty(), "cap {cap}");
        for layout in &results {
            assert!(
                layout.stone_count() <= usize::from(cap),
                "cap {cap}: {} stones",
                layout.stone_count()
            );
            assert_layout_geometry(layout, &rough, &settings);
        }
        // Every stone is a cube of the 5.6 mm cross-section, so the best layout fills the cap.
        let best = &results[0];
        assert_eq!(best.stone_count(), usize::from(cap), "cap {cap}");
        assert!(
            best.total_volume_mm3 >= previous,
            "cap {cap}: {} after {previous}",
            best.total_volume_mm3
        );
        previous = best.total_volume_mm3;
    }
    // Without a cap the layout holds as many as fit (about sixteen), and weighs more.
    let free = plan(&rough, &settings_with(1, 0.3, 0.2, 1.0), &designs);
    assert!(
        free[0].stone_count() > 8,
        "{} stones",
        free[0].stone_count()
    );
    assert!(free[0].total_volume_mm3 >= previous);
}

#[test]
fn the_best_capped_layout_beats_a_truncated_uncapped_one() {
    // 20 x 10 x 10 mm, no losses: two cubes of 10 mm fill the block (2000 mm^3). With one stone
    // the answer is NOT one of those cubes (1000 mm^3): the bar fills the whole block
    // (1.8 x 10^3 = 1800 mm^3).
    let rough = RoughBlock {
        x_mm: 20.0,
        y_mm: 10.0,
        z_mm: 10.0,
    };
    let designs = cube_and_bar();
    let free = plan(&rough, &settings_with(2, 0.0, 0.0, 0.01), &designs);
    let uncapped_best = &free[0];
    assert!(close(uncapped_best.total_volume_mm3, 2000.0, 1e-9));
    assert_eq!(uncapped_best.stone_count(), 2);
    let truncated = uncapped_best
        .stones
        .iter()
        .map(|stone| stone.volume_mm3)
        .fold(0.0, f64::max);
    assert!(close(truncated, 1000.0, 1e-9));

    let one_stone = capped(2, 1, 0.0, 0.0, 0.01);
    let results = plan(&rough, &one_stone, &designs);
    let best = &results[0];
    assert_eq!(best.stone_count(), 1);
    assert_eq!(best.composition(), vec![(2, 1)]);
    assert!(close(best.total_volume_mm3, 1800.0, 1e-9));
    assert!(best.total_volume_mm3 > truncated);

    // Every cut order's DP gets there on its own: its one piece is the whole block.
    let front = pareto_front(&designs);
    let grid = choose_grid(&rough, &one_stone);
    assert_eq!(grid.cells(), [48, 24, 24]);
    let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
    for order in CutOrder::ALL {
        let layouts =
            plan_rough_for_order(&grid, &table, &front, order, &mut |_| true).expect("order");
        assert_eq!(layouts.len(), 1, "{order}");
        assert_eq!(layouts[0].composition(), vec![(2, 1)], "{order}");
        assert!(
            close(layouts[0].total_volume_mm3, 1800.0, 1e-9),
            "{order}: {}",
            layouts[0].total_volume_mm3
        );
    }
}

#[test]
fn a_cap_that_does_not_bind_changes_nothing() {
    // A small rough of random designs and the long bar of cubes (about sixteen stones): the best
    // layouts hold far fewer than 99 stones (the loop below checks that the free optimum of every
    // order does), so a cap of 99 is never reached and the whole plan is the uncapped one, bit
    // for bit.
    let fixtures = [
        (
            RoughBlock {
                x_mm: 10.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            random_designs(&mut Lcg(31), 8),
        ),
        (long_bar(), vec![box_design(1)]),
    ];
    let free = settings_with(3, 0.3, 0.2, 1.0);
    let mut binding = 0;
    for (rough, designs) in &fixtures {
        // Per order: a cap equal to the free optimum's own piece count returns that optimum.
        let front = pareto_front(designs);
        let grid = choose_grid(rough, &free);
        let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
        for order in CutOrder::ALL {
            let expected =
                plan_rough_for_order(&grid, &table, &front, order, &mut |_| true).expect("order");
            let dp = run_order_dp(&grid, &table, order.axes(), &mut || true).expect("dp");
            let pieces = dp.reconstruct(&grid, &table).leaf_count();
            // The guard of the plan-level comparison below: a free optimum of more than 99 stones
            // would make the cap of 99 bind there.
            assert!(
                pieces <= 99,
                "{rough:?} {order}: the free optimum holds {pieces} stones"
            );
            if expected.is_empty() {
                continue;
            }
            let at = |cap: usize| {
                let settings = capped(3, u8::try_from(cap).expect("a small cap"), 0.3, 0.2, 1.0);
                let grid = choose_grid(rough, &settings);
                plan_rough_for_order(&grid, &table, &front, order, &mut |_| true).expect("order")
            };
            assert_eq!(at(pieces), expected, "{rough:?} {order}: cap {pieces}");
            // One stone fewer binds: the layout fits the cap and cannot weigh more.
            if pieces >= 2 {
                let tighter = at(pieces - 1);
                assert_eq!(tighter.len(), 1, "{order}");
                assert!(tighter[0].stone_count() < pieces, "{order}");
                assert!(
                    tighter[0].total_volume_mm3 <= expected[0].total_volume_mm3 * (1.0 + 1e-9),
                    "{order}"
                );
                binding += 1;
            }
        }

        assert_eq!(
            plan(rough, &capped(3, 99, 0.3, 0.2, 1.0), designs),
            plan(rough, &free, designs)
        );
    }
    assert!(binding > 0, "no order held more than one stone");
}

/// A rough and designs whose unconstrained mixed best layout uses at least three designs and
/// more than three stones, so a design cap of two and a stone cap of three each cut it down.
fn three_design_many_stone_fixture() -> (RoughBlock, Vec<CandidateDesign>) {
    let rough = RoughBlock {
        x_mm: 14.0,
        y_mm: 11.0,
        z_mm: 9.0,
    };
    let wide = settings_with(99, 0.3, 0.2, 0.5);
    for seed in 1..=200 {
        let designs = random_designs(&mut Lcg(seed), 8);
        let (_, best) = mixed_best(&rough, &wide, &designs);
        if best.design_count() >= 3 && best.stone_count() > 3 {
            return (rough, designs);
        }
    }
    panic!("no seed gave a mixed best layout of three designs and more than three stones");
}

#[test]
fn both_caps_apply_together() {
    let (rough, designs) = three_design_many_stone_fixture();
    let both = plan(&rough, &capped(2, 3, 0.3, 0.2, 0.5), &designs);
    assert_ne!(both.len(), 0);
    for layout in &both {
        assert!(
            layout.design_count() <= 2,
            "{} designs",
            layout.design_count()
        );
        assert!(layout.stone_count() <= 3, "{} stones", layout.stone_count());
    }

    // Each cap alone is a real limit on the best mixed layout of the DPs: the fixture's free best
    // (before any cap) has more than two designs and more than three stones.
    let (_, free_best) = mixed_best(&rough, &settings_with(99, 0.3, 0.2, 0.5), &designs);
    assert!(free_best.design_count() > 2);
    assert!(free_best.stone_count() > 3);

    // The two together can only cost volume. That is compared on the best mixed layout of the
    // DPs, which the caps order for certain: the final volumes of the plans are not, because the
    // refinement is a heuristic and refines a different top 20 in every run. The DPs do not see
    // the design cap, so the stone cap alone and both caps give the same best layout, and the
    // free optimum is at least as heavy.
    let (_, stones_best) = mixed_best(&rough, &capped(99, 3, 0.3, 0.2, 0.5), &designs);
    let (_, both_best) = mixed_best(&rough, &capped(2, 3, 0.3, 0.2, 0.5), &designs);
    assert_eq!(both_best, stones_best);
    assert!(stones_best.stone_count() <= 3);
    assert!(both_best.stone_count() <= 3);
    assert!(
        both_best.total_volume_mm3 <= free_best.total_volume_mm3 * (1.0 + 1e-9),
        "{} vs {}",
        both_best.total_volume_mm3,
        free_best.total_volume_mm3
    );
}

#[test]
fn the_chain_and_the_alternatives_keep_the_stone_cap() {
    // 30 x 10 x 10 mm, no losses, two stones at most: the best layout is a cube (1000 mm^3) and
    // a bar (1800 mm^3) side by side, two designs, so the chain of a design cap of one runs.
    let rough = RoughBlock {
        x_mm: 30.0,
        y_mm: 10.0,
        z_mm: 10.0,
    };
    let designs = cube_and_bar();
    let settings = capped(1, 2, 0.0, 0.0, 0.01);
    let (grid, best) = mixed_best(&rough, &settings, &designs);
    assert_eq!((best.stone_count(), best.design_count()), (2, 2));
    assert!(close(best.total_volume_mm3, 2800.0, 1e-9));
    let clean = sanitize(&designs);

    let chain = plan_design_cap(&grid, &clean, &best, 1, 1, &mut |_| true).expect("not cancelled");
    assert_ne!(chain.len(), 0);
    for group in &chain {
        assert_ne!(group.layouts.len(), 0);
        assert!(group.layouts.iter().all(|l| l.stone_count() <= 2));
    }
    let last = chain.last().expect("a group");
    let last_best = best_layout(&last.layouts).expect("a layout");
    assert_eq!(last_best.design_count(), 1);

    let alternatives =
        plan_alternatives(&grid, &clean, &best, &mut |_| true).expect("not cancelled");
    assert_ne!(alternatives.len(), 0);
    for group in &alternatives {
        assert!(group.layouts.iter().all(|l| l.stone_count() <= 2));
    }
}

/// The best total of an equal-cell layout of one design in `rough`, over every grid of at most
/// `cells` cells, every design and every assignment: the closed form of the uniform pass.
fn best_equal_cells(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
    cells: usize,
) -> f64 {
    let usable = usable_rough(rough, settings.skin_mm);
    let a2 = 2.0 * settings.allowance_mm;
    let mut best = 0.0_f64;
    for counts in enumerate_grids(cells) {
        let piece = cell_size(&usable, settings.kerf_mm, counts).map(|size| size - a2);
        let stones = counts.iter().product::<usize>() as f64;
        for design in designs {
            let norm = Norm::of(design);
            for orient in 0..6 {
                let value = stone_value(&norm, orient, piece, settings.min_width_mm);
                if value > 0.0 {
                    best = best.max(stones * value);
                }
            }
        }
    }
    best
}

#[test]
fn the_uniform_pass_keeps_the_stone_cap_and_finds_the_best_capped_grid() {
    let rough = RoughBlock {
        x_mm: 40.0,
        y_mm: 6.0,
        z_mm: 6.0,
    };
    let mut designs = random_designs(&mut Lcg(5), 4);
    designs.push(box_design(100));
    let settings = capped(4, 4, 0.3, 0.2, 1.0);
    let layouts = uniform_layouts(&rough, &settings, &designs, &mut |_| true).expect("uniform");
    assert_ne!(layouts.len(), 0);
    assert!(layouts.iter().all(|l| l.stone_count() <= 4));
    let best = layouts
        .iter()
        .map(|l| l.total_volume_mm3)
        .fold(0.0, f64::max);
    let expected = best_equal_cells(&rough, &settings, &designs, 4);
    assert!(close(best, expected, 1e-9), "{best} vs {expected}");

    // The uncapped pass has layouts above the cap (a cube design fits six stones along the
    // bar), and its best is the closed form over all 99-cell grids.
    let free = uniform_layouts(&rough, &capped(4, 0, 0.3, 0.2, 1.0), &designs, &mut |_| {
        true
    })
    .expect("uniform");
    assert!(free.iter().any(|l| l.stone_count() > 4));
    let free_best = free.iter().map(|l| l.total_volume_mm3).fold(0.0, f64::max);
    assert!(close(
        free_best,
        best_equal_cells(&rough, &settings, &designs, 99),
        1e-9
    ));
    assert!(free_best > best);
}

#[test]
fn capped_lanes_match_serial() {
    let designs = random_designs(&mut Lcg(41), 12);
    let rough = cube(14.0);
    let settings = capped(4, 3, 0.3, 0.2, 1.0);
    let grid = choose_grid(&rough, &settings);
    let mixed = order_layouts(&grid, &designs);
    assert!(mixed.iter().all(|l| l.stone_count() <= 3));
    let best = best_layout(&mixed).expect("a feasible layout").clone();

    let alternatives =
        plan_alternatives(&grid, &designs, &best, &mut |_| true).expect("alternatives");
    assert_ne!(alternatives.len(), 0);
    let uniform = uniform_layouts(&rough, &settings, &designs, &mut |_| true).expect("uniform");
    assert_ne!(uniform.len(), 0);
    let mut groups = vec![LayoutGroup {
        pool: pareto_front(&designs),
        layouts: mixed,
    }];
    groups.extend(alternatives.clone());
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniform.clone(),
    });
    let ranked = finish_plan(&rough, &settings, &designs, &groups, &mut |_| true).expect("final");
    assert_ne!(ranked.len(), 0);
    assert!(ranked.iter().all(|l| l.stone_count() <= 3));

    for lanes in LANES {
        let threaded = plan_alternatives_lanes(&grid, &designs, &best, lanes, &mut |_| true)
            .expect("alternatives");
        assert_eq!(threaded, alternatives, "lanes {lanes}");
        let threaded = uniform_layouts_lanes(&rough, &settings, &designs, lanes, &mut |_| true)
            .expect("uniform");
        assert_eq!(threaded, uniform, "lanes {lanes}");
        let threaded =
            finish_plan_lanes(&rough, &settings, &designs, &groups, lanes, &mut |_| true)
                .expect("final");
        assert_eq!(threaded, ranked, "lanes {lanes}");
    }
}

#[test]
fn the_capped_dp_ticks_once_more_per_plane_and_cancels() {
    let rough = long_bar();
    let designs = [box_design(1)];
    let front = pareto_front(&designs);
    let free_grid = choose_grid(&rough, &settings_with(1, 0.3, 0.2, 1.0));
    let table = build_piece_table(&free_grid, &front, &mut |_| true).expect("table");
    for order in CutOrder::ALL {
        let planes = free_grid.cells()[order.axes()[0]];
        let run = |grid: &Grid| {
            let mut events = 0_usize;
            let layouts = plan_rough_for_order(grid, &table, &front, order, &mut |event| {
                assert!(matches!(event, PlanProgress::Dp { .. }));
                events += 1;
                true
            })
            .expect("not cancelled");
            (events, layouts)
        };
        let (free_events, free_layouts) = run(&free_grid);
        assert_eq!(free_events, planes, "{order}");

        // The free optimum holds about sixteen stones: a cap of two binds and runs the count
        // DP as a second pass.
        let binding = choose_grid(&rough, &capped(1, 2, 0.3, 0.2, 1.0));
        let (binding_events, binding_layouts) = run(&binding);
        assert_eq!(binding_events, 2 * planes, "{order}");
        assert!(binding_layouts[0].stone_count() <= 2, "{order}");

        // A cap of 99 does not bind: one pass, and the free layout.
        let loose = choose_grid(&rough, &capped(1, 99, 0.3, 0.2, 1.0));
        let (loose_events, loose_layouts) = run(&loose);
        assert_eq!(loose_events, planes, "{order}");
        assert_eq!(loose_layouts, free_layouts, "{order}");

        // A cancel is seen in either pass, including at the first event of the second.
        for stop in [1, planes, planes + 1, 2 * planes] {
            let mut seen = 0;
            let cancelled = plan_rough_for_order(&binding, &table, &front, order, &mut |_| {
                seen += 1;
                seen < stop
            });
            assert!(cancelled.is_none(), "{order}: stop {stop}");
            assert_eq!(seen, stop, "{order}");
        }
    }
}

#[test]
fn refinement_never_adds_stones() {
    let designs = random_designs(&mut Lcg(33), 14);
    let rough = RoughBlock {
        x_mm: 17.0,
        y_mm: 11.0,
        z_mm: 9.0,
    };
    let front = pareto_front(&designs);
    let all = sanitize(&designs);
    let mut improved = 0;
    for cap in [2_u8, 3, 5] {
        let settings = capped(9, cap, 0.3, 0.2, 0.8);
        let grid = choose_grid(&rough, &settings);
        let mixed = order_layouts(&grid, &designs);
        let uniform = uniform_layouts(&rough, &settings, &designs, &mut |_| true).expect("uniform");
        assert!(!mixed.is_empty() && !uniform.is_empty(), "cap {cap}");
        for (layout, pool) in mixed
            .iter()
            .map(|l| (l, &front))
            .chain(uniform.iter().map(|l| (l, &all)))
        {
            assert!(layout.stone_count() <= usize::from(cap));
            let refined = refine(&rough, &settings, pool, layout);
            assert_eq!(refined.stone_count(), layout.stone_count(), "cap {cap}");
            let within = refine_capped(&rough, &settings, pool, &all, layout);
            assert_eq!(within.stone_count(), layout.stone_count(), "cap {cap}");
            improved += usize::from(refined.total_volume_mm3 > layout.total_volume_mm3);
        }
    }
    // The unit grid quantises every DP cut, so the refinement moves some of them; a refinement
    // that returned its input would pass the count checks above.
    assert!(improved > 0, "the refinement moved nothing");
}

#[test]
fn the_grid_does_not_depend_on_the_stone_cap() {
    for rough in [
        cube(20.0),
        RoughBlock {
            x_mm: 40.0,
            y_mm: 25.0,
            z_mm: 8.0,
        },
        long_bar(),
    ] {
        let reference = choose_grid(&rough, &settings_with(3, 0.3, 0.2, 1.0)).cells();
        for cap in [0_u8, 1, 5, 99] {
            assert_eq!(
                choose_grid(&rough, &capped(3, cap, 0.3, 0.2, 1.0)).cells(),
                reference,
                "{rough:?} cap {cap}"
            );
        }
    }
}

/// A layout of `(entry_id, volume)` stones with hand-set totals.
fn fake_layout(stones: &[(i64, f64)]) -> RoughLayout {
    let placed = |&(entry_id, volume): &(i64, f64)| PlacedStone {
        entry_id,
        piece_origin_mm: [0.0; 3],
        piece_size_mm: [1.0; 3],
        stone_size_mm: [1.0; 3],
        table_axis: Axis::Y,
        carat: volume / 10.0,
        volume_mm3: volume,
        pose: StonePose {
            center_mm: [0.0; 3],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            mm_per_unit: 1.0,
        },
    };
    let total: f64 = stones.iter().map(|stone| stone.1).sum();
    RoughLayout {
        cut_order: CutOrder::Xyz,
        stones: stones.iter().map(placed).collect(),
        cut_plan: CutPlan { slabs: Vec::new() },
        total_carat: total / 10.0,
        total_volume_mm3: total,
        yield_fraction: 0.1,
        exact_fit: false,
    }
}

#[test]
fn the_layout_caps_admit_by_designs_and_stones_with_a_minimum_of_one() {
    let two_of_one = fake_layout(&[(1, 5.0), (1, 5.0)]);
    let three_designs = fake_layout(&[(1, 5.0), (2, 4.0), (3, 3.0)]);
    let caps = |max_designs, max_stones| LayoutCaps {
        max_designs,
        max_stones,
    };
    assert!(LayoutCaps::NONE.admits(&two_of_one));
    assert!(LayoutCaps::NONE.admits(&three_designs));
    assert!(caps(1, 2).admits(&two_of_one));
    assert!(!caps(1, 1).admits(&two_of_one));
    assert!(!caps(2, usize::MAX).admits(&three_designs));
    assert!(caps(3, 3).admits(&three_designs));
    assert!(!caps(3, 2).admits(&three_designs));
    // A layout without a stone is never admitted, and a cap of zero counts as one.
    assert!(!LayoutCaps::NONE.admits(&fake_layout(&[])));
    assert!(caps(0, 0).admits(&fake_layout(&[(1, 5.0)])));
    assert!(!caps(0, 0).admits(&two_of_one));
    assert_eq!(
        LayoutCaps::designs_only(2),
        LayoutCaps {
            max_designs: 2,
            max_stones: usize::MAX
        }
    );
    assert_eq!(
        PlanSettings {
            max_stones: 7,
            max_designs: 2,
            ..PlanSettings::default()
        }
        .caps(),
        caps(2, 7)
    );
}

#[test]
fn the_within_rankings_filter_before_the_dedup_and_the_per_set_cap() {
    // Five layouts of design 1 (and one of three designs) in the same ranked list.
    let one = fake_layout(&[(1, 100.0)]);
    let two = fake_layout(&[(1, 75.0), (1, 75.0)]);
    let mixed = fake_layout(&[(1, 100.0), (2, 60.0), (3, 40.0)]);
    let four = fake_layout(&[(1, 47.5), (1, 47.5), (1, 47.5), (1, 47.5)]);
    let five = fake_layout(&[(1, 24.0), (1, 24.0), (1, 24.0), (1, 24.0), (1, 24.0)]);
    let all = vec![one.clone(), two.clone(), mixed, four.clone(), five];
    let caps = LayoutCaps {
        max_designs: 2,
        max_stones: 4,
    };

    // The cap applies first: design 1's best three (four, two, one stones) all qualify. Cutting
    // the plain ranking down afterwards loses the one-stone layout, which the per-set limit
    // already dropped for the five-stone one.
    let within = merge_and_rank_within(all.clone(), 10, caps);
    assert_eq!(within, vec![four.clone(), two.clone(), one]);
    let late: Vec<RoughLayout> = merge_and_rank(all.clone(), 10)
        .into_iter()
        .filter(|layout| caps.admits(layout))
        .collect();
    assert_eq!(late, vec![four, two]);

    // The indices agree, and the narrower forms are the same function.
    let indices = rank_indices_within(&all, 10, caps);
    assert_eq!(indices, vec![3, 1, 0]);
    assert_eq!(
        rank_indices_within(&all, 10, LayoutCaps::NONE),
        rank_indices(&all, 10)
    );
    assert_eq!(
        merge_and_rank_capped(all.clone(), 10, 2),
        merge_and_rank_within(all.clone(), 10, LayoutCaps::designs_only(2))
    );

    // The partial ranking of a stopped plan keeps both caps too.
    let groups = [LayoutGroup {
        pool: Vec::new(),
        layouts: all[..3].to_vec(),
    }];
    let extra = all[3..].to_vec();
    assert_eq!(
        super::partial_ranking_within(&groups, extra.clone(), caps),
        merge_and_rank_within(all.clone(), FINAL_LAYOUTS, caps)
    );
    assert_eq!(
        super::partial_ranking_within(&groups, extra, LayoutCaps::NONE),
        merge_and_rank(all, FINAL_LAYOUTS)
    );
}

#[test]
fn the_plain_block_entry_point_keeps_the_stone_cap_with_the_single_fits_merged_in() {
    let rough = RoughBlock {
        x_mm: 14.0,
        y_mm: 10.0,
        z_mm: 8.0,
    };
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: rough.x_mm,
            y_mm: rough.y_mm,
            z_mm: rough.z_mm,
        },
        Vec::new(),
    );
    let designs = [box_design(1)];
    let hulls = [box_hull(1, 1.0, 1.0, 1.0)];
    for cap in [1_u8, 2, 3] {
        let settings = capped(1, cap, 0.3, 0.2, 1.0);
        let input = PlanInput {
            model: &model,
            settings: &settings,
            designs: &designs,
            hulls: &hulls,
        };
        let planned = super::plan::plan(&input, &mut |_| true).expect("not cancelled");
        assert!(!planned.is_empty(), "cap {cap}");
        assert!(
            planned.iter().all(|l| l.stone_count() <= usize::from(cap)),
            "cap {cap}"
        );
        // The best layout is as heavy as the capped block planner's own (an exact fit is one
        // stone, so it is within any cap).
        let sawn = plan(&rough, &settings, &designs);
        assert!(planned[0].total_volume_mm3 >= sawn[0].total_volume_mm3 * (1.0 - 1e-9));
    }
}
