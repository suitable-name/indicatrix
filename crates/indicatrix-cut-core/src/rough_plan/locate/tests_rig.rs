//! Tests of the mesh-to-rig alignment and of the beam-splitter-cube calibration, on synthetic
//! photos (projected points and lines with noise, no rendering).
//!
//! Tolerances from the geometry. At 150 mm with a 4000 px focal length one pixel is 0.0375 mm at the
//! stone, and a rotation of 0.1 degree moves the image by 7 px. A camera shifted sideways by `t` and
//! turned by `t / D` looks the same, so the fit can only pin a camera's position to the prior's
//! width (here 0.5 mm) while its rotation is good to about `0.5 / 150 = 0.2` degrees at the very
//! worst and normally to a few hundredths of a degree.

use glam::{DQuat, DVec2, DVec3};

use super::{
    AlignOptions, CalibrationOptions, CubeSpec, DiagonalLine, EdgeLine, OutlineView,
    ViewObservations,
    align::convex_hull,
    align_mesh_to_rig,
    calibrate::{CUBE_EDGES, cube_corner, seen_through, visible_edges},
    calibrate_rig, check_diagonal, reproject,
    rig::{RigProfile, Rigid, ViewPose},
    shapes::{box_mesh, box_points},
    test_support::{
        DISTANCE, IMAGE, Lcg, N_BK7, noisy, noisy_on_same_face, octant_views, pinhole, rig_of,
    },
    trace::Scene,
};

/// The noise of an observed point or edge end, in pixels.
const NOISE_PX: f64 = 0.5;

fn rotation_error_deg(left: &Rigid, right: &Rigid) -> f64 {
    left.quat().angle_between(right.quat()).to_degrees()
}

/// The outline of the box with `half` extents under `truth`, as every view would photograph it.
///
/// It is the hull of the projected corners with every edge cut in six, and `sigma` pixels of noise.
fn box_outlines(
    rig: &RigProfile,
    half: DVec3,
    truth: &Rigid,
    rng: &mut Lcg,
    sigma: f64,
) -> Vec<OutlineView> {
    let corners = box_points(half);
    let mut outlines = Vec::new();
    for (view, pose) in rig.views.iter().enumerate() {
        let projected: Vec<DVec2> = corners
            .iter()
            .filter_map(|&corner| pose.project(truth.to_rig(corner)))
            .collect();
        let hull = convex_hull(&projected);
        let mut outline = Vec::new();
        for (index, &from) in hull.iter().enumerate() {
            let to = hull[(index + 1) % hull.len()];
            for piece in 0..6 {
                let point = from + (to - from) * (f64::from(piece) / 6.0);
                outline.push(noisy(point, rng, sigma).to_array());
            }
        }
        outlines.push(OutlineView { view, outline });
    }
    outlines
}

#[test]
fn a_rigid_transform_is_recovered_from_synthetic_silhouettes() {
    let half = DVec3::new(12.0, 7.5, 4.5);
    let mesh = box_mesh(half).expect("a box");
    let rig = rig_of(octant_views(pinhole()), N_BK7, 1.0);
    let axis_angle = DVec3::new(1.0, 2.0, 3.0).normalize() * 12.0_f64.to_radians();
    let truth = Rigid {
        rotation: axis_angle.to_array(),
        translation: [1.2, -0.8, 0.6],
    };
    let mut rng = Lcg::new(21);
    let outlines = box_outlines(&rig, half, &truth, &mut rng, 0.3);
    // The user's coarse start: about 3 degrees and 1.4 mm off.
    let start = Rigid {
        rotation: (axis_angle + DVec3::new(0.03, -0.03, 0.03)).to_array(),
        translation: [2.2, -1.5, 1.1],
    };
    let result = align_mesh_to_rig(&mesh, &rig, &outlines, start, &AlignOptions::default())
        .expect("valid outlines");
    assert!(result.accepted, "{:?}", result.note);
    assert_eq!(result.misfit.len(), 8);
    let turned = rotation_error_deg(&truth, &result.transform);
    assert!(turned < 0.3, "rotation error {turned} degrees");
    let moved = (DVec3::from_array(result.transform.translation)
        - DVec3::from_array(truth.translation))
    .length();
    assert!(moved < 0.15, "translation error {moved} mm");
    // The misfit is the noise: 0.3 px, well below a pixel; no outlier point beyond 3 px.
    for entry in &result.misfit {
        assert!(
            entry.rms_px < 1.0,
            "view {}: {} px",
            entry.view,
            entry.rms_px
        );
        assert!(
            entry.max_px < 3.0,
            "view {}: {} px",
            entry.view,
            entry.max_px
        );
    }
    assert!(result.worst_rms_px < 1.0);
}

#[test]
fn an_alignment_that_cannot_match_the_outline_is_refused_with_a_note() {
    let half = DVec3::new(12.0, 7.5, 4.5);
    let mesh = box_mesh(half).expect("a box");
    let rig = rig_of(octant_views(pinhole()), N_BK7, 1.0);
    let truth = Rigid::IDENTITY;
    let mut rng = Lcg::new(4);
    // The photographed stone is 30 % larger than the scan: a rigid transform cannot fix that.
    let outlines = box_outlines(&rig, half * 1.3, &truth, &mut rng, 0.3);
    let result = align_mesh_to_rig(&mesh, &rig, &outlines, truth, &AlignOptions::default())
        .expect("valid outlines");
    assert!(!result.accepted);
    assert!(result.worst_rms_px > 4.0, "{}", result.worst_rms_px);
    assert!(result.note.is_some());
}

#[test]
fn unusable_outlines_are_refused_before_any_fitting() {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a box");
    let rig = rig_of(octant_views(pinhole()), N_BK7, 1.0);
    let options = AlignOptions::default();
    let none = align_mesh_to_rig(&mesh, &rig, &[], Rigid::IDENTITY, &options);
    assert!(none.is_err());
    let two_points = [OutlineView {
        view: 0,
        outline: vec![[0.0, 0.0], [1.0, 1.0]],
    }];
    assert!(align_mesh_to_rig(&mesh, &rig, &two_points, Rigid::IDENTITY, &options).is_err());
    let missing_view = [OutlineView {
        view: 99,
        outline: vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]],
    }];
    assert!(align_mesh_to_rig(&mesh, &rig, &missing_view, Rigid::IDENTITY, &options).is_err());
}

/// Eight cameras around the cube, four azimuths in two elevation bands.
///
/// None is on a diagonal plane of the cube. Every coated-diagonal edge is seen through the glass by at least two of
/// them, and no two of those lie in one plane with the edge.
fn calibration_views() -> Vec<ViewPose> {
    let table = [
        (25.0_f64, 35.0_f64),
        (65.0, -30.0),
        (115.0, 30.0),
        (155.0, -35.0),
        (205.0, 30.0),
        (245.0, -35.0),
        (295.0, 35.0),
        (335.0, -30.0),
    ];
    table
        .iter()
        .enumerate()
        .map(|(index, &(azimuth, elevation))| {
            let (azimuth, elevation) = (azimuth.to_radians(), elevation.to_radians());
            let direction = DVec3::new(
                elevation.cos() * azimuth.cos(),
                elevation.cos() * azimuth.sin(),
                elevation.sin(),
            );
            ViewPose::look_at(
                &format!("cal {index}"),
                direction * DISTANCE,
                DVec3::ZERO,
                DVec3::Z,
                pinhole(),
                IMAGE,
            )
        })
        .collect()
}

/// The visible outer edges of the cube as two noisy points each.
fn edge_lines(pose: &ViewPose, edge_mm: f64, rng: &mut Lcg, sigma: f64) -> Vec<EdgeLine> {
    visible_edges(pose, edge_mm)
        .into_iter()
        .filter_map(|edge| {
            let (corner_0, corner_1) = CUBE_EDGES[edge];
            let first = pose.project(cube_corner(corner_0, edge_mm))?;
            let second = pose.project(cube_corner(corner_1, edge_mm))?;
            Some(EdgeLine {
                a: noisy(first, rng, sigma).to_array(),
                b: noisy(second, rng, sigma).to_array(),
            })
        })
        .collect()
}

/// Six clicks along every coated-diagonal edge in every view that sees it through the glass.
fn diagonal_lines(
    scene: &Scene<'_>,
    cube: &CubeSpec,
    edge_mm: f64,
    rng: &mut Lcg,
    sigma: f64,
) -> Vec<DiagonalLine> {
    let corners = cube.diagonal_corners(edge_mm);
    let mut lines = Vec::new();
    for edge in 0..4_u8 {
        let (end_a, end_b) = (
            corners[usize::from(edge)],
            corners[(usize::from(edge) + 1) % 4],
        );
        for view in 0..scene.rig.views.len() {
            if !seen_through(scene, view, end_a.midpoint(end_b)) {
                continue;
            }
            let pixels: Vec<[f64; 2]> = (0..6_u32)
                .filter_map(|k| {
                    let along = 0.14_f64.mul_add(f64::from(k), 0.15);
                    let target = end_a + (end_b - end_a) * along;
                    let image = reproject(scene, view, target, None)?;
                    (image.miss_mm < 1e-3).then(|| {
                        noisy_on_same_face(scene, view, DVec2::from_array(image.pixel), rng, sigma)
                            .to_array()
                    })
                })
                .collect();
            if pixels.len() >= 3 {
                lines.push(DiagonalLine { edge, view, pixels });
            }
        }
    }
    lines
}

/// The rig with every camera shifted by up to 0.2 mm per axis, turned by up to 0.15 degrees per
/// axis and its focal length changed by up to 2 %.
fn perturbed(rig: &RigProfile, rng: &mut Lcg) -> RigProfile {
    let mut out = rig.clone();
    for pose in &mut out.views {
        let shift = DVec3::new(
            rng.uniform(-0.2, 0.2),
            rng.uniform(-0.2, 0.2),
            rng.uniform(-0.2, 0.2),
        );
        pose.position = (DVec3::from_array(pose.position) + shift).to_array();
        let limit = 0.15_f64.to_radians();
        let turn = DQuat::from_scaled_axis(DVec3::new(
            rng.uniform(-limit, limit),
            rng.uniform(-limit, limit),
            rng.uniform(-limit, limit),
        ));
        pose.forward = (turn * DVec3::from_array(pose.forward)).to_array();
        pose.up = (turn * DVec3::from_array(pose.up)).to_array();
        let factor = 1.0 + rng.uniform(-0.02, 0.02);
        pose.projection = pose
            .projection
            .with_scale(pose.projection.scale_px() * factor);
    }
    out
}

#[test]
fn the_diagonal_is_a_rectangle_in_a_plane_of_the_cube() {
    for mirrored in [false, true] {
        for axis in 0..3_u8 {
            let cube = CubeSpec {
                diagonal_axis: axis,
                mirrored,
                ..CubeSpec::default()
            };
            let corners = cube.diagonal_corners(10.0);
            let normal = cube.diagonal_normal();
            for corner in corners {
                assert!(normal.dot(corner - corners[0]).abs() < 1e-12, "axis {axis}");
                assert!(
                    (corner.abs() - DVec3::splat(5.0)).length() < 1e-12,
                    "a cube corner"
                );
            }
            // Corners 0 and 1 share a cube edge along the axis; so do 2 and 3.
            assert!(((corners[0] - corners[1]).length() - 10.0).abs() < 1e-12);
            assert!(((corners[2] - corners[3]).length() - 10.0).abs() < 1e-12);
        }
    }
}

#[test]
fn calibration_recovers_poses_and_focal_lengths_and_the_diagonal_lands_within_the_noise() {
    let cube = CubeSpec::default();
    let truth_rig = rig_of(calibration_views(), N_BK7, 1.0);
    let mesh = CubeSpec::mesh(cube.edge_mm).expect("the cube");
    let truth_scene = Scene::new(&mesh, &truth_rig, Rigid::IDENTITY);
    let mut rng = Lcg::new(2026);
    let observations: Vec<ViewObservations> = truth_rig
        .views
        .iter()
        .enumerate()
        .map(|(view, pose)| ViewObservations {
            view,
            lines: edge_lines(pose, cube.edge_mm, &mut rng, NOISE_PX),
            mark: pose
                .project(cube_corner(7, cube.edge_mm))
                .map(|pixel| pixel.to_array()),
        })
        .collect();
    let diagonal = diagonal_lines(&truth_scene, &cube, cube.edge_mm, &mut rng, NOISE_PX);
    assert!(
        diagonal.len() >= 8,
        "{} diagonal observations",
        diagonal.len()
    );
    let start = perturbed(&truth_rig, &mut rng);

    let outcome = calibrate_rig(
        &start,
        &cube,
        &observations,
        &diagonal,
        &CalibrationOptions::default(),
    )
    .expect("a calibration");
    let result = &outcome.result;
    // The fit is as good as the noise: one end of an edge line carries about 0.5 px in one axis.
    assert!(
        result.edge_rms_px > 0.1 && result.edge_rms_px < 1.0,
        "edge rms {} px",
        result.edge_rms_px
    );
    assert!(
        result.lines_used >= 56,
        "{} lines matched",
        result.lines_used
    );
    // The scale stays within 1 % of the datasheet: the cube's tolerance prior is 0.4 % and the
    // position prior allows 0.3 % more.
    assert!(
        (result.scale_ratio - 1.0).abs() < 0.01,
        "ratio {}",
        result.scale_ratio
    );
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert!(
        outcome
            .mark_error_px
            .iter()
            .flatten()
            .all(|&error| error < 5.0)
    );

    for (fitted, truth) in outcome.rig.views.iter().zip(&truth_rig.views) {
        let unit = |v: [f64; 3]| DVec3::from_array(v).normalize();
        let pointing = unit(fitted.forward)
            .angle_between(unit(truth.forward))
            .to_degrees();
        let rolling = unit(fitted.up).angle_between(unit(truth.up)).to_degrees();
        // At worst the position prior's width over the distance, 0.2 degrees; in practice less.
        assert!(
            pointing < 0.2 && rolling < 0.3,
            "{}: {pointing} / {rolling} degrees",
            fitted.name
        );
        let focal = fitted.projection.scale_px() / truth.projection.scale_px() - 1.0;
        assert!(focal.abs() < 0.02, "{}: focal off by {focal}", fitted.name);
        let shift =
            (DVec3::from_array(fitted.position) - DVec3::from_array(truth.position)).length();
        // The perturbation was at most 0.35 mm; the data can only improve on the prior.
        assert!(shift < 0.45, "{}: position off by {shift} mm", fitted.name);
    }

    // Pass 2: the triangulated diagonal edges against the true ones. The rig's pose errors
    // (at most a few tenths of a millimetre, see above) bound the result; the noise alone gives
    // about 0.05 mm.
    let check = outcome.diagonal.as_ref().expect("pass 2 ran");
    assert!(
        check.edges.len() >= 3,
        "{:?} / {:?}",
        check.edges,
        check.skipped
    );
    assert!(check.edges.iter().all(|edge| edge.views >= 2));
    assert!(
        check.deviation_rms_mm < 0.8,
        "{} mm",
        check.deviation_rms_mm
    );
    assert!(check.plane_rms_mm < 0.8, "{} mm", check.plane_rms_mm);
    assert_eq!(result.diagonal_rms_mm, Some(check.deviation_rms_mm));
    assert_eq!(outcome.rig.calibration.as_ref(), Some(result));
}

#[test]
fn a_perfect_rig_stays_put_and_the_diagonal_through_the_glass_lands_on_the_true_plane() {
    let cube = CubeSpec::default();
    let truth_rig = rig_of(calibration_views(), N_BK7, 1.0);
    let mesh = CubeSpec::mesh(cube.edge_mm).expect("the cube");
    let truth_scene = Scene::new(&mesh, &truth_rig, Rigid::IDENTITY);
    let mut rng = Lcg::new(77);

    // Without noise and from the true poses nothing moves.
    let observations: Vec<ViewObservations> = truth_rig
        .views
        .iter()
        .enumerate()
        .map(|(view, pose)| ViewObservations {
            view,
            lines: edge_lines(pose, cube.edge_mm, &mut rng, 0.0),
            mark: None,
        })
        .collect();
    let exact = calibrate_rig(
        &truth_rig,
        &cube,
        &observations,
        &[],
        &CalibrationOptions::default(),
    )
    .expect("a calibration");
    assert!(
        exact.result.edge_rms_px < 1e-6,
        "{}",
        exact.result.edge_rms_px
    );
    assert!((exact.result.scale_ratio - 1.0).abs() < 1e-9);
    assert!(exact.result.scale_within_tolerance);
    assert!(exact.diagonal.is_none());

    // Pass 2 alone, on exact clicks: the solver starts a millimetre off the true edge and must
    // come back to it through the refraction.
    let clean = diagonal_lines(&truth_scene, &cube, cube.edge_mm, &mut rng, 0.0);
    let check = check_diagonal(&truth_rig, &cube, cube.edge_mm, &clean).expect("the diagonal");
    assert!(check.edges.len() >= 3, "{:?}", check.skipped);
    assert!(
        check.deviation_rms_mm < 0.01,
        "{} mm",
        check.deviation_rms_mm
    );
    assert!(check.plane_rms_mm < 0.01, "{} mm", check.plane_rms_mm);

    // With 0.5 px of noise: about 0.025 mm per ray at the edge, a line fitted to 12 to 24 rays
    // and read at its ends is good to about 0.05 mm; allow 3 times that.
    let noisy_lines = diagonal_lines(&truth_scene, &cube, cube.edge_mm, &mut rng, NOISE_PX);
    let noisy_check =
        check_diagonal(&truth_rig, &cube, cube.edge_mm, &noisy_lines).expect("the diagonal");
    assert!(
        noisy_check.deviation_rms_mm < 0.15,
        "{} mm",
        noisy_check.deviation_rms_mm
    );
    assert!(
        noisy_check.plane_rms_mm < 0.15,
        "{} mm",
        noisy_check.plane_rms_mm
    );
}
