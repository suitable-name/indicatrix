//! Unit tests for the replan/plan-worker path: [`super::super::plan_worker::
//! build_planned_frame`] and its round trip through
//! [`super::super::render::render_request`].

use super::{
    super::{
        FacetMap, MeshCache, SolidRasterizer, live_update,
        plan_worker::build_planned_frame,
        render::render_request,
        request::{PlanJob, RedrawRequest},
        state::{WorkerMemory, escaping_tier_label},
    },
    *,
};
use glam::Vec3;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
use std::{
    sync::{Arc, PoisonError},
    time::Duration,
};

fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0],
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// Every tier `ScaleReference` -- `plan_preview`'s "Pinned" tier, resolved
/// with no solver call, so this plans in microseconds regardless of budget.
/// No real closure guarantee -- fine for tests that never touch
/// `mesh_cache`; see [`closed_design`] for the render-path fixture.
fn pinned_design() -> Design {
    Design::new(
        PreformSpec::block(1.0, 1.0, 1.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        vec![
            tier("Table", 0.0, MeetConstraint::ScaleReference(0.5)),
            tier("Pavilion", -40.0, MeetConstraint::ScaleReference(0.6)),
        ],
    )
}

/// A synthetic "RBC-445"-style design, reauthored as [`ConstraintTier`]s
/// (mirrors `facet_map.rs`'s private `standard_round_brilliant_design`
/// fixture). Every tier pinned via `ScaleReference`, proven
/// `SolidStatus::Closed` elsewhere (`raster.rs`) -- used here, unlike
/// [`pinned_design`], because these tests need a real closed solid.
fn closed_design() -> Design {
    const GIRDLE_INDICES: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    const BREAK_INDICES: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    const MAIN_INDICES: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    const STAR_INDICES: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

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
            rbc_tier("Star", 15.0, &STAR_INDICES, 0.45),
            rbc_tier("Crown Main", 34.5, &MAIN_INDICES, 0.59),
            rbc_tier("Upper Girdle", 41.0, &BREAK_INDICES, 0.67),
            rbc_tier("Girdle", 90.0, &GIRDLE_INDICES, 1.0),
            rbc_tier("Pavilion Main", -41.0, &MAIN_INDICES, 0.67),
            rbc_tier("Lower Girdle", -42.5, &BREAK_INDICES, 0.68),
            rbc_tier("Culet", -0.0, &[], 0.88),
        ],
    )
}

/// One anchored tier plus one free (`MeetExisting`) tier -- a real
/// `resolve_dirty` call happens for this fixture, which is what the
/// `Duration::ZERO` budget below needs to force `Stale` deterministically.
fn free_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        vec![
            tier("C1", 30.0, MeetConstraint::ScaleReference(0.6)),
            tier("C2", 40.0, MeetConstraint::MeetExisting),
        ],
    )
}

/// A minimal, all-default [`PlanJob`] for a test that only cares about a
/// couple of fields -- built with `..` from this so a new `PlanJob` field
/// never forces every one of this module's tests to list it.
fn plan_job(design: Design) -> PlanJob {
    let n_d = design.effective_refractive_index();
    PlanJob {
        design: Arc::new(design),
        dirty: std::collections::BTreeSet::new(),
        last_solved: None,
        camera: CAMERA,
        size: (16, 16),
        selected_tier: None,
        n_d,
        view_mode: 0,
        generation: 0,
        show_preform: true,
        enlarged_panel: -1,
        tier_cutoff: None,
    }
}

#[test]
fn build_planned_frame_returns_the_solved_masts_for_a_pinned_design() {
    let design = pinned_design();
    let frame = build_planned_frame(plan_job(design), live_update::DEFAULT_PREVIEW_BUDGET);
    assert_ne!(frame.planes, Vec::<(Vec3, f32)>::new());
    assert_eq!(frame.solved.map(|s| s.len()), Some(2));
    assert!(!frame.stale, "a pinned design never goes over budget");
}

/// `ReplanRequest`/`PlanJob`/`PlannedFrame` carry `Arc<Design>` end to
/// end, reusing the same allocation across the full path instead of
/// cloning repeatedly. Verifies via `Arc::ptr_eq`.
#[test]
fn build_planned_frame_carries_the_same_design_allocation_through_to_planned_frame() {
    let design = Arc::new(closed_design());
    let job = PlanJob {
        design: Arc::clone(&design),
        ..plan_job(closed_design())
    };
    let frame = build_planned_frame(job, live_update::DEFAULT_PREVIEW_BUDGET);
    assert!(
        Arc::ptr_eq(&design, &frame.design),
        "build_planned_frame must not clone the design -- PlannedFrame::design \
         should be the exact same Arc allocation ReplanRequest/PlanJob were handed"
    );
}

/// Same fixture ("RBC-445"-style, the standard round-brilliant `closed_design`
/// this module's other `PlannedFrame` tests already use), proving the
/// `Arc<Design>` plumbing above changed nothing about what actually gets
/// planned: a real closed solid, all 8 tiers solved, never stale -- byte-
/// for-byte the same shape [`render_request_carries_solved_and_freshness_
/// through_to_the_worker_frame`] below already asserts end to end through
/// `render_request` too.
#[test]
fn build_planned_frame_output_for_the_round_brilliant_fixture_is_unchanged() {
    let design = closed_design();
    let frame = build_planned_frame(plan_job(design), live_update::DEFAULT_PREVIEW_BUDGET);
    assert!(
        !frame.stale,
        "a pinned round-brilliant design never goes over budget"
    );
    assert!(frame.unsolvable_status.is_none());
    assert_eq!(
        frame.solved.map(|s| s.len()),
        Some(8),
        "every one of the fixture's 8 tiers must solve"
    );
    assert_ne!(
        frame.planes,
        Vec::<(Vec3, f32)>::new(),
        "a closed round-brilliant design must produce a real plane arrangement"
    );
}

#[test]
fn a_zero_budget_forces_stale_and_marks_the_dirty_tier_pending() {
    let design = free_design();
    let previous = design.solve().expect("fixture must solve");
    let dirty = std::collections::BTreeSet::from([1]);
    let design_for_facet_map = design.clone();

    let frame = build_planned_frame(
        PlanJob {
            last_solved: Some(previous),
            dirty,
            ..plan_job(design)
        },
        Duration::ZERO,
    );
    assert!(
        frame.stale,
        "a real resolve_dirty call can never finish within 0ns"
    );
    assert!(
        frame.solved.is_some(),
        "the fresh (late) result must still be chained forward"
    );
    // Built from the SAME (new) solved masts `build_planned_frame` used
    // internally -- a `FacetMap` built from the OLD masts is not
    // guaranteed to assign the same facet ids.
    let facet_map = FacetMap::from_design(
        &design_for_facet_map,
        frame.solved.as_deref().unwrap_or(&[]),
    );
    assert!(
        facet_map
            .facets_of_tier(1)
            .iter()
            .all(|&id| frame.style.pending[id as usize]),
        "the edited tier's own facets must be marked pending"
    );
}

/// The `Unbounded` banner must name the escaping plane's owning tier,
/// not the raw index.
#[test]
fn escaping_tier_label_names_the_owning_tier() {
    let design = pinned_design();
    let solved = design.solve().expect("every tier is pinned");
    let preform_plane_count = design.preform.planes().len();
    // The first plane past the preform's own is tier 0's ("Table")
    // facet -- `Design::tier_for_plane_index`'s own doc comment: it
    // subtracts `preform.planes().len()` before mapping into the
    // schedule tiers, which are laid out in tier order.
    let label = escaping_tier_label(&design, &solved, preform_plane_count);
    assert_eq!(label, "Table (tier 1)");
}

#[test]
fn escaping_tier_label_falls_back_to_a_raw_plane_for_a_preform_plane() {
    let design = pinned_design();
    let solved = design.solve().expect("every tier is pinned");
    // Index 0 is always one of the preform's own planes -- not a
    // schedule-tier facet, so `Design::tier_for_plane_index` returns
    // `None` and the label falls back to the raw index.
    let label = escaping_tier_label(&design, &solved, 0);
    assert_eq!(label, "plane 0");
}

#[test]
fn selected_tier_flags_reach_solid_style_selected() {
    let design = pinned_design();
    let design_for_facet_map = design.clone();
    let frame = build_planned_frame(
        PlanJob {
            selected_tier: Some(0),
            ..plan_job(design)
        },
        live_update::DEFAULT_PREVIEW_BUDGET,
    );
    let style = frame.style;
    // Same masts `build_planned_frame` solved internally (an all-pinned
    // design's `Design::solve()` reads each tier's own `ScaleReference`
    // value).
    let solved = design_for_facet_map.solve().expect("every tier is pinned");
    let facet_map = FacetMap::from_design(&design_for_facet_map, &solved);
    assert!(
        facet_map
            .facets_of_tier(0)
            .iter()
            .all(|&id| style.selected[id as usize]),
        "tier 0's own facets must be marked selected"
    );
    assert!(
        facet_map
            .facets_of_tier(1)
            .iter()
            .all(|&id| !style.selected[id as usize]),
        "tier 1 was never selected"
    );
}

/// A `tier_cutoff` of `Some(0)` truncates planes to the first tier only.
#[test]
fn tier_cutoff_truncates_the_planned_frame() {
    let design = pinned_design();
    let full = build_planned_frame(
        plan_job(design.clone()),
        live_update::DEFAULT_PREVIEW_BUDGET,
    );
    let truncated = build_planned_frame(
        PlanJob {
            tier_cutoff: Some(0),
            ..plan_job(design)
        },
        live_update::DEFAULT_PREVIEW_BUDGET,
    );
    assert!(
        truncated.planes.len() < full.planes.len(),
        "cutting off after tier 0 must drop tier 1's (\"Pavilion\") facet(s): \
         full={}, truncated={}",
        full.planes.len(),
        truncated.planes.len()
    );
}

/// [`super::super::SolidPreviewState::set_tier_cutoff`] only caches the value
/// for the NEXT [`super::super::SolidPreviewState::request_replan`] -- this
/// guards that the getter side of that cache (the private `tier_cutoff` field
/// itself, read back through the same lock `set_tier_cutoff` writes through)
/// round-trips both `Some` and back to `None`, independent of the worker
/// thread machinery `request_replan` also drives.
#[test]
fn set_tier_cutoff_round_trips_through_the_cache() {
    let state = SolidPreviewState::new(FakeSink::new());
    let cached = || {
        *state
            .tier_cutoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    };
    assert_eq!(cached(), None);
    state.set_tier_cutoff(Some(3));
    assert_eq!(cached(), Some(3));
    state.set_tier_cutoff(None);
    assert_eq!(cached(), None);
}

/// Builds a `RedrawRequest::Planned` request: [`build_planned_frame`] on
/// a `PlanJob`, then boxed. A test-only stand-in kept synchronous.
fn planned_request(
    design: Design,
    view_mode: u8,
    size: (u32, u32),
    generation: u64,
) -> RedrawRequest {
    let frame = build_planned_frame(
        PlanJob {
            size,
            view_mode,
            generation,
            ..plan_job(design)
        },
        live_update::DEFAULT_PREVIEW_BUDGET,
    );
    RedrawRequest::Planned(Box::new(frame))
}

#[test]
fn render_request_carries_solved_and_freshness_through_to_the_worker_frame() {
    let design = closed_design();
    let mut mesh_cache = MeshCache::default();
    let mut rasterizer = SolidRasterizer::new(16, 16);
    let mut edges_rasterizer = SolidRasterizer::new(16, 16);
    let mut memory = WorkerMemory::default();

    let frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        planned_request(design, 0, (16, 16), 7),
    )
    .expect("a Planned request always resolves");
    assert!(frame.has_solid);
    assert!(!frame.stale);
    assert_eq!(frame.solved.map(|s| s.len()), Some(8));
    assert_eq!(
        frame.generation, 7,
        "the request's own generation must reach the finished frame unchanged"
    );
    assert!(
        frame.edges_image.is_none(),
        "view_mode 0 never builds edges"
    );
    assert!(
        frame.diagram_image.is_none(),
        "view_mode 0 never builds the diagram"
    );
}

#[test]
fn view_mode_both_also_produces_an_edges_image() {
    let design = closed_design();
    let mut mesh_cache = MeshCache::default();
    let mut rasterizer = SolidRasterizer::new(16, 16);
    let mut edges_rasterizer = SolidRasterizer::new(16, 16);
    let mut memory = WorkerMemory::default();

    let frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        planned_request(design, 2, (16, 16), 0),
    )
    .expect("a Planned request always resolves");
    assert!(frame.edges_image.is_some());
    assert!(
        frame.diagram_image.is_none(),
        "view_mode 2 never builds the diagram"
    );
}

/// View mode 3: the worker must build the 2D facet diagram, its own pick
/// buffer, and the facet-id-indexed hover-text/tier tables a diagram click
/// needs -- see `PreviewFrame::diagram_hover_text`'s doc comment for why
/// those travel with the frame instead of being recomputed on the UI thread.
#[test]
fn view_mode_diagram_produces_the_diagram_image_and_its_side_tables() {
    let design = closed_design();
    let mut mesh_cache = MeshCache::default();
    let mut rasterizer = SolidRasterizer::new(16, 16);
    let mut edges_rasterizer = SolidRasterizer::new(16, 16);
    let mut memory = WorkerMemory::default();

    let frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        planned_request(design, 3, (240, 120), 0),
    )
    .expect("a Planned request always resolves");
    assert!(frame.has_diagram);
    assert!(frame.diagram_image.is_some());
    let pick = frame
        .diagram_pick
        .expect("view_mode 3 must produce a diagram pick buffer");
    assert_eq!((pick.width, pick.height), (240, 120));
    // The index wheel's own tooth pick buffer must be threaded through
    // alongside the facet one, at the SAME size (both are built from the
    // same `DiagramFrame`).
    let tooth_pick = frame
        .diagram_tooth_pick
        .expect("view_mode 3 must also produce a tooth pick buffer");
    assert_eq!((tooth_pick.width, tooth_pick.height), (240, 120));
    let hover_text = frame
        .diagram_hover_text
        .expect("view_mode 3 must produce a facet-id-indexed hover-text table");
    assert_eq!(
        hover_text.len(),
        frame.diagram_facet_tier.as_ref().unwrap().len()
    );
    // The Table tier's facet (id 0, right after the preform's own planes on
    // this fixture -- see `facet_map.rs`'s own doc comment) must have a
    // non-empty hover string and a resolved tier index somewhere in the table.
    assert!(hover_text.iter().any(|t| t.contains("Table")));
}

/// Every frame -- not just Diagram-mode ones -- must carry a real,
/// positive bounding radius once a solid has closed, so `render::
/// camera_lighting`'s orbit-zoom clamp/"Fit" pose never reads the
/// `DEFAULT_MESH_BOUNDING_RADIUS` placeholder for a design that
/// actually solved.
#[test]
fn replan_frame_carries_a_positive_mesh_bounding_radius() {
    let design = closed_design();
    let mut mesh_cache = MeshCache::default();
    let mut rasterizer = SolidRasterizer::new(16, 16);
    let mut edges_rasterizer = SolidRasterizer::new(16, 16);
    let mut memory = WorkerMemory::default();

    let frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        planned_request(design, 0, (16, 16), 0),
    )
    .expect("a Planned request always resolves");
    assert!(frame.has_solid);
    assert!(frame.mesh_bounding_radius > 0.0);
}

/// A `Reproject`/`UpdateFacetOverlay` frame carries no generation of its
/// own; it must reuse the last `Replan`'s generation.
#[test]
fn a_reproject_frame_carries_forward_the_last_replans_generation() {
    let design = closed_design();
    let mut mesh_cache = MeshCache::default();
    let mut rasterizer = SolidRasterizer::new(16, 16);
    let mut edges_rasterizer = SolidRasterizer::new(16, 16);
    let mut memory = WorkerMemory::default();

    let replan_frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        planned_request(design, 0, (16, 16), 42),
    )
    .expect("a Planned request always resolves");
    assert_eq!(replan_frame.generation, 42);

    let reproject_frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        RedrawRequest::Reproject {
            planes: replan_frame.planes,
            camera: CAMERA,
            size: (16, 16),
            view_mode: 0,
            gear: None,
        },
    )
    .expect("a Reproject request always resolves");
    assert_eq!(
        reproject_frame.generation, 42,
        "a camera-follow reproject must not lose the last replan's generation"
    );
}
