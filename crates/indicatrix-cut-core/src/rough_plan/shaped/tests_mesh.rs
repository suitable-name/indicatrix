//! Plans of a non-convex rough: a C-shaped mesh against its own convex hull.
//!
//! The fixture is a 20 mm cube with a 10 x 10 x 20 mm notch cut into one face (see
//! [`C_SHAPE_OBJ`]): 6000 mm^3 of material inside an 8000 mm^3 hull. A planner that sees
//! only the hull fills the notch with stone; the mesh must keep it out.

use glam::DVec3;

use super::{
    clip::{
        BuildClipParams, CLASS_EXTERIOR, CLASS_INTERIOR, CLASS_PARTIAL, ClippedTable,
        build_clipped_table, classify_box_in,
    },
    ctx::ShapedCtx,
    dp::plan_shaped_for_order,
    grid::ShapedGrid,
    tests_dp::grid_with_cells,
    tree::{StoneFitter, layout_from_tree_shaped},
};
use crate::rough_plan::{
    CandidateDesign, CutOrder, DesignHull, PlanInput, PlanSettings, RoughCut, RoughLayout,
    RoughModel,
    fit::fit_single_stones_with,
    import_hull, import_mesh, pareto_front, plan,
    shape::{
        BoxState,
        hull::add_inclusion_points,
        mesh_fixture::{
            C_SHAPE_OBJ, CUBE_OBJ, box_hull, import_obj, import_parts, noisy_c_shape, parse_obj,
            pebble_scan, stone_enters_box, stone_enters_notch,
        },
    },
    tests::{box_design, settings_with},
    tree::{Bar, Leaf, Slab, Tree},
};

fn designs() -> Vec<CandidateDesign> {
    vec![
        box_design(1),
        CandidateDesign {
            entry_id: 2,
            width: 1.0,
            length: 1.6,
            height: 0.6,
            volume: 0.5,
        },
    ]
}

fn hulls() -> Vec<DesignHull> {
    vec![box_hull(1, 1.0, 1.0, 1.0), box_hull(2, 1.0, 0.6, 1.6)]
}

/// The C-shaped rough with its mesh.
fn c_model() -> RoughModel {
    RoughModel::new(import_obj(C_SHAPE_OBJ), Vec::new())
}

/// The same points as a plain convex hull: the control.
fn hull_model() -> RoughModel {
    let base = import_hull(&parse_obj(C_SHAPE_OBJ).0).expect("a hull");
    RoughModel::new(base, Vec::new())
}

fn run(model: &RoughModel) -> Vec<RoughLayout> {
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let designs = designs();
    let hulls = hulls();
    let input = PlanInput {
        model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };
    plan(&input, &mut |_| true).expect("not cancelled")
}

fn stones_in_notch(layouts: &[RoughLayout]) -> usize {
    let hulls = hulls();
    layouts
        .iter()
        .flat_map(|layout| layout.stones.iter().map(move |stone| (layout, stone)))
        .filter(|&(layout, stone)| stone_enters_notch(layout, stone, &hulls))
        .count()
}

#[test]
fn hull_only_plan_puts_a_stone_in_the_notch() {
    // The control: the hull is the whole cube, so the best plans fill the notch. If this
    // failed the next test would prove nothing.
    let layouts = run(&hull_model());
    assert_ne!(layouts, [] as [RoughLayout; 0]);
    assert!(
        stones_in_notch(&layouts) > 0,
        "the hull-only plan never uses the notch"
    );
}

#[test]
fn c_shape_plan_has_no_stone_in_the_notch() {
    let model = c_model();
    assert!(model.mesh().is_some(), "the C is not its own hull");
    let layouts = run(&model);
    assert!(!layouts.is_empty(), "the arms hold stones");
    assert!(layouts.iter().any(|layout| layout.stone_count() > 0));
    assert_eq!(
        stones_in_notch(&layouts),
        0,
        "a stone reaches into the notch"
    );
    // Smaller than what the hull allows.
    let best = |layouts: &[RoughLayout]| {
        layouts
            .iter()
            .map(|l| l.total_volume_mm3)
            .fold(0.0, f64::max)
    };
    assert!(best(&layouts) < best(&run(&hull_model())));
}

#[test]
fn yield_uses_mesh_volume() {
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(&c_model(), &settings).expect("ctx");
    assert!(
        (ctx.model_volume - 6000.0).abs() < 1e-6,
        "{}",
        ctx.model_volume
    );
    let hull_ctx = ShapedCtx::new(&hull_model(), &settings).expect("ctx");
    assert!((hull_ctx.model_volume - 8000.0).abs() < 1e-6);
    for layout in run(&c_model()) {
        let expected = layout.total_volume_mm3 / 6000.0;
        assert!(
            (layout.yield_fraction - expected).abs() < 1e-9,
            "yield {} vs {expected}",
            layout.yield_fraction
        );
    }
}

#[test]
fn a_face_cut_gets_the_volume_of_the_mesh_inside_it() {
    // x <= 15 (depth 5 from the face at 20): the left half plus two 5 x 5 x 20 arms.
    let cut = RoughCut::Face {
        normal: [1.0, 0.0, 0.0],
        depth_mm: 5.0,
    };
    let model = RoughModel::new(import_obj(C_SHAPE_OBJ), vec![cut]);
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    assert!(
        (ctx.model_volume - 5000.0).abs() < 1e-6,
        "{}",
        ctx.model_volume
    );
    // The hull's own volume with the same cut would have been 15 x 20 x 20.
    let hull = RoughModel::new(
        import_hull(&parse_obj(C_SHAPE_OBJ).0).expect("hull"),
        model.cuts,
    );
    let hull_volume = ShapedCtx::new(&hull, &settings).expect("ctx").model_volume;
    assert!((hull_volume - 6000.0).abs() < 1e-6, "{hull_volume}");
}

#[test]
fn boxes_are_classified_by_the_mesh() {
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(&c_model(), &settings).expect("ctx");
    let mesh = ctx.fit_mesh();
    let mut violated = Vec::new();
    let mut class =
        |min: [f64; 3], max: [f64; 3]| classify_box_in(min, max, &ctx.non_box, mesh, &mut violated);
    assert_eq!(class([1.0; 3], [8.0; 3]), CLASS_INTERIOR);
    assert_eq!(class([12.0, 7.0, 1.0], [18.0, 13.0, 19.0]), CLASS_EXTERIOR);
    assert_eq!(class([6.0, 6.0, 1.0], [14.0, 12.0, 9.0]), CLASS_PARTIAL);
    // The same box without the mesh sees only the (box) hull planes.
    let mut none = Vec::new();
    assert_eq!(
        classify_box_in(
            [12.0, 7.0, 1.0],
            [18.0, 13.0, 19.0],
            &ctx.non_box,
            None,
            &mut none
        ),
        CLASS_INTERIOR
    );
}

/// Where the inclusion of [`cube_with_inclusion`] is.
const INCLUSION_BOX: ([f64; 3], [f64; 3]) = ([6.0; 3], [14.0; 3]);

/// The 20 mm cube with an 8 mm cube of inclusion in the middle (no margin).
fn cube_with_inclusion() -> RoughModel {
    let (points, _) = parse_obj(CUBE_OBJ);
    let cube = import_hull(&points).expect("a cube");
    let (mut inner, tris) = parse_obj(CUBE_OBJ);
    for p in &mut inner {
        *p = *p * (8.0 / 20.0) + DVec3::splat(6.0);
    }
    let base = add_inclusion_points(&cube, &inner, &tris, 0.0).expect("the inclusion fits");
    RoughModel::new(base, Vec::new())
}

fn stones_in_inclusion(layouts: &[RoughLayout]) -> usize {
    let hulls = hulls();
    layouts
        .iter()
        .flat_map(|layout| layout.stones.iter().map(move |stone| (layout, stone)))
        .filter(|&(layout, stone)| stone_enters_box(layout, stone, &hulls, INCLUSION_BOX))
        .count()
}

#[test]
fn a_plan_keeps_every_stone_out_of_an_inclusion() {
    // The control: the plain cube is filled right through the middle.
    let plain = RoughModel::new(
        import_hull(&parse_obj(CUBE_OBJ).0).expect("a cube"),
        Vec::new(),
    );
    assert!(
        stones_in_inclusion(&run(&plain)) > 0,
        "the plain cube's plan never uses the middle"
    );
    let model = cube_with_inclusion();
    assert!(model.mesh().is_some(), "an inclusion makes it a mesh rough");
    let layouts = run(&model);
    assert!(layouts.iter().any(|layout| layout.stone_count() > 0));
    assert_eq!(
        stones_in_inclusion(&layouts),
        0,
        "a stone reaches into the inclusion"
    );
}

#[test]
fn the_yield_of_a_rough_with_an_inclusion_is_taken_against_the_gross_volume() {
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let model = cube_with_inclusion();
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    // The rough as bought: 8000 mm^3, although the stones can use only 8000 - 512.
    assert!(
        (ctx.model_volume - 8000.0).abs() < 1e-6,
        "{}",
        ctx.model_volume
    );
    let measure = model.measure().expect("measures");
    assert_eq!(measure.volume_mm3.to_bits(), ctx.model_volume.to_bits());
    assert_eq!(measure.inclusion_count, 1);
    assert!((measure.inclusion_mm3 - 512.0).abs() < 1e-6);
    assert!((model.mesh().expect("mesh").volume() - 7488.0).abs() < 1e-6);
    for layout in run(&model) {
        let expected = layout.total_volume_mm3 / 8000.0;
        assert!(
            (layout.yield_fraction - expected).abs() < 1e-9,
            "yield {} vs {expected}",
            layout.yield_fraction
        );
    }
}

#[test]
fn plan_twice_equal() {
    let model = c_model();
    assert_eq!(run(&model), run(&model));
}

#[test]
fn convex_obj_plans_identically_to_import_hull() {
    let (points, tris) = parse_obj(CUBE_OBJ);
    let (base, note) = import_mesh(&points, &tris).expect("imports");
    assert_eq!(note, None);
    assert_eq!(
        Ok(base),
        import_hull(&points),
        "the very same registered base"
    );
    let from_mesh = RoughModel::new(base, Vec::new());
    assert!(from_mesh.mesh().is_none(), "a convex mesh keeps no mesh");
    let from_hull = RoughModel::new(import_hull(&points).expect("hull"), Vec::new());
    assert_eq!(run(&from_mesh), run(&from_hull));
}

#[test]
fn a_stray_vertex_does_not_change_the_rough_across_a_save() {
    let (mut points, tris) = parse_obj(C_SHAPE_OBJ);
    // A `v` line no face names: it must not move the hull, the frame or the origin.
    points.push(DVec3::new(35.0, -7.0, 50.0));
    let (base, note) = import_mesh(&points, &tris).expect("imports");
    assert_eq!(note, None);
    assert_eq!(
        base,
        import_obj(C_SHAPE_OBJ),
        "as if the stray vertex was absent"
    );
    let model = RoughModel::new(base, Vec::new());
    let mesh = model.mesh().expect("the notch keeps the mesh");
    // What a saved plan stores: the welded, referenced vertices and the triangles.
    let (saved_points, saved_tris) = (mesh.vertices().to_vec(), mesh.triangles().to_vec());
    let (again, note) = import_mesh(&saved_points, &saved_tris).expect("reloads");
    assert_eq!(note, None);
    assert_eq!(again, base, "the reload is the very same registered rough");
    let reloaded = RoughModel::new(again, Vec::new());
    assert_eq!(
        model.base.bounding_box_extents(),
        reloaded.base.bounding_box_extents()
    );
    for extent in model.base.bounding_box_extents() {
        assert!((extent - 20.0).abs() < 1e-9, "{extent}");
    }
    let (before, after) = (model.mesh().expect("mesh"), reloaded.mesh().expect("mesh"));
    assert_eq!(before.bounds(), after.bounds());
    assert!(
        before.bounds().0.abs().max_element() < 1e-9,
        "{:?}",
        before.bounds()
    );
    assert_eq!(before.volume(), after.volume());
    assert!((before.volume() - 6000.0).abs() < 1e-9);
    assert_eq!(
        run(&model),
        run(&reloaded),
        "the same stones in the same places"
    );
}

#[test]
fn a_convex_mesh_with_a_stray_vertex_is_the_hull_of_all_its_points() {
    let (mut points, tris) = parse_obj(CUBE_OBJ);
    points.push(DVec3::new(30.0, 5.0, 5.0));
    let (base, note) = import_mesh(&points, &tris).expect("imports");
    assert_eq!(note, None);
    assert_eq!(
        Ok(base),
        import_hull(&points),
        "convex ids stay bit-identical"
    );
    assert!(RoughModel::new(base, Vec::new()).mesh().is_none());
}

#[test]
#[ignore = "slow rough-planner test (over 60 s); run with --ignored"]
fn a_noisy_scanned_notch_still_holds_stones_beside_it_and_none_in_it() {
    let (points, tris) = noisy_c_shape(2, 0.8);
    let model = RoughModel::new(import_parts(&points, &tris), Vec::new());
    assert!(model.mesh().is_some(), "the noisy C is still not convex");
    let layouts = run(&model);
    assert!(!layouts.is_empty(), "the arms hold stones");
    let placed: usize = layouts.iter().map(RoughLayout::stone_count).sum();
    assert!(placed > 0, "no stone was placed beside the notch");
    assert_eq!(
        stones_in_notch(&layouts),
        0,
        "a stone reaches into the notch"
    );
    let best = |layouts: &[RoughLayout]| {
        layouts
            .iter()
            .map(|l| l.total_volume_mm3)
            .fold(0.0, f64::max)
    };
    // The noise costs a little clearance, not the whole arm.
    assert!(
        best(&layouts) > 0.5 * best(&run(&c_model())),
        "{}",
        best(&layouts)
    );
}

/// The clipped table of `ctx`'s model over `grid` for `front`, with the model's mesh.
fn clipped_with_mesh(
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
        mesh: ctx.mesh.as_deref(),
        size_table: &size_table,
        settings,
        slice: 0..grid.cells[0],
        cached_classes: None,
    };
    build_clipped_table(&params, &mut |_| true).expect("clipped table")
}

#[test]
fn a_noisy_scanned_notch_keeps_stones_beside_it_and_none_in_it_on_a_small_grid() {
    // The fast sibling of the ignored full plan above: the same noisy fixture and the same
    // cutting-plane rows (the walls of the notch are a thicket of planes), but a 3 x 3 x 3
    // grid and two stones, which takes a moment where the full plan takes minutes.
    let (points, tris) = noisy_c_shape(2, 0.8);
    let model = RoughModel::new(import_parts(&points, &tris), Vec::new());
    assert!(model.mesh().is_some(), "the noisy C is still not convex");
    let settings = settings_with(2, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let grid = grid_with_cells(&ctx, &settings, [3, 3, 3]);
    let front = pareto_front(&designs());
    let table = clipped_with_mesh(&ctx, &grid, &front, &settings);
    let mesh = ctx.mesh.as_deref().expect("the mesh");
    let hulls = hulls();
    let mut placed = 0;
    for order in CutOrder::ALL {
        let layouts =
            plan_shaped_for_order(&grid, &table, &ctx, &front, order, &settings, &mut |_| true)
                .expect("not cancelled");
        for layout in &layouts {
            for stone in &layout.stones {
                placed += 1;
                assert!(
                    !stone_enters_notch(layout, stone, &hulls),
                    "{order:?}: a stone reaches into the notch"
                );
                // Beside the notch means in the material, whatever the noise did to the
                // walls: no surface triangle meets the stone and its centre is inside.
                let half = DVec3::from(stone.stone_size_mm) * 0.5;
                let centre = DVec3::from(stone.pose.center_mm);
                let state =
                    mesh.box_state((centre - half).to_array(), (centre + half).to_array(), 0.0);
                assert_eq!(
                    state,
                    BoxState::Clear,
                    "{order:?}: a stone leaves the material"
                );
            }
        }
    }
    assert!(placed > 0, "no stone was placed beside the notch");
}

#[test]
fn merging_a_bar_beside_the_notch_never_loses_a_stone() {
    // The right half of the C-shape, sawn along x into two slabs. The second slab has
    // three bars along y: the lower arm, the notch (nothing but air) and the upper arm.
    // The empty bar is merged into the lower arm, which makes that piece reach across the
    // notch; the mesh solver is not monotone there, so the re-fit may find less or nothing.
    // The layout must keep at least the stones, and the weight, the arms held before.
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(&c_model(), &settings).expect("ctx");
    let pool = [box_design(1)];
    let leaf = |len: f64| Leaf {
        len,
        design: 0,
        orient: 0,
    };
    let tree = Tree {
        slabs: vec![
            Slab {
                thickness: 10.0,
                bars: vec![Bar {
                    width: 20.0,
                    leaves: vec![leaf(20.0)],
                }],
            },
            Slab {
                thickness: 9.7,
                bars: vec![
                    Bar {
                        width: 5.0,
                        leaves: vec![leaf(20.0)],
                    },
                    Bar {
                        width: 9.4,
                        leaves: vec![leaf(20.0)],
                    },
                    Bar {
                        width: 5.0,
                        leaves: vec![leaf(20.0)],
                    },
                ],
            },
        ],
    };
    // What the pieces hold on their own: the left half, the lower arm and the upper arm.
    let mut fitter = StoneFitter::new(&settings, &pool, &ctx.non_box).with_mesh(ctx.fit_mesh());
    let pieces = [
        ([0.0, 0.0, 0.0], [10.0, 20.0, 20.0]),
        ([10.3, 0.0, 0.0], [9.7, 5.0, 20.0]),
        ([10.3, 15.0, 0.0], [9.7, 5.0, 20.0]),
    ];
    let before: Vec<f64> = pieces
        .iter()
        .map(|&(origin, size)| {
            fitter
                .fit(origin, size, &leaf(size[2]))
                .expect("the piece holds a stone")
                .volume_mm3
        })
        .collect();
    let (stones, volume) = (before.len(), before.iter().sum::<f64>());
    let layout = layout_from_tree_shaped(&ctx, CutOrder::Xyz, &tree, &pool, &settings);
    assert!(layout.stone_count() >= stones, "{}", layout.stone_count());
    assert!(
        layout.total_volume_mm3 >= volume * (1.0 - 1e-12),
        "{} < {volume}",
        layout.total_volume_mm3
    );
    // Whatever was kept or re-fitted, nothing reaches into the notch.
    let hulls = [box_hull(1, 1.0, 1.0, 1.0)];
    for stone in &layout.stones {
        assert!(!stone_enters_notch(&layout, stone, &hulls));
    }
}

/// The smooth pebble scan (5,120 triangles) as a mesh rough.
fn pebble_model() -> RoughModel {
    let (points, tris) = pebble_scan(4, 1);
    RoughModel::new(import_parts(&points, &tris), Vec::new())
}

#[test]
fn a_smooth_pebble_scan_holds_stones_on_a_small_grid() {
    // A smooth scan's outline is a polytope of supporting planes, so the stone always
    // rests on it and meets the surface at several corners at once: the cutting-plane loop
    // must clear all those contacts, or no stone is ever placed.
    let model = pebble_model();
    assert!(model.mesh().is_some(), "a pebble is a mesh rough");
    let settings = settings_with(10, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let grid = grid_with_cells(&ctx, &settings, [3, 3, 3]);
    let front = pareto_front(&designs());
    let table = clipped_with_mesh(&ctx, &grid, &front, &settings);
    assert!(
        table.values.iter().any(|v| v.is_finite()),
        "every table value is -inf"
    );
    let mesh = ctx.mesh.as_deref().expect("the mesh");
    for order in CutOrder::ALL {
        let layouts =
            plan_shaped_for_order(&grid, &table, &ctx, &front, order, &settings, &mut |_| true)
                .expect("not cancelled");
        assert!(
            layouts.iter().any(|layout| layout.stone_count() > 0),
            "{order:?}: no layout holds a stone"
        );
        for layout in &layouts {
            for stone in &layout.stones {
                let half = DVec3::from(stone.stone_size_mm) * 0.5;
                let centre = DVec3::from(stone.pose.center_mm);
                let state =
                    mesh.box_state((centre - half).to_array(), (centre + half).to_array(), 0.0);
                assert_eq!(
                    state,
                    BoxState::Clear,
                    "{order:?}: a stone leaves the material"
                );
            }
        }
    }
}

#[test]
fn a_pebble_scan_fits_a_single_stone_exactly() {
    let model = pebble_model();
    let settings = settings_with(1, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let coarse = model
        .canonical_coarse_usable_halfspaces(ctx.inset_mm)
        .expect("coarse region");
    let hulls = hulls();
    let fits = fit_single_stones_with(
        &ctx.usable,
        &coarse,
        &hulls,
        &settings,
        1,
        ctx.fit_mesh(),
        &mut |_| true,
    )
    .expect("not cancelled");
    assert!(!fits.is_empty(), "no stone fits the pebble");
}
