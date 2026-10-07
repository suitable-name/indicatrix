//! Tests of the non-convex mesh rough in the plan file: schema version 2, the reload by
//! import, the refusals, and that every other plan is written exactly as before.

use super::{
    dto::{MeshDto, SavedPlanDto},
    fixtures::{expect_error, plain_block, sample_layout_in, write},
    format::{LoadedPlan, parse_and_validate_plan, payload_version_of},
};
use crate::gui::rough_plan::obj_import::{C_SHAPE_OBJ, CUBE_OBJ, parse_obj_mesh};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{
    MAX_MESH_TRIANGLES, RoughBase, RoughCut, RoughModel, import_hull, import_mesh,
    shape::hull::{add_inclusion_points, remove_inclusion},
};

/// The base of the OBJ text `obj`, imported as a mesh without a note.
fn mesh_base(obj: &str) -> RoughBase {
    let (points, triangles) = parse_obj_mesh(obj).expect("the fixture parses");
    let (base, note) = import_mesh(&points, &triangles).expect("the fixture imports");
    assert_eq!(note, None);
    base
}

/// Whether `a` and `b` are the same rough: the same cuts and box, the same volume, and
/// the same mesh (or both without one). The registry id of an imported rough hashes
/// planes recomputed from the stored corners, so it is not compared.
fn assert_same_rough(a: &RoughModel, b: &RoughModel) {
    assert_eq!(a.cuts, b.cuts);
    assert_eq!(a.base.bounding_box_extents(), b.base.bounding_box_extents());
    let (va, vb) = (
        a.measure().expect("measures").volume_mm3,
        b.measure().expect("measures").volume_mm3,
    );
    assert!((va - vb).abs() <= 1e-9 * va.abs(), "{va} against {vb}");
    match (a.mesh(), b.mesh()) {
        (None, None) => {}
        (Some(ma), Some(mb)) => {
            assert_eq!(ma.triangles(), mb.triangles());
            assert_eq!(ma.vertices().len(), mb.vertices().len());
            for (p, q) in ma.vertices().iter().zip(mb.vertices()) {
                assert!((*p - *q).length() < 1e-9, "{p} against {q}");
            }
            assert!((ma.volume() - mb.volume()).abs() <= 1e-9 * ma.volume());
        }
        _ => panic!("one rough has a mesh and the other has not"),
    }
}

fn c_shape() -> RoughModel {
    RoughModel::new(mesh_base(C_SHAPE_OBJ), Vec::new())
}

#[test]
fn mesh_plan_roundtrips_with_version_2() {
    let cut = RoughCut::Face {
        normal: [0.0, 1.0, 0.0],
        depth_mm: 2.0,
    };
    for model in [
        c_shape(),
        RoughModel::new(mesh_base(C_SHAPE_OBJ), vec![cut]),
    ] {
        assert!(model.mesh().is_some(), "the notch keeps the mesh");
        let layouts = vec![sample_layout_in(&model)];
        let text = write(&model, &layouts);
        assert_eq!(payload_version_of(&text), Some(2), "{text}");
        assert!(text.contains("version = 2"), "{text}");
        assert!(text.contains("[rough.mesh]"), "{text}");
        assert!(!text.contains("hull = "), "the hull is derived: {text}");
        let loaded: LoadedPlan = parse_and_validate_plan(&text).expect("the plan loads");
        assert_eq!(loaded.version, 2);
        assert_eq!(loaded.layouts, layouts);
        // The reload is the same rough: the same planes and mesh, the same volume.
        assert_same_rough(&loaded.model, &model);
        // And it is a fixed point: writing it again gives the same text.
        assert_eq!(write(&loaded.model, &layouts), text);
    }
}

#[test]
fn version_1_files_still_load() {
    // A plan of a convex rough is written as version 1 and reads back.
    let text = write(&plain_block(), &[]);
    assert!(text.contains("version = 1"), "{text}");
    let loaded = parse_and_validate_plan(&text).expect("version 1 loads");
    assert_eq!(loaded.version, 1);
    assert_same_rough(&loaded.model, &plain_block());
    // So does a hull plan written as version 1.
    let hull = import_hull(&parse_obj_mesh(CUBE_OBJ).expect("parses").0).expect("hull");
    let hull_model = RoughModel::new(hull, Vec::new());
    let text = write(&hull_model, &[]);
    assert!(text.contains("version = 1"), "{text}");
    assert_same_rough(
        &parse_and_validate_plan(&text).expect("loads").model,
        &hull_model,
    );
    // A version 1 file may not carry a mesh.
    let mesh_text = write(&c_shape(), &[]);
    let downgraded = mesh_text.replace("version = 2", "version = 1");
    let error = expect_error(&downgraded);
    assert!(error.contains("rough.mesh"), "{error}");
}

/// The plan of the 20 x 12 x 10 mm block with no layouts, as it has always been written.
const BLOCK_PLAN_V1: &str = "\
format = \"indicatrix-rough-plan\"
version = 1
name = \"Test plan\"
created_at = 1790000000
library_id = 77
designs = []
layouts = []

[rough]
base = \"block\"
x_mm = 20.0
y_mm = 12.0
z_mm = 10.0
material = \"Aquamarine\"
specific_gravity = 3.51
weighed_ct = 8.9
cuts = []

[settings]
count = 6
kerf_mm = 0.3
allowance_mm = 0.2
skin_mm = 0.0
min_width_mm = 1.0
candidate_source = \"library\"
";

#[test]
fn saved_plan_bytes_unchanged_for_convex() {
    let block = write(&plain_block(), &[]);
    // The whole document as the code before the mesh schema wrote it: the `mesh` key is
    // skipped when empty and the version stays 1. The key order and layout are those of
    // the unchanged `SavedPlanDto` and `RoughDto` (base commit 9a0f59e).
    assert_eq!(block, BLOCK_PLAN_V1);
    // A hull rough writes its corners under `hull`, exactly as before the mesh schema.
    let (points, _) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    let hull_model = RoughModel::new(import_hull(&points).expect("hull"), Vec::new());
    let text = write(&hull_model, &[]);
    assert!(text.contains("version = 1\n"), "{text}");
    assert!(text.contains("base = \"hull\"\n"), "{text}");
    assert!(text.contains("hull = [\n"), "{text}");
    assert!(!text.contains("mesh"), "{text}");
    // A closed mesh whose solid is convex imports as the same hull, so its plan is the
    // hull's plan, byte for byte.
    let convex_mesh = RoughModel::new(mesh_base(CUBE_OBJ), Vec::new());
    assert_eq!(write(&convex_mesh, &[]), text);
    assert_same_rough(&convex_mesh, &hull_model);

    // A mesh rough without inclusions is still the version 2 plan it always was: the whole
    // mesh under `rough.mesh` and no `inclusions` key.
    let c_text = write(&c_shape(), &[]);
    assert!(c_text.contains("version = 2\n"), "{c_text}");
    assert!(!c_text.contains("inclusions"), "{c_text}");
    // An inclusion added and removed again leaves the very same rough, so the same bytes.
    let (inner, inner_triangles) = inner_cube(2.0, 6.0);
    let with = add_inclusion_points(&c_shape().base, &inner, &inner_triangles, 0.3)
        .expect("the inclusion fits the arm");
    let back = remove_inclusion(&with, 0).expect("removes");
    assert_eq!(write(&RoughModel::new(back, Vec::new()), &[]), c_text);
}

/// The cube `[lo, hi]` on every axis as points and triangles: an inclusion for the 20 mm
/// fixtures.
fn inner_cube(lo: f64, hi: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (mut points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    for p in &mut points {
        *p = *p * ((hi - lo) / 20.0) + DVec3::splat(lo);
    }
    (points, triangles)
}

/// `base` with the cubes of `spans` (each `(lo, hi)`) as inclusions with a 0.3 mm margin.
fn with_inclusions(base: RoughBase, spans: &[(f64, f64)]) -> RoughModel {
    let mut base = base;
    for &(lo, hi) in spans {
        let (points, triangles) = inner_cube(lo, hi);
        base = add_inclusion_points(&base, &points, &triangles, 0.3).expect("the inclusion fits");
    }
    RoughModel::new(base, Vec::new())
}

#[test]
fn a_plan_with_inclusions_roundtrips_with_version_3() {
    let cube = import_hull(&parse_obj_mesh(CUBE_OBJ).expect("parses").0).expect("hull");
    for base in [mesh_base(C_SHAPE_OBJ), cube] {
        let model = with_inclusions(base, &[(2.0, 6.0), (8.0, 9.0)]);
        let layouts = vec![sample_layout_in(&model)];
        let text = write(&model, &layouts);
        assert_eq!(payload_version_of(&text), Some(3), "{text}");
        assert!(text.contains("version = 3"), "{text}");
        assert!(text.contains("[rough.mesh]"), "{text}");
        assert!(text.contains("[[rough.inclusions]]"), "{text}");
        let loaded: LoadedPlan = parse_and_validate_plan(&text).expect("the plan loads");
        assert_eq!(loaded.version, 3);
        assert_eq!(loaded.layouts, layouts);
        assert_same_rough(&loaded.model, &model);
        let (a, b) = (
            model.mesh().expect("mesh"),
            loaded.model.mesh().expect("mesh"),
        );
        assert_eq!(a.inclusion_count(), 2);
        assert_eq!(b.inclusion_count(), 2);
        assert_eq!(a.inclusion_shells(), b.inclusion_shells());
        // The weight of the plan is the gross volume, before and after.
        let (ma, mb) = (
            model.measure().expect("m"),
            loaded.model.measure().expect("m"),
        );
        assert!((ma.volume_mm3 - mb.volume_mm3).abs() <= 1e-9 * ma.volume_mm3);
        assert!((ma.inclusion_mm3 - mb.inclusion_mm3).abs() <= 1e-9 * ma.volume_mm3);
        // And it is a fixed point: writing it again gives the same text.
        assert_eq!(write(&loaded.model, &layouts), text);
    }
}

#[test]
fn inclusions_need_version_3_and_a_mesh_and_must_fit() {
    let model = with_inclusions(mesh_base(C_SHAPE_OBJ), &[(2.0, 6.0)]);
    let text = write(&model, &[]);
    let downgraded = text.replace("version = 3", "version = 2");
    let error = expect_error(&downgraded);
    assert!(error.contains("rough.inclusions"), "{error}");

    let mut doc: SavedPlanDto = toml::from_str(&text).expect("parses");
    // Moved out of the rough: not in the material any more.
    let mut outside = doc.rough.inclusions[0].clone();
    for v in &mut outside.vertices {
        v[0] += 100.0;
    }
    doc.rough.inclusions = vec![outside];
    let error = expect_error(&toml::to_string_pretty(&doc).expect("writes"));
    assert!(error.contains("rough.inclusions"), "{error}");

    // Inclusions without the mesh they lie in.
    let mut doc: SavedPlanDto = toml::from_str(&text).expect("parses");
    doc.rough.mesh = None;
    doc.rough.hull = vec![
        [0.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [0.0, 20.0, 0.0],
        [0.0, 0.0, 20.0],
    ];
    let error = expect_error(&toml::to_string_pretty(&doc).expect("writes"));
    assert!(error.contains("rough.inclusions"), "{error}");
}

#[test]
fn a_bad_or_oversized_mesh_is_refused() {
    let good = write(&c_shape(), &[]);
    let mut doc: SavedPlanDto = toml::from_str(&good).expect("parses");
    let mesh = doc.rough.mesh.clone().expect("a mesh");

    // An index outside the vertices.
    let mut broken = mesh.clone();
    broken.triangles[0][0] = 9_999;
    doc.rough.mesh = Some(broken);
    let error = expect_error(&toml::to_string_pretty(&doc).expect("writes"));
    assert!(error.contains("rough.mesh"), "{error}");

    // An open surface: half the triangles missing. One missing triangle is a small hole
    // the mesh repair fills, so it would reload; a hole this large is never filled.
    let mut open = mesh.clone();
    open.triangles.truncate(open.triangles.len() / 2);
    doc.rough.mesh = Some(open);
    let error = expect_error(&toml::to_string_pretty(&doc).expect("writes"));
    assert!(error.contains("rough.mesh"), "{error}");

    // More triangles than a rough may have.
    let many = MeshDto {
        vertices: mesh.vertices.clone(),
        triangles: vec![[0, 1, 2]; MAX_MESH_TRIANGLES + 1],
    };
    doc.rough.mesh = Some(many);
    let error = expect_error(&toml::to_string_pretty(&doc).expect("writes"));
    assert!(error.contains("triangles"), "{error}");

    // A coordinate that is not a number.
    let mut nan = mesh;
    nan.vertices[0][0] = f64::NAN;
    doc.rough.mesh = Some(nan);
    let error = expect_error(&toml::to_string_pretty(&doc).expect("writes"));
    assert!(error.contains("rough.mesh"), "{error}");
}

#[test]
fn a_mesh_that_import_turns_into_a_plain_hull_is_refused() {
    let good = write(&c_shape(), &[]);
    let mut doc: SavedPlanDto = toml::from_str(&good).expect("parses");

    // A closed mesh that is convex: import gives the plain hull, which a version 2 file
    // must not carry as a mesh.
    let (vertices, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    doc.rough.mesh = Some(MeshDto {
        vertices: vertices.iter().map(DVec3::to_array).collect(),
        triangles,
    });
    let error = expect_error(&toml::to_string_pretty(&doc).expect("writes"));
    assert!(
        error.contains("rough.mesh") && error.contains("convex"),
        "{error}"
    );

    // A mesh without faces is a point cloud: the same.
    doc.rough.mesh = Some(MeshDto {
        vertices: vertices.iter().map(DVec3::to_array).collect(),
        triangles: Vec::new(),
    });
    let error = expect_error(&toml::to_string_pretty(&doc).expect("writes"));
    assert!(error.contains("rough.mesh"), "{error}");
}

#[test]
fn a_stray_vertex_in_the_obj_does_not_change_the_rough_after_a_save() {
    // A `v` line no face names, far from the C.
    let with_stray = format!("{C_SHAPE_OBJ}v 40 -9 55\n");
    let model = RoughModel::new(mesh_base(&with_stray), Vec::new());
    assert!(model.mesh().is_some());
    // The same rough as without the stray vertex: the hull and frame come from the mesh.
    let clean = c_shape();
    assert_eq!(model.base, clean.base);
    let text = write(&model, &[]);
    let loaded = parse_and_validate_plan(&text).expect("the plan loads");
    assert_same_rough(&loaded.model, &model);
    // The very same registered rough: bounding box, hull planes and mesh frame, hence the
    // same stone positions from the planner.
    assert_eq!(loaded.model.base, model.base);
    assert_eq!(
        loaded.model.halfspaces().expect("planes"),
        model.halfspaces().expect("planes")
    );
    let (before, after) = (
        model.mesh().expect("mesh"),
        loaded.model.mesh().expect("mesh"),
    );
    assert_eq!(before.bounds(), after.bounds());
    assert_eq!(before.volume(), after.volume());
    for extent in model.base.bounding_box_extents() {
        assert!((extent - 20.0).abs() < 1e-9, "{extent}");
    }
}
