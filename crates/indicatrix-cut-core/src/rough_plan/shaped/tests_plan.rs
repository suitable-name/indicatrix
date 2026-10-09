//! Plan-level tests of the shaped planner: alternatives, empty-cell merging,
//! uniform grids, progress and cancellation, poses, and the plain path.

use super::{
    clip::{BuildClipParams, CLASS_INTERIOR, ClippedTable, build_clipped_table},
    ctx::ShapedCtx,
    dp::{ShapedAltParams, alternative_table, plan_shaped_alternatives, plan_shaped_for_order},
    grid::{ShapedGrid, choose_shaped_grid, choose_shaped_grid_at},
    tests::{
        all_ranges, assert_carats, assert_pose_right_handed, assert_stone_inside, piece_total,
    },
    tree::{StoneFitter, layout_from_tree_shaped},
    uniform::shaped_uniform_layouts,
};
use crate::{
    rough_plan::{
        Axis, BoxFace, CandidateDesign, CutOrder, DesignHull, PlanInput, PlanProgress,
        PlanSettings, RoughBase, RoughBlock, RoughCut, RoughLayout, RoughModel, SingleFit,
        StonePose, layout_from_single_fit, pareto_front,
        piece::{ASSIGNMENTS, Norm, stone_value},
        plan, plan_rough,
        rank::{best_layout, most_used_design},
        tests::{Lcg, box_design, random_designs, settings_with},
        tree::{Bar, Leaf, Slab, Tree, assignment_axes, orient_of_pose},
    },
    yield_metrics::carat_weight,
};

/// The clipped table of `ctx`'s model over `grid` for `front`.
pub(super) fn clipped_for(
    ctx: &ShapedCtx,
    grid: &ShapedGrid,
    front: &[CandidateDesign],
    settings: &PlanSettings,
) -> ClippedTable {
    let size_table = grid
        .size_table(settings, front, &mut |_| true)
        .expect("size table");
    let params = BuildClipParams {
        grid,
        front,
        non_box_planes: &ctx.non_box,
        mesh: None,
        size_table: &size_table,
        settings,
        slice: 0..grid.cells[0],
        cached_classes: None,
    };
    build_clipped_table(&params, &mut |_| true).expect("clipped table")
}

/// Four designs none of which dominates another (ratios `l`, `h`, `f`).
pub(super) fn four_front_designs() -> Vec<CandidateDesign> {
    let make = |entry_id, length, height, volume| CandidateDesign {
        entry_id,
        width: 1.0,
        length,
        height,
        volume,
    };
    vec![
        make(1, 1.0, 1.0, 0.9),
        make(2, 1.5, 0.6, 0.6),
        make(3, 1.0, 0.5, 0.4),
        make(4, 2.0, 1.2, 1.1),
    ]
}

/// Every interior entry of `table` holds the best value any design of `pool`
/// reaches in that piece, and its recorded design and assignment reach it.
fn assert_interior_entries_belong_to_pool(
    grid: &ShapedGrid,
    settings: &PlanSettings,
    table: &ClippedTable,
    pool: &[CandidateDesign],
) {
    let [ga, gb, gc] = grid.cells;
    let norms: Vec<Norm> = pool.iter().map(Norm::of).collect();
    let a2 = 2.0 * settings.allowance_mm;
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0);
    let mut entry = 0;
    let mut checked = 0;
    for ra in all_ranges(ga) {
        for rb in all_ranges(gb) {
            for rc in all_ranges(gc) {
                if table.classes[entry] == CLASS_INTERIOR {
                    let (_, size) = grid.piece_box([ra, rb, rc]);
                    let usable = size.map(|s| s - a2);
                    let best = norms
                        .iter()
                        .flat_map(|norm| {
                            (0..ASSIGNMENTS.len())
                                .map(move |o| stone_value(norm, o, usable, settings.min_width_mm))
                        })
                        .fold(f64::NEG_INFINITY, f64::max);
                    let value = table.values[entry];
                    if best > f64::NEG_INFINITY {
                        assert!(close(value, best), "entry {entry}: {value} vs best {best}");
                        let own = stone_value(
                            &norms[table.design[entry] as usize],
                            usize::from(table.orient[entry]),
                            usable,
                            settings.min_width_mm,
                        );
                        assert!(close(own, value), "entry {entry}: {own} vs {value}");
                        checked += 1;
                    } else {
                        assert_eq!(value, f64::NEG_INFINITY);
                    }
                }
                entry += 1;
            }
        }
    }
    assert!(checked > 0);
}

#[test]
fn alternative_rounds_are_scored_with_their_own_pool() {
    let designs = four_front_designs();
    let front = pareto_front(&designs);
    assert_eq!(front.len(), 4);
    let settings = settings_with(4, 0.3, 0.2, 0.5);
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 16.0,
            y_mm: 12.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Corner {
            faces: [BoxFace::Top, BoxFace::Front, BoxFace::Right],
            setbacks_mm: [4.0, 4.0, 4.0],
        }],
    );
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
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
    let dropped = most_used_design(&best).expect("a stone");

    let params = ShapedAltParams {
        grid: &grid,
        table: &table,
        ctx: &ctx,
        all_designs: &designs,
        best: &best,
        settings: &settings,
        lanes: 1,
    };
    let remaining: Vec<CandidateDesign> = designs
        .iter()
        .filter(|d| d.entry_id != dropped)
        .copied()
        .collect();
    let pool = pareto_front(&remaining);
    assert_eq!(
        pool.len(),
        3,
        "dropping one of four front designs leaves three"
    );

    // The round's table holds the pool's own best design in every interior
    // piece, not an index into the baseline front.
    let round_table = alternative_table(&params, &pool, &mut |_| true).expect("round table");
    assert_interior_entries_belong_to_pool(&grid, &settings, &round_table, &pool);

    let groups = plan_shaped_alternatives(&params, &mut |_| true).expect("alternatives");
    assert_ne!(groups.len(), 0);
    assert_eq!(groups[0].pool, pool);
    for layout in &groups[0].layouts {
        assert_eq!(layout.stones.len(), piece_total(layout));
        assert_carats(layout, &settings);
        for stone in &layout.stones {
            assert_ne!(stone.entry_id, dropped);
            assert!(pool.iter().any(|d| d.entry_id == stone.entry_id));
        }
    }
}

#[test]
fn an_exterior_cell_is_merged_into_its_neighbour_bar_and_not_dropped() {
    let settings = settings_with(16, 0.3, 0.2, 0.5);
    // A slanted face removes the corner around (20, 20): x + y > 28.7.
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 20.0,
            y_mm: 20.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Face {
            normal: [1.0, 1.0, 0.0],
            depth_mm: 8.0,
        }],
    );
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    assert!(
        ctx.bbox_min.iter().all(|c| c.abs() < 1e-9),
        "the slanted cut keeps the box's minimum corner"
    );
    let pool = [box_design(1)];
    let bar = |width: f64| Bar {
        width,
        leaves: vec![Leaf {
            len: 10.0,
            design: 0,
            orient: 0,
        }],
    };
    // 4 x 4 cells of 4.7 mm; cell (3, 3) lies wholly behind the slanted face.
    let full = Tree {
        slabs: (0..4)
            .map(|_| Slab {
                thickness: 4.7,
                bars: (0..4).map(|_| bar(4.7)).collect(),
            })
            .collect(),
    };
    let merged = layout_from_tree_shaped(&ctx, CutOrder::Xyz, &full, &pool, &settings);

    let mut without = full;
    without.slabs[3].bars.pop();
    let reference = layout_from_tree_shaped(&ctx, CutOrder::Xyz, &without, &pool, &settings);

    assert_eq!(merged.stones.len(), 15);
    assert_eq!(merged.stones.len(), piece_total(&merged));
    assert_carats(&merged, &settings);
    assert_carats(&reference, &settings);
    let slabs = &merged.cut_plan.slabs;
    assert_eq!(slabs.len(), 4);
    assert!(slabs[..3].iter().all(|s| s.bars.len() == 4));
    assert_eq!(slabs[3].bars.len(), 3);
    assert!((slabs[3].bars[2].width_mm - (4.7 + 0.3 + 4.7)).abs() < 1e-9);

    // The merged piece spans both cells and keeps the position of the first.
    let last = merged.stones.last().expect("a stone");
    assert!((last.piece_origin_mm[0] - 15.0).abs() < 1e-9);
    assert!((last.piece_origin_mm[1] - 10.0).abs() < 1e-9);
    assert!((last.piece_size_mm[1] - 9.7).abs() < 1e-9);
    let unmerged_last = reference.stones.last().expect("a stone");
    assert!(last.volume_mm3 >= unmerged_last.volume_mm3 - 1e-9);
    assert!(merged.total_volume_mm3 >= reference.total_volume_mm3 - 1e-9);
    for stone in &merged.stones {
        assert!(stone.volume_mm3 > 0.0);
        assert_stone_inside(stone, &ctx.usable, settings.allowance_mm);
    }
}

#[test]
fn shaped_uniform_layouts_keep_the_extent_and_match_stones_to_pieces() {
    let designs = random_designs(&mut Lcg(5), 3);
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
    let layouts =
        shaped_uniform_layouts(&ctx, &designs, &settings, 1, &mut |_| true).expect("not cancelled");
    assert_ne!(layouts.len(), 0);

    let kerf = settings.kerf_mm;
    let extent = |sizes: &[f64]| kerf.mul_add((sizes.len() - 1) as f64, sizes.iter().sum::<f64>());
    for layout in &layouts {
        assert_eq!(layout.cut_order, CutOrder::Xyz);
        assert_eq!(layout.stones.len(), piece_total(layout));
        assert_carats(layout, &settings);
        assert!(layout.stone_count() <= 8);
        // Merged cells keep their material: the saw plan still covers the whole box.
        let slabs: Vec<f64> = layout
            .cut_plan
            .slabs
            .iter()
            .map(|s| s.thickness_mm)
            .collect();
        assert!((extent(&slabs) - ctx.bbox_extents[0]).abs() < 1e-9);
        for slab in &layout.cut_plan.slabs {
            let bars: Vec<f64> = slab.bars.iter().map(|b| b.width_mm).collect();
            assert!((extent(&bars) - ctx.bbox_extents[1]).abs() < 1e-9);
            for bar in &slab.bars {
                assert!((extent(&bar.pieces_mm) - ctx.bbox_extents[2]).abs() < 1e-9);
            }
        }
        for stone in &layout.stones {
            assert_stone_inside(stone, &ctx.usable, settings.allowance_mm);
        }
    }
}

#[test]
#[ignore = "slow rough-planner test (over 60 s); run with --ignored"]
fn shaped_plans_keep_stones_and_pieces_in_step_and_poses_right_handed() {
    let designs = random_designs(&mut Lcg(11), 5);
    let cases = [
        (
            RoughModel::new(
                RoughBase::Block {
                    x_mm: 18.0,
                    y_mm: 14.0,
                    z_mm: 12.0,
                },
                vec![RoughCut::Edge {
                    faces: [BoxFace::Top, BoxFace::Front],
                    setbacks_mm: [3.0, 3.0],
                }],
            ),
            2,
        ),
        (
            RoughModel::new(
                RoughBase::Cylinder {
                    diameter_mm: 10.0,
                    length_mm: 18.0,
                    axis: Axis::Y,
                },
                vec![],
            ),
            2,
        ),
    ];
    for (model, count) in &cases {
        let settings = settings_with(*count, 0.3, 0.2, 0.8);
        let hulls = Vec::new();
        let input = PlanInput {
            model,
            settings: &settings,
            designs: &designs,
            hulls: &hulls,
        };
        let results = plan(&input, &mut |_| true).expect("plan");
        assert_ne!(results.len(), 0);
        assert!(results.len() <= 10);
        let region = model
            .usable_halfspaces(settings.skin_mm + settings.allowance_mm)
            .expect("region");
        for layout in &results {
            assert_eq!(layout.stones.len(), piece_total(layout));
            assert_carats(layout, &settings);
            for stone in &layout.stones {
                assert_pose_right_handed(stone.pose.axes);
                assert_stone_inside(stone, &region, settings.allowance_mm);
            }
        }
        let mut compositions: Vec<_> = results.iter().map(RoughLayout::composition).collect();
        compositions.sort();
        compositions.dedup();
        assert_eq!(compositions.len(), results.len());
        for pair in results.windows(2) {
            assert!(pair[0].total_volume_mm3 >= pair[1].total_volume_mm3 * (1.0 - 1e-9));
        }
    }
}

#[test]
fn the_grid_and_the_pieces_follow_the_bounding_box_of_the_cut_solid() {
    let settings = settings_with(2, 0.3, 0.2, 0.5);
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 30.0,
            y_mm: 10.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Face {
            normal: [-1.0, 0.0, 0.0],
            depth_mm: 12.0,
        }],
    );
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    assert!((ctx.bbox_min[0] - 12.0).abs() < 1e-9);
    assert!((ctx.bbox_extents[0] - 18.0).abs() < 1e-9);
    assert!(ctx.bbox_min[1].abs() < 1e-9 && ctx.bbox_min[2].abs() < 1e-9);
    assert!((ctx.model_volume - 1800.0).abs() < 1e-6);

    let grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, &settings);
    assert_eq!(grid.origin_mm, ctx.origin_mm);
    assert!((grid.usable_mm[0] - 18.0).abs() < 1e-9);
    // Sized on the solid, the grid resolves the remaining 18 mm more finely
    // than a grid over the 30 mm base box does.
    let base = choose_shaped_grid([30.0, 10.0, 10.0], &settings);
    assert!(grid.unit[0] < base.unit[0]);

    let designs = random_designs(&mut Lcg(8), 4);
    let hulls = Vec::new();
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };
    let results = plan(&input, &mut |_| true).expect("plan");
    assert_ne!(results.len(), 0);
    for layout in &results {
        assert_eq!(layout.stones.len(), piece_total(layout));
        assert_carats(layout, &settings);
        for stone in &layout.stones {
            assert!(stone.piece_origin_mm[0] >= 12.0 - 1e-9);
            assert_stone_inside(stone, &ctx.usable, settings.allowance_mm);
        }
    }
}

#[test]
fn a_pebble_context_lies_inside_its_box() {
    let settings = settings_with(1, 0.3, 0.2, 0.5);
    let model = RoughModel::new(
        RoughBase::Pebble {
            x_mm: 14.0,
            y_mm: 10.0,
            z_mm: 8.0,
        },
        vec![],
    );
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    assert_eq!(
        ctx.non_box.len(),
        ctx.usable.len(),
        "a pebble has no box planes"
    );
    for (i, base) in [14.0, 10.0, 8.0].into_iter().enumerate() {
        assert!(ctx.bbox_min[i] >= -1e-9);
        assert!(ctx.bbox_extents[i] <= base + 1e-9);
    }
    let box_volume = 14.0 * 10.0 * 8.0;
    assert!(ctx.model_volume > 0.0 && ctx.model_volume < box_volume);
}

#[test]
fn the_dp_ticks_inside_the_bar_and_slab_fills_and_can_be_cancelled_there() {
    let designs = random_designs(&mut Lcg(2), 3);
    let front = pareto_front(&designs);
    let settings = settings_with(2, 0.3, 0.2, 0.5);
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 12.0,
            y_mm: 9.0,
            z_mm: 8.0,
        },
        vec![],
    );
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, &settings);
    let table = clipped_for(&ctx, &grid, &front, &settings);
    let ga = grid.cells[0];
    let run = |on_progress: &mut dyn FnMut(PlanProgress) -> bool| {
        plan_shaped_for_order(
            &grid,
            &table,
            &ctx,
            &front,
            CutOrder::Xyz,
            &settings,
            on_progress,
        )
    };

    // One tick per starting plane in the bar fill, again in the slab fill, and
    // one per plane in the root fill.
    let mut ticks = 0;
    let done = run(&mut |event| {
        if matches!(event, PlanProgress::Dp { .. }) {
            ticks += 1;
        }
        true
    });
    assert!(done.is_some());
    assert_eq!(ticks, 3 * ga);

    let mut seen = 0;
    assert!(
        run(&mut |_| {
            seen += 1;
            false
        })
        .is_none()
    );
    assert_eq!(seen, 1);

    // Cancelling on the first tick after the bar fill stops inside the slab fill.
    let mut seen = 0;
    assert!(
        run(&mut |_| {
            seen += 1;
            seen <= ga
        })
        .is_none()
    );
    assert_eq!(seen, ga + 1);
}

#[test]
fn an_exact_fit_layout_uses_the_stone_bounding_box_as_its_piece() {
    // A unit cube turned by the 3-4-5 rotation about z, at 2 mm per unit.
    let corners: Vec<[f64; 3]> = (0..8)
        .map(|i| {
            let sign = |bit: usize| if i & bit == 0 { -0.5 } else { 0.5 };
            [sign(1), sign(2), sign(4)]
        })
        .collect();
    let hull = DesignHull {
        entry_id: 7,
        vertices: corners,
        volume: 1.0,
        width: 1.0,
    };
    let pose = StonePose {
        center_mm: [5.0, 4.0, 3.0],
        axes: [[0.6, 0.8, 0.0], [-0.8, 0.6, 0.0], [0.0, 0.0, 1.0]],
        mm_per_unit: 2.0,
    };
    assert_pose_right_handed(pose.axes);
    // Eight cubic mm: 2 mm per unit on a unit cube, so the carat figure is the one the
    // fitter would report, 8 * sg / 200.
    let settings = settings_with(1, 0.3, 0.2, 0.5);
    let fit = SingleFit {
        entry_id: 7,
        pose,
        volume_mm3: 8.0,
        carat: carat_weight(8.0, settings.specific_gravity),
    };
    let layout = layout_from_single_fit(&fit, &hull, 100.0);
    assert_carats(&layout, &settings);
    assert!((layout.total_carat - 8.0 * 2.65 / 200.0).abs() < 1e-12);

    let stone = &layout.stones[0];
    let expected_size = [2.8, 2.8, 2.0];
    for (i, &expected) in expected_size.iter().enumerate() {
        assert!((stone.stone_size_mm[i] - expected).abs() < 1e-9);
        assert!((stone.piece_size_mm[i] - expected).abs() < 1e-9);
    }
    // The piece is the stone's bounding box, not the origin.
    let expected_origin = [3.6, 2.6, 2.0];
    for (&origin, &expected) in stone.piece_origin_mm.iter().zip(&expected_origin) {
        assert!((origin - expected).abs() < 1e-9);
    }
    assert_eq!(layout.stones.len(), piece_total(&layout));
    assert!((layout.cut_plan.slabs[0].thickness_mm - 2.8).abs() < 1e-9);
    assert!((layout.cut_plan.slabs[0].bars[0].pieces_mm[0] - 2.0).abs() < 1e-9);
    assert!((layout.yield_fraction - 0.08).abs() < 1e-12);
    assert_pose_right_handed(stone.pose.axes);
}

#[test]
fn the_plain_path_is_plan_rough_when_there_are_no_hulls() {
    let designs = random_designs(&mut Lcg(3), 6);
    let settings = settings_with(4, 0.3, 0.2, 0.5);
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 12.0,
            y_mm: 10.0,
            z_mm: 8.0,
        },
        Vec::new(),
    );
    let rough = RoughBlock {
        x_mm: 12.0,
        y_mm: 10.0,
        z_mm: 8.0,
    };
    let hulls = Vec::new();
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };

    let mut planned_events = Vec::new();
    let planned = plan(&input, &mut |event| {
        planned_events.push(event);
        true
    })
    .expect("plan");
    let mut legacy_events = Vec::new();
    let legacy = plan_rough(&rough, &settings, &designs, &mut |event| {
        legacy_events.push(event);
        true
    })
    .expect("plan_rough");

    assert_eq!(planned, legacy);
    assert_eq!(planned_events, legacy_events);
    for layout in &planned {
        assert_carats(layout, &settings);
    }
}

#[test]
fn the_plain_path_merges_exact_single_fits() {
    let designs = vec![box_design(1)];
    let settings = settings_with(1, 0.3, 0.2, 0.5);
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 12.0,
            y_mm: 10.0,
            z_mm: 8.0,
        },
        Vec::new(),
    );
    let rough = RoughBlock {
        x_mm: 12.0,
        y_mm: 10.0,
        z_mm: 8.0,
    };
    let hulls = vec![DesignHull {
        entry_id: 1,
        vertices: (0..8)
            .map(|i| {
                let sign = |bit: usize| if i & bit == 0 { -0.5 } else { 0.5 };
                [sign(1), sign(2), sign(4)]
            })
            .collect(),
        volume: 1.0,
        width: 1.0,
    }];
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };

    let mut fit_events = 0;
    let results = plan(&input, &mut |event| {
        if matches!(event, PlanProgress::Fit { .. }) {
            fit_events += 1;
        }
        true
    })
    .expect("plan");
    assert!(fit_events > 0, "the single-stone fit must run");
    assert!(!results.is_empty() && results.len() <= 10);

    let legacy = plan_rough(&rough, &settings, &designs, &mut |_| true).expect("plan_rough");
    assert!(results[0].total_volume_mm3 >= legacy[0].total_volume_mm3 - 1e-9);
    for layout in &results {
        assert_eq!(layout.stones.len(), piece_total(layout));
        assert_carats(layout, &settings);
        for stone in &layout.stones {
            assert_pose_right_handed(stone.pose.axes);
        }
    }
}

#[test]
fn an_invalid_model_is_an_empty_result_and_not_a_cancel() {
    let designs = random_designs(&mut Lcg(4), 3);
    let settings = settings_with(2, 0.3, 0.2, 0.5);
    let hulls = Vec::new();
    let models = [
        RoughModel::new(
            RoughBase::Block {
                x_mm: -1.0,
                y_mm: 5.0,
                z_mm: 5.0,
            },
            Vec::new(),
        ),
        RoughModel::new(
            RoughBase::Cylinder {
                diameter_mm: -3.0,
                length_mm: 5.0,
                axis: Axis::Z,
            },
            Vec::new(),
        ),
    ];
    for model in &models {
        let input = PlanInput {
            model,
            settings: &settings,
            designs: &designs,
            hulls: &hulls,
        };
        assert_eq!(plan(&input, &mut |_| true), Some(Vec::new()));
    }
}

#[test]
fn the_pose_maps_the_design_box_onto_the_stone_size_for_every_assignment() {
    let settings = settings_with(1, 0.3, 0.2, 0.5);
    let piece = [12.0, 9.0, 15.0];
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: piece[0],
            y_mm: piece[1],
            z_mm: piece[2],
        },
        Vec::new(),
    );
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    // A design with three different dimensions, so a swapped axis shows.
    let pool = [CandidateDesign {
        entry_id: 5,
        width: 2.0,
        length: 3.0,
        height: 1.5,
        volume: 6.0,
    }];
    // The hull frame: x is the width, y the height, z the length.
    let half = [1.0, 0.75, 1.5];
    let mut fitter = StoneFitter::new(&settings, &pool, &ctx.non_box);
    let mut sizes = Vec::new();
    for orient in 0..ASSIGNMENTS.len() {
        let leaf = Leaf {
            len: piece[2],
            design: 0,
            orient,
        };
        let stone = fitter.fit([0.0; 3], piece, &leaf).expect("the stone fits");
        assert_pose_right_handed(stone.pose.axes);

        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for corner in 0..8 {
            let sign = |bit: usize| if corner & bit == 0 { -1.0 } else { 1.0 };
            let v = [sign(1) * half[0], sign(2) * half[1], sign(4) * half[2]];
            for k in 0..3 {
                let offset = (0..3).map(|j| v[j] * stone.pose.axes[j][k]).sum::<f64>();
                let p = stone
                    .pose
                    .mm_per_unit
                    .mul_add(offset, stone.pose.center_mm[k]);
                lo[k] = lo[k].min(p);
                hi[k] = hi[k].max(p);
            }
        }
        for k in 0..3 {
            assert!(
                (hi[k] - lo[k] - stone.stone_size_mm[k]).abs() < 1e-9,
                "assignment {orient}, axis {k}: posed box {} vs stone size {}",
                hi[k] - lo[k],
                stone.stone_size_mm[k]
            );
            assert!(
                (hi[k].midpoint(lo[k]) - stone.pose.center_mm[k]).abs() < 1e-9,
                "assignment {orient}, axis {k}: posed box is off its centre"
            );
        }
        sizes.push(stone.stone_size_mm);
    }
    // The assignments really differ: no two of them give the same box.
    for (i, a) in sizes.iter().enumerate() {
        for b in &sizes[i + 1..] {
            assert!(a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-6));
        }
    }
}

#[test]
fn the_pose_of_a_placed_stone_reads_back_as_its_assignment() {
    for (index, assignment) in ASSIGNMENTS.iter().enumerate() {
        let pose = StonePose {
            center_mm: [0.0; 3],
            axes: assignment_axes(assignment),
            mm_per_unit: 1.0,
        };
        assert_pose_right_handed(pose.axes);
        assert_eq!(orient_of_pose(&pose), Some(index));
    }
    let tilted = StonePose {
        center_mm: [0.0; 3],
        axes: [[0.6, 0.8, 0.0], [-0.8, 0.6, 0.0], [0.0, 0.0, 1.0]],
        mm_per_unit: 1.0,
    };
    assert_eq!(orient_of_pose(&tilted), None);
}
