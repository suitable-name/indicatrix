//! Tests of the zone geometry (written, not run, by lane G1).
//!
//! The primitive fits on noisy synthetic points with fixed seeds, the editing backend, the
//! overlay on a cube, and the Gauss-Newton loop of the refinement with an analytic residual. The
//! tests that need the forward tracer and the solver are in `tests_pipeline`.

use std::{
    f64::consts::{PI, TAU},
    sync::atomic::AtomicBool,
};

use glam::{DMat3, DQuat, DVec3};
use indicatrix::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption, ZoningError},
};

use super::{
    BoundaryPoints, Evaluation, OverlayOptions, RadialRole, RefineError, RefineOptions, ZoneEdit,
    ZoneEditError, ZoneLocks, ZoneParameter, apply, apply_with_locks, fit_cylinder, fit_half_space,
    fit_prism, fit_sector, fit_slab,
    geometry::{axis_reference, least_squares, solve_linear, sym_eigen3},
    parameter_value, project_overlay, refine_with, surface_traces,
};
use crate::rough_plan::locate::{Projection, RigProfile, Rigid, Scene, ViewPose, box_mesh};

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// `SplitMix64` with Box-Muller normals.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1_u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.uniform()
    }

    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }

    fn noise(&mut self, sigma: f64) -> DVec3 {
        DVec3::new(self.gauss(), self.gauss(), self.gauss()) * sigma
    }
}

fn line_distance(p: DVec3, point: DVec3, dir: DVec3) -> f64 {
    let q = p - point;
    (q - dir * q.dot(dir)).length()
}

fn angle_gap(a: f64, b: f64, period: f64) -> f64 {
    let d = (a - b).rem_euclid(period);
    d.min(period - d)
}

fn plane_points(rng: &mut Rng, normal: DVec3, offset: f64, count: usize, sigma: f64) -> Vec<DVec3> {
    let (u, v) = axis_reference(normal);
    (0..count)
        .map(|_| {
            let (a, b) = (rng.range(-5.0, 5.0), rng.range(-5.0, 5.0));
            normal * offset + u * a + v * b + rng.noise(sigma)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Numerics
// ---------------------------------------------------------------------------------------------

#[test]
fn the_eigen_solver_recovers_a_known_spectrum() {
    let q = DQuat::from_axis_angle(DVec3::new(1.0, 2.0, -0.5).normalize(), 0.9);
    let r = DMat3::from_quat(q);
    let d = DMat3::from_diagonal(DVec3::new(1.0, 2.0, 5.0));
    let m = (r * d * r.transpose()).to_cols_array_2d();
    let (values, vectors) = sym_eigen3(m);
    for (value, expected) in values.iter().zip([1.0, 2.0, 5.0]) {
        assert!((value - expected).abs() < 1e-12, "eigenvalue {value}");
    }
    for (vector, column) in vectors.iter().zip([r.x_axis, r.y_axis, r.z_axis]) {
        assert!(vector.dot(column).abs() > 1.0 - 1e-12);
    }
    // A diagonal matrix needs no rotation and keeps the order.
    let (values, _) = sym_eigen3([[3.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 2.0]]);
    assert_eq!(values, [1.0, 2.0, 3.0]);
}

#[test]
fn the_linear_solver_solves_and_reports_a_singular_system() {
    let mut a = vec![2.0, 1.0, 1.0, 3.0];
    let mut b = vec![5.0, 10.0];
    assert!(solve_linear(2, &mut a, &mut b));
    assert!((b[0] - 1.0).abs() < 1e-12 && (b[1] - 3.0).abs() < 1e-12);
    let mut s = vec![1.0, 2.0, 2.0, 4.0];
    let mut c = vec![1.0, 2.0];
    assert!(!solve_linear(2, &mut s, &mut c));
}

#[test]
fn the_gauss_newton_driver_fits_a_circle() {
    let mut rng = Rng(5);
    let pts: Vec<(f64, f64)> = (0..50)
        .map(|_| {
            let t = rng.range(0.0, TAU);
            (
                2.0 + 3.0 * t.cos() + 0.01 * rng.gauss(),
                -1.0 + 3.0 * t.sin() + 0.01 * rng.gauss(),
            )
        })
        .collect();
    let residuals = |x: &[f64], out: &mut Vec<f64>| {
        out.clear();
        for (px, py) in &pts {
            out.push((px - x[0]).hypot(py - x[1]) - x[2]);
        }
    };
    let sol = least_squares(&[1.0, 0.0, 2.0], pts.len(), &[1e-6; 3], 60, &residuals);
    assert!((sol.x[0] - 2.0).abs() < 0.02 && (sol.x[1] + 1.0).abs() < 0.02);
    assert!((sol.x[2] - 3.0).abs() < 0.02);
    assert!(sol.converged);
}

// ---------------------------------------------------------------------------------------------
// Primitive fits (within 0.05 mm on noisy points)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_plane_is_recovered_from_noisy_points() {
    let mut rng = Rng(1);
    let normal = DVec3::new(0.3, -0.5, 0.8).normalize();
    let pts = plane_points(&mut rng, normal, 1.7, 60, 0.01);
    let fit = fit_half_space(&BoundaryPoints::from_points(pts), Some(normal * 10.0))
        .expect("a plane fit");
    match fit.shape {
        ZoneShape::HalfSpace { normal: n, offset } => {
            assert!(n.dot(normal) > 0.9999, "normal {n:?}");
            assert!((offset - 1.7).abs() < 0.05, "offset {offset}");
        }
        other => panic!("not a half space: {other:?}"),
    }
    assert!(fit.rms_mm < 0.03, "rms {}", fit.rms_mm);
    assert_eq!(fit.per_point_residual.len(), 60);
    assert!(fit.leave_one_view_out_rms.is_none());
}

#[test]
fn the_plane_orientation_follows_the_hint_and_collinear_points_are_refused() {
    let mut rng = Rng(2);
    let normal = DVec3::Z;
    let pts = plane_points(&mut rng, normal, 0.0, 30, 0.0);
    let up = fit_half_space(
        &BoundaryPoints::from_points(pts.clone()),
        Some(DVec3::Z * 3.0),
    )
    .expect("up");
    let down =
        fit_half_space(&BoundaryPoints::from_points(pts), Some(DVec3::Z * -3.0)).expect("down");
    let z = |f: &super::FittedZoneShape| match f.shape {
        ZoneShape::HalfSpace { normal, .. } => normal.z,
        _ => f64::NAN,
    };
    assert!(z(&up) > 0.99 && z(&down) < -0.99);
    let line: Vec<DVec3> = (0..10)
        .map(|i| DVec3::new(f64::from(i), 0.0, 0.0))
        .collect();
    assert!(fit_half_space(&BoundaryPoints::from_points(line), None).is_err());
    assert!(fit_half_space(&BoundaryPoints::from_points(vec![DVec3::ZERO; 2]), None).is_err());
}

#[test]
fn the_leave_one_view_out_residual_is_reported_and_small_for_a_consistent_set() {
    let mut rng = Rng(3);
    let normal = DVec3::new(0.0, 1.0, 0.2).normalize();
    let pts = plane_points(&mut rng, normal, 2.0, 40, 0.01);
    let mut bp = BoundaryPoints::from_points(pts.clone());
    for view in 0..4 {
        let looser: Vec<DVec3> = pts.iter().map(|p| *p + rng.noise(0.02)).collect();
        bp = bp.with_left_out(view, looser);
    }
    let fit = fit_half_space(&bp, None).expect("a plane fit");
    let lovo = fit
        .leave_one_view_out_rms
        .expect("left-out sets were given");
    assert!(lovo < 0.1, "leave-one-view-out rms {lovo}");
}

#[test]
fn a_slab_is_recovered_from_two_polylines() {
    let mut rng = Rng(4);
    let normal = DVec3::new(0.6, 0.0, 0.8);
    let a = plane_points(&mut rng, normal, 1.0, 30, 0.01);
    let b = plane_points(&mut rng, normal, 3.5, 30, 0.01);
    let fit = fit_slab(
        &BoundaryPoints::from_points(b),
        &BoundaryPoints::from_points(a),
    )
    .expect("a slab fit");
    match fit.shape {
        ZoneShape::Slab {
            normal: n,
            offset_min,
            offset_max,
        } => {
            assert!(n.dot(normal) > 0.9999);
            assert!((offset_min - 1.0).abs() < 0.05, "min {offset_min}");
            assert!((offset_max - 3.5).abs() < 0.05, "max {offset_max}");
        }
        other => panic!("not a slab: {other:?}"),
    }
    assert_eq!(fit.per_point_residual.len(), 60);
    assert!(fit.rms_mm < 0.03);
    let same = fit_slab(
        &BoundaryPoints::from_points(plane_points(&mut rng, normal, 1.0, 10, 0.0)),
        &BoundaryPoints::from_points(plane_points(&mut rng, normal, 1.0, 10, 0.0)),
    );
    assert!(same.is_err(), "coincident planes must be refused");
}

#[allow(
    clippy::too_many_arguments,
    reason = "a test fixture with named geometry"
)]
fn cylinder_points(
    rng: &mut Rng,
    point: DVec3,
    axis: DVec3,
    radius: f64,
    half_length: f64,
    count: usize,
    arc: (f64, f64),
    sigma: f64,
) -> Vec<DVec3> {
    let (u, v) = axis_reference(axis);
    (0..count)
        .map(|_| {
            let theta = rng.range(arc.0, arc.1);
            let h = rng.range(-half_length, half_length);
            point + axis * h + (u * theta.cos() + v * theta.sin()) * radius + rng.noise(sigma)
        })
        .collect()
}

#[test]
fn a_cylinder_with_a_free_axis_is_recovered() {
    let mut rng = Rng(6);
    let axis = DVec3::new(0.2, 0.9, -0.3).normalize();
    let point = DVec3::new(1.0, -2.0, 0.5);
    let pts = cylinder_points(&mut rng, point, axis, 3.0, 6.0, 150, (0.0, TAU), 0.01);
    let fit = fit_cylinder(&BoundaryPoints::from_points(pts), None, RadialRole::Core)
        .expect("a cylinder fit");
    match fit.shape {
        ZoneShape::CoaxialCylinder {
            axis_point,
            axis_dir,
            r_in,
            r_out,
        } => {
            assert_eq!(r_in, 0.0);
            assert!((r_out - 3.0).abs() < 0.05, "radius {r_out}");
            assert!(axis_dir.dot(axis).abs() > 0.9995, "axis {axis_dir:?}");
            assert!(line_distance(axis_point, point, axis) < 0.05);
        }
        other => panic!("not a cylinder: {other:?}"),
    }
    assert!(fit.rms_mm < 0.03);
}

#[test]
fn a_cylinder_with_a_fixed_axis_is_recovered_from_a_half_arc() {
    let mut rng = Rng(7);
    let axis = DVec3::new(0.0, 0.3, 1.0).normalize();
    let point = DVec3::new(-1.0, 0.5, 0.0);
    let pts = cylinder_points(&mut rng, point, axis, 2.5, 4.0, 80, (0.2, 0.2 + PI), 0.01);
    let fit = fit_cylinder(
        &BoundaryPoints::from_points(pts),
        Some(axis),
        RadialRole::Core,
    )
    .expect("a fixed-axis cylinder fit");
    match fit.shape {
        ZoneShape::CoaxialCylinder {
            axis_point,
            axis_dir,
            r_out,
            ..
        } => {
            assert!(axis_dir.dot(axis) > 0.999_999, "the axis stays fixed");
            assert!((r_out - 2.5).abs() < 0.05, "radius {r_out}");
            assert!(line_distance(axis_point, point, axis) < 0.05);
        }
        other => panic!("not a cylinder: {other:?}"),
    }
}

#[test]
fn the_role_beyond_gives_a_tube_wall() {
    let mut rng = Rng(8);
    let axis = DVec3::Z;
    let pts = cylinder_points(&mut rng, DVec3::ZERO, axis, 2.0, 3.0, 60, (0.0, TAU), 0.005);
    let fit = fit_cylinder(
        &BoundaryPoints::from_points(pts.clone()),
        Some(axis),
        RadialRole::Beyond { r_out: 5.0 },
    )
    .expect("a tube");
    match fit.shape {
        ZoneShape::CoaxialCylinder { r_in, r_out, .. } => {
            assert!((r_in - 2.0).abs() < 0.05 && r_out == 5.0);
        }
        other => panic!("not a cylinder: {other:?}"),
    }
    let too_small = fit_cylinder(
        &BoundaryPoints::from_points(pts),
        Some(axis),
        RadialRole::Beyond { r_out: 1.0 },
    );
    assert!(too_small.is_err());
}

#[allow(
    clippy::too_many_arguments,
    reason = "a test fixture with named geometry"
)]
fn prism_points(
    rng: &mut Rng,
    point: DVec3,
    axis: DVec3,
    sides: u32,
    apothem: f64,
    phase: f64,
    half_length: f64,
    count: usize,
    sigma: f64,
) -> Vec<DVec3> {
    let (u, v) = axis_reference(axis);
    let half_side = apothem * (PI / f64::from(sides)).tan() * 0.95;
    (0..count)
        .map(|i| {
            let k = (i as u32) % sides;
            let theta = phase + TAU * f64::from(k) / f64::from(sides);
            let normal = u * theta.cos() + v * theta.sin();
            let tangent = -u * theta.sin() + v * theta.cos();
            let s = rng.range(-half_side, half_side);
            let h = rng.range(-half_length, half_length);
            point + axis * h + normal * apothem + tangent * s + rng.noise(sigma)
        })
        .collect()
}

#[test]
fn a_hexagonal_prism_with_a_fixed_axis_recovers_apothem_and_phase() {
    let mut rng = Rng(9);
    let axis = DVec3::new(0.0, 0.3, 1.0).normalize();
    let point = DVec3::new(0.5, -0.5, 0.0);
    let pts = prism_points(&mut rng, point, axis, 6, 3.0, 0.2, 5.0, 180, 0.01);
    let fit = fit_prism(
        &BoundaryPoints::from_points(pts),
        6,
        Some(axis),
        RadialRole::Core,
    )
    .expect("a prism fit");
    match fit.shape {
        ZoneShape::CoaxialPrism {
            axis_point,
            n_sides,
            r_out,
            phase,
            ..
        } => {
            assert_eq!(n_sides, 6);
            assert!((r_out - 3.0).abs() < 0.05, "apothem {r_out}");
            assert!(
                angle_gap(phase, 0.2, TAU / 6.0) < 0.02,
                "phase {phase} against 0.2"
            );
            assert!(line_distance(axis_point, point, axis) < 0.05);
        }
        other => panic!("not a prism: {other:?}"),
    }
    assert!(fit.rms_mm < 0.03);
}

#[test]
fn a_trigonal_prism_with_a_free_axis_is_recovered() {
    let mut rng = Rng(10);
    let axis = DVec3::new(0.1, -0.2, 1.0).normalize();
    let point = DVec3::new(0.0, 0.0, 0.0);
    let pts = prism_points(&mut rng, point, axis, 3, 2.5, 1.0, 7.0, 150, 0.01);
    let fit = fit_prism(&BoundaryPoints::from_points(pts), 3, None, RadialRole::Core)
        .expect("a prism fit");
    match fit.shape {
        ZoneShape::CoaxialPrism {
            axis_point,
            axis_dir,
            r_out,
            phase,
            ..
        } => {
            assert!((r_out - 2.5).abs() < 0.05, "apothem {r_out}");
            assert!(axis_dir.dot(axis).abs() > 0.999, "axis {axis_dir:?}");
            assert!(line_distance(axis_point, point, axis) < 0.05);
            // The phase is measured in the frame of the FITTED axis, which differs from the true
            // one by a fraction of a degree: compare through the frame of the fitted axis.
            let (u, v) = axis_reference(axis_dir);
            let (tu, tv) = axis_reference(axis);
            let true_normal = tu * 1.0_f64.cos() + tv * 1.0_f64.sin();
            let true_phase = true_normal
                .dot(v)
                .atan2(true_normal.dot(u))
                .rem_euclid(TAU / 3.0);
            assert!(
                angle_gap(phase, true_phase, TAU / 3.0) < 0.03,
                "phase {phase}"
            );
        }
        other => panic!("not a prism: {other:?}"),
    }
    assert!(
        fit_prism(
            &BoundaryPoints::from_points(vec![]),
            2,
            None,
            RadialRole::Core
        )
        .is_err()
    );
}

fn sector_edge(
    rng: &mut Rng,
    point: DVec3,
    axis: DVec3,
    angle: f64,
    count: usize,
    sigma: f64,
) -> Vec<DVec3> {
    let (u, v) = axis_reference(axis);
    let along = u * angle.cos() + v * angle.sin();
    (0..count)
        .map(|_| {
            point + axis * rng.range(-4.0, 4.0) + along * rng.range(1.0, 5.0) + rng.noise(sigma)
        })
        .collect()
}

#[test]
fn a_sector_is_recovered_from_its_two_edges() {
    let mut rng = Rng(11);
    let axis = DVec3::new(0.1, -0.2, 1.0).normalize();
    let point = DVec3::new(0.5, 0.5, 0.0);
    let a = sector_edge(&mut rng, point, axis, 0.4, 50, 0.01);
    let b = sector_edge(&mut rng, point, axis, 1.9, 50, 0.01);
    let (u, v) = axis_reference(axis);
    let inside = point + (u * 1.15_f64.cos() + v * 1.15_f64.sin()) * 3.0;
    let fit = fit_sector(
        &BoundaryPoints::from_points(a.clone()),
        &BoundaryPoints::from_points(b.clone()),
        None,
        Some(inside),
    )
    .expect("a sector fit");
    match fit.shape {
        ZoneShape::Sector {
            axis_point,
            axis_dir,
            angle_from,
            angle_to,
        } => {
            assert!(axis_dir.dot(axis).abs() > 0.999, "axis {axis_dir:?}");
            assert!(line_distance(axis_point, point, axis) < 0.05);
            // The angles are measured in the frame of the fitted axis; the tilt is tiny.
            assert!(angle_gap(angle_from, 0.4, TAU) < 0.02, "from {angle_from}");
            assert!(angle_gap(angle_to, 1.9, TAU) < 0.02, "to {angle_to}");
            assert!(angle_to > angle_from && angle_to - angle_from < PI);
        }
        other => panic!("not a sector: {other:?}"),
    }
    // A hint on the other side selects the complementary wedge.
    let outside = point - (u * 1.15_f64.cos() + v * 1.15_f64.sin()) * 3.0;
    let other = fit_sector(
        &BoundaryPoints::from_points(a),
        &BoundaryPoints::from_points(b),
        Some(axis),
        Some(outside),
    )
    .expect("the complementary sector");
    match other.shape {
        ZoneShape::Sector {
            angle_from,
            angle_to,
            ..
        } => {
            assert!(
                ((angle_to - angle_from) - (TAU - 1.5)).abs() < 0.02,
                "span {}",
                angle_to - angle_from
            );
        }
        other => panic!("not a sector: {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------------------------

fn band(peak: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        550.0, 40.0, peak,
    )]))
}

fn half_space(offset: f64) -> Zone {
    Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset,
        },
        absorption: band(0.2),
    }
}

fn tube(r_in: f64, r_out: f64) -> Zone {
    Zone {
        shape: ZoneShape::CoaxialCylinder {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            r_in,
            r_out,
        },
        absorption: band(0.3),
    }
}

fn two_zones() -> ZonedAbsorption {
    let mut z = ZonedAbsorption::new(band(0.05));
    z.zones.push(half_space(0.0));
    z.zones.push(tube(0.0, 2.0));
    z
}

fn offset_of(z: &ZonedAbsorption, zone: usize) -> f64 {
    parameter_value(&z.zones[zone - 1].shape, ZoneParameter::Offset).expect("an offset")
}

#[test]
fn zones_are_added_removed_and_reordered() {
    let z = two_zones();
    let added = apply(
        &z,
        &ZoneEdit::Add {
            zone: half_space(1.0),
            position: Some(1),
        },
    )
    .expect("an insertion");
    assert_eq!(added.zones.len(), 3);
    assert_eq!(offset_of(&added, 1), 1.0);
    assert_eq!(z.zones.len(), 2, "the input is untouched");

    let removed = apply(&added, &ZoneEdit::Remove { zone: 1 }).expect("a removal");
    assert_eq!(removed, z);

    let swapped = apply(&z, &ZoneEdit::Reorder { from: 1, to: 2 }).expect("a reorder");
    assert!(matches!(
        swapped.zones[0].shape,
        ZoneShape::CoaxialCylinder { .. }
    ));
    assert!(matches!(
        swapped.zones[1].shape,
        ZoneShape::HalfSpace { .. }
    ));

    assert!(matches!(
        apply(&z, &ZoneEdit::Remove { zone: 0 }),
        Err(ZoneEditError::BaseHasNoGeometry)
    ));
    assert!(matches!(
        apply(&z, &ZoneEdit::Remove { zone: 3 }),
        Err(ZoneEditError::NoSuchZone { zone: 3 })
    ));
    assert!(matches!(
        apply(
            &z,
            &ZoneEdit::Add {
                zone: half_space(0.0),
                position: Some(9)
            }
        ),
        Err(ZoneEditError::BadPosition { position: 9 })
    ));
    assert!(matches!(
        apply(&z, &ZoneEdit::Reorder { from: 1, to: 3 }),
        Err(ZoneEditError::BadPosition { position: 3 })
    ));
}

#[test]
fn a_fifth_zone_is_refused() {
    let mut z = ZonedAbsorption::new(band(0.05));
    for i in 0..4 {
        z = apply(
            &z,
            &ZoneEdit::Add {
                zone: half_space(f64::from(i)),
                position: None,
            },
        )
        .expect("up to four zones");
    }
    assert!(matches!(
        apply(
            &z,
            &ZoneEdit::Add {
                zone: half_space(9.0),
                position: None
            }
        ),
        Err(ZoneEditError::Invalid(ZoningError::TooManyZones { .. }))
    ));
}

#[test]
fn parameters_are_set_and_validated() {
    let z = two_zones();
    let moved = apply(
        &z,
        &ZoneEdit::SetParameter {
            zone: 1,
            parameter: ZoneParameter::Offset,
            value: 0.75,
        },
    )
    .expect("an offset");
    assert_eq!(offset_of(&moved, 1), 0.75);
    assert!(matches!(
        apply(
            &z,
            &ZoneEdit::SetParameter {
                zone: 1,
                parameter: ZoneParameter::ROut,
                value: 1.0
            }
        ),
        Err(ZoneEditError::NoSuchParameter { zone: 1, .. })
    ));
    assert!(matches!(
        apply(
            &z,
            &ZoneEdit::SetParameter {
                zone: 1,
                parameter: ZoneParameter::Offset,
                value: f64::NAN
            }
        ),
        Err(ZoneEditError::NotFinite)
    ));
    // An inner radius above the outer one fails the kernels' validation and changes nothing.
    assert!(matches!(
        apply(
            &z,
            &ZoneEdit::SetParameter {
                zone: 2,
                parameter: ZoneParameter::RIn,
                value: 5.0
            }
        ),
        Err(ZoneEditError::Invalid(ZoningError::BadRadii { zone: 2 }))
    ));
    assert_eq!(z, two_zones());
}

#[test]
fn directions_points_absorption_and_softness_are_edited() {
    let z = two_zones();
    let tilted = apply(
        &z,
        &ZoneEdit::SetDirection {
            zone: 1,
            direction: DVec3::new(0.0, 0.0, 2.0),
        },
    )
    .expect("a direction");
    match tilted.zones[0].shape {
        ZoneShape::HalfSpace { normal, .. } => assert!((normal - DVec3::Z).length() < 1e-12),
        _ => panic!("not a half space"),
    }
    assert!(matches!(
        apply(
            &z,
            &ZoneEdit::SetDirection {
                zone: 1,
                direction: DVec3::ZERO
            }
        ),
        Err(ZoneEditError::NotFinite)
    ));
    let shifted = apply(
        &z,
        &ZoneEdit::SetAxisPoint {
            zone: 2,
            point: DVec3::new(1.0, 2.0, 3.0),
        },
    )
    .expect("an axis point");
    assert!(matches!(
        shifted.zones[1].shape,
        ZoneShape::CoaxialCylinder { axis_point, .. } if axis_point == DVec3::new(1.0, 2.0, 3.0)
    ));
    assert!(matches!(
        apply(
            &z,
            &ZoneEdit::SetAxisPoint {
                zone: 1,
                point: DVec3::ONE
            }
        ),
        Err(ZoneEditError::NoSuchParameter { .. })
    ));
    let recoloured = apply(
        &z,
        &ZoneEdit::SetAbsorption {
            zone: 0,
            absorption: band(0.4),
        },
    )
    .expect("a base absorption");
    assert_eq!(recoloured.base, band(0.4));
    let soft = apply(&z, &ZoneEdit::SetSoftness { millimetres: 0.3 }).expect("softness");
    assert!((soft.boundary_softness_mm - 0.3).abs() < 1e-6);
    assert!(matches!(
        apply(&z, &ZoneEdit::SetSoftness { millimetres: -1.0 }),
        Err(ZoneEditError::Invalid(ZoningError::BadSoftness))
    ));
}

#[test]
fn locks_block_edits_and_follow_the_zones() {
    let z = two_zones();
    let locks = ZoneLocks::new();
    let (z, locks) = apply_with_locks(
        &z,
        &locks,
        &ZoneEdit::Lock {
            zone: 2,
            parameter: ZoneParameter::ROut,
        },
    )
    .expect("a lock");
    assert!(locks.is_locked(2, ZoneParameter::ROut));
    assert!(matches!(
        apply_with_locks(
            &z,
            &locks,
            &ZoneEdit::SetParameter {
                zone: 2,
                parameter: ZoneParameter::ROut,
                value: 3.0
            }
        ),
        Err(ZoneEditError::Locked { zone: 2, .. })
    ));
    // Locking something the shape lacks is an error.
    assert!(
        apply_with_locks(
            &z,
            &locks,
            &ZoneEdit::Lock {
                zone: 1,
                parameter: ZoneParameter::ROut
            }
        )
        .is_err()
    );
    // Removing zone 1 moves the lock of zone 2 down to zone 1.
    let (removed, after) =
        apply_with_locks(&z, &locks, &ZoneEdit::Remove { zone: 1 }).expect("a removal");
    assert_eq!(removed.zones.len(), 1);
    assert!(after.is_locked(1, ZoneParameter::ROut) && !after.is_locked(2, ZoneParameter::ROut));
    // Inserting before it moves it up; reordering follows the zone.
    let (inserted, up) = apply_with_locks(
        &z,
        &locks,
        &ZoneEdit::Add {
            zone: half_space(5.0),
            position: Some(1),
        },
    )
    .expect("an insertion");
    assert_eq!(inserted.zones.len(), 3);
    assert!(up.is_locked(3, ZoneParameter::ROut));
    let (_, swapped) =
        apply_with_locks(&z, &locks, &ZoneEdit::Reorder { from: 2, to: 1 }).expect("a reorder");
    assert!(swapped.is_locked(1, ZoneParameter::ROut));
    // A shape replacement that changes the locked radius is refused, one that keeps it is not.
    assert!(matches!(
        apply_with_locks(
            &z,
            &locks,
            &ZoneEdit::SetShape {
                zone: 2,
                shape: tube(0.0, 2.5).shape
            }
        ),
        Err(ZoneEditError::Locked { .. })
    ));
    assert!(
        apply_with_locks(
            &z,
            &locks,
            &ZoneEdit::SetShape {
                zone: 2,
                shape: tube(0.5, 2.0).shape
            }
        )
        .is_ok()
    );
    let (_, released) = apply_with_locks(
        &z,
        &locks,
        &ZoneEdit::Unlock {
            zone: 2,
            parameter: ZoneParameter::ROut,
        },
    )
    .expect("an unlock");
    assert!(released.is_empty());
    // Plain apply has no locks: a lock edit returns the zoning unchanged.
    let same = apply(
        &z,
        &ZoneEdit::Lock {
            zone: 2,
            parameter: ZoneParameter::ROut,
        },
    )
    .expect("a lock edit without locks");
    assert_eq!(same, z);
}

// ---------------------------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------------------------

fn front_pose() -> ViewPose {
    ViewPose::look_at(
        "front",
        DVec3::new(0.0, -100.0, 0.0),
        DVec3::ZERO,
        DVec3::Z,
        Projection::Orthographic { px_per_mm: 10.0 },
        [200, 200],
    )
}

fn oblique_pose() -> ViewPose {
    ViewPose::look_at(
        "oblique",
        DVec3::new(-70.0, -70.0, 30.0),
        DVec3::ZERO,
        DVec3::Z,
        Projection::Orthographic { px_per_mm: 10.0 },
        [200, 200],
    )
}

fn bicolour(offset: f64) -> ZonedAbsorption {
    let mut z = ZonedAbsorption::new(band(0.05));
    z.zones.push(half_space(offset));
    z
}

#[test]
fn a_half_space_meets_a_cube_in_a_closed_square_loop() {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a cube");
    let traces = surface_traces(&bicolour(0.0), &mesh, &OverlayOptions::default());
    assert_eq!(traces.len(), 1, "one loop");
    let trace = &traces[0];
    assert!(trace.closed);
    assert!(trace.points.len() >= 4);
    for p in &trace.points {
        assert!(p.x.abs() < 1e-9, "off the plane: {p:?}");
        assert!(
            (p.y.abs().max(p.z.abs()) - 5.0).abs() < 1e-9,
            "off the surface: {p:?}"
        );
    }
    assert_eq!(trace.normals.len(), trace.points.len());
}

#[test]
fn the_overlay_of_a_front_view_is_a_straight_line_at_the_expected_pixels() {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a cube");
    let rig = RigProfile::new("rig", vec![front_pose(), oblique_pose()], 1.5);
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let overlay = project_overlay(&bicolour(0.0), &scene, &OverlayOptions::default());
    assert_eq!(overlay.len(), 2);
    let front = &overlay[0];
    assert_eq!(front.polylines.len(), 1, "only the front face is drawn");
    let line = &front.polylines[0];
    assert!(!line.closed && line.zone == 1);
    for [u, _] in &line.pixels {
        assert!((u - 100.0).abs() < 1e-9, "u = {u}");
    }
    let v_min = line
        .pixels
        .iter()
        .map(|p| p[1])
        .fold(f64::INFINITY, f64::min);
    let v_max = line
        .pixels
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (v_min - 50.0).abs() < 1e-9 && (v_max - 150.0).abs() < 1e-9,
        "{v_min} {v_max}"
    );

    // Without the facing test the whole loop is drawn, the top and bottom edges on the same u.
    let all = project_overlay(
        &bicolour(0.0),
        &scene,
        &OverlayOptions {
            front_only: false,
            ..OverlayOptions::default()
        },
    );
    let drawn: usize = all[0].polylines.iter().map(|p| p.pixels.len()).sum();
    assert!(drawn >= 4);
    for line in &all[0].polylines {
        for [u, _] in &line.pixels {
            assert!((u - 100.0).abs() < 1e-9);
        }
    }
}

#[test]
fn an_oblique_overlay_projects_the_traced_points_through_the_camera() {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a cube");
    let rig = RigProfile::new("rig", vec![oblique_pose()], 1.5);
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let zoned = bicolour(0.0);
    let overlay = project_overlay(&zoned, &scene, &OverlayOptions::default());
    let traces = surface_traces(&zoned, &mesh, &OverlayOptions::default());
    let expected: Vec<[f64; 2]> = traces
        .iter()
        .flat_map(|t| t.points.iter())
        .map(|p| oblique_pose().project(*p).expect("in front").to_array())
        .collect();
    assert_ne!(
        overlay[0].polylines,
        [] as [crate::rough_plan::colour_fit::zones::overlay::OverlayPolyline; 0]
    );
    for line in &overlay[0].polylines {
        for pixel in &line.pixels {
            assert!(
                expected
                    .iter()
                    .any(|e| (e[0] - pixel[0]).abs() < 1e-9 && (e[1] - pixel[1]).abs() < 1e-9),
                "{pixel:?} is not the projection of a traced vertex"
            );
        }
    }
}

#[test]
fn a_cylinder_meets_the_cube_faces_in_circles() {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a cube");
    let mut z = ZonedAbsorption::new(band(0.05));
    z.zones.push(tube(0.0, 3.0));
    let traces = surface_traces(&z, &mesh, &OverlayOptions::default());
    assert!(traces.len() >= 2);
    for trace in &traces {
        for p in &trace.points {
            assert!((p.z.abs() - 5.0).abs() < 1e-9, "not on a cap: {p:?}");
            assert!(
                (p.x.hypot(p.y) - 3.0).abs() < 0.03,
                "radius {}",
                p.x.hypot(p.y)
            );
        }
    }
}

#[test]
fn a_sector_meets_the_cube_along_its_two_half_planes() {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a cube");
    let mut z = ZonedAbsorption::new(band(0.05));
    z.zones.push(Zone {
        shape: ZoneShape::Sector {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            angle_from: 0.0,
            angle_to: PI / 2.0,
        },
        absorption: band(0.2),
    });
    let traces = surface_traces(&z, &mesh, &OverlayOptions::default());
    assert!(traces.iter().any(|t| t.boundary == 0) && traces.iter().any(|t| t.boundary == 1));
    for trace in &traces {
        for p in &trace.points {
            if trace.boundary == 0 {
                assert!(p.y.abs() < 1e-9 && p.x > -1e-9, "edge 0: {p:?}");
            } else {
                assert!(p.x.abs() < 1e-9 && p.y > -1e-9, "edge 1: {p:?}");
            }
        }
    }
}

#[test]
fn a_mesh_shell_meets_the_cube_in_a_square() {
    let cube = box_mesh(DVec3::splat(5.0)).expect("a cube");
    let shell_mesh = box_mesh(DVec3::splat(2.5)).expect("a shell");
    let mut z = ZonedAbsorption::new(band(0.05));
    z.zones.push(Zone {
        shape: ZoneShape::MeshShell {
            vertices: shell_mesh
                .vertices()
                .iter()
                .map(|v| (*v + DVec3::new(4.0, 0.0, 0.0)).to_array())
                .collect(),
            triangles: shell_mesh.triangles().to_vec(),
        },
        absorption: band(0.2),
    });
    let traces = surface_traces(&z, &cube, &OverlayOptions::default());
    assert_ne!(
        traces,
        [] as [crate::rough_plan::colour_fit::zones::overlay::SurfaceTrace; 0]
    );
    let mut max_y = 0.0_f64;
    let mut max_z = 0.0_f64;
    for trace in &traces {
        for p in &trace.points {
            assert!((p.x - 5.0).abs() < 1e-9, "not on the face x = 5: {p:?}");
            assert!(p.y.abs() <= 2.5 + 1e-9 && p.z.abs() <= 2.5 + 1e-9);
            max_y = max_y.max(p.y.abs());
            max_z = max_z.max(p.z.abs());
        }
    }
    assert!((max_y - 2.5).abs() < 1e-9 && (max_z - 2.5).abs() < 1e-9);
}

// ---------------------------------------------------------------------------------------------
// Refinement loop (analytic residual)
// ---------------------------------------------------------------------------------------------

/// A residual vector `tanh(g_k (parameter - truth))` for the offset of zone 1 (and the two
/// offsets of a slab in zone 1 when present).
fn analytic(
    truths: &[(ZoneParameter, f64)],
) -> impl FnMut(&ZonedAbsorption) -> Result<Evaluation, RefineError> {
    let truths = truths.to_vec();
    move |zoned: &ZonedAbsorption| {
        let mut residuals = Vec::new();
        for (parameter, truth) in &truths {
            let value = parameter_value(&zoned.zones[0].shape, *parameter).expect("a parameter");
            for k in 0..40 {
                let g = 0.5 + 0.05 * f64::from(k);
                residuals.push((g * (value - truth)).tanh());
            }
        }
        let valid = vec![true; residuals.len()];
        Ok(Evaluation {
            residuals,
            valid,
            fit: None,
        })
    }
}

#[test]
fn the_refinement_moves_a_boundary_offset_onto_the_truth() {
    let start = bicolour(0.0);
    let mut evaluate = analytic(&[(ZoneParameter::Offset, 0.3)]);
    let mut reports = 0;
    let result = refine_with(
        &start,
        &ZoneLocks::new(),
        &RefineOptions {
            step_mm: 0.1,
            ..RefineOptions::default()
        },
        &AtomicBool::new(false),
        &mut |_| reports += 1,
        &mut evaluate,
    )
    .expect("the refinement runs");
    let offset = offset_of(&result.zoned, 1);
    assert!((offset - 0.3).abs() < 0.02, "offset {offset}");
    assert!(result.chi2_after < 1e-3 * result.chi2_before);
    assert!(result.iterations >= 1 && result.iterations <= 3);
    assert_eq!(result.moves.len(), 1);
    assert_eq!(result.moves[0].before, 0.0);
    assert!(reports >= result.evaluations);
    assert_eq!(offset_of(&start, 1), 0.0, "the input is untouched");
}

#[test]
fn the_refinement_moves_both_planes_of_a_slab_and_respects_locks() {
    let mut start = ZonedAbsorption::new(band(0.05));
    start.zones.push(Zone {
        shape: ZoneShape::Slab {
            normal: DVec3::X,
            offset_min: -0.8,
            offset_max: 2.3,
        },
        absorption: band(0.2),
    });
    let truths = [
        (ZoneParameter::OffsetMin, -1.0),
        (ZoneParameter::OffsetMax, 2.0),
    ];
    let mut evaluate = analytic(&truths);
    let result = refine_with(
        &start,
        &ZoneLocks::new(),
        &RefineOptions {
            step_mm: 0.1,
            ..RefineOptions::default()
        },
        &AtomicBool::new(false),
        &mut |_| {},
        &mut evaluate,
    )
    .expect("the refinement runs");
    let get = |p| parameter_value(&result.zoned.zones[0].shape, p).expect("a parameter");
    assert!((get(ZoneParameter::OffsetMin) + 1.0).abs() < 0.03);
    assert!((get(ZoneParameter::OffsetMax) - 2.0).abs() < 0.03);

    // With the upper plane locked only the lower one moves.
    let (_, locks) = apply_with_locks(
        &start,
        &ZoneLocks::new(),
        &ZoneEdit::Lock {
            zone: 1,
            parameter: ZoneParameter::OffsetMax,
        },
    )
    .expect("a lock");
    let mut evaluate = analytic(&truths);
    let result = refine_with(
        &start,
        &locks,
        &RefineOptions {
            step_mm: 0.1,
            ..RefineOptions::default()
        },
        &AtomicBool::new(false),
        &mut |_| {},
        &mut evaluate,
    )
    .expect("the refinement runs");
    let get = |p| parameter_value(&result.zoned.zones[0].shape, p).expect("a parameter");
    assert!((get(ZoneParameter::OffsetMin) + 1.0).abs() < 0.03);
    assert_eq!(get(ZoneParameter::OffsetMax), 2.3, "the locked plane stays");
}

#[test]
fn the_refinement_reports_nothing_to_do_and_honours_cancel() {
    let start = bicolour(0.0);
    let (_, locks) = apply_with_locks(
        &start,
        &ZoneLocks::new(),
        &ZoneEdit::Lock {
            zone: 1,
            parameter: ZoneParameter::Offset,
        },
    )
    .expect("a lock");
    let mut evaluate = analytic(&[(ZoneParameter::Offset, 0.3)]);
    assert!(matches!(
        refine_with(
            &start,
            &locks,
            &RefineOptions::default(),
            &AtomicBool::new(false),
            &mut |_| {},
            &mut evaluate
        ),
        Err(RefineError::NoFreeParameters)
    ));
    assert!(matches!(
        refine_with(
            &start,
            &ZoneLocks::new(),
            &RefineOptions::default(),
            &AtomicBool::new(true),
            &mut |_| {},
            &mut evaluate
        ),
        Err(RefineError::Cancelled)
    ));
    assert!(matches!(
        refine_with(
            &start,
            &ZoneLocks::new(),
            &RefineOptions {
                step_mm: 0.0,
                ..RefineOptions::default()
            },
            &AtomicBool::new(false),
            &mut |_| {},
            &mut evaluate
        ),
        Err(RefineError::BadOptions)
    ));
}
