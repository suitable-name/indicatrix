//! Tests for the zoning types and path-length kernels (written, not run, by lane K1).
//!
//! Property tests use fixed seeds (a splitmix64 generator, no dependency).

use super::{
    MAX_MESH_TRIANGLES, MAX_ZONES, MeshProblem, PleochroicMode, Zone, ZoneAbsorption, ZoneFrame,
    ZoneKernel, ZoneShape, ZonedAbsorption, ZoningError, kernels::perp_basis,
    segment_optical_depth, zone_lengths, zone_lengths_f32,
};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::AbsorptionUnit,
};
use glam::{DQuat, DVec3, Vec3};
use std::f64::consts::{PI, TAU};

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        (hi - lo).mul_add(self.unit(), lo)
    }

    fn point(&mut self, half: f64) -> DVec3 {
        DVec3::new(
            self.range(-half, half),
            self.range(-half, half),
            self.range(-half, half),
        )
    }

    fn direction(&mut self) -> DVec3 {
        loop {
            let v = self.point(1.0);
            let l = v.length();
            if l > 0.1 && l <= 1.0 {
                return v / l;
            }
        }
    }
}

fn unit(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z).normalize()
}

fn absorb(peak: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        550.0, 1.0e6, peak,
    )]))
}

fn test_frame() -> ZoneFrame {
    ZoneFrame {
        rotation: DQuat::from_axis_angle(unit(1.0, 2.0, 3.0), 0.7),
        translation: DVec3::new(0.3, -0.2, 0.5),
    }
}

fn build(shapes: Vec<ZoneShape>, softness: f32) -> ZonedAbsorption {
    let mut z = ZonedAbsorption::new(absorb(0.1));
    z.frame = test_frame();
    z.boundary_softness_mm = softness;
    for (i, shape) in shapes.into_iter().enumerate() {
        z.zones.push(Zone {
            shape,
            absorption: absorb(0.2 * (i + 1) as f32),
        });
    }
    assert_eq!(z.validate(), Ok(()), "fixture must validate");
    z
}

fn one_zone(shape: ZoneShape) -> ZonedAbsorption {
    let mut z = ZonedAbsorption::new(absorb(0.1));
    z.zones.push(Zone {
        shape,
        absorption: absorb(0.2),
    });
    z
}

fn cube_parts(centre: DVec3, half: f64) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
    let mut vertices = Vec::new();
    for i in 0..8u32 {
        let sx = if i & 1 == 0 { -1.0 } else { 1.0 };
        let sy = if i & 2 == 0 { -1.0 } else { 1.0 };
        let sz = if i & 4 == 0 { -1.0 } else { 1.0 };
        // Index bits: x = bit 0, y = bit 1, z = bit 2 -> v0(-,-,-) v1(+,-,-) v2(-,+,-) ...
        vertices.push([
            f64::mul_add(sx, half, centre.x),
            f64::mul_add(sy, half, centre.y),
            f64::mul_add(sz, half, centre.z),
        ]);
    }
    // The vertex order above is (x fastest); the triangle list below is written for the
    // order v0(-,-,-) v1(+,-,-) v2(+,+,-) v3(-,+,-) v4(-,-,+) v5(+,-,+) v6(+,+,+) v7(-,+,+),
    // so re-order the vertices to that.
    let order = [0usize, 1, 3, 2, 4, 5, 7, 6];
    let vertices: Vec<[f64; 3]> = order.iter().map(|&i| vertices[i]).collect();
    let triangles = vec![
        [0, 3, 2],
        [0, 2, 1],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 7, 6],
        [3, 6, 2],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    (vertices, triangles)
}

fn cube_shape(centre: DVec3, half: f64) -> ZoneShape {
    let (vertices, triangles) = cube_parts(centre, half);
    ZoneShape::MeshShell {
        vertices,
        triangles,
    }
}

fn analytic_shapes() -> Vec<(&'static str, Vec<ZoneShape>)> {
    let cylinder = ZoneShape::CoaxialCylinder {
        axis_point: DVec3::new(0.3, 0.1, -0.2),
        axis_dir: unit(0.2, -0.5, 0.9),
        r_in: 0.5,
        r_out: 1.4,
    };
    vec![
        (
            "half space",
            vec![ZoneShape::HalfSpace {
                normal: unit(0.3, 0.5, 0.8),
                offset: 0.2,
            }],
        ),
        (
            "slab",
            vec![ZoneShape::Slab {
                normal: unit(-0.6, 0.2, 0.7),
                offset_min: -0.4,
                offset_max: 0.9,
            }],
        ),
        ("tube", vec![cylinder]),
        (
            "prism 3",
            vec![ZoneShape::CoaxialPrism {
                axis_point: DVec3::new(0.1, -0.2, 0.0),
                axis_dir: unit(-0.4, 0.3, 0.8),
                n_sides: 3,
                r_in: 0.0,
                r_out: 1.0,
                phase: 0.4,
            }],
        ),
        (
            "prism 6 tube",
            vec![ZoneShape::CoaxialPrism {
                axis_point: DVec3::ZERO,
                axis_dir: unit(0.7, 0.1, -0.3),
                n_sides: 6,
                r_in: 0.5,
                r_out: 1.5,
                phase: 0.1,
            }],
        ),
        (
            "sector narrow",
            vec![ZoneShape::Sector {
                axis_point: DVec3::ZERO,
                axis_dir: unit(0.1, 0.9, 0.4),
                angle_from: 0.3,
                angle_to: 1.3,
            }],
        ),
        (
            "sector wide",
            vec![ZoneShape::Sector {
                axis_point: DVec3::new(0.2, 0.0, 0.1),
                axis_dir: unit(0.5, -0.4, 0.6),
                angle_from: -1.0,
                angle_to: 3.2,
            }],
        ),
        (
            "three overlapping",
            vec![
                ZoneShape::HalfSpace {
                    normal: unit(0.1, 0.2, 1.0),
                    offset: -0.3,
                },
                ZoneShape::CoaxialCylinder {
                    axis_point: DVec3::ZERO,
                    axis_dir: unit(1.0, 0.3, 0.2),
                    r_in: 0.0,
                    r_out: 1.2,
                },
                ZoneShape::Slab {
                    normal: unit(0.0, 1.0, 0.2),
                    offset_min: -0.5,
                    offset_max: 0.6,
                },
            ],
        ),
    ]
}

/// The fixtures; mesh fixtures only when sharp (a mesh shell has sharp edges only).
fn fixtures(softness: f32) -> Vec<(&'static str, ZonedAbsorption)> {
    let mut out: Vec<(&'static str, ZonedAbsorption)> = analytic_shapes()
        .into_iter()
        .map(|(name, shapes)| (name, build(shapes, softness)))
        .collect();
    if softness == 0.0 {
        let cube = cube_shape(DVec3::new(0.2, -0.1, 0.3), 1.1);
        out.push(("cube shell", build(vec![cube.clone()], 0.0)));
        out.push((
            "cube shell and tube",
            build(
                vec![
                    cube,
                    ZoneShape::CoaxialCylinder {
                        axis_point: DVec3::ZERO,
                        axis_dir: unit(0.3, 0.3, 1.0),
                        r_in: 0.0,
                        r_out: 0.8,
                    },
                ],
                0.0,
            ),
        ));
    }
    out
}

fn assert_close(name: &str, got: &[f64], want: &[f64], tol: f64) {
    for (k, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() <= tol,
            "{name}: zone {k}: got {g}, want {w} (tol {tol})"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Brute-force reference (independent of the kernels' interval algebra)
// ---------------------------------------------------------------------------------------------

fn depth_of(shape: &ZoneShape, local: DVec3, sharp: bool) -> f64 {
    match shape {
        ZoneShape::HalfSpace { normal, offset } => normal.dot(local) - offset,
        ZoneShape::Slab {
            normal,
            offset_min,
            offset_max,
        } => {
            let along = normal.dot(local);
            (along - offset_min).min(offset_max - along)
        }
        ZoneShape::CoaxialCylinder {
            axis_point,
            axis_dir,
            r_in,
            r_out,
        } => {
            let rel = local - *axis_point;
            let radius = (rel - *axis_dir * rel.dot(*axis_dir)).length();
            let outer = r_out - radius;
            if *r_in > 0.0 {
                outer.min(radius - r_in)
            } else {
                outer
            }
        }
        ZoneShape::CoaxialPrism {
            axis_point,
            axis_dir,
            n_sides,
            r_in,
            r_out,
            phase,
        } => {
            let (basis_u, basis_v) = perp_basis(*axis_dir);
            let rel = local - *axis_point;
            let (qx, qy) = (rel.dot(basis_u), rel.dot(basis_v));
            let mut support = f64::NEG_INFINITY;
            for k in 0..*n_sides {
                let side_angle = phase + TAU * f64::from(k) / f64::from(*n_sides);
                support = support.max(side_angle.sin().mul_add(qy, side_angle.cos() * qx));
            }
            let outer = r_out - support;
            if *r_in > 0.0 {
                outer.min(support - r_in)
            } else {
                outer
            }
        }
        ZoneShape::Sector {
            axis_point,
            axis_dir,
            angle_from,
            angle_to,
        } => {
            let (basis_u, basis_v) = perp_basis(*axis_dir);
            let rel = local - *axis_point;
            let (qx, qy) = (rel.dot(basis_u), rel.dot(basis_v));
            let span = angle_to - angle_from;
            if span >= TAU - 1e-12 {
                return 1.0;
            }
            if sharp {
                let ang = (qy.atan2(qx) - angle_from).rem_euclid(TAU);
                return if ang <= span { 1.0 } else { -1.0 };
            }
            let (e0x, e0y) = (angle_from.cos(), angle_from.sin());
            let (e1x, e1y) = (angle_to.cos(), angle_to.sin());
            if span <= PI {
                f64::mul_add(e0y, -qx, e0x * qy).min(f64::mul_add(qy, -e1x, qx * e1y))
            } else {
                -(f64::mul_add(e1y, -qx, e1x * qy).min(f64::mul_add(qy, -e0x, qx * e0y)))
            }
        }
        ZoneShape::MeshShell { vertices, .. } => {
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for vert in vertices {
                for i in 0..3 {
                    lo[i] = lo[i].min(vert[i]);
                    hi[i] = hi[i].max(vert[i]);
                }
            }
            let inside = (0..3).all(|i| local[i] >= lo[i] && local[i] <= hi[i]);
            if inside { 1.0 } else { -1.0 }
        }
    }
}

fn smooth(sd: f64, width: f64) -> f64 {
    if width > 0.0 {
        let x = (sd / width + 0.5).clamp(0.0, 1.0);
        x * x * 2.0f64.mul_add(-x, 3.0)
    } else if sd >= 0.0 {
        1.0
    } else {
        0.0
    }
}

fn brute_weights(z: &ZonedAbsorption, p: DVec3) -> [f64; MAX_ZONES + 1] {
    let local = z.frame.inverse_point(p);
    let width = f64::from(z.boundary_softness_mm);
    let mut out = [0.0; MAX_ZONES + 1];
    let mut rest = 1.0;
    for j in (0..z.zones.len()).rev() {
        let weight = smooth(depth_of(&z.zones[j].shape, local, width == 0.0), width);
        out[j + 1] = weight * rest;
        rest *= 1.0 - weight;
    }
    out[0] = rest;
    out
}

fn brute_lengths(
    z: &ZonedAbsorption,
    start: DVec3,
    end: DVec3,
    steps: usize,
) -> [f64; MAX_ZONES + 1] {
    let mut acc = [0.0; MAX_ZONES + 1];
    for i in 0..steps {
        let t = (i as f64 + 0.5) / steps as f64;
        let weights = brute_weights(z, start + (end - start) * t);
        for k in 0..=MAX_ZONES {
            acc[k] += weights[k];
        }
    }
    let scale = (end - start).length() / steps as f64;
    for v in &mut acc {
        *v *= scale;
    }
    acc
}

// ---------------------------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------------------------

#[test]
fn lengths_are_non_negative_and_sum_to_the_segment_length() {
    for softness in [0.0_f32, 0.15] {
        for (name, z) in fixtures(softness) {
            let mut rng = Rng(11);
            for _ in 0..200 {
                let a = rng.point(2.5);
                let b = rng.point(2.5);
                let l = zone_lengths(&z, a, b);
                let total = (b - a).length();
                assert!(
                    l.iter().all(|&x| x >= 0.0),
                    "{name} (softness {softness}): negative length {l:?}"
                );
                let sum: f64 = l.iter().sum();
                assert!(
                    (sum - total).abs() <= 1e-12 * total,
                    "{name} (softness {softness}): sum {sum} vs segment {total}"
                );
            }
        }
    }
}

#[test]
fn unused_zone_slots_stay_zero() {
    for (name, z) in fixtures(0.0) {
        let mut rng = Rng(5);
        for _ in 0..50 {
            let l = zone_lengths(&z, rng.point(2.5), rng.point(2.5));
            for (k, v) in l.iter().enumerate().skip(z.zones.len() + 1) {
                assert_eq!(*v, 0.0, "{name}: slot {k}");
            }
        }
    }
}

#[test]
fn lengths_are_invariant_under_a_rigid_motion_of_segment_and_zone_frame() {
    for softness in [0.0_f32, 0.15] {
        for (name, z) in fixtures(softness) {
            let mut rng = Rng(23);
            for _ in 0..50 {
                let rigid = ZoneFrame {
                    rotation: DQuat::from_axis_angle(rng.direction(), rng.range(0.0, TAU)),
                    translation: rng.point(3.0),
                };
                let moved = z.transformed(&rigid);
                let a = rng.point(2.5);
                let b = rng.point(2.5);
                let before = zone_lengths(&z, a, b);
                let after = zone_lengths(&moved, rigid.point(a), rigid.point(b));
                assert_close(
                    &format!("{name} (softness {softness})"),
                    &after,
                    &before,
                    1e-8,
                );
            }
        }
    }
}

#[test]
fn translating_only_the_zone_frame_shifts_the_zones_not_the_total() {
    // Pure translation of segment and frame together.
    for (name, z) in fixtures(0.0) {
        let mut rng = Rng(31);
        for _ in 0..30 {
            let shift = ZoneFrame {
                rotation: DQuat::IDENTITY,
                translation: rng.point(4.0),
            };
            let moved = z.transformed(&shift);
            let a = rng.point(2.5);
            let b = rng.point(2.5);
            assert_close(
                name,
                &zone_lengths(&moved, shift.point(a), shift.point(b)),
                &zone_lengths(&z, a, b),
                1e-8,
            );
        }
    }
}

#[test]
fn reversing_the_segment_gives_the_same_lengths() {
    for softness in [0.0_f32, 0.15] {
        for (name, z) in fixtures(softness) {
            let mut rng = Rng(41);
            for _ in 0..100 {
                let a = rng.point(2.5);
                let b = rng.point(2.5);
                assert_close(
                    &format!("{name} (softness {softness})"),
                    &zone_lengths(&z, b, a),
                    &zone_lengths(&z, a, b),
                    1e-10,
                );
            }
        }
    }
}

#[test]
fn agrees_with_brute_force_midpoint_sampling() {
    // 400_000 midpoints: each boundary crossing costs at most half a step, 1.25e-6 of the
    // segment, so even eight crossings stay under the 1e-5 relative (to the segment) bound.
    for softness in [0.0_f32, 0.15] {
        for (name, z) in fixtures(softness) {
            let mut rng = Rng(77);
            for _ in 0..3 {
                let a = rng.point(2.5);
                let b = rng.point(2.5);
                let exact = zone_lengths(&z, a, b);
                let brute = brute_lengths(&z, a, b, 400_000);
                assert_close(
                    &format!("{name} (softness {softness})"),
                    &exact,
                    &brute,
                    1e-5 * (b - a).length(),
                );
            }
        }
    }
}

#[test]
fn f32_twin_matches_f64_within_1e_4() {
    for softness in [0.0_f32, 0.15] {
        for (name, z) in fixtures(softness) {
            let Some(_) = zone_lengths_f32(&z, Vec3::ZERO, Vec3::X) else {
                continue; // mesh shells are f64 only
            };
            let mut rng = Rng(59);
            for _ in 0..100 {
                // Round the endpoints to f32 first so both precisions see the same segment.
                let a = rng.point(2.5).as_vec3();
                let b = rng.point(2.5).as_vec3();
                let l32 = zone_lengths_f32(&z, a, b).expect("no mesh");
                let l64 = zone_lengths(&z, a.as_dvec3(), b.as_dvec3());
                let got: Vec<f64> = l32.iter().map(|&x| f64::from(x)).collect();
                assert_close(
                    &format!("{name} (softness {softness})"),
                    &got,
                    &l64,
                    1e-4 * f64::from((b - a).length()),
                );
            }
        }
    }
}

#[test]
fn f32_twin_refuses_mesh_shells() {
    let z = build(vec![cube_shape(DVec3::ZERO, 1.0)], 0.0);
    assert!(zone_lengths_f32(&z, Vec3::ZERO, Vec3::X).is_none());
}

#[test]
fn tiny_softness_converges_to_the_sharp_result() {
    for (name, sharp) in fixtures(0.0) {
        if sharp
            .zones
            .iter()
            .any(|zone| matches!(zone.shape, ZoneShape::MeshShell { .. }))
        {
            continue;
        }
        let mut soft = sharp.clone();
        soft.boundary_softness_mm = 1e-6;
        let mut rng = Rng(67);
        for _ in 0..20 {
            let a = rng.point(2.5);
            let b = rng.point(2.5);
            assert_close(
                name,
                &zone_lengths(&soft, a, b),
                &zone_lengths(&sharp, a, b),
                1e-5 * (b - a).length(),
            );
        }
    }
}

#[test]
fn scaling_the_zoning_scales_the_lengths() {
    let factor = 1.7;
    for softness in [0.0_f32, 0.15] {
        for (name, z) in fixtures(softness) {
            let scaled = z.scaled(factor);
            assert_eq!(scaled.validate(), Ok(()), "{name}");
            let mut rng = Rng(83);
            for _ in 0..30 {
                let a = rng.point(2.5);
                let b = rng.point(2.5);
                let base = zone_lengths(&z, a, b);
                let want: Vec<f64> = base.iter().map(|v| v * factor).collect();
                assert_close(
                    &format!("{name} (softness {softness})"),
                    &zone_lengths(&scaled, a * factor, b * factor),
                    &want,
                    1e-6,
                );
            }
        }
    }
}

#[test]
fn a_kernel_built_once_equals_the_one_shot_function() {
    for (name, z) in fixtures(0.15) {
        let kernel = ZoneKernel::new(&z);
        let mut rng = Rng(91);
        for _ in 0..20 {
            let a = rng.point(2.5);
            let b = rng.point(2.5);
            assert_eq!(kernel.lengths(a, b), zone_lengths(&z, a, b), "{name}");
        }
    }
}

#[test]
fn no_zones_and_degenerate_segments() {
    let z = ZonedAbsorption::new(absorb(0.1));
    let l = zone_lengths(&z, DVec3::new(1.0, 2.0, 3.0), DVec3::new(4.0, 6.0, 3.0));
    assert_close("base only", &l, &[5.0, 0.0, 0.0, 0.0, 0.0], 1e-12);
    for (name, fixture) in fixtures(0.0) {
        let p = DVec3::new(0.2, 0.3, 0.4);
        assert_eq!(zone_lengths(&fixture, p, p), [0.0; MAX_ZONES + 1], "{name}");
    }
}

#[test]
fn frames_compose_and_invert() {
    let mut rng = Rng(101);
    for _ in 0..50 {
        let a = ZoneFrame {
            rotation: DQuat::from_axis_angle(rng.direction(), rng.range(0.0, TAU)),
            translation: rng.point(3.0),
        };
        let b = ZoneFrame {
            rotation: DQuat::from_axis_angle(rng.direction(), rng.range(0.0, TAU)),
            translation: rng.point(3.0),
        };
        let p = rng.point(2.0);
        let composed = a.compose(&b).point(p);
        let stepwise = a.point(b.point(p));
        assert!((composed - stepwise).length() < 1e-12);
        let back = a.inverse().point(a.point(p));
        assert!((back - p).length() < 1e-12);
        assert!((a.inverse_point(a.point(p)) - p).length() < 1e-12);
    }
    let rv = [0.1, -0.2, 0.3];
    let from_vec = ZoneFrame::from_rotation_vector(rv, [1.0, 2.0, 3.0]);
    let q = DQuat::from_scaled_axis(DVec3::from_array(rv));
    assert!((from_vec.rotation.dot(q) - 1.0).abs() < 1e-12);
}

// ---------------------------------------------------------------------------------------------
// Hand-computed cases
// ---------------------------------------------------------------------------------------------

fn watermelon() -> ZonedAbsorption {
    // Base everywhere; zone 1: the bicolour half space z >= 0; zone 2: a solid trigonal prism
    // of apothem 1 around the z axis (side normals at 0, 120, 240 degrees from +X; the
    // reference direction of axis +Z is +X, the second +Y), overriding both.
    let mut z = ZonedAbsorption::new(absorb(0.1));
    z.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::Z,
            offset: 0.0,
        },
        absorption: absorb(0.2),
    });
    z.zones.push(Zone {
        shape: ZoneShape::CoaxialPrism {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            n_sides: 3,
            r_in: 0.0,
            r_out: 1.0,
            phase: 0.0,
        },
        absorption: absorb(0.4),
    });
    assert_eq!(z.validate(), Ok(()));
    z
}

#[test]
fn watermelon_prism_in_a_bicolour_half_space_has_the_hand_computed_lengths() {
    let z = watermelon();
    let s3 = 3.0_f64.sqrt();

    // Along x at y = 0, z = +1: the triangle spans x in [-2, 1] (3 mm), the rest is the
    // half space. [base, half space, prism].
    let l = zone_lengths(&z, DVec3::new(-3.0, 0.0, 1.0), DVec3::new(3.0, 0.0, 1.0));
    assert_close("upper x line", &l, &[0.0, 3.0, 3.0, 0.0, 0.0], 1e-12);

    // The same line below the half space: the rest is the base.
    let l = zone_lengths(&z, DVec3::new(-3.0, 0.0, -1.0), DVec3::new(3.0, 0.0, -1.0));
    assert_close("lower x line", &l, &[3.0, 0.0, 3.0, 0.0, 0.0], 1e-12);

    // Along the axis: always inside the prism, which overrides the half space.
    let l = zone_lengths(&z, DVec3::new(0.0, 0.0, -2.0), DVec3::new(0.0, 0.0, 3.0));
    assert_close("axis", &l, &[0.0, 0.0, 5.0, 0.0, 0.0], 1e-12);

    // Along y at x = 0.5, z = 1: the chord of the triangle is 2 * 1.25 / (sqrt(3) / 2) / 2
    // = 5 / sqrt(3).
    let chord = 5.0 / s3;
    let l = zone_lengths(&z, DVec3::new(0.5, -3.0, 1.0), DVec3::new(0.5, 3.0, 1.0));
    assert_close("y line", &l, &[0.0, 6.0 - chord, chord, 0.0, 0.0], 1e-12);

    // A slanted line that leaves the base, enters the prism, crosses z = 0 inside it and
    // leaves into the half space: from (-3, 0, -1) to (3, 0, 1), parameter t:
    //   t in [0, 1/6]   base          (x < -2, z < 0)
    //   t in [1/6, 2/3] prism         (x in [-2, 1])
    //   t in [2/3, 1]   half space    (x > 1, z > 0)
    let a = DVec3::new(-3.0, 0.0, -1.0);
    let b = DVec3::new(3.0, 0.0, 1.0);
    let len = (b - a).length();
    let l = zone_lengths(&z, a, b);
    assert_close(
        "slanted line",
        &l,
        &[len / 6.0, len / 3.0, len / 2.0, 0.0, 0.0],
        1e-12,
    );
}

#[test]
fn watermelon_optical_depth_is_the_alpha_weighted_sum() {
    let z = watermelon();
    let a = DVec3::new(-3.0, 0.0, 1.0);
    let b = DVec3::new(3.0, 0.0, 1.0);
    let expected = f64::mul_add(f64::from(0.4_f32), 3.0, f64::from(0.2_f32) * 3.0);
    let got = segment_optical_depth(&z, a, b, 550.0);
    assert!((got - expected).abs() < 1e-9, "got {got}, want {expected}");

    let a = DVec3::new(-3.0, 0.0, -1.0);
    let b = DVec3::new(3.0, 0.0, 1.0);
    let len = (b - a).length();
    let expected =
        len * (f64::from(0.1_f32) / 6.0 + f64::from(0.2_f32) / 3.0 + f64::from(0.4_f32) / 2.0);
    let got = segment_optical_depth(&z, a, b, 550.0);
    assert!((got - expected).abs() < 1e-9, "got {got}, want {expected}");
}

#[test]
fn simple_shapes_have_the_hand_computed_lengths() {
    // Slab z in [0, 1].
    let z = one_zone(ZoneShape::Slab {
        normal: DVec3::Z,
        offset_min: 0.0,
        offset_max: 1.0,
    });
    let l = zone_lengths(&z, DVec3::new(0.0, 0.0, -1.0), DVec3::new(0.0, 0.0, 2.0));
    assert_close("slab", &l, &[2.0, 1.0, 0.0, 0.0, 0.0], 1e-12);

    // Tube r in [1, 2] about z: along x through the axis the core goes to the base.
    let z = one_zone(ZoneShape::CoaxialCylinder {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        r_in: 1.0,
        r_out: 2.0,
    });
    let l = zone_lengths(&z, DVec3::new(-3.0, 0.0, 0.0), DVec3::new(3.0, 0.0, 0.0));
    assert_close("tube", &l, &[4.0, 2.0, 0.0, 0.0, 0.0], 1e-12);

    // First-quadrant sector about +Z (reference direction +X, second +Y).
    let z = one_zone(ZoneShape::Sector {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        angle_from: 0.0,
        angle_to: PI / 2.0,
    });
    let l = zone_lengths(&z, DVec3::new(-2.0, 1.0, 0.0), DVec3::new(2.0, 1.0, 0.0));
    assert_close("sector", &l, &[2.0, 2.0, 0.0, 0.0, 0.0], 1e-12);

    // Three-quarter sector (everything but the fourth quadrant): wide, a convex complement.
    let z = one_zone(ZoneShape::Sector {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        angle_from: 0.0,
        angle_to: 1.5 * PI,
    });
    let l = zone_lengths(&z, DVec3::new(-2.0, -1.0, 0.0), DVec3::new(2.0, -1.0, 0.0));
    assert_close("wide sector", &l, &[2.0, 2.0, 0.0, 0.0, 0.0], 1e-12);

    // Full turn: all space.
    let z = one_zone(ZoneShape::Sector {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        angle_from: 0.0,
        angle_to: TAU,
    });
    let l = zone_lengths(&z, DVec3::new(-2.0, -1.0, 0.0), DVec3::new(2.0, -1.0, 0.0));
    assert_close("full sector", &l, &[0.0, 4.0, 0.0, 0.0, 0.0], 1e-12);
}

#[test]
fn mesh_cube_has_the_hand_computed_lengths_including_through_an_edge() {
    let z = one_zone(cube_shape(DVec3::ZERO, 1.0));
    assert_eq!(z.validate(), Ok(()));
    // Straight through two faces.
    let l = zone_lengths(&z, DVec3::new(-3.0, 0.2, 0.1), DVec3::new(3.0, 0.2, 0.1));
    assert_close("through faces", &l, &[4.0, 2.0, 0.0, 0.0, 0.0], 1e-12);
    // Along the diagonal of the square section, exactly through two vertical edges.
    let l = zone_lengths(&z, DVec3::new(-2.0, -2.0, 0.0), DVec3::new(2.0, 2.0, 0.0));
    let s2 = 2.0_f64.sqrt();
    assert_close(
        "through edges",
        &l,
        &[2.0 * s2, 2.0 * s2, 0.0, 0.0, 0.0],
        1e-9,
    );
    // Starting inside, ending outside.
    let l = zone_lengths(&z, DVec3::ZERO, DVec3::new(3.0, 0.1, 0.2));
    let total = DVec3::new(3.0, 0.1, 0.2).length();
    let inside = total / 3.0;
    assert_close(
        "from inside",
        &l,
        &[total - inside, inside, 0.0, 0.0, 0.0],
        1e-12,
    );
}

#[test]
fn soft_half_space_matches_the_closed_form_integral() {
    // Half space z >= 0 with a 0.4 mm smoothstep: weight S(z / 0.4 + 0.5), S(x) = 3x^2 - 2x^3,
    // antiderivative x^3 - x^4 / 2 in x.
    let width = 0.4_f64;
    let mut zoning = one_zone(ZoneShape::HalfSpace {
        normal: DVec3::Z,
        offset: 0.0,
    });
    zoning.boundary_softness_mm = width as f32;
    let band = f64::from(zoning.boundary_softness_mm);
    let prim = |t: f64| (0.5 * t * t * t).mul_add(-t, t * t * t);

    // Across the whole band the smoothstep is symmetric, so the zone gets exactly its sharp
    // share.
    let lengths = zone_lengths(
        &zoning,
        DVec3::new(0.0, 0.0, -1.0),
        DVec3::new(0.0, 0.0, 2.0),
    );
    assert_close(
        "symmetric band",
        &lengths,
        &[1.0, 2.0, 0.0, 0.0, 0.0],
        1e-12,
    );

    // Inside the band only: z from -0.3 w to 0.4 w, x from 0.2 to 0.9.
    let start = DVec3::new(0.0, 0.0, -0.3 * band);
    let end = DVec3::new(0.0, 0.0, 0.4 * band);
    let zone_len = band * (prim(0.9) - prim(0.2));
    let lengths = zone_lengths(&zoning, start, end);
    assert_close(
        "inside the band",
        &lengths,
        &[(end - start).length() - zone_len, zone_len, 0.0, 0.0, 0.0],
        1e-13,
    );
}

#[test]
fn soft_slab_thicker_than_the_band_keeps_its_sharp_length() {
    let mut z = one_zone(ZoneShape::Slab {
        normal: DVec3::Z,
        offset_min: 0.0,
        offset_max: 1.0,
    });
    z.boundary_softness_mm = 0.2;
    let l = zone_lengths(&z, DVec3::new(0.0, 0.0, -1.0), DVec3::new(0.0, 0.0, 2.0));
    assert_close("soft slab", &l, &[2.0, 1.0, 0.0, 0.0, 0.0], 1e-12);
}

// ---------------------------------------------------------------------------------------------
// Optical depth and tensors
// ---------------------------------------------------------------------------------------------

fn band(peak: f32) -> Vec<AbsorptionBand> {
    vec![AbsorptionBand::new(550.0, 1.0e6, peak)]
}

#[test]
fn pleochroic_zone_absorption_follows_the_tracer_evaluation() {
    let zone = ZoneAbsorption::per_mm(AbsorptionTensor::uniaxial(band(1.0), band(3.0)));
    // Orientation mean.
    assert!((zone.alpha(550.0, None) - 5.0 / 3.0).abs() < 1e-6);
    // E field along the c axis reads the extraordinary coefficient, across it the ordinary.
    let along = PleochroicMode {
        e_mode_hat: Vec3::Z,
        c_axis: Vec3::Z,
    };
    let across = PleochroicMode {
        e_mode_hat: Vec3::X,
        c_axis: Vec3::Z,
    };
    assert!((zone.alpha(550.0, Some(&along)) - 3.0).abs() < 1e-6);
    assert!((zone.alpha(550.0, Some(&across)) - 1.0).abs() < 1e-6);
    assert!((zone.alpha_midpoint(550.0, Vec3::X, Vec3::Z, Vec3::Z) - 2.0).abs() < 1e-6);

    // Biaxial orientation mean.
    let tri = ZoneAbsorption::per_mm(AbsorptionTensor::biaxial(band(1.0), band(2.0), band(6.0)));
    assert!((tri.alpha(550.0, None) - 3.0).abs() < 1e-6);

    // Not pleochroic: the single band set, mode ignored.
    let iso = absorb(0.5);
    assert!((iso.alpha(550.0, Some(&along)) - 0.5).abs() < 1e-6);
}

#[test]
fn optical_depth_of_a_pleochroic_zone_uses_the_mode() {
    let mut z = ZonedAbsorption::new(absorb(0.0));
    z.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::Z,
            offset: 0.0,
        },
        absorption: ZoneAbsorption::per_mm(AbsorptionTensor::uniaxial(band(1.0), band(3.0))),
    });
    let a = DVec3::new(0.0, 0.0, -1.0);
    let b = DVec3::new(0.0, 0.0, 3.0);
    let mean = segment_optical_depth(&z, a, b, 550.0);
    assert!((mean - 3.0 * 5.0 / 3.0).abs() < 1e-5, "mean form: {mean}");
    let mode = PleochroicMode {
        e_mode_hat: Vec3::Z,
        c_axis: Vec3::Z,
    };
    let along = super::segment_optical_depth_mode(&z, a, b, 550.0, Some(&mode));
    assert!((along - 9.0).abs() < 1e-5, "along c: {along}");
}

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

fn valid_half_space() -> ZonedAbsorption {
    one_zone(ZoneShape::HalfSpace {
        normal: DVec3::Z,
        offset: 0.0,
    })
}

#[test]
fn validation_checks_zone_count_finiteness_and_unit_directions() {
    let ok = valid_half_space();
    assert_eq!(ok.validate(), Ok(()));

    // Zone count.
    let mut too_many = ok;
    let first = too_many.zones[0].clone();
    for _ in 0..MAX_ZONES {
        too_many.zones.push(first.clone());
    }
    assert_eq!(
        too_many.validate(),
        Err(ZoningError::TooManyZones {
            count: MAX_ZONES + 1
        })
    );
    // Exactly the maximum is fine.
    too_many.zones.pop();
    assert_eq!(too_many.validate(), Ok(()));

    // Non-finite parameters.
    let nan_offset = one_zone(ZoneShape::HalfSpace {
        normal: DVec3::Z,
        offset: f64::NAN,
    });
    assert!(matches!(
        nan_offset.validate(),
        Err(ZoningError::NotFinite { zone: Some(1), .. })
    ));
    let inf_radius = one_zone(ZoneShape::CoaxialCylinder {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        r_in: 0.0,
        r_out: f64::INFINITY,
    });
    assert!(matches!(
        inf_radius.validate(),
        Err(ZoningError::NotFinite { .. })
    ));

    // Directions must be unit.
    let long_normal = one_zone(ZoneShape::HalfSpace {
        normal: DVec3::new(0.0, 0.0, 2.0),
        offset: 0.0,
    });
    assert!(matches!(
        long_normal.validate(),
        Err(ZoningError::NotUnit { zone: 1, .. })
    ));
    let zero_axis = one_zone(ZoneShape::Sector {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::ZERO,
        angle_from: 0.0,
        angle_to: 1.0,
    });
    assert!(matches!(
        zero_axis.validate(),
        Err(ZoningError::NotUnit { .. })
    ));
}

#[test]
fn validation_rejects_bad_slab_radii_side_counts_and_angles() {
    // Slab, radii, sides, angles.
    let slab = one_zone(ZoneShape::Slab {
        normal: DVec3::Z,
        offset_min: 1.0,
        offset_max: 1.0,
    });
    assert_eq!(slab.validate(), Err(ZoningError::BadSlab { zone: 1 }));
    for (r_in, r_out) in [(1.0, 1.0), (2.0, 1.0), (-0.1, 1.0)] {
        let tube = one_zone(ZoneShape::CoaxialCylinder {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            r_in,
            r_out,
        });
        assert_eq!(
            tube.validate(),
            Err(ZoningError::BadRadii { zone: 1 }),
            "r_in {r_in}, r_out {r_out}"
        );
    }
    for n_sides in [0, 2, 13, 100] {
        let prism = one_zone(ZoneShape::CoaxialPrism {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            n_sides,
            r_in: 0.0,
            r_out: 1.0,
            phase: 0.0,
        });
        assert_eq!(
            prism.validate(),
            Err(ZoningError::BadSideCount { zone: 1, n_sides })
        );
    }
    for n_sides in [3, 12] {
        let prism = one_zone(ZoneShape::CoaxialPrism {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            n_sides,
            r_in: 0.0,
            r_out: 1.0,
            phase: 0.0,
        });
        assert_eq!(prism.validate(), Ok(()));
    }
    for (from, to) in [(1.0, 1.0), (2.0, 1.0), (0.0, 7.0)] {
        let sector = one_zone(ZoneShape::Sector {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            angle_from: from,
            angle_to: to,
        });
        assert_eq!(
            sector.validate(),
            Err(ZoningError::BadAngles { zone: 1 }),
            "{from}..{to}"
        );
    }
}

#[test]
fn validation_rejects_bad_units_bands_softness_and_frames() {
    let ok = valid_half_space();

    // Units.
    let mut model_unit = ok.clone();
    model_unit.zones[0].absorption.unit = AbsorptionUnit::ModelUnit;
    assert_eq!(
        model_unit.validate(),
        Err(ZoningError::NotPerMm { zone: Some(1) })
    );
    let mut base_unit = ok.clone();
    base_unit.base.unit = AbsorptionUnit::ModelUnit;
    assert_eq!(
        base_unit.validate(),
        Err(ZoningError::NotPerMm { zone: None })
    );

    // Bands.
    let mut bad_band = ok.clone();
    bad_band.base = ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        550.0, 0.0, 0.1,
    )]));
    assert_eq!(
        bad_band.validate(),
        Err(ZoningError::BadBands { zone: None })
    );

    // Softness and frame.
    for softness in [-0.1_f32, f32::NAN, f32::INFINITY] {
        let mut z = ok.clone();
        z.boundary_softness_mm = softness;
        assert_eq!(z.validate(), Err(ZoningError::BadSoftness));
    }
    let mut bad_rotation = ok.clone();
    bad_rotation.frame.rotation = DQuat::from_xyzw(0.0, 0.0, 0.0, 2.0);
    assert_eq!(bad_rotation.validate(), Err(ZoningError::BadFrame));
    let mut bad_translation = ok;
    bad_translation.frame.translation = DVec3::new(f64::NAN, 0.0, 0.0);
    assert_eq!(bad_translation.validate(), Err(ZoningError::BadFrame));
}

#[test]
fn validation_checks_mesh_shells() {
    let good = one_zone(cube_shape(DVec3::ZERO, 1.0));
    assert_eq!(good.validate(), Ok(()));

    let mesh_problem = |vertices: Vec<[f64; 3]>, triangles: Vec<[u32; 3]>| {
        one_zone(ZoneShape::MeshShell {
            vertices,
            triangles,
        })
        .validate()
    };
    let (vertices, triangles) = cube_parts(DVec3::ZERO, 1.0);
    let problem = |p: MeshProblem| -> Result<(), ZoningError> {
        Err(ZoningError::BadMesh {
            zone: 1,
            problem: p,
        })
    };

    // A hole.
    let mut open = triangles.clone();
    open.pop();
    assert_eq!(
        mesh_problem(vertices.clone(), open),
        problem(MeshProblem::NotClosed)
    );
    // One flipped triangle.
    let mut flipped = triangles.clone();
    let t = flipped[3];
    flipped[3] = [t[0], t[2], t[1]];
    assert_eq!(
        mesh_problem(vertices.clone(), flipped),
        problem(MeshProblem::InconsistentWinding)
    );
    // An index past the end.
    let mut bad_index = triangles.clone();
    bad_index[0] = [0, 1, 99];
    assert_eq!(
        mesh_problem(vertices.clone(), bad_index),
        problem(MeshProblem::IndexOutOfRange)
    );
    // A repeated vertex.
    let mut repeated = triangles.clone();
    repeated[0] = [0, 0, 1];
    assert_eq!(
        mesh_problem(vertices.clone(), repeated),
        problem(MeshProblem::DegenerateTriangle)
    );
    // Too few and too many triangles.
    assert_eq!(
        mesh_problem(vertices.clone(), triangles[..3].to_vec()),
        problem(MeshProblem::TooFewTriangles)
    );
    let many: Vec<[u32; 3]> = (0..=MAX_MESH_TRIANGLES).map(|_| triangles[0]).collect();
    assert_eq!(
        mesh_problem(vertices.clone(), many),
        problem(MeshProblem::TooManyTriangles)
    );
    // A non-finite vertex.
    let mut nan_vertices = vertices;
    nan_vertices[2][1] = f64::NAN;
    assert_eq!(
        mesh_problem(nan_vertices, triangles),
        problem(MeshProblem::NotFinite)
    );

    // Softness and a mesh do not mix.
    let mut soft = good;
    soft.boundary_softness_mm = 0.1;
    assert_eq!(soft.validate(), Err(ZoningError::SoftMeshShell));
}

#[test]
fn errors_display_something_useful() {
    let text = ZoningError::TooManyZones { count: 5 }.to_string();
    assert!(text.contains('5') && text.contains(&MAX_ZONES.to_string()));
    assert!(
        ZoningError::NotPerMm { zone: None }
            .to_string()
            .contains("base zone")
    );
}
