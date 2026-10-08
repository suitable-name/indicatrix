//! The `_lanes` twins of the block planner against their serial functions: the same result
//! for every lane count, monotonic progress, and a cancel that reaches every lane.

use super::{
    CandidateDesign, CutOrder, LayoutGroup, PieceTable, PlanProgress, PlanSettings, RoughBlock,
    best_layout, build_piece_table, build_piece_table_lanes, choose_grid, finish_plan,
    finish_plan_lanes, pareto_front, plan_alternatives, plan_alternatives_lanes,
    plan_rough_for_order,
    tests::{Lcg, cube, random_designs, settings_with},
    uniform_layouts, uniform_layouts_lanes,
};

/// Lane counts below, at and above the work available in the small fixtures.
const LANES: [usize; 3] = [1, 3, 7];

/// A small block, its settings and a dozen candidate designs.
fn fixture() -> (RoughBlock, PlanSettings, Vec<CandidateDesign>) {
    let designs = random_designs(&mut Lcg(41), 12);
    (cube(14.0), settings_with(4, 0.3, 0.2, 1.0), designs)
}

fn same_table(a: &PieceTable, b: &PieceTable, lanes: usize) {
    assert_eq!(a.cells, b.cells, "lanes {lanes}");
    assert_eq!(a.values, b.values, "lanes {lanes}");
    assert_eq!(a.design, b.design, "lanes {lanes}");
    assert_eq!(a.orient, b.orient, "lanes {lanes}");
}

#[test]
fn the_piece_table_on_lanes_is_the_serial_table_with_one_event_per_plane() {
    let (rough, settings, designs) = fixture();
    let front = pareto_front(&designs);
    let grid = choose_grid(&rough, &settings);
    let serial = build_piece_table(&grid, &front, &mut |_| true).expect("serial");
    let nx = grid.cells()[0];
    for lanes in LANES {
        let mut events = Vec::new();
        let table = build_piece_table_lanes(&grid, &front, lanes, &mut |event| {
            events.push(event);
            true
        })
        .expect("lanes");
        same_table(&table, &serial, lanes);

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
        planes.sort_unstable();
        assert_eq!(planes, (1..=nx).collect::<Vec<_>>(), "lanes {lanes}");

        let mut seen = 0;
        let cancelled = build_piece_table_lanes(&grid, &front, lanes, &mut |_| {
            seen += 1;
            seen < 2
        });
        assert!(cancelled.is_none(), "lanes {lanes}");
        assert_eq!(seen, 2, "lanes {lanes}");
    }
}

/// The best mixed layout of the six orders over `front`, as the alternatives start from.
fn best_of_orders(
    grid: &super::Grid,
    table: &PieceTable,
    front: &[CandidateDesign],
    settings: &PlanSettings,
) -> (Vec<super::RoughLayout>, super::RoughLayout) {
    let mut mixed = Vec::new();
    for order in CutOrder::ALL {
        mixed.extend(
            plan_rough_for_order(grid, table, front, order, settings.count, &mut |_| true)
                .expect("order"),
        );
    }
    let best = best_layout(&mixed).expect("a feasible layout").clone();
    (mixed, best)
}

#[test]
fn the_alternatives_on_lanes_are_the_serial_groups() {
    let (rough, settings, designs) = fixture();
    let front = pareto_front(&designs);
    let grid = choose_grid(&rough, &settings);
    let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
    let (_, best) = best_of_orders(&grid, &table, &front, &settings);
    let serial =
        plan_alternatives(&grid, &designs, &best, settings.count, &mut |_| true).expect("serial");
    assert_ne!(serial.len(), 0);
    for lanes in LANES {
        let mut last = None;
        let groups = plan_alternatives_lanes(
            &grid,
            &designs,
            &best,
            settings.count,
            lanes,
            &mut |event| {
                last = Some(event);
                true
            },
        )
        .expect("lanes");
        assert_eq!(groups, serial, "lanes {lanes}");
        assert!(
            matches!(last, Some(PlanProgress::Alternatives { done: 3, total: 3 })),
            "lanes {lanes}: {last:?}"
        );

        let mut seen = 0;
        let cancelled =
            plan_alternatives_lanes(&grid, &designs, &best, settings.count, lanes, &mut |_| {
                seen += 1;
                seen < 4
            });
        assert!(cancelled.is_none(), "lanes {lanes}");
    }
}

#[test]
fn the_uniform_pass_on_lanes_is_the_serial_scan_with_monotonic_progress() {
    let (rough, mut settings, _) = fixture();
    settings.count = 6;
    // More designs than one poll interval, so several chunks and several reports.
    let designs = random_designs(&mut Lcg(5), 300);
    let serial = uniform_layouts(&rough, &settings, &designs, &mut |_| true).expect("serial");
    assert_ne!(serial.len(), 0);
    for lanes in LANES {
        let mut done_seen = Vec::new();
        let layouts = uniform_layouts_lanes(&rough, &settings, &designs, lanes, &mut |event| {
            match event {
                PlanProgress::Uniform { done, total } => {
                    assert_eq!(total, 300);
                    done_seen.push(done);
                }
                other => panic!("unexpected event {other:?}"),
            }
            true
        })
        .expect("lanes");
        assert_eq!(layouts, serial, "lanes {lanes}");
        assert!(done_seen.is_sorted(), "lanes {lanes}: {done_seen:?}");
        assert_eq!(done_seen.last(), Some(&300), "lanes {lanes}");

        let mut seen = 0;
        let cancelled = uniform_layouts_lanes(&rough, &settings, &designs, lanes, &mut |_| {
            seen += 1;
            seen < 2
        });
        assert!(cancelled.is_none(), "lanes {lanes}");
    }
}

#[test]
fn the_refinement_on_lanes_is_the_serial_final_ranking() {
    let (rough, settings, designs) = fixture();
    let front = pareto_front(&designs);
    let grid = choose_grid(&rough, &settings);
    let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
    let (mixed, best) = best_of_orders(&grid, &table, &front, &settings);
    let mut groups = vec![LayoutGroup {
        pool: front,
        layouts: mixed,
    }];
    groups.extend(
        plan_alternatives(&grid, &designs, &best, settings.count, &mut |_| true)
            .expect("alternatives"),
    );
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniform_layouts(&rough, &settings, &designs, &mut |_| true).expect("uniform"),
    });
    let total: usize = groups.iter().map(|g| g.layouts.len()).sum();
    assert!(
        total > super::REFINE_TOP,
        "the fixture must exceed the refine cap"
    );

    let serial = finish_plan(&rough, &settings, &designs, &groups, &mut |_| true).expect("serial");
    assert!(serial.len() <= super::FINAL_TOP);
    assert_ne!(serial.len(), 0);
    let mut serial_refines = 0;
    let _ = finish_plan(&rough, &settings, &designs, &groups, &mut |_| {
        serial_refines += 1;
        true
    });
    assert!((1..=super::REFINE_TOP).contains(&serial_refines));
    for lanes in LANES {
        let mut refines = 0;
        let ranked = finish_plan_lanes(&rough, &settings, &designs, &groups, lanes, &mut |event| {
            assert!(matches!(event, PlanProgress::Refine));
            refines += 1;
            true
        })
        .expect("lanes");
        assert_eq!(ranked, serial, "lanes {lanes}");
        assert_eq!(refines, serial_refines, "lanes {lanes}");

        let mut seen = 0;
        let cancelled = finish_plan_lanes(&rough, &settings, &designs, &groups, lanes, &mut |_| {
            seen += 1;
            seen < 3
        });
        assert!(cancelled.is_none(), "lanes {lanes}");
    }
}
