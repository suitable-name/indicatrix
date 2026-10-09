//! Zone boundary error (plan section 10.1): within 0.1 mm for boundaries found from marks, within
//! 0.2 mm for boundaries suggested from the residual.
//!
//! The mark-based tests need no tracing: the user's marks are made by projecting known points of
//! the true boundary on the stone's SURFACE through the orthographic cameras (a marker line drawn
//! on the stone), adding sub-pixel click noise, and locating them with `locate_polyline` exactly
//! as the application does. The suggestion test traces the rig.

use std::f64::consts::TAU;

use glam::{DVec2, DVec3};
use indicatrix::optics::zoning::ZoneShape;
use indicatrix_cut_core::rough_plan::{
    colour_fit::{
        forward::evaluate,
        zones::{
            BoundaryPoints, RadialRole, SuggestOptions, SuggestView, fit_half_space, fit_prism,
            suggest_zones,
        },
    },
    locate::{LocateOptions, RigProfile, Rigid, Scene, ViewPolyline, box_mesh},
};

use crate::synth::{
    ColourCase, POSITIONS, RoughKind, STONE_N, Setup, SetupSpec, SplitMix, WATERMELON_APOTHEM_MM,
    ortho_pose, plane_error_mm, prism_error_mm,
};

/// The standard deviation of a click, in photo pixels (10 px per mm: 0.03 mm).
const CLICK_SIGMA_PX: f64 = 0.3;

/// Three cameras that all see the top face (z = +5) and the front face (y = -5) of the cube.
const MARK_POSITIONS: [(f64, f64, f64); 3] = [
    (-60.0, -60.0, 60.0),
    (60.0, -60.0, 60.0),
    (0.0, -80.0, 70.0),
];

fn mark_rig() -> RigProfile {
    let poses = MARK_POSITIONS
        .iter()
        .map(|&(x, y, z)| ortho_pose(DVec3::new(x, y, z), 10.0, 200))
        .collect();
    RigProfile::new("marks", poses, STONE_N)
}

/// The polylines the user would click for the surface points `points`: one polyline per camera,
/// vertex `k` the same physical point in all of them.
fn click_points(rig: &RigProfile, points: &[DVec3], seed: u64) -> Vec<ViewPolyline> {
    let mut rng = SplitMix::new(seed);
    rig.views
        .iter()
        .enumerate()
        .map(|(view, pose)| ViewPolyline {
            view,
            pixels: points
                .iter()
                .map(|&p| {
                    let pixel: DVec2 = pose.project(p).expect("an orthographic camera sees all");
                    [
                        CLICK_SIGMA_PX.mul_add(rng.gauss(), pixel.x),
                        CLICK_SIGMA_PX.mul_add(rng.gauss(), pixel.y),
                    ]
                })
                .collect(),
        })
        .collect()
}

#[test]
fn marks_find_a_half_space_boundary_within_0_1_mm() {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a cube");
    let rig = mark_rig();
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let offset = 0.3;
    // The boundary x = 0.3 meets the top face along a line and the front face along another.
    let mut points = Vec::new();
    for y in [-3.0, 0.0, 3.0] {
        points.push(DVec3::new(offset, y, 5.0));
    }
    for z in [-3.0, 0.0, 3.0] {
        points.push(DVec3::new(offset, -5.0, z));
    }
    let clicks = click_points(&rig, &points, 0xB0D1);
    let located = BoundaryPoints::locate(&scene, &clicks, false, &LocateOptions::default())
        .expect("every mark is located");
    let fitted = fit_half_space(&located, Some(DVec3::X)).expect("a plane fits six points");
    let ZoneShape::HalfSpace {
        normal,
        offset: fitted_offset,
    } = fitted.shape
    else {
        panic!("not a half space: {:?}", fitted.shape);
    };
    let error = plane_error_mm(normal, fitted_offset, DVec3::X, offset, 8.0);
    assert!(
        error <= 0.1,
        "boundary error {error} mm (rms {} mm, normal {normal:?}, offset {fitted_offset})",
        fitted.rms_mm
    );
}

#[test]
fn marks_find_a_prism_boundary_within_0_1_mm() {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a cube");
    let rig = mark_rig();
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    // The trigonal prism core (axis Z, apothem 2 mm) meets the top face in an equilateral
    // triangle; mark two points on each of its sides.
    let circumradius = 2.0 * WATERMELON_APOTHEM_MM;
    let corner = |k: u32| {
        // The side normals are at 0, 120, 240 degrees; the corners lie between them.
        let angle = TAU * (f64::from(k) + 0.5) / 3.0;
        DVec3::new(circumradius * angle.cos(), circumradius * angle.sin(), 5.0)
    };
    let mut points = Vec::new();
    for k in 0..3 {
        let (a, b) = (corner(k), corner((k + 1) % 3));
        points.push(a.lerp(b, 0.25));
        points.push(a.lerp(b, 0.75));
    }
    let clicks = click_points(&rig, &points, 0xB0D2);
    let located = BoundaryPoints::locate(&scene, &clicks, false, &LocateOptions::default())
        .expect("every mark is located");
    // The crystal's axis is known (Z), as the planner provides it.
    let fitted = fit_prism(&located, 3, Some(DVec3::Z), RadialRole::Core)
        .expect("a prism fits six points with a fixed axis");
    let ZoneShape::CoaxialPrism {
        axis_point,
        axis_dir,
        n_sides,
        r_out,
        phase,
        ..
    } = fitted.shape
    else {
        panic!("not a prism: {:?}", fitted.shape);
    };
    let error = prism_error_mm(axis_point, axis_dir, n_sides, r_out, phase, 8.0);
    assert!(
        error <= 0.1,
        "prism boundary error {error} mm (apothem {r_out}, phase {phase}, rms {} mm)",
        fitted.rms_mm
    );
}

/// The residual-suggested boundary (section 10.1: within 0.2 mm). The three cameras look along
/// directions inside the boundary plane x = 0, the case the suggestion's back-projection is
/// exact for (see the docs of `zones::suggest`: an oblique view biases the boundary).
#[test]
fn suggested_boundary_is_within_0_2_mm() {
    let spec = SetupSpec {
        rough: RoughKind::Cube,
        colour: ColourCase::Bicolour,
        offset_mm: 0.0,
        positions: Some(vec![POSITIONS[0], (0.0, -80.0, 60.0), (0.0, 80.0, -60.0)]),
        grid_px: 10,
        samples: 64,
        ..SetupSpec::default()
    };
    let setup = Setup::new(&spec);
    let records = setup.trace();
    let truth = &setup.truth;
    let alpha = |zone: usize, lambda: f64| truth.alpha(zone, lambda);
    let views: Vec<SuggestView> = evaluate(&records, &alpha)
        .iter()
        .map(SuggestView::from_prediction)
        .collect();
    let options = SuggestOptions {
        k: Some(2),
        ..SuggestOptions::default()
    };
    let report = suggest_zones(&setup.scene(), &views, &options).expect("a suggestion report");
    let best = report.suggestions.first().expect("at least one suggestion");
    let ZoneShape::HalfSpace { normal, offset } = best.shape.clone() else {
        panic!("the best suggestion is not a half space: {:?}", best.shape);
    };
    let error = plane_error_mm(normal, offset, DVec3::X, 0.0, 8.0);
    assert!(
        error <= 0.2,
        "suggested boundary error {error} mm (score {}, normal {normal:?}, offset {offset})",
        best.score
    );
}
