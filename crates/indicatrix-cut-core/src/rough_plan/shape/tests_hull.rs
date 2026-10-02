//! The convex hull base: planes from a point cloud, registration, scaling.

use glam::DVec3;

use super::{HullError, MAX_HULL_PLANES, RoughBase, RoughModel, hull::import_hull};

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
