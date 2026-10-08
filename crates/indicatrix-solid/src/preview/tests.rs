//! The identity test (the web app's pipeline entry points reproduce the desktop's
//! pre-move frame bytes) and the new pure helpers' tests.

use super::{
    CameraPose, FacetOverlay, PlanJob, PreviewPipeline, RedrawRequest, StoneGeometryBuf,
    WorkerMemory, build_planned_frame, camera, render_request,
    view::{self, RasterLimits, ReplanBasis, ReplanInputs},
};
use crate::{
    diagram2d::PanelKind,
    facet_map::{FacetKind, FacetMap},
    live_update,
    live_update::Clock,
    mesh_cache::MeshCache,
    raster::SolidRasterizer,
};
use indicatrix::geometry::{ToolPrimitive, meet_solver::MeetConstraint};
use indicatrix_cut_core::{
    ConstraintTier, Design, PreformSpec, ScheduleMeta,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
};
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
//
// Re-pinned 2026-10-05: slot 3 (the diagram's colour image) of both diagram pins moved,
// and nothing else did (the solid image, every pick buffer and the tooth buffer are the
// recorded ones, so the geometry and the layout are unchanged). The cause is the
// canonical facet labelling of 2026-10-04: a facet's on-diagram label is now its tier
// code plus its index (`C2 0`, `P1 0`, `Table`), no longer the tier's own name
// (`Crown Main 0`). Only text pixels differ. See
// `the_diagram_labels_are_the_canonical_tier_codes`, which pins the label text itself.
//
// Re-pinned 2026-10-06 (cutting-order lane): the table's code became `T`, so its diagram
// label is `T` where it was `Table`; slot 3 (the diagram's colour image) of `PIN_DIAGRAM`
// moved by the pixels of that one label and nothing else did. `PIN_SINGLE_PANEL` is
// unchanged: its enlarged panel 1 is the pavilion panel, which never draws the table.
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
    0x3885_c661_6f2d_25a1,
    0xd1a3_9e15_eccb_fb54,
    0xb0c8_4221_f413_1c25,
];
const PIN_SINGLE_PANEL: [u64; 6] = [
    0x61f0_28b7_7429_a350,
    0x2c7a_1a3e_c524_5679,
    0,
    0xcec8_3723_ef1e_4e76,
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
        cut_steps: None,
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
    assert_eq!(web.cut_steps, desktop.cut_steps);
    assert_eq!(web.cut_limit(), desktop.cut_limit());
    assert_eq!(web.design.tiers, desktop.design.tiers);
}

/// The desktop's Cut slider: `cut_steps` plans the stone after that many steps, `0`
/// being the preform alone, and wins over the web's `tier_cutoff`.
#[test]
fn a_cut_steps_job_plans_the_stone_after_that_many_steps() {
    let plan = |steps: Option<usize>, tier_cutoff: Option<usize>| {
        let mut job = desktop_job(0, (160, 120), -1);
        job.cut_steps = steps;
        job.tier_cutoff = tier_cutoff;
        build_planned_frame(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock)
    };
    let finished = plan(None, None);
    let preform = finished.design.preform.planes().len();
    let rough = plan(Some(0), None);
    assert_eq!(
        rough.planes.len(),
        preform,
        "the rough is the preform alone"
    );
    assert_eq!(
        rough.solved.as_ref().map(Vec::len),
        Some(finished.design.tiers.len()),
        "the masts of the whole design are chained forward, not a truncated list"
    );
    // The steps follow `Design::cutting_order()`: the pavilion section first, so the first
    // step of this top-down fixture is the girdle tier (stored index 4), sixteen planes.
    let girdle_only = plan(Some(1), None);
    assert_eq!(
        girdle_only.planes.len(),
        preform + 16,
        "step one is the girdle, sixteen planes"
    );
    let five = plan(Some(5), None);
    assert!(five.planes.len() > girdle_only.planes.len());
    assert!(five.planes.len() < finished.planes.len());
    let all = plan(Some(finished.design.tiers.len()), None);
    assert_eq!(all.planes, finished.planes);
    assert_eq!(
        plan(Some(0), Some(3)).planes.len(),
        preform,
        "cut_steps wins over tier_cutoff"
    );
    assert_eq!(
        plan(None, Some(0)).planes.len(),
        preform + 1,
        "the web's tier_cutoff is unchanged"
    );
}

/// The rough must still mesh: a preform alone is a closed solid, so the Cut slider's
/// first position shows a stone, never the finished gem or an empty view.
#[test]
fn the_rough_meshes_to_a_closed_solid() {
    let mut job = desktop_job(0, (160, 120), -1);
    job.cut_steps = Some(0);
    let planned = build_planned_frame(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock);
    let mut cache = MeshCache::default();
    assert!(
        cache.get_or_build(&planned.planes).is_some(),
        "the preform's own planes close"
    );
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

/// The text the diagram draws on a facet is its canonical tier code plus the facet's
/// index (`C2 0`), or the bare code for a tier of one facet (`T`, `Culet`), never the tier's own
/// name (`Crown Main 0`). The diagram pins above hash the pixels of these labels, so they
/// moved when the labels changed (2026-10-04, and again when the table's code became `T`);
/// this pins the text itself. The codes are numbered in cutting order, which for this
/// fixture (stored top-down, every block in cutting order) gives the same numbers as before.
#[test]
fn the_diagram_labels_are_the_canonical_tier_codes() {
    let design = pin_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);
    let first_label = |tier: usize| map.facet_label(map.facets_of_tier(tier)[0] as usize);
    // `pin_design`'s tiers, in order: Table, Star, Crown Main, Upper Girdle, Girdle,
    // Pavilion Main, Lower Girdle, Culet.
    assert_eq!(first_label(0), "T");
    assert_eq!(first_label(1), "C1 6");
    assert_eq!(first_label(2), "C2 0");
    assert_eq!(first_label(3), "C3 95");
    assert_eq!(first_label(4), "G1 0");
    assert_eq!(first_label(5), "P1 0");
    assert_eq!(first_label(6), "P2 95");
    assert_eq!(first_label(7), "Culet");
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

/// The desktop's reproject planes are the planned ones round-tripped through a non-idempotent
/// `normalize()`, so they can differ by an ULP: that must not read as a new design and wipe the
/// tier highlight; a real plane move still must.
#[test]
fn a_reproject_with_ulp_noisy_planes_keeps_the_selection() {
    let mut pipeline = PreviewPipeline::new();
    pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let planned = pipeline.memory().planes.clone().expect("planes remembered");
    let noisy: Vec<(glam::Vec3, f32)> = planned
        .iter()
        .map(|&(normal, m)| {
            let bump = |v: f32| f32::from_bits(v.to_bits().wrapping_add(1));
            (
                glam::Vec3::new(bump(normal.x), normal.y, bump(normal.z)),
                bump(m),
            )
        })
        .collect();
    let mut reproject = |planes: &[(glam::Vec3, f32)]| {
        pipeline
            .render(RedrawRequest::Reproject {
                geometry: StoneGeometryBuf::from_halfspaces(planes),
                camera: camera::orbit_step(PIN_CAMERA, 40.0, 0.0),
                size: (64, 48),
                view_mode: 0,
                gear: None,
            })
            .expect("reprojected");
        pipeline.memory().style.selected.iter().any(|&s| s)
    };
    assert!(
        reproject(&noisy),
        "an ULP of plane noise keeps the highlight"
    );
    assert!(reproject(&planned), "and so does the exact round trip back");
    let mut moved = planned;
    moved[0].1 += 0.05;
    assert!(!reproject(&moved), "a genuinely different stone resets it");
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
        frame.stone.planes.len(),
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
            geometry: StoneGeometryBuf::from_halfspaces(&[
                (glam::Vec3::X, 1.0),
                (glam::Vec3::NEG_X, 1.0),
            ]),
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

/// The Diagram view's panel layout rides on the frame's geometry, so an app places drag
/// handles on the pixels that were drawn; every other view mode carries none.
#[test]
fn a_diagram_frame_carries_the_layout_it_was_drawn_with() {
    let plan = |view_mode: u8, enlarged: i32| {
        PreviewPipeline::new()
            .replan(
                desktop_job(view_mode, (360, 180), enlarged),
                live_update::DEFAULT_PREVIEW_BUDGET,
                &ZeroClock,
            )
            .expect("planned")
    };
    let solid = plan(0, -1);
    assert!(
        solid.geometry.expect("geometry").diagram.is_none(),
        "only the Diagram view has panels"
    );

    let three = plan(3, -1);
    let drawn = three.diagram.expect("the fixture closes");
    let layout = three
        .geometry
        .expect("geometry")
        .diagram
        .expect("a Diagram frame carries its panel layout");
    assert_eq!(*layout, drawn.layout);
    assert_eq!(layout.panels.len(), 3);
    assert!(!layout.enlarged);
    assert_eq!(layout.gear_teeth, 96);

    let enlarged = plan(3, 1);
    let layout = enlarged
        .geometry
        .expect("geometry")
        .diagram
        .expect("an enlarged frame carries its layout too");
    assert!(layout.enlarged);
    assert_eq!(layout.panels.len(), 1);
    assert_eq!(layout.panels[0].kind, PanelKind::Pavilion);
}

/// A partly cut stone's frame says which tiers its facet ids number, so a pick or an
/// outline built from the frame uses the same facets; the finished stone says nothing.
#[test]
fn a_cut_frame_names_the_tiers_its_facet_ids_number() {
    let tier_count = pin_design().tiers.len();
    let geometry_at = |steps: Option<usize>| {
        let mut job = desktop_job(0, (160, 120), -1);
        job.cut_steps = steps;
        PreviewPipeline::new()
            .replan(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock)
            .expect("planned")
            .geometry
            .expect("a cut stone still meshes")
    };
    assert!(geometry_at(None).visible_tiers.is_none());

    let rough = geometry_at(Some(0));
    let tiers = rough.visible_tiers.expect("the rough names its tiers");
    assert_eq!(tiers.len(), tier_count);
    assert!(tiers.iter().all(|&shown| !shown), "no tier is cut yet");

    let three = geometry_at(Some(3));
    let tiers = three
        .visible_tiers
        .expect("a partly cut stone names its tiers");
    let shown: Vec<usize> = tiers
        .iter()
        .enumerate()
        .filter(|&(_, &shown)| shown)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        shown,
        [4, 5, 6],
        "the first three steps of the cutting order are the girdle, the pavilion mains and \
         the lower girdles (stored at 4, 5, 6), not the first three stored tiers"
    );
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

// ---- concave stones -------------------------------------------------------------
//
// The fixture is the pin design's planes with one hand-made cylinder groove across the
// table, so this crate's tests need nothing from cut-core's tool resolver.

/// The pin design plus the concave tier the groove below stands for.
fn concave_design() -> Design {
    let mut design = pin_design();
    design.concave_tiers.push(ConcaveTier {
        name: "Groove".to_string(),
        angle_deg: 10.0,
        indices: vec![0.0],
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 90.0,
        displacement: [0.0, 0.0, 0.0],
        diameter_ratio: 0.25,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    });
    design.ensure_concave_tier_ids();
    design
}

/// The pin design's flat planes plus a cylinder along `z` cutting 0.19 into the table
/// (`y = 0.32`), as one placement of concave tier 0.
fn concave_fixture() -> StoneGeometryBuf {
    let planned = build_planned_frame(
        desktop_job(0, (64, 48), -1),
        live_update::DEFAULT_PREVIEW_BUDGET,
        &ZeroClock,
    );
    StoneGeometryBuf {
        tools: vec![ToolPrimitive::cylinder(
            glam::Vec3::new(0.0, 0.25, 0.0),
            glam::Vec3::Z,
            0.12,
            2.0,
        )],
        placements: vec![(0, 0)],
        ..StoneGeometryBuf::from_halfspaces(&planned.planes)
    }
}

/// A camera above the stone, so the groove floor and walls face it.
fn concave_camera(yaw: f32, pitch: f32) -> CameraPose {
    CameraPose {
        yaw,
        pitch,
        distance: 2.4,
    }
}

fn reproject(
    pipeline: &mut PreviewPipeline,
    geometry: &StoneGeometryBuf,
    camera: CameraPose,
    size: (u32, u32),
    view_mode: u8,
) -> super::RenderedFrame {
    pipeline
        .render(RedrawRequest::Reproject {
            geometry: geometry.clone(),
            camera,
            size,
            view_mode,
            gear: Some((96, 0.0)),
        })
        .expect("a reproject always resolves")
}

/// `[solid image, solid pick, diagram image, diagram pick]` hashes of one frame.
fn frame_hashes(pipeline: &PreviewPipeline, frame: &super::RenderedFrame) -> [u64; 4] {
    let diagram = frame.diagram.as_ref();
    [
        fnv(pipeline.solid_rgba()),
        fnv_u32(&frame.pick.pick),
        diagram.map_or(0, |d| fnv(&d.color)),
        diagram.map_or(0, |d| fnv_u32(&d.pick)),
    ]
}

/// Golden hashes of [`concave_fixture`] for the three standard views, per
/// [`CONCAVE_VIEWS`]. Like the planar pins they hold the platform math library's bits,
/// so they are only compared on Windows; `None` until they are recorded there.
const CONCAVE_PINS: Option<[[u64; 4]; 3]> = None;

/// The three standard views: Solid, the three-panel Diagram and a square Diagram
/// (a `Reproject` carries no enlarged-panel choice, so all three panels show).
const CONCAVE_VIEWS: [(u8, (u32, u32)); 3] = [(0, (160, 120)), (3, (360, 180)), (3, (240, 240))];

#[test]
fn concave_fixture_frames_are_pinned() {
    let stone = concave_fixture();
    let planar = StoneGeometryBuf::planes_only(stone.planes.clone());
    let camera = concave_camera(0.6, 1.0);
    let mut recorded = Vec::new();
    for (view_mode, size) in CONCAVE_VIEWS {
        // Path A: a long-lived pipeline, hit twice so the second frame is served from
        // the mesh cache.
        let mut pipeline = PreviewPipeline::new();
        let _ = reproject(&mut pipeline, &stone, camera, size, view_mode);
        let again = reproject(&mut pipeline, &stone, camera, size, view_mode);
        let hashes = frame_hashes(&pipeline, &again);
        // Path B: the same request on fresh worker state.
        let mut fresh = PreviewPipeline::new();
        let first = reproject(&mut fresh, &stone, camera, size, view_mode);
        assert_eq!(
            frame_hashes(&fresh, &first),
            hashes,
            "view {view_mode}: a cache hit must draw the same bytes as a cold build"
        );
        // The groove changes the picture.
        let mut flat = PreviewPipeline::new();
        let flat_frame = reproject(&mut flat, &planar, camera, size, view_mode);
        assert_ne!(
            frame_hashes(&flat, &flat_frame)[..2],
            hashes[..2],
            "view {view_mode}: the groove must change the solid image"
        );
        recorded.push(hashes);
    }
    if cfg!(windows)
        && let Some(pins) = CONCAVE_PINS
    {
        assert_eq!(recorded, pins.to_vec());
    }
}

#[test]
fn pick_buffer_ids_on_concave_fixture_resolve_to_tier_and_placement() {
    let design = concave_design();
    let solved = design.solve().expect("every tier is pinned");
    let stone = concave_fixture();
    let tool_id = stone.planes.len();
    let map = FacetMap::from_design_with_tools(&design, &solved, &stone.placements);
    assert_eq!(map.facet_count(), tool_id + 1);

    let mut pipeline = PreviewPipeline::new();
    let frame = reproject(
        &mut pipeline,
        &stone,
        concave_camera(0.6, 1.0),
        (160, 120),
        0,
    );
    let mut seen_tool = false;
    let mut seen_flat = false;
    for y in 0..frame.pick.height {
        for x in 0..frame.pick.width {
            let Some(id) = frame.pick.facet_at(x, y) else {
                continue;
            };
            match map.kind_of(id as usize) {
                FacetKind::Concave { tier, placement } => {
                    assert_eq!((tier, placement), (0, 0));
                    assert_eq!(id as usize, tool_id);
                    seen_tool = true;
                }
                FacetKind::Flat => {
                    assert!((id as usize) < tool_id);
                    seen_flat = true;
                }
            }
        }
    }
    assert!(seen_tool, "the groove is visible from above");
    assert!(seen_flat);
    assert_eq!(
        map.hover_text(tool_id, 1.54),
        "Groove CYL θ 90.0° D 0.250",
        "a picked tool id reads as its tier name, tool code, theta and diameter"
    );
}

/// A planned frame of a design with concave tiers carries a concave-owner table parallel
/// to `facet_tier`: `Some(concave index)` for exactly the tool facets, `None` for flat ids,
/// while `facet_tier` itself still names no tier for a tool facet.
#[test]
fn planned_frame_carries_the_concave_owner_of_every_tool_facet() {
    let design = Design::concave_fixture();
    let n_d = design.effective_refractive_index();
    let job = PlanJob {
        design: Arc::new(design.clone()),
        dirty: BTreeSet::new(),
        last_solved: None,
        camera: PIN_CAMERA,
        size: (64, 48),
        selected_tier: None,
        n_d,
        view_mode: 3,
        generation: 1,
        show_preform: true,
        enlarged_panel: -1,
        tier_cutoff: None,
        cut_steps: None,
    };
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .replan(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock)
        .expect("a planned request always resolves");
    let solved = design.solve().expect("the fixture solves");
    let map = FacetMap::from_design_with_tools(&design, &solved, &frame.stone.placements);
    assert_eq!(frame.facet_concave_tier.len(), map.facet_count());
    assert_eq!(frame.facet_tier.len(), map.facet_count());
    let mut tool_facets = 0;
    for id in 0..map.facet_count() {
        match map.kind_of(id) {
            FacetKind::Concave { tier, .. } => {
                tool_facets += 1;
                assert_eq!(frame.facet_concave_tier[id], Some(tier), "facet {id}");
                assert_eq!(frame.facet_tier[id], None, "facet {id} has no flat tier");
            }
            FacetKind::Flat => {
                assert_eq!(frame.facet_concave_tier[id], None, "facet {id}");
                assert_eq!(frame.facet_tier[id], map.tier_of(id), "facet {id}");
            }
        }
    }
    assert!(tool_facets > 0, "the fixture's tools are placed");
    assert_eq!(
        frame.diagram_facet_concave_tier.as_deref(),
        Some(frame.facet_concave_tier.as_slice()),
        "a Diagram-mode frame carries the same table"
    );
}

#[test]
fn concave_raster_has_no_hairline_gaps() {
    let stone = concave_fixture();
    let mut pipeline = PreviewPipeline::new();
    for camera in [
        concave_camera(0.6, 1.0),
        concave_camera(2.2, 0.7),
        concave_camera(4.0, 1.3),
        PIN_CAMERA,
    ] {
        let frame = reproject(&mut pipeline, &stone, camera, (200, 150), 0);
        let (w, h) = (frame.pick.width as usize, frame.pick.height as usize);
        let at = |x: usize, y: usize| frame.pick.pick[y * w + x];
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let all_stone = at(x - 1, y) != 0
                    && at(x + 1, y) != 0
                    && at(x, y - 1) != 0
                    && at(x, y + 1) != 0;
                assert!(
                    at(x, y) != 0 || !all_stone,
                    "unset pick pixel ({x}, {y}) surrounded by stone at {camera:?}"
                );
            }
        }
    }
}

#[test]
fn a_reproject_with_different_tools_resets_the_remembered_style() {
    let stone = concave_fixture();
    let planar = StoneGeometryBuf::planes_only(stone.planes.clone());
    let mut pipeline = PreviewPipeline::new();
    let camera = concave_camera(0.6, 1.0);
    let _ = reproject(&mut pipeline, &stone, camera, (64, 48), 0);
    assert_eq!(pipeline.memory().tools.len(), 1);
    assert_eq!(pipeline.memory().geometry().as_ref(), Some(&stone));
    let _ = reproject(&mut pipeline, &planar, camera, (64, 48), 0);
    assert!(
        pipeline.memory().tools.is_empty(),
        "a planar stone forgets the earlier tools"
    );
    assert_eq!(pipeline.memory().geometry().as_ref(), Some(&planar));
}
