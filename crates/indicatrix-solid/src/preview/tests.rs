//! The identity test (the web app's pipeline entry points reproduce the desktop's
//! pre-move frame bytes) and the new pure helpers' tests.

use super::{
    CameraPose, FacetOverlay, PlanJob, PreviewPipeline, RedrawRequest, WorkerMemory,
    build_planned_frame, camera, render_request,
    view::{self, RasterLimits, ReplanBasis, ReplanInputs},
};
use crate::{
    diagram2d::PanelKind, live_update, live_update::Clock, mesh_cache::MeshCache,
    raster::SolidRasterizer,
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
use std::{collections::BTreeSet, sync::Arc};

/// A clock that never advances: every fixture here is pinned (no solver call), so
/// the budget check never runs -- the desktop's `Instant` clock would give the
/// same result.
struct ZeroClock;

impl Clock for ZeroClock {
    fn now_ms(&self) -> f64 {
        0.0
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    crate::mesh_cache::fnv1a_64(bytes.iter().copied())
}

fn fnv_u32(values: &[u32]) -> u64 {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    fnv(&bytes)
}

fn rbc_tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(mast),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// The desktop pins' fixture (`preview_state::tests::pins::pin_design`), verbatim.
fn pin_design() -> Design {
    const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    const BREAK: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    const MAIN: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    const STAR: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: "GemCad 5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 8,
            mirror: true,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        },
        vec![
            rbc_tier("Table", 0.0, &[], 0.32),
            rbc_tier("Star", 15.0, &STAR, 0.45),
            rbc_tier("Crown Main", 34.5, &MAIN, 0.59),
            rbc_tier("Upper Girdle", 41.0, &BREAK, 0.67),
            rbc_tier("Girdle", 90.0, &GIRDLE, 1.0),
            rbc_tier("Pavilion Main", -41.0, &MAIN, 0.67),
            rbc_tier("Lower Girdle", -42.5, &BREAK, 0.68),
            rbc_tier("Culet", -0.0, &[], 0.88),
        ],
    )
}

const PIN_CAMERA: CameraPose = CameraPose {
    yaw: 0.6,
    pitch: 0.45,
    distance: 2.4,
};

fn pin_overlay() -> FacetOverlay {
    FacetOverlay {
        hovered: Some(20),
        selected_facet: Some(21),
        multi_selected: vec![30, 31],
        provisional: Vec::new(),
        moved: Vec::new(),
    }
}

// The desktop's pins, recorded from its pipeline BEFORE the move
// (`apps/indicatrix-cut/src/gui/solid_preview/preview_state/tests/pins.rs`):
// `[solid image, solid pick, edges image, diagram image, diagram pick, tooth]`.
const PIN_SOLID: [u64; 6] = [0x0f10_7ca8_5f93_74bf, 0x5f3d_a2da_7a33_a030, 0, 0, 0, 0];
const PIN_BOTH: [u64; 6] = [
    0x0f10_7ca8_5f93_74bf,
    0x5f3d_a2da_7a33_a030,
    0x21d0_25ae_bd67_13d0,
    0,
    0,
    0,
];
const PIN_DIAGRAM: [u64; 6] = [
    0x20dd_549d_dad6_8174,
    0xa8f1_2ee3_d05d_e3be,
    0,
    0x98f7_6d59_8966_3328,
    0xd1a3_9e15_eccb_fb54,
    0xb0c8_4221_f413_1c25,
];
const PIN_SINGLE_PANEL: [u64; 6] = [
    0x61f0_28b7_7429_a350,
    0x2c7a_1a3e_c524_5679,
    0,
    0xbdeb_c6f9_ce8b_811b,
    0x4cf4_6279_7dc9_2530,
    0x0f8f_adba_ccdc_0009,
];

/// The job exactly as the desktop's `submit_preview_replan_for` builds it for
/// the pins (`pin_job` there): full solve, tier 2 selected, generation 3.
fn desktop_job(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> PlanJob {
    let design = pin_design();
    let n_d = design.effective_refractive_index();
    PlanJob {
        design: Arc::new(design),
        dirty: BTreeSet::new(),
        last_solved: None,
        camera: PIN_CAMERA,
        size,
        selected_tier: Some(2),
        n_d,
        view_mode,
        generation: 3,
        show_preform: true,
        enlarged_panel,
        tier_cutoff: None,
    }
}

/// The same replan as the web app's `src/views` builds it: [`view::plan_job`]
/// from [`ReplanInputs`] in their UI encodings.
fn web_job(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> PlanJob {
    view::plan_job(ReplanInputs {
        design: Arc::new(pin_design()),
        generation: 3,
        basis: ReplanBasis::FullSolve,
        camera: PIN_CAMERA,
        size,
        selected_tier: Some(2),
        custom_materials: &[],
        view_mode,
        show_preform: true,
        enlarged_panel,
        tier_cutoff: -1,
    })
}

/// The web app's path: [`PreviewPipeline::replan`] then
/// [`PreviewPipeline::update_overlay`], hashed from the pipeline's own buffers.
fn web_hashes(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> [u64; 6] {
    let mut pipeline = PreviewPipeline::new();
    pipeline
        .replan(
            web_job(view_mode, size, enlarged_panel),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("a planned request always resolves");
    let frame = pipeline
        .update_overlay(pin_overlay())
        .expect("an overlay after a frame always resolves");
    let diagram = frame.diagram.as_ref();
    [
        fnv(pipeline.solid_rgba()),
        fnv_u32(&frame.pick.pick),
        if frame.has_edges {
            fnv(pipeline.edges_rgba())
        } else {
            0
        },
        diagram.map_or(0, |d| fnv(&d.color)),
        diagram.map_or(0, |d| fnv_u32(&d.pick)),
        diagram.map_or(0, |d| fnv_u32(&d.tooth)),
    ]
}

/// The desktop's path, called directly: its PLAN worker's `build_planned_frame`
/// and its RENDER worker's `render_request` on worker-owned state.
fn desktop_hashes(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> [u64; 6] {
    let mut mesh_cache = MeshCache::default();
    let mut rasterizer = SolidRasterizer::new(1, 1);
    let mut edges_rasterizer = SolidRasterizer::new(1, 1);
    let mut memory = WorkerMemory::default();
    let planned = build_planned_frame(
        desktop_job(view_mode, size, enlarged_panel),
        live_update::DEFAULT_PREVIEW_BUDGET,
        &ZeroClock,
    );
    render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        RedrawRequest::Planned(Box::new(planned)),
    )
    .expect("a planned request always resolves");
    let frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        RedrawRequest::UpdateFacetOverlay(pin_overlay()),
    )
    .expect("an overlay after a frame always resolves");
    let diagram = frame.diagram.as_ref();
    [
        fnv(&rasterizer.color),
        fnv_u32(&frame.pick.pick),
        if frame.has_edges {
            fnv(&edges_rasterizer.color)
        } else {
            0
        },
        diagram.map_or(0, |d| fnv(&d.color)),
        diagram.map_or(0, |d| fnv_u32(&d.pick)),
        diagram.map_or(0, |d| fnv_u32(&d.tooth)),
    ]
}

#[test]
fn web_plan_job_matches_the_desktop_job_field_by_field() {
    let web = web_job(3, (360, 180), 1);
    let desktop = desktop_job(3, (360, 180), 1);
    assert_eq!(web.dirty, desktop.dirty);
    assert_eq!(web.last_solved.is_none(), desktop.last_solved.is_none());
    assert_eq!(web.camera, desktop.camera);
    assert_eq!(web.size, desktop.size);
    assert_eq!(web.selected_tier, desktop.selected_tier);
    assert_eq!(web.n_d.to_bits(), desktop.n_d.to_bits());
    assert_eq!(web.view_mode, desktop.view_mode);
    assert_eq!(web.generation, desktop.generation);
    assert_eq!(web.show_preform, desktop.show_preform);
    assert_eq!(web.enlarged_panel, desktop.enlarged_panel);
    assert_eq!(web.tier_cutoff, desktop.tier_cutoff);
    assert_eq!(web.design.tiers, desktop.design.tiers);
}

/// The web and the desktop pipeline entry points produce the same frames, and on
/// Windows those frames are the pinned ones. The pins hold Windows bits: the
/// rasteriser's projection (`tan`) and facet angles (`sin`, `cos`) and the diagram
/// layout's `sin_cos` run through the platform math library, which rounds a few
/// arguments differently under glibc, so a boundary pixel moves and the hashes differ
/// on Linux (seen 2026-10-01). Off Windows only the web-versus-desktop equality is
/// checked, which holds on any platform since both run on the same library.
#[test]
fn web_frames_reproduce_the_desktop_pins() {
    for (view_mode, size, enlarged, pin) in [
        (0, (160, 120), -1, PIN_SOLID),
        (2, (160, 120), -1, PIN_BOTH),
        (3, (360, 180), -1, PIN_DIAGRAM),
        (3, (240, 240), 1, PIN_SINGLE_PANEL),
    ] {
        let desktop = desktop_hashes(view_mode, size, enlarged);
        let web = web_hashes(view_mode, size, enlarged);
        assert_eq!(
            web, desktop,
            "view mode {view_mode}: web and desktop paths differ"
        );
        if cfg!(windows) {
            assert_eq!(
                web, pin,
                "view mode {view_mode}: differs from the pre-move pin"
            );
        }
    }
}

#[test]
fn a_reproject_keeps_the_planned_style_and_follows_the_camera() {
    let mut pipeline = PreviewPipeline::new();
    assert!(
        pipeline.reproject(PIN_CAMERA, (64, 48), 0, None).is_none(),
        "nothing planned yet"
    );
    pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let before = fnv(pipeline.solid_rgba());
    let turned = camera::orbit_step(PIN_CAMERA, 40.0, 0.0);
    let frame = pipeline
        .reproject(turned, (64, 48), 0, None)
        .expect("reprojected");
    assert!(frame.has_solid);
    assert_eq!(
        frame.generation, 3,
        "a reproject keeps the planned generation"
    );
    assert_ne!(
        fnv(pipeline.solid_rgba()),
        before,
        "the pose changed the pixels"
    );
    assert!(
        pipeline.memory().style.selected.iter().any(|&s| s),
        "the planned selection tint survives a camera move"
    );
}

#[test]
fn a_replan_frame_carries_geometry_at_the_request_size_and_pose() {
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let geometry = frame
        .geometry
        .expect("a closed design always carries geometry");
    assert_eq!(geometry.size, (64, 48), "the size the raster used");
    assert_eq!(geometry.camera, PIN_CAMERA, "the pose the raster used");
    assert!(!geometry.corner_points.is_empty());
    assert_eq!(
        geometry.facet_centroids.len(),
        frame.planes.len(),
        "one centroid slot per plane"
    );
    assert!(geometry.facet_centroids.iter().any(Option::is_some));
    assert!(geometry.bounding_radius > 0.0);
    assert!((geometry.bounding_radius - frame.mesh_bounding_radius).abs() < 1e-12);
}

#[test]
fn a_reproject_frame_carries_geometry_shared_with_the_replan() {
    let mut pipeline = PreviewPipeline::new();
    let planned = pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned")
        .geometry
        .expect("planned geometry");
    let turned = camera::orbit_step(PIN_CAMERA, 40.0, 0.0);
    let reprojected = pipeline
        .reproject(turned, (80, 60), 0, None)
        .expect("reprojected")
        .geometry
        .expect("a reproject carries geometry too");
    assert_eq!(reprojected.size, (80, 60));
    assert_eq!(reprojected.camera, turned);
    assert!(
        Arc::ptr_eq(&planned.corner_points, &reprojected.corner_points)
            && Arc::ptr_eq(&planned.facet_centroids, &reprojected.facet_centroids),
        "the corner and centroid tables are built once per mesh, not per frame"
    );
}

#[test]
fn shared_outlines_are_stamped_onto_any_style_and_absent_ones_change_nothing() {
    use super::{Outlines, SharedOutlines};
    use crate::raster::SolidStyle;
    use std::sync::Mutex;

    let plain = WorkerMemory::default();
    let untouched = plain.with_outlines(SolidStyle {
        provisional: vec![7],
        ..SolidStyle::default()
    });
    assert_eq!(
        untouched.provisional,
        vec![7],
        "no shared handle: the request's own style"
    );

    let shared: SharedOutlines = Arc::new(Mutex::new(Outlines::default()));
    let memory = WorkerMemory {
        outlines: Some(Arc::clone(&shared)),
        ..WorkerMemory::default()
    };
    // A style a `Planned` request built from scratch: empty outlines.
    let empty = memory.with_outlines(SolidStyle::default());
    assert!(empty.provisional.is_empty() && empty.moved.is_empty());

    *shared.lock().expect("lock") = Outlines {
        provisional: vec![3, 4],
        moved: vec![9],
    };
    let stamped = memory.with_outlines(SolidStyle::default());
    assert_eq!(stamped.provisional, vec![3, 4]);
    assert_eq!(stamped.moved, vec![9]);
    // Cleared by the app: the very next draw drops them, whatever request it serves.
    *shared.lock().expect("lock") = Outlines::default();
    let cleared = memory.with_outlines(stamped);
    assert!(cleared.provisional.is_empty() && cleared.moved.is_empty());
}

#[test]
fn a_pipeline_that_never_closed_a_solid_has_no_geometry() {
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .render(RedrawRequest::Reproject {
            planes: vec![(glam::Vec3::X, 1.0), (glam::Vec3::NEG_X, 1.0)],
            camera: PIN_CAMERA,
            size: (32, 32),
            view_mode: 0,
            gear: None,
        })
        .expect("a reproject always resolves");
    assert!(!frame.has_solid);
    assert!(frame.geometry.is_none());
}

#[test]
fn orbit_step_uses_the_desktop_signs_factor_and_wrap() {
    let pose = CameraPose {
        yaw: 1.0,
        pitch: 0.5,
        distance: 3.0,
    };
    let moved = camera::orbit_step(pose, 10.0, -5.0);
    assert!((moved.yaw - 0.92).abs() < 1e-6);
    assert!((moved.pitch - 0.46).abs() < 1e-6);
    assert_eq!(moved.distance, 3.0);
    let over_the_pole = camera::orbit_step(CameraPose { pitch: 3.1, ..pose }, 0.0, 10.0);
    assert!(over_the_pole.pitch < 0.0, "pitch wraps into [-pi, pi)");
}

#[test]
fn zoom_step_moves_closer_on_scroll_up_and_clamps_to_the_mesh() {
    assert!((camera::zoom_step(2.4, 100.0, 1.5) - 2.2).abs() < 1e-6);
    assert!((camera::zoom_step(2.4, -100.0, 1.5) - 2.6).abs() < 1e-6);
    assert!((camera::zoom_step(1.3, 1000.0, 1.5) - 1.2).abs() < 1e-6);
    assert!((camera::zoom_step(7.9, -1000.0, 1.5) - 8.0).abs() < 1e-5);
}

#[test]
fn standard_views_match_the_desktop_pose_pills() {
    use std::f32::consts::FRAC_PI_2;
    let pose = camera::RESET_POSE;
    let top = camera::standard_view(1, pose, 1.5);
    assert_eq!((top.yaw, top.pitch, top.distance), (0.0, FRAC_PI_2, 2.4));
    let right = camera::standard_view(4, pose, 1.5);
    assert_eq!((right.yaw, right.pitch), (FRAC_PI_2, 0.0));
    let front = camera::standard_view(99, pose, 1.5);
    assert_eq!((front.yaw, front.pitch), (0.0, 0.0));
    let fit = camera::standard_view(5, pose, 1.5);
    assert_eq!(
        (fit.yaw, fit.pitch),
        (pose.yaw, pose.pitch),
        "Fit keeps the angle"
    );
    let (min, max) = camera::orbit_distance_bounds(1.5);
    assert!(fit.distance >= min && fit.distance <= max);
}

const LIMITS: RasterLimits = RasterLimits {
    max_device_pixel_ratio: 1.5,
    min_edge: 64,
    max_edge: 960,
};

#[test]
fn raster_size_keeps_the_aspect_and_caps_the_longer_edge() {
    assert_eq!(
        view::raster_size_for_view(800.0, 600.0, 1.0, LIMITS),
        (800, 600)
    );
    assert_eq!(
        view::raster_size_for_view(400.0, 300.0, 2.0, LIMITS),
        (600, 450)
    );
    assert_eq!(
        view::raster_size_for_view(1600.0, 800.0, 1.0, LIMITS),
        (960, 480)
    );
    assert_eq!(
        view::raster_size_for_view(0.0, f32::NAN, 1.0, LIMITS),
        (64, 64)
    );
    assert_eq!(
        view::raster_size_for_view(300.0, 200.0, 0.5, LIMITS),
        (300, 200)
    );
}

#[test]
fn a_narrow_solid_view_is_drawn_at_the_minimum_aspect_and_a_wide_one_is_left_alone() {
    let four_thirds = 4.0 / 3.0;
    // 560 x 520 is narrower than 4:3: the height drops to 420 (560 / 4:3).
    assert_eq!(
        view::with_min_aspect((560, 520), four_thirds, 64),
        (560, 420)
    );
    // Tall: same width, a much shorter raster.
    assert_eq!(
        view::with_min_aspect((400, 800), four_thirds, 64),
        (400, 300)
    );
    // Already 4:3 or wider: untouched.
    assert_eq!(
        view::with_min_aspect((800, 600), four_thirds, 64),
        (800, 600)
    );
    assert_eq!(
        view::with_min_aspect((1000, 500), four_thirds, 64),
        (1000, 500)
    );
    // The minimum edge holds, and the height never grows.
    assert_eq!(view::with_min_aspect((64, 900), four_thirds, 64), (64, 64));
    assert_eq!(view::with_min_aspect((100, 60), 0.0, 64), (100, 60));
}

#[test]
fn diagram_raster_grows_a_small_view_to_the_minimum_width_and_keeps_the_aspect() {
    let limits = RasterLimits {
        max_edge: 1600,
        ..LIMITS
    };
    // Already wide enough: exactly `raster_size_for_view`.
    assert_eq!(
        view::diagram_raster_size(1300.0, 700.0, 1.0, limits, 1200),
        view::raster_size_for_view(1300.0, 700.0, 1.0, limits)
    );
    // A narrow view is drawn 1200 wide, same aspect.
    assert_eq!(
        view::diagram_raster_size(600.0, 400.0, 1.0, limits, 1200),
        (1200, 800)
    );
    // A tall view hits the long-edge cap first: uniformly scaled down, aspect kept.
    let (w, h) = view::diagram_raster_size(390.0, 700.0, 1.0, limits, 1200);
    assert_eq!(h, 1600);
    assert!(
        (w as f32 / h as f32 - 390.0 / 700.0).abs() < 0.01,
        "{w}x{h}"
    );
    // A zero-size view still gets a valid (positive) raster.
    let (w, h) = view::diagram_raster_size(0.0, 0.0, 1.0, limits, 1200);
    assert!(w >= limits.min_edge && h >= limits.min_edge);
}

#[test]
fn contain_pixel_maps_through_the_letterbox() {
    // A 100x50 image in a 200x200 view: scale 2, drawn at y 50..150.
    assert_eq!(
        view::contain_pixel(0.0, 50.0, 200.0, 200.0, 100, 50),
        Some((0, 0))
    );
    assert_eq!(
        view::contain_pixel(199.0, 149.0, 200.0, 200.0, 100, 50),
        Some((99, 49))
    );
    assert_eq!(
        view::contain_pixel(100.0, 20.0, 200.0, 200.0, 100, 50),
        None
    );
    assert_eq!(
        view::contain_pixel(100.0, 150.0, 200.0, 200.0, 100, 50),
        None
    );
    assert_eq!(view::contain_pixel(10.0, 10.0, 0.0, 200.0, 100, 50), None);
    // Same aspect: a plain scale.
    assert_eq!(
        view::contain_pixel(30.0, 20.0, 400.0, 300.0, 800, 600),
        Some((60, 40))
    );
}

/// The web's pointer -> pick-frame conversion: the fractional, unclamped image position
/// under a logical pointer, through the same letterboxing `contain_pixel` uses.
#[test]
fn contain_fit_converts_the_pointer_to_the_pick_frame_through_the_letterbox() {
    // A 100x50 raster in a 200x200 view: scale 2, bars above and below (drawn y 50..150).
    let fit = view::contain_fit(200.0, 200.0, 100, 50).expect("a real view");
    assert_eq!((fit.offset_x, fit.offset_y, fit.scale), (0.0, 50.0, 2.0));
    assert_eq!(fit.to_image(0.0, 50.0), (0.0, 0.0));
    assert_eq!(fit.to_image(100.0, 100.0), (50.0, 25.0));
    assert_eq!(fit.to_image(200.0, 150.0), (100.0, 50.0));
    assert_eq!(fit.to_view(50.0, 25.0), (100.0, 100.0));
    // Fractional pixels survive (a handle drag is measured in sub-pixels).
    assert_eq!(fit.to_image(101.0, 101.0), (50.5, 25.5));
    // A pointer in a bar maps outside the raster instead of being clamped, so a drag
    // that leaves the image keeps its travel.
    assert_eq!(fit.to_image(100.0, 20.0), (50.0, -15.0));
    assert_eq!(fit.to_image(100.0, 190.0), (50.0, 70.0));
    // Empty views and images have no placement.
    assert_eq!(view::contain_fit(0.0, 200.0, 100, 50), None);
    assert_eq!(view::contain_fit(200.0, 200.0, 0, 50), None);
}

/// The conversion agrees with `contain_pixel` everywhere inside the image, and the two
/// directions are exact inverses, for the awkward aspects a rounded raster gives.
#[test]
fn contain_fit_agrees_with_contain_pixel_and_round_trips() {
    // (view size, raster size): a 1.5x display, an odd width, a capped long edge.
    let cases = [
        ((811.3_f32, 519.7_f32), (1217_u32, 780_u32)),
        ((640.0, 360.0), (960, 540)),
        ((333.3, 900.1), (500, 1350)),
        ((1200.0, 500.0), (960, 400)),
    ];
    for ((view_w, view_h), (image_w, image_h)) in cases {
        let fit = view::contain_fit(view_w, view_h, image_w, image_h).expect("a real view");
        for step_x in 0..=20 {
            for step_y in 0..=20 {
                let x = view_w * step_x as f32 / 20.0;
                let y = view_h * step_y as f32 / 20.0;
                let (px, py) = fit.to_image(x, y);
                let inside = px >= 0.0 && py >= 0.0 && px < image_w as f32 && py < image_h as f32;
                let pixel = view::contain_pixel(x, y, view_w, view_h, image_w, image_h);
                assert_eq!(
                    pixel,
                    inside.then(|| (px.floor() as u32, py.floor() as u32)),
                    "({x}, {y}) in {view_w}x{view_h} for {image_w}x{image_h}"
                );
                let (back_x, back_y) = fit.to_view(px, py);
                assert!(
                    (back_x - x).abs() < 1e-2 && (back_y - y).abs() < 1e-2,
                    "({x}, {y}) came back as ({back_x}, {back_y})"
                );
            }
        }
    }
}

#[test]
fn a_double_click_enlarges_the_panel_under_the_pointer_and_toggles_back() {
    assert_eq!(
        view::toggle_enlarged_panel(-1, Some(PanelKind::Pavilion)),
        1
    );
    assert_eq!(view::toggle_enlarged_panel(-1, None), -1);
    assert_eq!(
        view::toggle_enlarged_panel(1, Some(PanelKind::Pavilion)),
        -1
    );
    assert_eq!(view::toggle_enlarged_panel(2, None), -1);
    for panel in [PanelKind::Crown, PanelKind::Pavilion, PanelKind::Profile] {
        assert_eq!(
            super::panel_kind_from_index(view::panel_index(panel)),
            Some(panel)
        );
    }
}

#[test]
fn a_real_diagram_double_click_resolves_its_panel() {
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .replan(
            web_job(3, (360, 180), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let diagram = frame.diagram.expect("the fixture closes");
    // The three panels sit side by side; the middle of each third is inside it.
    let panel_at_view = |x: f32| {
        view::contain_pixel(x, 180.0, 720.0, 360.0, diagram.width, diagram.height)
            .and_then(|(px, py)| diagram.panel_at(px, py))
    };
    let crown = panel_at_view(120.0);
    assert_eq!(crown, Some(PanelKind::Crown));
    assert_eq!(view::toggle_enlarged_panel(-1, crown), 0);
}

#[test]
fn step_selection_follows_the_desktop_rule() {
    assert_eq!(view::step_selection(None, 1, 5), Some(0));
    assert_eq!(view::step_selection(None, -1, 5), Some(4));
    assert_eq!(view::step_selection(Some(3), 10, 5), Some(4));
    assert_eq!(view::step_selection(Some(3), -10, 5), Some(0));
    assert_eq!(view::step_selection(Some(2), 1, 5), Some(3));
    assert_eq!(view::step_selection(Some(2), 1, 0), Some(2));
}

#[test]
fn facets_of_tiers_collects_every_facet_of_the_multi_selection() {
    let table = [None, Some(0), Some(1), Some(0), Some(2)];
    assert_eq!(
        view::facets_of_tiers(&table, &BTreeSet::from([0, 2])),
        vec![1, 3, 4]
    );
    assert_eq!(
        view::facets_of_tiers(&table, &BTreeSet::new()),
        Vec::<u32>::new()
    );
}

#[test]
fn dirty_tiers_names_edited_tiers_and_refuses_structural_edits() {
    let before = pin_design();
    let mut after = before.clone();
    after.tiers[2].angle_deg += 0.5;
    after.tiers[5].name = "Renamed".to_string();
    assert_eq!(
        view::dirty_tiers(&before, &after),
        Some(BTreeSet::from([2, 5]))
    );
    assert_eq!(view::dirty_tiers(&before, &before), Some(BTreeSet::new()));

    let mut added = before.clone();
    added.tiers.push(rbc_tier("Extra", 20.0, &[], 0.5));
    assert_eq!(view::dirty_tiers(&before, &added), None);
    let mut regeared = before.clone();
    regeared.meta.gear_teeth = 80;
    assert_eq!(view::dirty_tiers(&before, &regeared), None);
}

#[test]
fn a_pinned_design_plans_without_the_solver_and_a_free_tier_does_not() {
    let mut design = pin_design();
    assert!(view::plans_without_solver(&design));
    design.tiers[3].constraint = MeetConstraint::MeetExisting;
    assert!(!view::plans_without_solver(&design));
}

#[test]
fn replan_basis_prefers_the_current_solve_then_the_cached_masts() {
    let design = pin_design();
    let solved = design.solve().expect("pinned");
    let masts = |basis: &ReplanBasis| match basis {
        ReplanBasis::Chain { last_solved, dirty } => Some((
            last_solved.iter().map(|t| t.mast).collect::<Vec<_>>(),
            dirty.clone(),
        )),
        ReplanBasis::FullSolve => None,
    };
    let expected: Vec<f64> = solved.iter().map(|t| t.mast).collect();
    assert_eq!(
        masts(&view::replan_basis(&design, Some(&solved), None)),
        Some((expected.clone(), BTreeSet::new()))
    );
    let mut edited = design.clone();
    edited.tiers[1].angle_deg += 1.0;
    assert_eq!(
        masts(&view::replan_basis(&edited, None, Some((&design, &solved)))),
        Some((expected, BTreeSet::from([1])))
    );
    let mut shorter = design.clone();
    shorter.tiers.pop();
    assert!(
        matches!(
            view::replan_basis(&shorter, Some(&solved), Some((&design, &solved))),
            ReplanBasis::FullSolve
        ),
        "misaligned masts never chain"
    );
}
