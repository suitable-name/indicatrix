//! Identity pins for the solid-preview frame pipeline (`build_planned_frame` ->
//! `render_request`), recorded BEFORE its pure half moved into
//! `indicatrix_solid::preview`, so the move is provably
//! byte-identical: every hash below is FNV-1a over the exact bytes the desktop
//! hands its sink (RGBA image, facet pick buffer, diagram tooth buffer).
//!
//! If one of these fails, the desktop's Solid/Diagram pixels changed -- that is
//! only acceptable as a deliberate rendering change, never as a side effect of a
//! refactor.
//!
//! The hashes are Windows bits. The rasteriser's projection (`tan` of the field of view)
//! and the facet map's directions (`sin`, `cos`), and the diagram layout's teeth and
//! labels (`sin_cos`), all run through the platform math library, and glibc rounds a
//! few of those arguments differently from the Windows runtime, so a boundary pixel or
//! tooth moves and every hash here differs on Linux (seen 2026-10-01: the solid image
//! and the edge layer, and both diagram frames; only the facet pick buffer agreed). The
//! four tests are therefore ignored off Windows; running them there with `--ignored`
//! prints that platform's hashes, which is all a per-platform table would need.

use super::super::{
    CameraPose, FacetOverlay, MeshCache, SolidRasterizer, live_update,
    plan_worker::build_planned_frame,
    render::render_request,
    request::{PlanJob, RedrawRequest},
    state::WorkerMemory,
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
use std::sync::Arc;

/// FNV-1a over `bytes`.
fn fnv(bytes: &[u8]) -> u64 {
    indicatrix_solid::mesh_cache::fnv1a_64(bytes.iter().copied())
}

/// FNV-1a over a `u32` buffer's little-endian bytes.
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

/// The same pinned round-brilliant fixture `replan.rs`'s `closed_design` uses.
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

fn pin_job(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> PlanJob {
    let design = pin_design();
    let n_d = design.effective_refractive_index();
    PlanJob {
        design: Arc::new(design),
        dirty: std::collections::BTreeSet::new(),
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

/// `[solid image, solid pick, edges image, diagram image, diagram pick, diagram
/// tooth]` hashes (`0` where the frame has none) for one planned request followed
/// by an overlay update (hover facet 20, selected facet 21, multi {30, 31}).
fn pipeline_hashes(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> [u64; 6] {
    let mut mesh_cache = MeshCache::default();
    let mut rasterizer = SolidRasterizer::new(1, 1);
    let mut edges_rasterizer = SolidRasterizer::new(1, 1);
    let mut memory = WorkerMemory::default();
    let planned = build_planned_frame(
        pin_job(view_mode, size, enlarged_panel),
        live_update::DEFAULT_PREVIEW_BUDGET,
    );
    let _first = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        RedrawRequest::Planned(Box::new(planned)),
    )
    .expect("a Planned request always resolves");
    let frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        RedrawRequest::UpdateFacetOverlay(FacetOverlay {
            hovered: Some(20),
            selected_facet: Some(21),
            multi_selected: vec![30, 31],
            provisional: Vec::new(),
            moved: Vec::new(),
        }),
    )
    .expect("an overlay after a frame always resolves");
    [
        fnv(frame.image.as_bytes()),
        fnv_u32(&frame.pick.pick),
        frame.edges_image.as_ref().map_or(0, |i| fnv(i.as_bytes())),
        frame
            .diagram_image
            .as_ref()
            .map_or(0, |i| fnv(i.as_bytes())),
        frame.diagram_pick.as_ref().map_or(0, |p| fnv_u32(&p.pick)),
        frame
            .diagram_tooth_pick
            .as_ref()
            .map_or(0, |p| fnv_u32(&p.pick)),
    ]
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "pinned to the Windows math library's rounding of the projection and facet angles"
)]
fn pin_solid_view_frame() {
    let hashes = pipeline_hashes(0, (160, 120), -1);
    println!("pin_solid_view_frame: {hashes:#018x?}");
    assert_eq!(hashes, PIN_SOLID);
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "pinned to the Windows math library's rounding of the projection and facet angles"
)]
fn pin_both_view_edges_frame() {
    let hashes = pipeline_hashes(2, (160, 120), -1);
    println!("pin_both_view_edges_frame: {hashes:#018x?}");
    assert_eq!(hashes, PIN_BOTH);
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "pinned to the Windows math library's rounding of the diagram layout's angles"
)]
fn pin_diagram_view_frame() {
    let hashes = pipeline_hashes(3, (360, 180), -1);
    println!("pin_diagram_view_frame: {hashes:#018x?}");
    assert_eq!(hashes, PIN_DIAGRAM);
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "pinned to the Windows math library's rounding of the diagram layout's angles"
)]
fn pin_single_panel_diagram_frame() {
    let hashes = pipeline_hashes(3, (240, 240), 1);
    println!("pin_single_panel_diagram_frame: {hashes:#018x?}");
    assert_eq!(hashes, PIN_SINGLE_PANEL);
}

// Recorded 2026-09-29 from the pre-move desktop pipeline. The same
// values are asserted by `indicatrix_solid::preview`'s own identity test, through
// the web app's pipeline entry points.
const PIN_SOLID: [u64; 6] = [0x0f10_7ca8_5f93_74bf, 0x5f3d_a2da_7a33_a030, 0, 0, 0, 0];
const PIN_BOTH: [u64; 6] = [
    0x0f10_7ca8_5f93_74bf,
    0x5f3d_a2da_7a33_a030,
    0x21d0_25ae_bd67_13d0,
    0,
    0,
    0,
];
// Slot 3 (the diagram's colour image) of both diagram pins was re-recorded 2026-10-05: since
// 2026-10-04 the diagram labels its facets with the canonical tier codes
// (`indicatrix_cut_core::design::labelling`, "Pavilion Main 0" is now "P1 0"), which changes
// text pixels only. The solid image, both pick buffers and the tooth buffers did not move
// (slots 0, 1, 2, 4 and 5 equal the pins recorded 2026-09-29). The same values are pinned in
// `indicatrix_solid::preview`'s identity test.
//
// Re-pinned 2026-10-06 (cutting-order lane): the table's code became `T`, so its diagram
// label is `T` where it was `Table`. Slot 3 (the diagram's colour image) of `PIN_DIAGRAM`
// moved by the pixels of that one label and nothing else did; `PIN_SINGLE_PANEL` is
// unchanged because its enlarged panel is the pavilion panel, which never draws the table.
// The same constants live in `indicatrix_solid::preview`'s identity test (lane F1 updates it).
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
