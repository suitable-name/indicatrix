//! The convex hull base: planes from a point cloud, registration, scaling.

use glam::DVec3;

use super::{
    HullError, MAX_HULL_PLANES, RoughBase, RoughModel, ShapeError,
    hull::{
        BUILT_SCALED_CAP, OUTLINE_PLANES, built_scaled_copies, import_hull, import_mesh, is_built,
        is_registered, mesh,
    },
    mesh_fixture::{C_SHAPE_OBJ, CUBE_OBJ, parse_obj, pebble_scan},
};

/// The corners of the box `[0, x] x [0, y] x [0, z]` shifted by `offset`.
fn box_points(x: f64, y: f64, z: f64, offset: DVec3) -> Vec<DVec3> {
    let mut points = Vec::new();
    for cx in [0.0, x] {
        for cy in [0.0, y] {
            for cz in [0.0, z] {
                points.push(DVec3::new(cx, cy, cz) + offset);
            }
        }
    }
    points
}

#[test]
fn a_box_with_interior_and_face_points_is_six_planes_at_the_origin() {
    let mut points = box_points(4.0, 6.0, 8.0, DVec3::new(10.0, -3.0, 2.0));
    points.push(DVec3::new(12.0, 0.0, 6.0));
    points.push(DVec3::new(12.0, -3.0, 6.0));
    let base = import_hull(&points).expect("a box is a hull");
    let RoughBase::Hull {
        x_mm, y_mm, z_mm, ..
    } = base
    else {
        panic!("not a hull base");
    };
    assert!((x_mm - 4.0).abs() < 1e-9 && (y_mm - 6.0).abs() < 1e-9 && (z_mm - 8.0).abs() < 1e-9);
    assert_eq!(base.to_halfspaces(true).expect("planes").len(), 6);
    let volume = RoughModel::new(base, Vec::new())
        .measure()
        .expect("measures")
        .volume_mm3;
    assert!((volume - 192.0).abs() < 1e-6, "volume {volume}");
}

#[test]
fn importing_the_same_points_twice_gives_the_same_base() {
    let points = box_points(3.0, 3.0, 3.0, DVec3::ZERO);
    assert_eq!(import_hull(&points), import_hull(&points));
}

#[test]
fn a_scaled_hull_scales_the_volume_cubically() {
    let base = import_hull(&box_points(2.0, 3.0, 4.0, DVec3::ZERO)).expect("hull");
    let volume = |b: RoughBase| {
        RoughModel::new(b, Vec::new())
            .measure()
            .expect("m")
            .volume_mm3
    };
    let ratio = volume(base.scaled(2.0)) / volume(base);
    assert!((ratio - 8.0).abs() < 1e-6, "ratio {ratio}");
}

/// The volume of the mesh of the mesh rough `base`.
fn mesh_volume(base: RoughBase) -> f64 {
    RoughModel::new(base, Vec::new())
        .mesh()
        .expect("a mesh rough")
        .volume()
}

#[test]
fn many_scalings_of_a_mesh_keep_a_bounded_number_of_copies_and_every_id_valid() {
    // Fit to weight pressed over and over on a scan, each time on the result of the last.
    // The registry keeps a recipe per press, not a mesh, and builds a copy again when an id
    // whose copy was dropped is asked for, so no id an undo step may still name goes dead.
    let (points, tris) = parse_obj(C_SHAPE_OBJ);
    let (root, note) = import_mesh(&points, &tris).expect("imports");
    assert_eq!(note, None);
    assert!((mesh_volume(root) - 6000.0).abs() < 1e-9);
    let mut history = vec![(root, 1.0_f64)];
    for _ in 0..30 {
        let (last, total) = *history.last().expect("a base");
        history.push((last.scaled(1.02), total * 1.02));
        assert!(built_scaled_copies() <= BUILT_SCALED_CAP);
    }
    // Far more copies than the cap were made, so most are no longer built.
    assert!(history.len() > 4 * BUILT_SCALED_CAP);
    for (index, &(base, total)) in history.iter().enumerate() {
        let volume = mesh_volume(base);
        let expected = 6000.0 * total * total * total;
        assert!(
            (volume - expected).abs() < 1e-9 * expected,
            "scaling {index}: {volume} vs {expected}"
        );
        let [x, y, z] = base.bounding_box_extents();
        for extent in [x, y, z] {
            assert!(
                20.0_f64.mul_add(-total, extent).abs() < 1e-9 * extent,
                "scaling {index}"
            );
        }
        assert!(built_scaled_copies() <= BUILT_SCALED_CAP);
    }
    // Every scaling named a different hull, and naming one again names the same.
    let ids: std::collections::BTreeSet<u64> = history
        .iter()
        .map(|&(base, _)| match base {
            RoughBase::Hull { id, .. } => id,
            other => panic!("not a hull: {other:?}"),
        })
        .collect();
    assert_eq!(ids.len(), history.len());
    assert_eq!(root.scaled(1.02), history[1].0);
}

#[test]
fn validating_a_base_does_not_build_a_dropped_scaled_copy_again() {
    // A root no other test imports, so nothing else touches these ids.
    let (points, tris) = parse_obj(C_SHAPE_OBJ);
    let points: Vec<DVec3> = points.iter().map(|&p| p * 1.3711).collect();
    let (root, note) = import_mesh(&points, &tris).expect("imports");
    assert_eq!(note, None);
    let mut copies = vec![root];
    for factor in [1.011, 1.012, 1.013, 1.014, 1.015, 1.016] {
        let last = *copies.last().expect("a base");
        copies.push(last.scaled(factor));
    }
    // Six copies went through a list that keeps four built, so the first is dropped.
    let first = copies[1];
    let RoughBase::Hull { id, .. } = first else {
        panic!("not a hull: {first:?}");
    };
    assert!(!is_built(id), "the first copy is no longer built");
    assert!(is_registered(id), "but its id is still registered");
    first.validate().expect("a registered hull is valid");
    assert!(!is_built(id), "validating must not build the copy again");
    // An id that was never registered is told apart without a rebuild either.
    let unknown = RoughBase::Hull {
        id: 0xdead_beef_u64,
        x_mm: 1.0,
        y_mm: 1.0,
        z_mm: 1.0,
    };
    assert_eq!(unknown.validate(), Err(ShapeError::NothingLeft));
    assert!(!is_registered(0xdead_beef_u64));
    // The copy still resolves when something does need it: its planes build it again.
    let planes = first.to_halfspaces(true).expect("planes");
    assert!(planes.len() >= 4, "{} planes", planes.len());
}

#[test]
fn scaling_a_convex_hull_twice_is_one_scaling_by_the_product() {
    let base = import_hull(&box_points(2.0, 3.0, 4.0, DVec3::ZERO)).expect("hull");
    let twice = base.scaled(2.0).scaled(1.5);
    assert_eq!(twice, base.scaled(2.0 * 1.5));
    let volume = |b: RoughBase| {
        RoughModel::new(b, Vec::new())
            .measure()
            .expect("measures")
            .volume_mm3
    };
    assert!((volume(twice) / volume(base) - 27.0).abs() < 1e-6);
}

#[test]
fn scaling_an_unregistered_hull_or_by_a_bad_factor_changes_nothing() {
    let base = import_hull(&box_points(2.0, 3.0, 4.0, DVec3::ZERO)).expect("hull");
    for factor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(base.scaled(factor), base, "factor {factor}");
    }
    let unknown = RoughBase::Hull {
        id: 0xdead_beef,
        x_mm: 1.0,
        y_mm: 1.0,
        z_mm: 1.0,
    };
    assert_eq!(unknown.scaled(2.0), unknown);
}

#[test]
fn flat_and_non_finite_points_are_refused() {
    let flat = [DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::new(1.0, 1.0, 0.0)];
    assert_eq!(import_hull(&flat), Err(HullError::Flat));
    let bad = [
        DVec3::ZERO,
        DVec3::X,
        DVec3::Y,
        DVec3::new(0.0, 0.0, f64::NAN),
    ];
    assert_eq!(import_hull(&bad), Err(HullError::NotFinite));
}

#[test]
fn a_sphere_with_too_many_faces_is_refused() {
    let mut points = Vec::new();
    for i in 0..40 {
        for j in 0..40 {
            let (a, b) = (f64::from(i) * 0.157, f64::from(j).mul_add(0.157, 0.01));
            points.push(DVec3::new(b.sin() * a.cos(), b.sin() * a.sin(), b.cos()) * 10.0);
        }
    }
    assert!(matches!(import_hull(&points), Err(HullError::TooComplex(n)) if n > MAX_HULL_PLANES));
}

#[test]
fn the_corners_of_a_hull_come_back_for_saving() {
    let base = import_hull(&box_points(2.0, 2.0, 2.0, DVec3::ZERO)).expect("hull");
    assert_eq!(base.hull_corners().map(|c| c.len()), Some(8));
    assert_eq!(
        RoughBase::Block {
            x_mm: 1.0,
            y_mm: 1.0,
            z_mm: 1.0
        }
        .hull_corners(),
        None
    );
}

#[test]
fn a_large_scan_takes_the_quick_hull_and_a_small_one_the_exact_one() {
    // 10,242 welded vertices: above the quick-hull threshold, simplified.
    let (points, tris) = pebble_scan(5, 1);
    assert!(points.len() > 4096);
    let (base, _) = import_mesh(&points, &tris).expect("a large scan imports");
    let RoughBase::Hull { id, .. } = base else {
        panic!("not a hull");
    };
    let kept = mesh(id).expect("a smooth scan keeps its mesh");
    let planes = base.to_halfspaces(true).expect("planes");
    assert!(planes.len() > 6 && planes.len() <= OUTLINE_PLANES);
    for &v in kept.vertices() {
        for &(n, d) in &planes {
            assert!(n.dot(v) <= d + 1e-7, "a vertex outside the outline");
        }
    }

    // A cube with 5,000 more points ON its faces: more than the threshold, but the exact
    // hull has six planes, so the outline is the old algorithm's, bit for bit, and a convex
    // mesh is not kept.
    let (mut points, tris) = parse_obj(CUBE_OBJ);
    for axis in 0..3 {
        for side in [0.0, 20.0] {
            for i in 0..29 {
                for j in 0..29 {
                    let u = 20.0 * (f64::from(i) + 0.5) / 29.0;
                    let v = 20.0 * (f64::from(j) + 0.5) / 29.0;
                    points.push(match axis {
                        0 => DVec3::new(side, u, v),
                        1 => DVec3::new(u, side, v),
                        _ => DVec3::new(u, v, side),
                    });
                }
            }
        }
    }
    assert!(points.len() > 5000);
    let (base, note) = import_mesh(&points, &tris).expect("imports");
    assert_eq!(note, None);
    assert_eq!(Ok(base), import_hull(&points));
    let RoughBase::Hull { id, .. } = base else {
        panic!("not a hull");
    };
    assert!(mesh(id).is_none(), "a convex mesh keeps no mesh");
    assert_eq!(base.to_halfspaces(true).expect("planes").len(), 6);
}
