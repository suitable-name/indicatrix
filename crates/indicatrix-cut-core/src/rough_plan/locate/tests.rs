//! Tests of the locating pipeline, without alignment and calibration (those are in `tests_rig`).
//!
//! They cover refraction, the camera model, ray queries, refractive triangulation (noise, total
//! internal reflection, ghosts, immersion, birefringence), reprojection, polylines and the
//! conversion to an inclusion.
//!
//! The tolerances come from the geometry, not from a run. A pixel of noise `s` in a camera of
//! focal length `f` at distance `D` tilts the ray by `s / f` and moves it by `s D / f` at the
//! target: with `s = 0.5`, `f = 4000`, `D = 150` that is `SIGMA_MM = 0.019` mm. The refracted ray
//! is less sensitive to the angle but meets the surface obliquely (a factor of about 1.3 on the
//! surface), so the per-ray error at the target is about 0.025 mm, and the least-squares point of
//! 8 views, 16 constraints on 3 unknowns, is good to about half of that.

use glam::{DVec2, DVec3};

use super::{
    add_located_point, closest_point_to_lines, critical_angle_deg, locate_point, locate_polyline,
    predict_ghosts, reflect, refract, reproject,
    rig::{Projection, RigProfile, Rigid, ViewPose},
    shapes::sphere_shell,
    suggested_margin_mm,
    test_support::{
        DISTANCE, FOCAL, IMAGE, Lcg, N_BK7, cube_mesh, marks_for, octant_views, pinhole, rig_of,
    },
    trace::{Scene, TraceError, trace_pixel},
    triangulate::{InsideState, Line, LocateError, LocateOptions, Located, ViewPolyline},
};
use crate::rough_plan::shape::{
    RoughMesh,
    mesh_fixture::{CUBE_OBJ, icosphere, import_obj},
};

/// The noise of a pixel in the synthetic photos.
const NOISE_PX: f64 = 0.5;

/// What the pixel noise comes to at the target, in mm (see the module documentation).
const SIGMA_MM: f64 = NOISE_PX / FOCAL * DISTANCE;

#[test]
fn snell_gives_the_textbook_angle_and_is_reversible() {
    let angle = 30.0_f64.to_radians();
    let dir = DVec3::new(angle.sin(), 0.0, -angle.cos());
    let inside = refract(dir, DVec3::Z, 1.0, 1.5).expect("entering glass never reflects totally");
    let theta = (angle.sin() / 1.5).asin();
    let expected = DVec3::new(theta.sin(), 0.0, -theta.cos());
    assert!((inside - expected).length() < 1e-12);
    // The normal may face either way.
    let flipped = refract(dir, DVec3::NEG_Z, 1.0, 1.5).expect("same ray");
    assert!((inside - flipped).length() < 1e-15);
    // Out through the parallel bottom face the ray is the original one again.
    let outside = refract(inside, DVec3::NEG_Z, 1.5, 1.0).expect("the way out is the way in");
    assert!((outside - dir).length() < 1e-12);
}

#[test]
fn equal_indices_do_not_bend_a_ray() {
    let dir = DVec3::new(0.3, -0.2, -0.9).normalize();
    let out = refract(dir, DVec3::new(0.1, 0.2, 1.0), 1.4, 1.4).expect("no bending, no reflection");
    assert!((out - dir).length() < 1e-15);
}

#[test]
fn total_internal_reflection_starts_at_the_critical_angle() {
    let critical = critical_angle_deg(N_BK7, 1.0).expect("glass into air has one");
    assert!((41.0..41.5).contains(&critical), "{critical}");
    assert!(critical_angle_deg(1.0, N_BK7).is_none());
    let leaving = |degrees: f64| {
        let angle = degrees.to_radians();
        refract(
            DVec3::new(angle.sin(), 0.0, angle.cos()),
            DVec3::Z,
            N_BK7,
            1.0,
        )
    };
    assert!(leaving(critical - 1.0).is_some());
    assert!(leaving(critical + 1.0).is_none());
    let mirrored = reflect(DVec3::new(1.0, 0.0, -1.0), DVec3::Z);
    assert!((mirrored - DVec3::new(1.0, 0.0, 1.0).normalize()).length() < 1e-12);
}

#[test]
fn the_camera_model_round_trips_pixels_and_points() {
    for projection in [pinhole(), Projection::Orthographic { px_per_mm: 30.0 }] {
        let views = octant_views(projection);
        let pose = &views[3];
        for pixel in [DVec2::new(1000.0, 700.0), DVec2::new(100.0, 1400.0)] {
            let (origin, dir) = pose.pixel_ray(pixel);
            let point = origin + dir * 140.0;
            let back = pose.project(point).expect("in front of the camera");
            assert!((back - pixel).length() < 1e-8, "{projection:?}");
        }
        // The principal point looks straight ahead; right is +u and down is +v.
        let basis = pose.basis();
        let (_, centre_dir) = pose.pixel_ray(DVec2::new(1024.0, 768.0));
        assert!((centre_dir - basis.forward).length() < 1e-12);
        let target = DVec3::ZERO;
        let right = pose.project(target + basis.right * 5.0).expect("visible");
        let down = pose.project(target + basis.down * 5.0).expect("visible");
        assert!(right.x > 1024.0 && (right.y - 768.0).abs() < 1e-6);
        assert!(down.y > 768.0 && (down.x - 1024.0).abs() < 1e-6);
    }
}

#[test]
fn a_pinhole_camera_does_not_see_behind_itself() {
    let views = octant_views(pinhole());
    let pose = &views[0];
    let behind = pose.position_vec() - pose.basis().forward * 10.0;
    assert!(pose.project(behind).is_none());
}

#[test]
fn rigid_transforms_map_axes_and_invert() {
    let start = Rigid::from_axes(DVec3::Y, DVec3::Z, DVec3::new(1.0, 2.0, 3.0)).expect("axes");
    let moved_y = start.dir_to_rig(DVec3::Y);
    let moved_z = start.dir_to_rig(DVec3::Z);
    assert!((moved_y - DVec3::Z).length() < 1e-12);
    assert!((moved_z - DVec3::X).length() < 1e-12);
    assert!((start.dir_to_rig(DVec3::X) - DVec3::Y).length() < 1e-12);
    assert!((start.to_rig(DVec3::ZERO) - DVec3::new(1.0, 2.0, 3.0)).length() < 1e-12);
    let point = DVec3::new(0.4, -2.5, 7.0);
    assert!((start.to_mesh(start.to_rig(point)) - point).length() < 1e-12);
    assert!(Rigid::from_axes(DVec3::Y, DVec3::Y, DVec3::ZERO).is_none());
}

#[test]
fn rays_find_the_outer_surface_and_skip_inclusions() {
    let mesh = cube_mesh(10.0);
    let hit = mesh
        .first_hit(DVec3::new(-50.0, 3.0, 4.0), DVec3::X, 0.0)
        .expect("the ray crosses the cube");
    assert!((hit.t - 40.0).abs() < 1e-9);
    assert!((hit.normal - DVec3::NEG_X).length() < 1e-12);
    let leaving = mesh
        .first_hit(DVec3::new(0.0, 1.0, 2.0), DVec3::X, 1e-9)
        .expect("from inside the ray leaves");
    assert!((leaving.t - 10.0).abs() < 1e-9);
    assert!((leaving.normal - DVec3::X).length() < 1e-12);
    assert!(
        mesh.first_hit(DVec3::new(-50.0, 30.0, 0.0), DVec3::X, 0.0)
            .is_none()
    );
    assert!(
        mesh.first_hit(DVec3::new(-50.0, 0.0, 0.0), DVec3::NEG_X, 0.0)
            .is_none()
    );
    // An inclusion's faces are not the way out.
    let combined = RoughMesh::with_inclusions(&mesh, &[cube_mesh(2.0)]).expect("fits inside");
    let from_inside = combined
        .first_hit(DVec3::new(-5.0, 0.5, 0.5), DVec3::X, 1e-9)
        .expect("the ray leaves the rough");
    assert!((from_inside.t - 15.0).abs() < 1e-9, "{}", from_inside.t);
}

/// A rig of two cameras around a cube of half edge 10: a steep one that meets the +X face at
/// about 80 degrees, and a shallow one at about 6 degrees.
fn steep_and_shallow(surround_n: f64) -> (Vec<ViewPose>, RigProfile) {
    let steep_at = 77.5_f64.to_radians();
    let steep = DVec3::new(steep_at.cos(), steep_at.sin(), 0.0) * 200.0;
    let shallow = DVec3::new(200.0, 20.0, 0.0);
    let views = vec![
        ViewPose::look_at("steep", steep, DVec3::ZERO, DVec3::Z, pinhole(), IMAGE),
        ViewPose::look_at("shallow", shallow, DVec3::ZERO, DVec3::Z, pinhole(), IMAGE),
    ];
    let rig = rig_of(views.clone(), N_BK7, surround_n);
    (views, rig)
}

#[test]
fn a_ray_beyond_the_critical_angle_is_flagged_and_others_are_not() {
    let mesh = cube_mesh(10.0);
    // In a liquid denser than the stone (n 1.74 against 1.5168) light entering at more than
    // asin(1.5168 / 1.74) = 60.7 degrees is totally reflected.
    let (views, dense) = steep_and_shallow(1.74);
    let scene = Scene::new(&mesh, &dense, Rigid::IDENTITY);
    let steep_pixel = views[0]
        .project(DVec3::new(10.0, 5.0, 0.0))
        .expect("visible");
    assert_eq!(
        trace_pixel(&scene, 0, steep_pixel, 0).expect_err("beyond the critical angle"),
        TraceError::TotalInternalReflection
    );
    let shallow_pixel = views[1].project(DVec3::ZERO).expect("visible");
    let shallow = trace_pixel(&scene, 1, shallow_pixel, 0).expect("a shallow ray enters");
    assert!(
        (5.0..7.0).contains(&shallow.incidence_deg),
        "{}",
        shallow.incidence_deg
    );
    // In air the steep ray enters as well, at about 80 degrees.
    let (_, air) = steep_and_shallow(1.0);
    let air_scene = Scene::new(&mesh, &air, Rigid::IDENTITY);
    let steep = trace_pixel(&air_scene, 0, steep_pixel, 0).expect("air never reflects at entry");
    assert!(steep.incidence_deg > 75.0, "{}", steep.incidence_deg);
    // A mark on that view is reported, not used.
    let marks = [
        super::Mark {
            view: 0,
            pixel: steep_pixel.to_array(),
        },
        super::Mark {
            view: 1,
            pixel: shallow_pixel.to_array(),
        },
    ];
    let error = locate_point(&scene, &marks, &LocateOptions::default())
        .expect_err("one usable view is not enough");
    let LocateError::TooFewViews {
        usable,
        views: reports,
    } = error
    else {
        panic!("expected TooFewViews");
    };
    assert_eq!(usable, 1);
    assert_eq!(
        reports[0].status,
        super::ViewStatus::TotalInternalReflection
    );
    assert_eq!(reports[1].status, super::ViewStatus::Used);
}

#[test]
fn eight_views_with_half_pixel_noise_locate_a_point_in_a_cube() {
    let mesh = cube_mesh(10.0);
    let rig = rig_of(octant_views(pinhole()), N_BK7, 1.0);
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let target = DVec3::new(1.7, -2.3, 3.1);
    let mut rng = Lcg::new(7);
    let marks = marks_for(&scene, target, 1e-4, &mut rng, NOISE_PX);
    assert_eq!(marks.len(), 8, "every octant sees an exact image");
    let located = locate_point(&scene, &marks, &LocateOptions::default()).expect("8 views");
    assert_eq!(located.used_views, 8);
    assert_eq!(located.inside, InsideState::Inside);
    let error = (located.point_vec() - target).length();
    assert!(error < 6.0 * SIGMA_MM, "position error {error} mm");
    // The uncertainty is consistent with the noise: about 1.3 sigma for 8 views.
    assert!(
        located.rms_mm > 0.3 * SIGMA_MM && located.rms_mm < 5.0 * SIGMA_MM,
        "rms {} mm against sigma {SIGMA_MM} mm",
        located.rms_mm
    );
    assert!(located.sigma_mm > 0.3 * SIGMA_MM && located.sigma_mm < 5.0 * SIGMA_MM);
    assert_eq!(located.outlier_views(), [] as [usize; 0]);
    assert!(located.views.iter().all(|view| !view.beyond_segment));
    // The measured uncertainty is below the default margin, so the default wins.
    assert!((located.suggested_margin_mm() - 0.3).abs() < 1e-12);
}

#[test]
fn eight_views_locate_a_point_in_a_sphere_like_mesh() {
    let (points, tris) = icosphere(2, 10.0);
    let mesh = RoughMesh::new(&points, &tris).expect("a closed icosphere");
    let rig = rig_of(octant_views(pinhole()), N_BK7, 1.0);
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let target = DVec3::new(1.0, -2.0, 1.5);
    let mut rng = Lcg::new(3);
    // The facets make the surface piecewise flat, so a view whose exact image falls into a gap
    // between facet directions is left out; the others are exact to a hundredth of a mm.
    let marks = marks_for(&scene, target, 0.01, &mut rng, NOISE_PX);
    assert!(
        marks.len() >= 5,
        "only {} views have an exact image",
        marks.len()
    );
    let located = locate_point(&scene, &marks, &LocateOptions::default()).expect("enough views");
    let error = (located.point_vec() - target).length();
    assert!(error < 8.0 * SIGMA_MM, "position error {error} mm");
    assert!(located.rms_mm < 6.0 * SIGMA_MM, "rms {} mm", located.rms_mm);
}

#[test]
fn noise_free_marks_reproject_onto_themselves() {
    let mesh = cube_mesh(10.0);
    let rig = rig_of(octant_views(pinhole()), N_BK7, 1.0);
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let target = DVec3::new(-3.2, 4.1, -1.3);
    let mut rng = Lcg::new(1);
    let marks = marks_for(&scene, target, 1e-4, &mut rng, 0.0);
    assert_eq!(marks.len(), 8);
    let located = locate_point(&scene, &marks, &LocateOptions::default()).expect("8 views");
    assert!((located.point_vec() - target).length() < 1e-4);
    assert!(located.rms_mm < 1e-4);
    for mark in &marks {
        let image = reproject(&scene, mark.view, located.point_vec(), Some(mark.pixel))
            .expect("the solved point has an image");
        let error = image.error_px.expect("a mark was given");
        assert!(error < 0.05, "view {}: {error} px", mark.view);
    }
}

#[test]
fn a_mark_replaced_by_a_ghost_image_is_flagged_by_leave_one_out() {
    let mesh = cube_mesh(10.0);
    let rig = rig_of(octant_views(pinhole()), N_BK7, 1.0);
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    // A point 2 mm from the -y wall. From the octant at +x +y +z the ray refracts at the top face
    // by asin(sin(54.7 degrees) / 1.5168) = 32.6 degrees from the vertical, runs down and toward
    // -x and -y, meets the -y wall at 67.6 degrees (beyond the critical angle of 41.2 degrees, so
    // it is reflected totally) and then passes through the point: unfolded, the path is the
    // straight line from the entry point at about (6.1, -7.9, 10) to the point's mirror image
    // (2, -12, 1) in the wall, which the refracted ray of that pixel hits.
    let target = DVec3::new(2.0, -8.0, 1.0);
    let mut rng = Lcg::new(11);
    let mut marks = marks_for(&scene, target, 1e-4, &mut rng, NOISE_PX);
    assert_eq!(marks.len(), 8);
    let ghost = (0..8)
        .find_map(|view| {
            predict_ghosts(&scene, view, target, 1, 0.01)
                .first()
                .copied()
        })
        .expect("the octant at +x +y +z has a one-bounce ghost of this point");
    assert_eq!(ghost.bounces, 1);
    assert_eq!(marks[ghost.view].view, ghost.view);
    marks[ghost.view].pixel = ghost.pixel;
    let located = locate_point(&scene, &marks, &LocateOptions::default()).expect("8 marks");
    // The ghost's own ray, before the reflection, passes 2 * 2 mm * sin(67.6 degrees) = 3.7 mm from
    // the point; the seven other rays agree to a few hundredths of a millimetre.
    assert_eq!(located.outlier_views(), [ghost.view]);
    let flagged = &located.views[ghost.view];
    assert!(flagged.leave_one_out_mm.expect("tested") > 1.0);
}

#[test]
fn immersion_in_a_liquid_of_the_stones_index_is_straight_line_triangulation() {
    let mesh = cube_mesh(10.0);
    let rig = rig_of(octant_views(pinhole()), N_BK7, N_BK7);
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let target = DVec3::new(1.7, -2.3, 3.1);
    // Without noise the straight lines meet exactly at the target.
    let mut rng = Lcg::new(5);
    let exact = marks_for(&scene, target, 1e-4, &mut rng, 0.0);
    assert_eq!(exact.len(), 8);
    let located = locate_point(&scene, &exact, &LocateOptions::default()).expect("8 views");
    assert!((located.point_vec() - target).length() < 1e-4);
    // With noise the result is exactly the closest point of the camera rays.
    let noisy = marks_for(&scene, target, 1e-4, &mut rng, NOISE_PX);
    let located = locate_point(&scene, &noisy, &LocateOptions::default()).expect("8 views");
    let lines: Vec<Line> = noisy
        .iter()
        .map(|mark| {
            let (origin, dir) = scene
                .camera_ray(mark.view, DVec2::from_array(mark.pixel))
                .expect("the view exists");
            Line { origin, dir }
        })
        .collect();
    let straight = closest_point_to_lines(&lines).expect("the rays are not parallel");
    assert!((located.point_vec() - straight).length() < 1e-6);
}

#[test]
fn a_birefringent_stone_is_traced_with_the_ordinary_index() {
    let mesh = cube_mesh(10.0);
    // Calcite: n_o 1.658, n_e 1.486. The marks are the ordinary images.
    let ordinary = rig_of(octant_views(pinhole()), 1.658, 1.0);
    let extraordinary = rig_of(octant_views(pinhole()), 1.486, 1.0);
    let scene_o = Scene::new(&mesh, &ordinary, Rigid::IDENTITY);
    let scene_e = Scene::new(&mesh, &extraordinary, Rigid::IDENTITY);
    let target = DVec3::new(5.0, -4.0, 6.0);
    let mut rng = Lcg::new(9);
    let marks = marks_for(&scene_o, target, 1e-4, &mut rng, 0.0);
    assert_eq!(marks.len(), 8);
    let options = LocateOptions::default();
    let with_n_o = locate_point(&scene_o, &marks, &options).expect("8 views");
    let with_n_e = locate_point(&scene_e, &marks, &options).expect("8 views");
    let error_o = (with_n_o.point_vec() - target).length();
    let error_e = (with_n_e.point_vec() - target).length();
    assert!(error_o < 1e-3, "{error_o}");
    assert!(
        error_e > 5e-3 && error_e > 10.0 * error_o,
        "{error_e} against {error_o}"
    );
}

#[test]
fn polylines_are_located_vertex_by_vertex() {
    let mesh = cube_mesh(10.0);
    let rig = rig_of(octant_views(pinhole()), N_BK7, 1.0);
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let vertices = [
        DVec3::new(-3.0, -2.0, 1.0),
        DVec3::new(2.0, 3.0, -1.5),
        DVec3::new(4.0, -3.0, 2.0),
    ];
    let mut polylines: Vec<ViewPolyline> = (0..8)
        .map(|view| ViewPolyline {
            view,
            pixels: vertices
                .iter()
                .map(|&vertex| {
                    reproject(&scene, view, vertex, None)
                        .expect("an image in every octant view")
                        .pixel
                })
                .collect(),
        })
        .collect();
    let options = LocateOptions::default();
    let located = locate_polyline(&scene, &polylines, false, &options).expect("a polyline");
    assert_eq!(located.vertices.len(), 3);
    assert!(!located.closed);
    for (found, truth) in located.vertices.iter().zip(vertices) {
        assert!((found.point_vec() - truth).length() < 1e-3);
    }
    assert!(located.worst_rms_mm() < 1e-3);
    assert!((located.suggested_margin_mm() - 0.3).abs() < 1e-12);
    // Views that disagree on the vertex count cannot be matched vertex by vertex.
    polylines[2].pixels.pop();
    assert_eq!(
        locate_polyline(&scene, &polylines, true, &options).expect_err("mismatch"),
        LocateError::MismatchedVertexCount
    );
}

#[test]
fn the_margin_is_the_larger_of_the_default_and_twice_the_uncertainty() {
    assert!((suggested_margin_mm(0.05) - 0.3).abs() < 1e-12);
    assert!((suggested_margin_mm(0.5) - 1.0).abs() < 1e-12);
    assert!((suggested_margin_mm(0.0) - 0.3).abs() < 1e-12);
}

#[test]
fn a_located_point_becomes_a_closed_shell_that_the_rough_accepts() {
    let centre = DVec3::new(1.0, 2.0, 3.0);
    let (points, tris) = sphere_shell(centre, 1.0);
    let mesh = RoughMesh::new(&points, &tris).expect("a closed shell");
    let sphere = 4.0 / 3.0 * std::f64::consts::PI;
    let ratio = mesh.volume() / sphere;
    // An icosahedron whose faces touch the sphere holds 1.207 times its volume.
    assert!((1.15..1.25).contains(&ratio), "{ratio}");
    let (lo, hi) = mesh.bounds();
    assert!(((lo + hi) * 0.5 - centre).length() < 1e-9);

    let base = import_obj(CUBE_OBJ);
    let located = Located {
        point: [10.0, 10.0, 10.0],
        rms_mm: 0.05,
        sigma_mm: 0.05,
        used_views: 8,
        inside: InsideState::Inside,
        views: Vec::new(),
    };
    let shell = located.to_shell(1.0);
    assert!((shell.margin_mm - 0.3).abs() < 1e-12);
    assert!(shell.mesh().is_ok());
    let with = add_located_point(&base, &located, 1.0).expect("it fits in the middle of the cube");
    assert_ne!(base, with);
    // A point on the surface does not fit.
    let on_surface = Located {
        point: [0.0, 10.0, 10.0],
        ..located
    };
    assert!(add_located_point(&base, &on_surface, 1.0).is_err());
}

#[test]
fn a_rig_profile_round_trips_through_json() {
    let views = RigProfile::side_layout(DISTANCE, 30.0, pinhole(), IMAGE);
    assert_eq!(views.len(), 8);
    let mut rig = rig_of(views, 1.54, 1.0);
    assert!(rig.validate().is_ok());
    rig.calibration = Some(super::CalibrationResult {
        datasheet_edge_mm: 25.4,
        tolerance_mm: 0.1,
        cube_n_d: N_BK7,
        fitted_edge_mm: 25.43,
        scale_ratio: 25.43 / 25.4,
        scale_within_tolerance: true,
        edge_rms_px: 0.4,
        lines_used: 70,
        diagonal_rms_mm: Some(0.08),
        diagonal_plane_rms_mm: Some(0.05),
    });
    let text = serde_json::to_string(&rig).expect("serialises");
    let back: RigProfile = serde_json::from_str(&text).expect("reads back");
    assert_eq!(back, rig);
    let broken = RigProfile {
        stone_n: 0.5,
        ..rig
    };
    assert!(broken.validate().is_err());
}
