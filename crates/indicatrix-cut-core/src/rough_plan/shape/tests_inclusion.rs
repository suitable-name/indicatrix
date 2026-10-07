//! Inclusions of an imported mesh rough, on the base: adding, listing, removing, weighing,
//! scaling and reading back. The mesh-level behaviour is in `mesh/tests.rs`.

use glam::DVec3;

use super::{
    HullError, MeshError, RoughBase, RoughMesh, RoughModel,
    hull::{
        add_inclusion_mesh, add_inclusion_points, import_hull, import_mesh_with_inclusions,
        inclusion_list, remove_inclusion, source_frame,
    },
    mesh_fixture::{C_SHAPE_OBJ, CUBE_OBJ, import_obj, parse_obj},
};

/// The cube `[lo, hi]` on every axis as points and triangles.
fn cube(lo: f64, hi: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (mut points, tris) = parse_obj(CUBE_OBJ);
    for p in &mut points {
        *p = *p * ((hi - lo) / 20.0) + DVec3::splat(lo);
    }
    (points, tris)
}

/// The 20 mm cube as a plain convex hull rough.
fn cube_rough() -> RoughBase {
    import_hull(&cube(0.0, 20.0).0).expect("a cube")
}

/// The C-shaped rough (6000 mm^3 in a 20 mm cube).
fn c_rough() -> RoughBase {
    import_obj(C_SHAPE_OBJ)
}

fn model(base: RoughBase) -> RoughModel {
    RoughModel::new(base, Vec::new())
}

/// `base` with the cube `[lo, hi]` as an inclusion with `margin`.
fn add(base: &RoughBase, lo: f64, hi: f64, margin: f64) -> Result<RoughBase, HullError> {
    let (points, tris) = cube(lo, hi);
    add_inclusion_points(base, &points, &tris, margin)
}

fn id_of(base: &RoughBase) -> u64 {
    let RoughBase::Hull { id, .. } = *base else {
        panic!("not a hull base");
    };
    id
}

#[test]
fn an_inclusion_in_a_convex_rough_makes_it_a_mesh_rough_that_weighs_the_whole() {
    let base = cube_rough();
    assert!(model(base).mesh().is_none());
    let with = add(&base, 8.0, 12.0, 0.0).expect("the inclusion fits");
    assert_ne!(with, base);
    // The old base is untouched and still a plain hull.
    assert!(model(base).mesh().is_none());
    assert_eq!(with.bounding_box_extents(), base.bounding_box_extents());

    let rough = model(with);
    let mesh = rough.mesh().expect("an inclusion makes a mesh rough");
    assert!((mesh.volume() - 7936.0).abs() < 1e-6, "{}", mesh.volume());
    let measure = rough.measure().expect("measures");
    // What you weigh is the whole cube; 64 mm^3 of it is the inclusion.
    assert!((measure.volume_mm3 - 8000.0).abs() < 1e-6, "{measure:?}");
    assert_eq!(measure.inclusion_count, 1);
    assert!((measure.inclusion_mm3 - 64.0).abs() < 1e-6);
    // The stones cannot go there.
    assert_eq!(
        mesh.box_state([9.0; 3], [11.0; 3], 0.0),
        super::BoxState::Air
    );

    let plain = model(base).measure().expect("measures");
    assert!((plain.volume_mm3 - 8000.0).abs() < 1e-6);
    assert_eq!(plain.inclusion_count, 0);
    assert_eq!(plain.inclusion_mm3.to_bits(), 0.0_f64.to_bits());
}

#[test]
fn a_non_convex_rough_takes_an_inclusion_in_its_material_and_only_there() {
    let base = c_rough();
    let with = add(&base, 2.0, 6.0, 0.0).expect("in the left arm");
    let measure = model(with).measure().expect("measures");
    assert!((measure.volume_mm3 - 6000.0).abs() < 1e-6, "{measure:?}");
    assert!((measure.inclusion_mm3 - 64.0).abs() < 1e-6);
    assert!((model(with).mesh().expect("mesh").volume() - 5936.0).abs() < 1e-6);
    // In the notch (x 10..20, y 5..15) there is no material to hold an inclusion.
    assert_eq!(
        add(&base, 11.0, 14.0, 0.0),
        Err(HullError::Inclusion(MeshError::InclusionOutside(0)))
    );
    // One that starts in the notch and reaches into the upper arm crosses the surface at
    // y = 15 (the cube 12..16 is 3 mm in the notch and 1 mm in the arm).
    assert_eq!(
        add(&base, 12.0, 16.0, 0.0),
        Err(HullError::Inclusion(MeshError::InclusionReachesSurface(0)))
    );
}

#[test]
fn an_inclusion_that_reaches_the_surface_is_refused_with_the_notch_advice() {
    let error = add(&cube_rough(), 15.0, 25.0, 0.0).expect_err("it pokes out");
    assert_eq!(
        error,
        HullError::Inclusion(MeshError::InclusionReachesSurface(0))
    );
    let text = error.to_string();
    assert!(
        text.contains("an inclusion that reaches the surface must be cut away")
            && text.contains("model it as a notch in the rough's own mesh"),
        "{text}"
    );
}

#[test]
fn a_block_is_not_an_imported_mesh_and_takes_no_inclusion() {
    let block = RoughBase::Block {
        x_mm: 20.0,
        y_mm: 20.0,
        z_mm: 20.0,
    };
    assert_eq!(add(&block, 8.0, 12.0, 0.0), Err(HullError::NotHull));
    assert_eq!(remove_inclusion(&block, 0), Err(HullError::NotHull));
}

#[test]
fn the_margin_pads_the_inclusion_and_is_refused_when_it_does_not_fit() {
    let base = cube_rough();
    let padded = add(&base, 8.0, 12.0, 0.3).expect("fits with its margin");
    let measure = model(padded).measure().expect("measures");
    // The weight is still the whole cube; the inclusion's own volume carries the margin.
    assert!((measure.volume_mm3 - 8000.0).abs() < 1e-6);
    assert!((measure.inclusion_mm3 - 4.6_f64.powi(3)).abs() < 1e-5);
    let list = inclusion_list(id_of(&padded));
    assert_eq!(list.len(), 1);
    assert!((list[0].extents_mm - DVec3::splat(4.6)).abs().max_element() < 1e-8);
    // 0.1 mm from the surface: fine as it is, too close for 0.3 mm of margin.
    assert!(add(&base, 0.1, 5.0, 0.0).is_ok());
    assert_eq!(add(&base, 0.1, 5.0, 0.3), Err(HullError::InclusionTooClose));
    assert!(
        HullError::InclusionTooClose
            .to_string()
            .contains("lower the margin")
    );
}

#[test]
fn inclusions_are_listed_in_order_and_removed_by_number() {
    let base = cube_rough();
    let one = add(&base, 2.0, 6.0, 0.0).expect("one");
    let two = add(&one, 10.0, 14.0, 0.0).expect("two");
    let list = inclusion_list(id_of(&two));
    assert_eq!(list.len(), 2);
    assert!((list[0].volume_mm3 - 64.0).abs() < 1e-9);
    assert!((list[0].centre_mm - DVec3::splat(4.0)).length() < 1e-9);
    assert!((list[1].centre_mm - DVec3::splat(12.0)).length() < 1e-9);
    assert_eq!(inclusion_list(id_of(&base)), []);

    // Removing the first leaves the second.
    let rest = remove_inclusion(&two, 0).expect("removes");
    let list = inclusion_list(id_of(&rest));
    assert_eq!(list.len(), 1);
    assert!((list[0].centre_mm - DVec3::splat(12.0)).length() < 1e-9);
    assert_eq!(remove_inclusion(&two, 2), Err(HullError::NoInclusion));
    assert_eq!(remove_inclusion(&base, 0), Err(HullError::NoInclusion));
}

#[test]
fn removing_the_last_inclusion_gives_back_the_rough_it_was_added_to() {
    // A convex rough comes back as its plain hull, with its own id.
    let base = cube_rough();
    let with = add(&base, 8.0, 12.0, 0.3).expect("fits");
    let back = remove_inclusion(&with, 0).expect("removes");
    assert_eq!(back, base);
    assert!(model(back).mesh().is_none());
    // A non-convex rough comes back as the very same registered rough.
    let c = c_rough();
    let with = add(&c, 2.0, 6.0, 0.0).expect("fits");
    assert_eq!(remove_inclusion(&with, 0), Ok(c));
}

#[test]
fn adding_an_inclusion_twice_gives_the_same_rough_and_the_two_entry_points_agree() {
    let base = c_rough();
    assert_eq!(add(&base, 2.0, 6.0, 0.3), add(&base, 2.0, 6.0, 0.3));
    let (points, tris) = cube(2.0, 6.0);
    let body = RoughMesh::new(&points, &tris).expect("a closed cube");
    assert_eq!(
        add_inclusion_mesh(&base, &body, 0.3),
        add(&base, 2.0, 6.0, 0.3)
    );
}

#[test]
fn an_inclusion_and_a_cavity_of_one_shape_are_two_different_roughs() {
    let base = cube_rough();
    let with = add(&base, 8.0, 12.0, 0.0).expect("fits");
    let mesh = model(with).mesh().expect("mesh");
    // The same triangles and vertices imported as a plain mesh: a cube with a hollow in it.
    let (hollow_base, note) =
        super::hull::import_mesh(mesh.vertices(), mesh.triangles()).expect("a hollow cube");
    assert_eq!(note, None);
    let hollow = model(hollow_base)
        .mesh()
        .expect("the hollow is a mesh rough");
    // Building a mesh numbers its vertices by first use, and an inclusion's triangles are
    // wound the other way, so the hollow holds the same points in another order.
    let points = |mesh: &RoughMesh| {
        let mut bits: Vec<[u64; 3]> = mesh
            .vertices()
            .iter()
            // `+ 0.0` makes a negative zero a zero, which compares equal.
            .map(|p| [p.x, p.y, p.z].map(|c| (c + 0.0).to_bits()))
            .collect();
        bits.sort_unstable();
        bits
    };
    assert_eq!(points(&hollow), points(&mesh));
    assert_eq!(hollow.triangles().len(), mesh.triangles().len());
    assert_eq!(hollow.inclusion_count(), 0);
    // Weighed, the hollow is air and the inclusion is stone.
    let weight = |base: RoughBase| model(base).measure().expect("measures").volume_mm3;
    assert!((weight(hollow_base) - 7936.0).abs() < 1e-6);
    assert!((weight(with) - 8000.0).abs() < 1e-6);
    // And they do not share a registry id (the hull module tests the hash on identical
    // content).
    assert_ne!(id_of(&hollow_base), id_of(&with));
}

#[test]
fn a_saved_rough_reads_back_as_the_same_rough() {
    for base in [cube_rough(), c_rough()] {
        let with = add(&base, 2.0, 6.0, 0.3).expect("fits");
        let with = add(&with, 8.0, 9.0, 0.3).expect("a second one fits");
        let a = model(with).mesh().expect("mesh");
        // What a plan file stores: the rough's own mesh and each inclusion's.
        let inclusions: Vec<_> = a
            .inclusions()
            .iter()
            .map(|body| (body.vertices().to_vec(), body.triangles().to_vec()))
            .collect();
        let frame = source_frame(id_of(&with));
        let back = import_mesh_with_inclusions(
            a.outer_vertices(),
            a.outer_triangles(),
            &inclusions,
            frame,
        )
        .expect("reads back");
        let b = model(back).mesh().expect("mesh");
        assert_eq!(a.triangles(), b.triangles());
        assert_eq!(a.inclusion_shells(), b.inclusion_shells());
        assert_eq!(a.inclusion_count(), 2);
        assert_eq!(b.inclusion_count(), 2);
        for (p, q) in a.vertices().iter().zip(b.vertices()) {
            assert!((*p - *q).length() < 1e-9, "{p} against {q}");
        }
        assert!((a.volume() - b.volume()).abs() < 1e-9);
        assert!((a.gross_volume() - b.gross_volume()).abs() < 1e-9);
        let (fa, fb) = (
            frame.expect("a frame"),
            source_frame(id_of(&back)).expect("a frame"),
        );
        assert!((fa.scale - fb.scale).abs() < 1e-12 && (fa.offset - fb.offset).length() < 1e-9);
        let (ma, mb) = (
            model(with).measure().expect("measures"),
            model(back).measure().expect("measures"),
        );
        assert!((ma.volume_mm3 - mb.volume_mm3).abs() < 1e-9);
        assert!((ma.inclusion_mm3 - mb.inclusion_mm3).abs() < 1e-9);
        // The rough's own mesh alone, without inclusions, is what the plan's `mesh` holds.
        assert!(a.outer_triangles().len() < a.triangles().len());
    }
}

#[test]
fn a_saved_rough_whose_inclusion_does_not_fit_is_refused() {
    let (points, tris) = cube(0.0, 20.0);
    let outside = cube(30.0, 40.0);
    assert_eq!(
        import_mesh_with_inclusions(&points, &tris, &[outside], None),
        Err(HullError::Inclusion(MeshError::InclusionOutside(0)))
    );
    let open = (points.clone(), tris[..tris.len() - 1].to_vec());
    assert!(matches!(
        import_mesh_with_inclusions(&points, &tris, &[open], None),
        Err(HullError::Inclusion(_))
    ));
}

#[test]
fn inclusions_scale_with_the_rough() {
    let base = c_rough();
    let with = add(&base, 2.0, 6.0, 0.0).expect("fits");
    let scaled = model(with).scaled(1.5);
    let measure = scaled.measure().expect("measures");
    assert!((measure.volume_mm3 - 20_250.0).abs() < 1e-3, "{measure:?}");
    assert_eq!(measure.inclusion_count, 1);
    assert!((measure.inclusion_mm3 - 216.0).abs() < 1e-3);
    // Scaling back is exact enough to reach the same weight.
    let again = scaled.scaled(1.0 / 1.5).measure().expect("measures");
    assert!((again.volume_mm3 - 6000.0).abs() < 1e-6);
}

#[test]
fn a_cut_through_an_inclusion_counts_the_inclusion_inside_the_cut_as_material() {
    // x <= 15 (depth 5 from the face at 20) on the cube with the inclusion [12, 18] in x:
    // the cut leaves half of the inclusion (3 of 6 mm), 6 x 6 x 3 = 108 mm^3 of it.
    let with = add(&cube_rough(), 12.0, 18.0, 0.0).expect("fits");
    let cut = super::RoughCut::Face {
        normal: [1.0, 0.0, 0.0],
        depth_mm: 5.0,
    };
    let rough = RoughModel::new(with, vec![cut]);
    let measure = rough.measure().expect("measures");
    assert!((measure.volume_mm3 - 6000.0).abs() < 1e-6, "{measure:?}");
    assert!((measure.inclusion_mm3 - 108.0).abs() < 1e-6, "{measure:?}");
}

#[test]
fn a_files_coordinates_map_into_the_rough_frame_and_scale_with_it() {
    // A rough whose file puts its corner at (100, 200, 300): the import moves it to the origin.
    let shift = DVec3::new(100.0, 200.0, 300.0);
    let points: Vec<DVec3> = cube(0.0, 20.0).0.iter().map(|&p| p + shift).collect();
    let base = import_hull(&points).expect("a cube");
    let frame = source_frame(id_of(&base)).expect("registered");
    assert!(frame.apply(shift).length() < 1e-9);
    assert!((frame.apply(shift + DVec3::splat(10.0)) - DVec3::splat(10.0)).length() < 1e-9);
    // Fit to weight scales the rough; the file's coordinates then land twice as far out.
    let scaled = model(base).scaled(2.0).base;
    let frame = source_frame(id_of(&scaled)).expect("registered");
    assert!((frame.apply(shift + DVec3::splat(10.0)) - DVec3::splat(20.0)).length() < 1e-9);
    assert!((frame.scale - 2.0).abs() < 1e-12);
    // An inclusion given in the file's coordinates, moved the same way, fits.
    let inner: Vec<DVec3> = cube(8.0, 12.0)
        .0
        .iter()
        .map(|&p| frame.apply(p + shift))
        .collect();
    let tris = cube(8.0, 12.0).1;
    assert!(add_inclusion_points(&scaled, &inner, &tris, 0.0).is_ok());
    // A rough with no known frame (a plan read back without one) is its own.
    let (verts, tris) = cube(0.0, 20.0);
    let plain =
        import_mesh_with_inclusions(&verts, &tris, &[cube(8.0, 12.0)], None).expect("reads back");
    let frame = source_frame(id_of(&plain)).expect("registered");
    assert!((frame.apply(DVec3::splat(5.0)) - DVec3::splat(5.0)).length() < 1e-9);
}
