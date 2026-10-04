//! Plans of a non-convex rough: a C-shaped mesh against its own convex hull.
//!
//! The fixture is a 20 mm cube with a 10 x 10 x 20 mm notch cut into one face (see
//! [`C_SHAPE_OBJ`]): 6000 mm^3 of material inside an 8000 mm^3 hull. A planner that sees
//! only the hull fills the notch with stone; the mesh must keep it out.

use glam::DVec3;

use super::{
    clip::{CLASS_EXTERIOR, CLASS_INTERIOR, CLASS_PARTIAL, classify_box_in},
    ctx::ShapedCtx,
};
use crate::rough_plan::{
    CandidateDesign, DesignHull, PlanInput, RoughCut, RoughLayout, RoughModel, import_hull,
    import_mesh, plan,
    shape::mesh_fixture::{
        C_SHAPE_OBJ, CUBE_OBJ, box_hull, import_obj, import_parts, noisy_c_shape, parse_obj,
        stone_enters_notch,
    },
    tests::{box_design, settings_with},
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
    assert!(!layouts.is_empty());
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
