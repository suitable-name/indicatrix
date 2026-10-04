use glam::DVec3;

use super::{
    clip::{
        BuildClipParams, CLASS_EXTERIOR, CLASS_INTERIOR, CLASS_PARTIAL, ClippedTable,
        build_clipped_table, build_clipped_table_lanes, classify_box, slice_entry_range,
    },
    ctx::ShapedCtx,
    dp::plan_shaped_for_order,
    grid::{
        SHAPED_OP_CAP, SHAPED_PIECE_CAP, ShapedGrid, choose_shaped_grid, choose_shaped_grid_at,
        piece_count, shaped_op_estimate,
    },
    refine::refine_shaped,
};
use crate::{
    rough_plan::{
        Axis, BoxFace, CandidateDesign, CutOrder, Grid, PlacedStone, PlanInput, PlanSettings,
        RoughBase, RoughBlock, RoughCut, RoughLayout, RoughModel, build_piece_table, pareto_front,
        plan, plan_rough_for_order,
        tests::{Lcg, close, random_designs, settings_with},
    },
    yield_metrics::carat_weight,
};

/// Asserts that every carat figure of `layout` follows from its volumes at the
/// specific gravity of `settings`: each stone's carat is `carat_weight` of its volume,
/// and the layout's totals are the stone sums (volume and carat) to within rounding.
pub(super) fn assert_carats(layout: &RoughLayout, settings: &PlanSettings) {
    let sg = settings.specific_gravity;
    let (mut volume, mut carat) = (0.0, 0.0);
    for stone in &layout.stones {
        assert!(
            close(stone.carat, carat_weight(stone.volume_mm3, sg), 1e-12),
            "stone {}: carat {} vs {} from its volume",
            stone.entry_id,
            stone.carat,
            carat_weight(stone.volume_mm3, sg)
        );
        volume += stone.volume_mm3;
        carat += stone.carat;
    }
    assert!(
        close(layout.total_volume_mm3, volume, 1e-12),
        "total volume {} vs stone sum {volume}",
        layout.total_volume_mm3
    );
    assert!(
        close(layout.total_carat, carat, 1e-12),
        "total carat {} vs stone sum {carat}",
        layout.total_carat
    );
    assert!(
        close(
            layout.total_carat,
            carat_weight(layout.total_volume_mm3, sg),
            1e-9
        ),
        "total carat {} vs {} from the total volume",
        layout.total_carat,
        carat_weight(layout.total_volume_mm3, sg)
    );
}

/// The number of pieces in the cut plan of `layout`.
pub(super) fn piece_total(layout: &RoughLayout) -> usize {
    layout
        .cut_plan
        .slabs
        .iter()
        .flat_map(|slab| &slab.bars)
        .map(|bar| bar.pieces_mm.len())
        .sum()
}

/// Asserts that the box of `stone` (its centre plus and minus half its size, in
/// rough axes) lies inside every halfspace of `region` and inside its piece box
/// shrunk by `allowance` on every side.
pub(super) fn assert_stone_inside(stone: &PlacedStone, region: &[(DVec3, f64)], allowance: f64) {
    let centre = DVec3::from(stone.pose.center_mm);
    let half = DVec3::from(stone.stone_size_mm) * 0.5;
    for i in 0..3 {
        let lo = stone.piece_origin_mm[i] + allowance;
        let hi = stone.piece_origin_mm[i] + stone.piece_size_mm[i] - allowance;
        assert!(
            centre[i] - half[i] >= lo - 1e-6 && centre[i] + half[i] <= hi + 1e-6,
            "stone box leaves its piece on axis {i}: [{}, {}] vs [{lo}, {hi}]",
            centre[i] - half[i],
            centre[i] + half[i]
        );
    }
    for &(n, m) in region {
        for corner in 0..8 {
            let sign = |bit: usize| if corner & bit == 0 { -1.0 } else { 1.0 };
            let p = centre + DVec3::new(sign(1), sign(2), sign(4)) * half;
            assert!(
                n.dot(p) <= m + 1e-6,
                "stone corner {p:?} violates region halfspace n={n:?}, m={m}"
            );
        }
    }
}

/// Asserts that `axes` are orthonormal with `det = +1`.
pub(super) fn assert_pose_right_handed(axes: [[f64; 3]; 3]) {
    let [a0, a1, a2] = axes.map(DVec3::from);
    for (i, a) in [a0, a1, a2].iter().enumerate() {
        assert!((a.length_squared() - 1.0).abs() < 1e-9, "axis {i} not unit");
    }
    assert!(a0.dot(a1).abs() < 1e-9 && a0.dot(a2).abs() < 1e-9 && a1.dot(a2).abs() < 1e-9);
    let det = a0.dot(a1.cross(a2));
    assert!((det - 1.0).abs() < 1e-9, "det(axes) != +1: {det}");
}

/// All ranges `(start, end)` with `0 <= start < end <= g`, in table order.
pub(super) fn all_ranges(g: usize) -> Vec<(usize, usize)> {
    (0..g)
        .flat_map(|start| ((start + 1)..=g).map(move |end| (start, end)))
        .collect()
}

/// The block with a large corner cut that the region tests share.
pub(super) fn corner_cut_model() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 20.0,
            y_mm: 20.0,
            z_mm: 20.0,
        },
        vec![RoughCut::Corner {
            faces: [BoxFace::Top, BoxFace::Front, BoxFace::Right],
            setbacks_mm: [12.0, 12.0, 12.0],
        }],
    )
}

#[test]
fn plain_block_equivalence_over_multiple_roughs_and_stone_counts() {
    let mut rng = Lcg(12345);
    let designs = random_designs(&mut rng, 8);
    let roughs = [
        [10.0, 10.0, 10.0],
        [12.0, 9.0, 8.0],
        [20.0, 15.0, 10.0],
        [14.0, 10.0, 7.0],
        [16.0, 12.0, 10.0],
    ];

    let mut exact_grid_cases = 0;
    for ext in roughs {
        for &k in &[1u8, 4, 12] {
            let settings = settings_with(k, 0.3, 0.2, 0.5);
            let rough_block = RoughBlock {
                x_mm: ext[0],
                y_mm: ext[1],
                z_mm: ext[2],
            };
            let model = RoughModel::new(
                RoughBase::Block {
                    x_mm: ext[0],
                    y_mm: ext[1],
                    z_mm: ext[2],
                },
                Vec::new(),
            );

            let clean = crate::rough_plan::pareto::sanitize(&designs);
            let front = pareto_front(&clean);

            // The plain planner's best mixed DP value on the cells the shaped grid uses
            // (its per-axis cap is lower than the plain grid's, so a larger rough would
            // otherwise compare two different resolutions).
            let shaped_grid = choose_shaped_grid(ext, &settings);
            let grid = Grid::with_cells(&rough_block, &settings, shaped_grid.cells);
            let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
            let mut existing_best = f64::NEG_INFINITY;
            for order in CutOrder::ALL {
                let layouts = plan_rough_for_order(&grid, &table, &front, order, k, &mut |_| true)
                    .expect("order dp");
                for l in layouts {
                    existing_best = existing_best.max(l.total_volume_mm3);
                }
            }

            // Shaped planner
            let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
            assert!(
                ctx.non_box.is_empty(),
                "uncut block must have no non-box planes"
            );
            let size_table = shaped_grid
                .size_table(&settings, &front, &mut |_| true)
                .expect("table");

            let clip_params = BuildClipParams {
                grid: &shaped_grid,
                front: &front,
                non_box_planes: &ctx.non_box,
                mesh: None,
                size_table: &size_table,
                settings: &settings,
                slice: 0..shaped_grid.cells[0],
                cached_classes: None,
            };
            let clipped = build_clipped_table(&clip_params, &mut |_| true).expect("clipped");

            assert!(
                clipped.classes.iter().all(|&c| c == CLASS_INTERIOR),
                "every piece in uncut block must classify as interior"
            );

            let mut shaped_best = f64::NEG_INFINITY;
            for order in CutOrder::ALL {
                let layouts = plan_shaped_for_order(
                    &shaped_grid,
                    &clipped,
                    &ctx,
                    &front,
                    order,
                    &settings,
                    &mut |_| true,
                )
                .expect("shaped dp");
                for l in layouts {
                    shaped_best = shaped_best.max(l.total_volume_mm3);
                }
            }

            assert!(
                existing_best > 0.0,
                "the plain planner found no layout for ext={ext:?}, k={k}"
            );
            assert!(
                shaped_best >= 0.97 * existing_best,
                "shaped {shaped_best} was lower than 97% of existing {existing_best} for ext={ext:?}, k={k}"
            );
            // On the same grid both planners search the same cuts over the same
            // interior values, so they agree. A coarser grid that is not a subset of
            // the finer one can place cuts the finer one cannot, so the comparison
            // has no such guarantee there.
            if shaped_grid.cells == grid.cells() {
                exact_grid_cases += 1;
                assert!(
                    close(shaped_best, existing_best, 1e-9),
                    "shaped {shaped_best} differs from existing {existing_best} on the same grid for ext={ext:?}, k={k}"
                );
            }
        }
    }
    assert!(
        exact_grid_cases > 0,
        "no rough and count put both planners on one grid"
    );
}

#[test]
fn exterior_pieces_never_hold_stones() {
    let mut rng = Lcg(777);
    let designs = random_designs(&mut rng, 5);
    let settings = settings_with(6, 0.3, 0.2, 0.5);
    let model = corner_cut_model();

    let hulls = Vec::new();
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };

    let results = plan(&input, &mut |_| true).expect("plan");
    assert_ne!(results.len(), 0);

    let region_halfspaces = model
        .usable_halfspaces(settings.skin_mm + settings.allowance_mm)
        .expect("usable halfspaces");

    for layout in &results {
        assert_eq!(layout.stones.len(), piece_total(layout));
        assert_carats(layout, &settings);
        for stone in &layout.stones {
            assert!(stone.volume_mm3 > 0.0);
            assert_stone_inside(stone, &region_halfspaces, settings.allowance_mm);
        }
    }
}

/// The clipped table of the corner-cut block for `settings` and four random designs.
struct CornerCutFixture {
    ctx: ShapedCtx,
    grid: ShapedGrid,
    front: Vec<CandidateDesign>,
    table: ClippedTable,
}

fn corner_cut_fixture(settings: &crate::rough_plan::PlanSettings) -> CornerCutFixture {
    let mut rng = Lcg(31);
    let designs = random_designs(&mut rng, 4);
    let model = corner_cut_model();
    let front = pareto_front(&designs);
    let ctx = ShapedCtx::new(&model, settings).expect("ctx");
    let grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, settings);
    let size_table = grid
        .size_table(settings, &front, &mut |_| true)
        .expect("size table");
    let params = BuildClipParams {
        grid: &grid,
        front: &front,
        non_box_planes: &ctx.non_box,
        mesh: None,
        size_table: &size_table,
        settings,
        slice: 0..grid.cells[0],
        cached_classes: None,
    };
    let table = build_clipped_table(&params, &mut |_| true).expect("clipped table");
    CornerCutFixture {
        ctx,
        grid,
        front,
        table,
    }
}

#[test]
fn table_classes_match_direct_classification_and_exterior_pieces_are_worthless() {
    let settings = settings_with(1, 0.3, 0.2, 0.5);
    let fx = corner_cut_fixture(&settings);
    let [ga, gb, gc] = fx.grid.cells;
    let mut entry = 0;
    let (mut exterior, mut partial) = (0usize, 0usize);
    for ra in all_ranges(ga) {
        for rb in all_ranges(gb) {
            for rc in all_ranges(gc) {
                let (origin, size) = fx.grid.piece_box([ra, rb, rc]);
                let (lo, hi) = ShapedGrid::stone_box(origin, size, settings.allowance_mm);
                let (class, _) = classify_box(lo, hi, &fx.ctx.non_box);
                assert_eq!(fx.table.classes[entry], class, "entry {entry}");
                match class {
                    CLASS_EXTERIOR => {
                        exterior += 1;
                        assert_eq!(fx.table.values[entry], f64::NEG_INFINITY);
                    }
                    CLASS_PARTIAL => partial += 1,
                    _ => {}
                }
                if fx.table.values[entry] > f64::NEG_INFINITY {
                    assert!((fx.table.design[entry] as usize) < fx.front.len());
                }
                entry += 1;
            }
        }
    }
    assert_eq!(entry, fx.table.values.len());
    assert!(exterior > 0, "the corner cut must leave exterior pieces");
    assert!(partial > 0, "the corner cut must leave partial pieces");
}

/// Builds the table of `slice` of the cylinder fixture.
fn build_slice(
    fx: &CylinderFixture,
    settings: &crate::rough_plan::PlanSettings,
    slice: std::ops::Range<usize>,
    cached: Option<&[u8]>,
) -> ClippedTable {
    let params = BuildClipParams {
        grid: &fx.grid,
        front: &fx.front,
        non_box_planes: &fx.ctx.non_box,
        mesh: None,
        size_table: &fx.size_table,
        settings,
        slice,
        cached_classes: cached,
    };
    build_clipped_table(&params, &mut |_| true).expect("slice")
}

struct CylinderFixture {
    ctx: ShapedCtx,
    grid: ShapedGrid,
    front: Vec<CandidateDesign>,
    size_table: crate::rough_plan::PieceTable,
}

#[test]
fn slicing_invariance_for_clipped_piece_table() {
    let mut rng = Lcg(99);
    let designs = random_designs(&mut rng, 4);
    let settings = settings_with(1, 0.3, 0.2, 0.5);
    let model = RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 15.0,
            length_mm: 25.0,
            axis: Axis::Z,
        },
        vec![],
    );
    let front = pareto_front(&designs);
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, &settings);
    let size_table = grid
        .size_table(&settings, &front, &mut |_| true)
        .expect("table");
    let fx = CylinderFixture {
        ctx,
        grid,
        front,
        size_table,
    };
    let ga = fx.grid.cells[0];
    assert!(ga >= 4, "the fixture needs a few planes to slice");

    let full = build_slice(&fx, &settings, 0..ga, None);
    assert!(full.classes.contains(&CLASS_PARTIAL));

    let slicings = [vec![0, 1, ga], vec![0, ga / 3, 2 * ga / 3, ga]];
    for cuts in &slicings {
        let slices: Vec<_> = cuts.windows(2).map(|w| w[0]..w[1]).collect();
        let plain: Vec<ClippedTable> = slices
            .iter()
            .map(|s| build_slice(&fx, &settings, s.clone(), None))
            .collect();
        assert_eq!(
            ClippedTable::concat(full.cells, plain),
            full,
            "slicing {cuts:?}"
        );

        // The cached path: the classes of the full table, cut per slice.
        let cached: Vec<ClippedTable> = slices
            .iter()
            .map(|s| {
                let range = slice_entry_range(&fx.grid, s);
                build_slice(&fx, &settings, s.clone(), Some(&full.classes[range]))
            })
            .collect();
        assert_eq!(
            ClippedTable::concat(full.cells, cached),
            full,
            "cached slicing {cuts:?}"
        );
    }

    // The threaded build is the same table whatever the lane count, with and without
    // the cached classes (a lane cuts its own part out of them).
    for lanes in [1, 2, 5] {
        for cached in [None, Some(&full.classes[..])] {
            let params = BuildClipParams {
                grid: &fx.grid,
                front: &fx.front,
                non_box_planes: &fx.ctx.non_box,
                mesh: None,
                size_table: &fx.size_table,
                settings: &settings,
                slice: 0..ga,
                cached_classes: cached,
            };
            let table = build_clipped_table_lanes(&params, lanes, &mut |_| true).expect("lanes");
            assert_eq!(table, full, "lanes {lanes}, cached {}", cached.is_some());
        }
    }
}

#[test]
fn slice_entry_ranges_tile_the_full_table() {
    let settings = settings_with(1, 0.3, 0.2, 0.5);
    let fx = corner_cut_fixture(&settings);
    let ga = fx.grid.cells[0];
    let full = slice_entry_range(&fx.grid, &(0..ga));
    assert_eq!(full, 0..fx.table.values.len());
    let mut next = 0;
    for a0 in 0..ga {
        let range = slice_entry_range(&fx.grid, &(a0..a0 + 1));
        assert_eq!(range.start, next);
        next = range.end;
    }
    assert_eq!(next, fx.table.values.len());
}

#[test]
fn op_and_piece_caps_hold_for_k99() {
    let settings = settings_with(99, 0.3, 0.2, 0.5);

    let cases = [
        ("plate 100x100x2", [100.0, 100.0, 2.0]),
        ("cube 40mm", [40.0, 40.0, 40.0]),
        ("cylinder D30x60", [30.0, 60.0, 30.0]),
        ("cube 20mm", [20.0, 20.0, 20.0]),
    ];

    for (name, ext) in cases {
        let grid = choose_shaped_grid(ext, &settings);
        let pieces = piece_count(grid.cells);
        let ops = shaped_op_estimate(grid.cells, 99);

        // The documented resolution: K = 99 on a 20 mm cube starts at 20 cells per
        // axis, and the 5 % shrink steps 20, 19, 18, 17, 16 stop at 16 because
        // 136^3 = 2,515,456 entries fit the cap while 153^3 = 3,581,577 (17 cells)
        // do not.
        if name == "cube 20mm" {
            assert_eq!(grid.cells, [16, 16, 16]);
            assert_eq!(pieces, 136 * 136 * 136);
        }

        assert!(
            pieces <= SHAPED_PIECE_CAP,
            "{name}: pieces {pieces} > {SHAPED_PIECE_CAP}"
        );
        assert!(ops <= SHAPED_OP_CAP, "{name}: ops {ops} > {SHAPED_OP_CAP}");
    }
}

#[test]
fn refinement_never_worse_and_all_stones_inside_region() {
    let mut rng = Lcg(456);
    let designs = random_designs(&mut rng, 6);
    let settings = settings_with(4, 0.3, 0.2, 0.5);

    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 18.0,
            y_mm: 14.0,
            z_mm: 12.0,
        },
        vec![
            RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [3.0, 3.0],
            },
            RoughCut::Corner {
                faces: [BoxFace::Bottom, BoxFace::Back, BoxFace::Left],
                setbacks_mm: [2.0, 2.0, 2.0],
            },
        ],
    );

    let clean = crate::rough_plan::pareto::sanitize(&designs);
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let shaped_grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, &settings);
    let size_table = shaped_grid
        .size_table(&settings, &clean, &mut |_| true)
        .expect("table");

    let clip_params = BuildClipParams {
        grid: &shaped_grid,
        front: &clean,
        non_box_planes: &ctx.non_box,
        mesh: None,
        size_table: &size_table,
        settings: &settings,
        slice: 0..shaped_grid.cells[0],
        cached_classes: None,
    };
    let clipped = build_clipped_table(&clip_params, &mut |_| true).expect("clipped");

    let layouts = plan_shaped_for_order(
        &shaped_grid,
        &clipped,
        &ctx,
        &clean,
        CutOrder::Xyz,
        &settings,
        &mut |_| true,
    )
    .expect("dp");
    assert_ne!(layouts.len(), 0);

    let region = model
        .usable_halfspaces(settings.skin_mm + settings.allowance_mm)
        .expect("region");

    for layout in layouts {
        assert_eq!(layout.stones.len(), piece_total(&layout));
        let refined = refine_shaped(&ctx, &layout, &clean, &settings);
        assert!(
            refined.total_volume_mm3 >= layout.total_volume_mm3 - 1e-9,
            "refined volume {} was worse than unrefined {}",
            refined.total_volume_mm3,
            layout.total_volume_mm3
        );
        assert_eq!(refined.stones.len(), piece_total(&refined));
        assert_carats(&layout, &settings);
        assert_carats(&refined, &settings);

        for stone in &refined.stones {
            assert_stone_inside(stone, &region, settings.allowance_mm);
        }
    }
}

#[test]
fn yield_fraction_uses_model_volume() {
    let mut rng = Lcg(101);
    let designs = random_designs(&mut rng, 4);
    let settings = settings_with(2, 0.3, 0.2, 0.5);

    let model = RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 12.0,
            length_mm: 20.0,
            axis: Axis::Y,
        },
        vec![],
    );

    let hulls = Vec::new();
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };

    let results = plan(&input, &mut |_| true).expect("plan");
    let model_vol = model.measure().expect("measure").volume_mm3;
    assert!(model_vol > 0.0);

    for layout in results {
        assert_carats(&layout, &settings);
        let expected_yield = layout.total_volume_mm3 / model_vol;
        assert!(
            (layout.yield_fraction - expected_yield).abs() < 1e-9,
            "yield fraction {} != expected {}",
            layout.yield_fraction,
            expected_yield
        );
    }
}
