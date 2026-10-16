//! The design cap: the number of stones is free, the number of DIFFERENT designs is capped
//! by [`PlanSettings::max_designs`] through the design-cap chain, the capped rankings and
//! the capped refinement.

use super::{
    pareto::sanitize,
    tests::{Lcg, box_design, close, plan, random_designs, settings_with},
    *,
};

/// The six orders' mixed DP over the Pareto front of `designs`: the grid, the layouts and the
/// best one, as the planner computes them before the design-cap chain.
fn mixed_best(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
) -> (Grid, RoughLayout) {
    let front = pareto_front(designs);
    let grid = choose_grid(rough, settings);
    let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
    let mut mixed = Vec::new();
    for order in CutOrder::ALL {
        mixed.extend(
            plan_rough_for_order(&grid, &table, &front, order, &mut |_| true).expect("order"),
        );
    }
    let best = best_layout(&mixed).expect("a feasible layout").clone();
    (grid, best)
}

#[test]
fn stones_are_not_capped_one_design_fills_a_long_bar_with_many_stones() {
    // 100 x 6 x 6 mm: about sixteen cubes of 5.6 mm fit along the bar. The old planner stopped
    // at the stone cap; now the layout holds as many as fit best, all of one design.
    let rough = RoughBlock {
        x_mm: 100.0,
        y_mm: 6.0,
        z_mm: 6.0,
    };
    let settings = settings_with(1, 0.3, 0.2, 1.0);
    let results = plan(&rough, &settings, &[box_design(1)]);
    assert_ne!(results.len(), 0);
    let best = &results[0];
    assert!(
        best.stone_count() > 8,
        "{} stones is not more than the old defaults allowed",
        best.stone_count()
    );
    assert_eq!(best.design_count(), 1);
    assert!(results.iter().all(|l| l.design_count() == 1));
}

#[test]
fn with_several_designs_and_a_cap_of_one_every_layout_is_one_design_of_many_stones() {
    let rough = RoughBlock {
        x_mm: 100.0,
        y_mm: 6.0,
        z_mm: 6.0,
    };
    let flat = CandidateDesign {
        entry_id: 2,
        width: 1.0,
        length: 2.0,
        height: 0.5,
        volume: 0.8,
    };
    let settings = settings_with(1, 0.3, 0.2, 1.0);
    let results = plan(&rough, &settings, &[box_design(1), flat]);
    assert_ne!(results.len(), 0);
    assert!(results.iter().all(|l| l.design_count() == 1));
    assert!(
        results[0].stone_count() > 1,
        "one design, but more than one stone"
    );
}

/// A rough and designs whose unconstrained mixed best layout uses at least three designs.
fn three_design_fixture() -> (RoughBlock, Vec<CandidateDesign>) {
    let rough = RoughBlock {
        x_mm: 14.0,
        y_mm: 11.0,
        z_mm: 9.0,
    };
    let wide = settings_with(99, 0.3, 0.2, 0.5);
    for seed in 1..=100 {
        let designs = random_designs(&mut Lcg(seed), 8);
        let (_, best) = mixed_best(&rough, &wide, &designs);
        if best.design_count() >= 3 {
            return (rough, designs);
        }
    }
    panic!("no seed gave a mixed best layout of three designs");
}

#[test]
fn a_cap_of_two_leaves_only_layouts_of_at_most_two_designs() {
    let (rough, designs) = three_design_fixture();
    let wide = settings_with(99, 0.3, 0.2, 0.5);
    let capped = settings_with(2, 0.3, 0.2, 0.5);

    let unconstrained = plan(&rough, &wide, &designs);
    let limited = plan(&rough, &capped, &designs);
    assert_ne!(limited.len(), 0);
    assert!(
        limited.iter().all(|l| l.design_count() <= 2),
        "{:?}",
        limited
            .iter()
            .map(RoughLayout::design_count)
            .collect::<Vec<_>>()
    );
    for pair in limited.windows(2) {
        assert!(pair[0].total_volume_mm3 >= pair[1].total_volume_mm3 * (1.0 - 1e-9));
    }
    // A cap can only cost volume.
    assert!(
        unconstrained[0].total_volume_mm3 >= limited[0].total_volume_mm3 * (1.0 - 1e-9),
        "{} vs {}",
        unconstrained[0].total_volume_mm3,
        limited[0].total_volume_mm3
    );
}

#[test]
fn the_design_cap_chain_ends_in_a_group_whose_best_layout_is_within_the_cap() {
    let (rough, designs) = three_design_fixture();
    let wide = settings_with(99, 0.3, 0.2, 0.5);
    let (grid, best) = mixed_best(&rough, &wide, &designs);
    assert!(best.design_count() >= 3);
    let clean = sanitize(&designs);

    let groups = plan_design_cap(&grid, &clean, &best, 2, 1, &mut |_| true).expect("not cancelled");
    assert_ne!(groups.len(), 0);
    let last = groups.last().expect("a group");
    let last_best = best_layout(&last.layouts).expect("a layout");
    assert!(
        last_best.design_count() <= 2,
        "{}",
        last_best.design_count()
    );
    // Each round drops at least one design from the pool, and a group's layouts only use
    // its own pool (the refinement cannot bring a dropped design back).
    let mut previous = best.design_count();
    for group in &groups {
        assert!(group.pool.len() < previous, "the pool did not shrink");
        previous = group.pool.len();
        for layout in &group.layouts {
            assert!(
                layout
                    .stones
                    .iter()
                    .all(|s| group.pool.iter().any(|d| d.entry_id == s.entry_id))
            );
        }
    }
    // The lane count never changes the result.
    let threaded = plan_design_cap(&grid, &clean, &best, 2, 3, &mut |_| true).expect("lanes");
    assert_eq!(threaded, groups);
    // Nothing runs when the best layout is already within the cap.
    let none = plan_design_cap(&grid, &clean, &best, best.design_count(), 1, &mut |_| true)
        .expect("not cancelled");
    assert_eq!(none, Vec::new());
}

#[test]
fn a_tie_between_stone_counts_keeps_the_fewest_stones_not_more_smaller_cubes() {
    // 10 x 10 x 5 mm of unit cubes with no losses: 4 cubes of 5 mm fill the block, and so do
    // 32 cubes of 2.5 mm, 108 and so on. The totals tie, the DP keeps the longest run and
    // the ranking the fewest stones, so the best layout is the four large cubes.
    let rough = RoughBlock {
        x_mm: 10.0,
        y_mm: 10.0,
        z_mm: 5.0,
    };
    let settings = settings_with(1, 0.0, 0.0, 0.01);
    let results = plan(&rough, &settings, &[box_design(1)]);
    assert_ne!(results.len(), 0);
    let best = &results[0];
    assert!(
        close(best.total_volume_mm3, 500.0, 1e-9),
        "{}",
        best.total_volume_mm3
    );
    assert_eq!(best.stone_count(), 4);
    assert_eq!(best.design_count(), 1);

    // Every order's single candidate fills the block too (whatever its stone count).
    let grid = choose_grid(&rough, &settings);
    let front = pareto_front(&[box_design(1)]);
    let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
    for order in CutOrder::ALL {
        let layouts =
            plan_rough_for_order(&grid, &table, &front, order, &mut |_| true).expect("order");
        assert_eq!(layouts.len(), 1, "{order}: one candidate per order");
        assert!(
            close(layouts[0].total_volume_mm3, 500.0, 1e-9),
            "{order}: {}",
            layouts[0].total_volume_mm3
        );
    }
}

/// A hand-built two-stone layout of design 1 in a 3 x 3 x 12 mm rough: pieces of 9 and 3 mm
/// along z.
fn two_stones_of_the_weak_design() -> RoughLayout {
    let stone = |z: f64, len: f64| PlacedStone {
        entry_id: 1,
        piece_origin_mm: [0.0, 0.0, z],
        piece_size_mm: [3.0, 3.0, len],
        stone_size_mm: [3.0, 3.0, len.min(3.0)],
        table_axis: Axis::Y,
        carat: 0.0,
        volume_mm3: 0.1 * 27.0,
        pose: StonePose {
            center_mm: [1.5, 1.5, z + len / 2.0],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            mm_per_unit: 3.0,
        },
    };
    RoughLayout {
        cut_order: CutOrder::Xyz,
        stones: vec![stone(0.0, 9.0), stone(9.0, 3.0)],
        cut_plan: CutPlan {
            slabs: vec![SlabCut {
                thickness_mm: 3.0,
                bars: vec![BarCut {
                    width_mm: 3.0,
                    pieces_mm: vec![9.0, 3.0],
                }],
            }],
        },
        total_carat: 0.0,
        total_volume_mm3: 0.2 * 27.0,
        yield_fraction: 0.0,
        exact_fit: false,
    }
}

#[test]
fn refine_capped_never_returns_more_designs_than_the_cap() {
    // Design 1 is a weak cube. Design 2 is a bar, which is far better in the 3 x 3 x 9 piece;
    // design 3 is a good cube, which is better in the 3 x 3 x 3 piece. Re-picking the leaves
    // over all three would give the layout two other designs.
    let weak = CandidateDesign {
        volume: 0.1,
        ..box_design(1)
    };
    let bar = CandidateDesign {
        entry_id: 2,
        width: 1.0,
        length: 3.0,
        height: 1.0,
        volume: 2.7,
    };
    let good = CandidateDesign {
        volume: 0.9,
        ..box_design(3)
    };
    let all = [weak, bar, good];
    let rough = RoughBlock {
        x_mm: 3.0,
        y_mm: 3.0,
        z_mm: 12.0,
    };
    let layout = two_stones_of_the_weak_design();
    assert_eq!(layout.design_count(), 1);

    // Without a cap the refinement does re-pick (the case below is not vacuous).
    let free = refine(&rough, &settings_with(2, 0.0, 0.0, 0.5), &all, &layout);
    assert_eq!(free.composition(), vec![(2, 1), (3, 1)]);

    // A cap of two allows it.
    let two = refine_capped(
        &rough,
        &settings_with(2, 0.0, 0.0, 0.5),
        &all,
        &all,
        &layout,
    );
    assert_eq!(two, free);

    // A cap of one keeps the layout on its own design.
    let one = refine_capped(
        &rough,
        &settings_with(1, 0.0, 0.0, 0.5),
        &all,
        &all,
        &layout,
    );
    assert_eq!(one.design_count(), 1);
    assert_eq!(one.composition(), vec![(1, 2)]);
    assert!(one.total_volume_mm3 >= layout.total_volume_mm3);
}
