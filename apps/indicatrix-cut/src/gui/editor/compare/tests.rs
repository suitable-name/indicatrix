//! Slint-free tests for the compare window: session construction, pose
//! arithmetic, the split clamp, stale-frame rejection, and the pure render paths.

use super::{
    overlay::{
        ADDED_TINT, AFTER_OUTLINE, BEFORE_OUTLINE, FACET_EDGE, REMOVED_TINT, difference_overlay,
    },
    render::{
        TRACED_MAX_EDGE, TRACED_SPP, pixel_size, placeholder_rgba, render_solid_pick,
        render_solid_rgba, render_traced_rgba, traced_size,
    },
    session::{
        CompareOrigin, CompareSession, FrameBook, FrameKind, MAX_SETTLE_POLLS, Renderer,
        SLOW_TRACED_SIDE_SECS, SideInput, TracedProgress, clamp_split_fraction, default_pose,
        fitted_pose, orbit, reduced_spp_note, resolve_metrics_material, resolve_side_material,
        status_text, traced_may_start, traced_spp_for, traced_waits_for_layout, zoom,
    },
};
use crate::gui::{
    render::camera_lighting::{fit_distance_for_radius, orbit_distance_bounds},
    solid_preview::preview_state::CameraPose,
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design, MaterialSelection, PreformSpec, ScheduleMeta};
use indicatrix_editor::solve_policy::design_to_gpu_planes_from_solved;
use indicatrix_solid::preview::StoneGeometryBuf;
use std::f32::consts::FRAC_PI_2;

/// The standard round brilliant, named Diamond so the traced material resolves.
fn round_brilliant() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        ..MaterialSelection::default()
    };
    design
}

/// [`round_brilliant`] with the crown mains steepened from 34.5 to 37 degrees --
/// one tier angle changed, as a Retarget or Optimize result would.
fn round_brilliant_steeper_crown() -> Design {
    let mut design = round_brilliant();
    let main = design
        .tiers
        .iter_mut()
        .find(|tier| tier.name == "Crown Main")
        .expect("the standard round brilliant has a Crown Main tier");
    main.angle_deg = 37.0;
    design
}

/// A design with no scale-reference anchor at all: it cannot solve.
fn unsolvable_design() -> Design {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 30.0,
        name: "A".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design
}

fn input(design: Design, label: &str) -> SideInput {
    let material = resolve_side_material(&design, &[]);
    let metrics_material = resolve_metrics_material(&design, &[]);
    SideInput {
        design,
        label: label.to_string(),
        material,
        metrics_material,
    }
}

const START_POSE: CameraPose = CameraPose {
    yaw: 0.6,
    pitch: 0.45,
    distance: 2.4,
};

#[test]
fn a_session_of_two_solving_designs_keeps_labels_and_can_keep() {
    let session = CompareSession::build(
        input(round_brilliant(), "Current design"),
        input(round_brilliant_steeper_crown(), "Optimize candidate"),
        CompareOrigin::Optimize,
        START_POSE,
    );
    assert_eq!(session.before.label, "Current design");
    assert_eq!(session.after.label, "Optimize candidate");
    assert!(session.before.is_solved() && session.after.is_solved());
    assert!(!session.before.stone.planes.is_empty() && !session.after.stone.planes.is_empty());
    assert_eq!(
        session.before.preform_planes,
        round_brilliant().preform.planes().len()
    );
    assert!(session.before.material.is_ok() && session.after.material.is_ok());
    assert!(session.can_keep());
    assert_eq!(
        status_text(Some(&session), Renderer::Solid, TracedProgress::Done, 48),
        "Solid"
    );
}

#[test]
fn a_snapshot_session_never_offers_keep() {
    let session = CompareSession::build(
        input(round_brilliant(), "Snapshot"),
        input(round_brilliant(), "Current design"),
        CompareOrigin::Snapshot,
        START_POSE,
    );
    assert!(session.before.is_solved() && session.after.is_solved());
    assert!(!session.can_keep());
    assert!(!CompareOrigin::Snapshot.offers_keep());
    assert!(CompareOrigin::Retarget.offers_keep());
}

#[test]
fn a_variants_session_never_offers_keep() {
    let session = CompareSession::build(
        input(round_brilliant(), "Variant \"Steeper\""),
        input(round_brilliant_steeper_crown(), "Current design"),
        CompareOrigin::Variants,
        START_POSE,
    );
    assert_eq!(session.before.label, "Variant \"Steeper\"");
    assert_eq!(session.after.label, "Current design");
    assert!(session.before.is_solved() && session.after.is_solved());
    assert!(!session.can_keep());
    assert!(!CompareOrigin::Variants.offers_keep());
    assert!(CompareOrigin::Optimize.offers_keep());
}

#[test]
fn a_side_that_does_not_solve_blocks_keep_and_says_so() {
    let session = CompareSession::build(
        input(round_brilliant(), "Current design"),
        input(unsolvable_design(), "Retarget to Ruby (Shift)"),
        CompareOrigin::Retarget,
        START_POSE,
    );
    assert!(session.before.is_solved());
    assert!(!session.after.is_solved());
    assert_eq!(session.after.stone, StoneGeometryBuf::default());
    assert!(!session.can_keep());
    let status = status_text(Some(&session), Renderer::Solid, TracedProgress::Done, 48);
    assert!(status.contains("After does not solve"), "{status}");
    assert!(!status.contains("Before does not solve"), "{status}");
}

#[test]
fn status_reports_solving_and_traced_progress() {
    assert_eq!(
        status_text(None, Renderer::Traced, TracedProgress::Waiting, 48),
        "Solving both sides…"
    );
    let session = CompareSession::build(
        input(round_brilliant(), "a"),
        input(round_brilliant(), "b"),
        CompareOrigin::Retarget,
        START_POSE,
    );
    let rendering = TracedProgress::Rendering { side: 2 };
    assert_eq!(
        status_text(Some(&session), Renderer::Traced, rendering, 48),
        "Tracing… 2 of 2"
    );
    assert_eq!(
        status_text(Some(&session), Renderer::Traced, TracedProgress::Done, 48),
        "Traced (48 spp)"
    );
}

#[test]
fn an_unresolvable_material_is_refused_not_substituted() {
    let mut design = round_brilliant();
    design.material = MaterialSelection {
        name: Some("Unobtainium".to_string()),
        ..MaterialSelection::default()
    };
    assert!(resolve_side_material(&design, &[]).is_err());
    assert!(resolve_side_material(&round_brilliant(), &[]).is_ok());
}

#[test]
fn orbit_turns_both_axes_and_clamps_pitch_at_the_poles() {
    let turned = orbit(START_POSE, 10.0, 10.0);
    assert!((turned.yaw - (START_POSE.yaw - 0.08)).abs() < 1e-6);
    assert!((turned.pitch - (START_POSE.pitch + 0.08)).abs() < 1e-6);
    assert!((turned.distance - START_POSE.distance).abs() < f32::EPSILON);
    assert!((orbit(START_POSE, 0.0, 10_000.0).pitch - FRAC_PI_2).abs() < f32::EPSILON);
    assert!((orbit(START_POSE, 0.0, -10_000.0).pitch + FRAC_PI_2).abs() < f32::EPSILON);
}

#[test]
fn zoom_clamps_distance_to_the_sessions_own_bounds() {
    let radius = 1.5;
    let (min, max) = orbit_distance_bounds(radius);
    assert!((zoom(START_POSE, 1.0e6, radius).distance - min).abs() < f32::EPSILON);
    assert!((zoom(START_POSE, -1.0e6, radius).distance - max).abs() < f32::EPSILON);
    let small = zoom(START_POSE, 100.0, radius);
    assert!((small.distance - (START_POSE.distance - 0.2)).abs() < 1e-5);
    assert!((small.yaw - START_POSE.yaw).abs() < f32::EPSILON);
}

#[test]
fn the_reset_pose_is_the_front_view_inside_the_zoom_bounds() {
    let radius = 1.5;
    let (min, max) = orbit_distance_bounds(radius);
    let pose = default_pose(radius);
    assert!(pose.yaw.abs() < f32::EPSILON && pose.pitch.abs() < f32::EPSILON);
    assert!(pose.distance >= min && pose.distance <= max);
}

#[test]
fn split_fraction_clamps_and_nan_falls_back_to_the_centre() {
    assert!((clamp_split_fraction(-0.3)).abs() < f32::EPSILON);
    assert!((clamp_split_fraction(1.7) - 1.0).abs() < f32::EPSILON);
    assert!((clamp_split_fraction(0.25) - 0.25).abs() < f32::EPSILON);
    assert!((clamp_split_fraction(f32::NAN) - 0.5).abs() < f32::EPSILON);
}

#[test]
fn a_stale_frame_is_dropped_and_solid_never_paints_over_traced() {
    let mut book = FrameBook::default();
    let first = book.bump();
    let second = book.bump();
    assert_ne!(first, second);
    // A frame issued for an older view is dropped, whatever its kind.
    assert!(!book.accept(FrameKind::Solid, first, Renderer::Solid));
    assert!(!book.accept(FrameKind::Traced, first, Renderer::Traced));
    // A traced frame is dropped once the cutter switched back to Solid.
    assert!(!book.accept(FrameKind::Traced, second, Renderer::Solid));
    // The current solid pair lands, then the traced pair replaces it...
    assert!(book.accept(FrameKind::Solid, second, Renderer::Traced));
    assert!(book.accept(FrameKind::Traced, second, Renderer::Traced));
    // ...and a late solid pair for the same view no longer paints over it.
    assert!(!book.accept(FrameKind::Solid, second, Renderer::Traced));
    // A new view starts clean.
    let third = book.bump();
    assert_eq!(book.generation(), third);
    assert!(book.accept(FrameKind::Solid, third, Renderer::Traced));
}

#[test]
fn solid_renders_are_identical_for_identical_inputs_and_see_one_changed_tier() {
    let session = CompareSession::build(
        input(round_brilliant(), "before"),
        input(round_brilliant(), "same"),
        CompareOrigin::Retarget,
        START_POSE,
    );
    let changed = CompareSession::build(
        input(round_brilliant(), "before"),
        input(round_brilliant_steeper_crown(), "after"),
        CompareOrigin::Retarget,
        START_POSE,
    );
    let size = (64, 64);
    let render = |side: &super::session::CompareSide| {
        render_solid_rgba(&side.stone, side.preform_planes, START_POSE, size)
    };
    let before = render(&session.before);
    assert_eq!(before.len(), 64 * 64 * 4);
    assert_eq!(
        before,
        render(&session.after),
        "identical inputs, identical pixels"
    );
    assert_eq!(
        before,
        render(&changed.before),
        "rendering is deterministic"
    );
    assert_ne!(
        before,
        render(&changed.after),
        "one steeper crown tier must show up in the solid render"
    );
}

#[test]
fn traced_size_caps_the_long_edge_and_keeps_the_aspect() {
    assert_eq!(traced_size((200, 100)), (200, 100));
    assert_eq!(traced_size((0, 0)), (1, 1));
    let (width, height) = traced_size((1000, 500));
    assert_eq!(width, TRACED_MAX_EDGE);
    assert_eq!(height, TRACED_MAX_EDGE / 2);
}

#[test]
fn pixel_size_scales_rounds_and_rejects_nonsense() {
    assert_eq!(pixel_size(100.0, 50.0, 1.5), (150, 75));
    assert_eq!(pixel_size(-5.0, f32::NAN, 1.0), (0, 0));
    assert_eq!(pixel_size(1.0e6, 10.0, 1.0), (4096, 10));
}

#[test]
fn the_placeholder_fills_the_whole_frame() {
    assert_eq!(placeholder_rgba((12, 7)).len(), 12 * 7 * 4);
}

/// A `size` pick buffer with `id` inside the inclusive pixel box `x0..=x1, y0..=y1`.
fn filled_box(
    size: (u32, u32),
    (x0, x1): (usize, usize),
    (y0, y1): (usize, usize),
    id: u32,
) -> Vec<u32> {
    let width = size.0 as usize;
    let mut pick = vec![0u32; width * size.1 as usize];
    for y in y0..=y1 {
        for x in x0..=x1 {
            pick[y * width + x] = id;
        }
    }
    pick
}

/// The RGBA of pixel `(x, y)` in an overlay of width `width`.
fn overlay_px(overlay: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let o = (y * width + x) * 4;
    [overlay[o], overlay[o + 1], overlay[o + 2], overlay[o + 3]]
}

const CLEAR: [u8; 4] = [0, 0, 0, 0];
const GRID: (u32, u32) = (12, 12);

#[test]
fn overlay_classifies_removed_added_shared_and_neither() {
    // Before covers x 2..=7, after covers x 5..=10, both y 2..=9.
    let before = filled_box(GRID, (2, 7), (2, 9), 1);
    let after = filled_box(GRID, (5, 10), (2, 9), 1);
    let overlay = difference_overlay(&before, &after, None, GRID);
    let px = |x, y| overlay_px(&overlay, 12, x, y);
    assert_eq!(px(3, 5), REMOVED_TINT, "before only -> removed");
    assert_eq!(px(9, 5), ADDED_TINT, "after only -> added");
    assert_eq!(px(6, 5), CLEAR, "covered by both -> transparent");
    assert_eq!(px(0, 0), CLEAR, "covered by neither -> transparent");
    assert_eq!(px(11, 11), CLEAR, "covered by neither -> transparent");
}

#[test]
fn overlay_draws_the_after_outline_two_pixels_wide_and_the_before_outline_one() {
    let before = filled_box(GRID, (2, 7), (2, 9), 1);
    let after = filled_box(GRID, (5, 10), (2, 9), 1);
    let overlay = difference_overlay(&before, &after, None, GRID);
    let px = |x, y| overlay_px(&overlay, 12, x, y);
    // After's top boundary: the covered row and the uncovered row above it.
    assert_eq!(px(8, 2), AFTER_OUTLINE);
    assert_eq!(px(8, 1), AFTER_OUTLINE);
    assert_eq!(px(8, 3), ADDED_TINT, "a third row in is back to the tint");
    assert_eq!(px(8, 0), CLEAR, "a second row out is clear");
    // After's left boundary runs through before's area.
    assert_eq!(px(5, 5), AFTER_OUTLINE);
    assert_eq!(px(4, 5), AFTER_OUTLINE);
    // Before's outline is one pixel wide, on its inside.
    assert_eq!(px(2, 5), BEFORE_OUTLINE);
    assert_eq!(px(3, 5), REMOVED_TINT, "one pixel in, the tint again");
    assert_eq!(px(1, 5), CLEAR, "no outline outside before's silhouette");
    // Before's right boundary lies inside after's area: still drawn there.
    assert_eq!(px(7, 5), BEFORE_OUTLINE);
    // Where both silhouettes run through the same pixel, after's line wins.
    assert_eq!(px(6, 9), AFTER_OUTLINE);
}

#[test]
fn overlay_draws_facet_edges_only_inside_the_shared_area() {
    let mut after = filled_box(GRID, (2, 9), (2, 9), 1);
    for y in 2..=9 {
        for x in 6..=9 {
            after[y * 12 + x] = 2;
        }
    }
    let before = filled_box(GRID, (2, 9), (2, 9), 1);
    let derived = difference_overlay(&before, &after, None, GRID);
    assert_eq!(overlay_px(&derived, 12, 5, 5), FACET_EDGE, "facet 1 | 2");
    assert_eq!(overlay_px(&derived, 12, 4, 5), CLEAR, "not on the seam");
    assert_eq!(
        overlay_px(&derived, 12, 6, 5),
        CLEAR,
        "the seam is one pixel"
    );
    // An explicit edge mask replaces the derived edges.
    let mut mask = vec![0u8; 144];
    mask[5 * 12 + 3] = 255;
    let explicit = difference_overlay(&before, &after, Some(&mask), GRID);
    assert_eq!(overlay_px(&explicit, 12, 3, 5), FACET_EDGE);
    assert_eq!(overlay_px(&explicit, 12, 5, 5), CLEAR);
    // Outside after's coverage a mask bit draws nothing.
    mask[0] = 255;
    let outside = difference_overlay(&before, &after, Some(&mask), GRID);
    assert_eq!(overlay_px(&outside, 12, 0, 0), CLEAR);
}

#[test]
fn overlay_has_the_frame_size_and_only_the_defined_alphas() {
    let before = filled_box(GRID, (1, 6), (3, 8), 1);
    let after = filled_box(GRID, (4, 10), (1, 9), 3);
    for edges in [None, Some(vec![255u8; 144])] {
        let overlay = difference_overlay(&before, &after, edges.as_deref(), GRID);
        assert_eq!(overlay.len(), 12 * 12 * 4);
        for pixel in overlay.as_chunks::<4>().0.iter().map(<[u8; 4]>::as_slice) {
            match pixel[3] {
                0 => assert_eq!(pixel, CLEAR, "a transparent pixel carries no color"),
                a => assert!(
                    [
                        REMOVED_TINT,
                        ADDED_TINT,
                        AFTER_OUTLINE,
                        BEFORE_OUTLINE,
                        FACET_EDGE
                    ]
                    .iter()
                    .any(|c| c[3] == a && c[..3] == pixel[..3]),
                    "unexpected pixel {pixel:?}"
                ),
            }
        }
    }
}

#[test]
fn identical_masks_leave_no_tint_and_empty_masks_stay_transparent() {
    let mask = filled_box(GRID, (2, 9), (2, 9), 1);
    let overlay = difference_overlay(&mask, &mask, None, GRID);
    assert!(
        overlay
            .as_chunks::<4>()
            .0
            .iter()
            .map(<[u8; 4]>::as_slice)
            .all(|p| p != REMOVED_TINT && p != ADDED_TINT),
        "no removed or added pixel when nothing changed"
    );
    let empty = vec![0u32; 144];
    let overlay = difference_overlay(&empty, &empty, None, GRID);
    assert_eq!(overlay, vec![0u8; 144 * 4]);
}

#[test]
fn a_mismatched_buffer_gives_a_transparent_layer_not_a_panic() {
    let overlay = difference_overlay(&[1, 1], &[1; 144], None, GRID);
    assert_eq!(overlay, vec![0u8; 144 * 4]);
    let overlay = difference_overlay(&[1; 144], &[1; 144], Some(&[1, 2]), GRID);
    assert_eq!(overlay, vec![0u8; 144 * 4]);
    assert_eq!(difference_overlay(&[], &[], None, (0, 0)), Vec::<u8>::new());
}

#[test]
fn a_stone_clipped_by_the_frame_still_gets_a_closed_outline() {
    let full = filled_box(GRID, (0, 11), (0, 11), 1);
    let overlay = difference_overlay(&full, &full, None, GRID);
    assert_eq!(overlay_px(&overlay, 12, 0, 6), AFTER_OUTLINE);
    assert_eq!(overlay_px(&overlay, 12, 6, 11), AFTER_OUTLINE);
}

#[test]
fn real_solid_renders_give_an_overlay_that_sees_the_change() {
    let same = CompareSession::build(
        input(round_brilliant(), "a"),
        input(round_brilliant(), "b"),
        CompareOrigin::Retarget,
        START_POSE,
    );
    let size = (96, 96);
    let pick = |side: &super::session::CompareSide| {
        render_solid_pick(&side.stone, side.preform_planes, START_POSE, size)
    };
    let (before, after) = (pick(&same.before), pick(&same.after));
    assert!(before.iter().any(|&p| p != 0), "the stone covers pixels");
    let overlay = difference_overlay(&before, &after, None, size);
    assert_eq!(overlay.len(), 96 * 96 * 4);
    assert!(
        overlay
            .as_chunks::<4>()
            .0
            .iter()
            .map(<[u8; 4]>::as_slice)
            .all(|p| p != REMOVED_TINT && p != ADDED_TINT),
        "identical stones differ nowhere"
    );
    assert!(
        overlay
            .as_chunks::<4>()
            .0
            .iter()
            .map(<[u8; 4]>::as_slice)
            .any(|p| p == AFTER_OUTLINE),
        "the silhouette is outlined"
    );
    // A stone against nothing: everything it covers is added.
    let nothing = vec![0u32; before.len()];
    let added = difference_overlay(&nothing, &after, None, size);
    assert!(
        added
            .as_chunks::<4>()
            .0
            .iter()
            .map(<[u8; 4]>::as_slice)
            .any(|p| p == ADDED_TINT)
    );
    assert!(
        added
            .as_chunks::<4>()
            .0
            .iter()
            .map(<[u8; 4]>::as_slice)
            .all(|p| p != REMOVED_TINT)
    );
}

#[test]
fn the_session_radius_is_the_larger_of_the_two_and_symmetric() {
    let ab = CompareSession::build(
        input(round_brilliant(), "a"),
        input(round_brilliant_steeper_crown(), "b"),
        CompareOrigin::Retarget,
        START_POSE,
    );
    let ba = CompareSession::build(
        input(round_brilliant_steeper_crown(), "b"),
        input(round_brilliant(), "a"),
        CompareOrigin::Retarget,
        START_POSE,
    );
    let (r_before, r_after) = (
        ab.before.bounding_radius.expect("closes"),
        ab.after.bounding_radius.expect("closes"),
    );
    assert!((ab.radius - r_before.max(r_after)).abs() < f64::EPSILON);
    assert!((ab.radius - ba.radius).abs() < f64::EPSILON);
    // A side that does not close falls back to the other's radius.
    let one = CompareSession::build(
        input(round_brilliant(), "a"),
        input(unsolvable_design(), "b"),
        CompareOrigin::Retarget,
        START_POSE,
    );
    assert!((one.radius - r_before).abs() < f64::EPSILON);
}

#[test]
fn the_start_pose_fits_the_larger_radius_and_keeps_a_wider_view() {
    let radius = 3.0;
    let (min, max) = orbit_distance_bounds(radius);
    let fit = fit_distance_for_radius(radius);
    let near = fitted_pose(START_POSE, radius);
    assert!(
        near.distance >= fit.clamp(min, max) - 1e-4,
        "pulled out to fit"
    );
    assert!((near.yaw - START_POSE.yaw).abs() < f32::EPSILON);
    let far = CameraPose {
        distance: fit * 1.3,
        ..START_POSE
    };
    assert!((fitted_pose(far, radius).distance - far.distance).abs() < 1e-4);
    let huge = CameraPose {
        distance: 1.0e6,
        pitch: 9.0,
        ..START_POSE
    };
    let clamped = fitted_pose(huge, radius);
    assert!((clamped.distance - max).abs() < 1e-3);
    assert!((clamped.pitch - FRAC_PI_2).abs() < f32::EPSILON);
    let larger = fitted_pose(START_POSE, 5.0).distance;
    assert!(
        larger > near.distance,
        "a wider stone fits from further out"
    );
}

#[test]
fn traced_never_starts_while_the_button_is_held() {
    assert!(!traced_may_start(true));
    assert!(traced_may_start(false));
}

#[test]
fn a_layout_switch_makes_the_trace_wait_for_the_new_slot_size_but_only_briefly() {
    // No switch pending: trace right away.
    assert!(!traced_waits_for_layout(false, 0));
    // A switch whose size push has not arrived: wait, up to the cap.
    assert!(traced_waits_for_layout(true, 0));
    assert!(traced_waits_for_layout(true, MAX_SETTLE_POLLS - 1));
    // A layout that kept its size never pushes one: trace anyway after the cap.
    assert!(!traced_waits_for_layout(true, MAX_SETTLE_POLLS));
}

#[test]
fn the_selected_renderer_index_maps_to_the_renderer_that_renders() {
    // Both layout directions re-read this index, so Path traced stays traced.
    assert_eq!(Renderer::from_index(1), Renderer::Traced);
    assert_eq!(Renderer::from_index(0), Renderer::Solid);
}

#[test]
fn the_metrics_material_is_the_named_one_and_otherwise_follows_the_refractive_index() {
    // A named material is the very material the tracer would use.
    let named = round_brilliant();
    assert_eq!(
        resolve_metrics_material(&named, &[]),
        resolve_side_material(&named, &[]).expect("Diamond resolves")
    );
    // A design that names no material and sits between two built-ins: the tracer refuses to
    // pick a species, the figures are still measured -- at the design's own index.
    let mut nameless = round_brilliant();
    nameless.material = MaterialSelection::none();
    nameless.meta.refractive_index = 2.05;
    assert!(resolve_side_material(&nameless, &[]).is_err());
    let at_205 = resolve_metrics_material(&nameless, &[]);
    nameless.meta.refractive_index = 1.50;
    let at_150 = resolve_metrics_material(&nameless, &[]);
    assert_ne!(at_205, at_150, "the refractive index decides the material");
    assert_ne!(
        at_205,
        resolve_metrics_material(&named, &[]),
        "and it is not silently Diamond"
    );
}

// ---- concave stones: Compare draws, traces and measures them with their tools ----

/// The concave fixture (8 flat tiers, a cylinder groove and a sphere dimple), and the same
/// design with its concave tiers cleared.
fn concave_design() -> Design {
    Design::concave_fixture()
}

fn concave_design_cleared() -> Design {
    let mut design = Design::concave_fixture();
    design.concave_tiers.clear();
    design
}

/// A pose from below the girdle, where the pavilion groove is in view.
fn below(session: &CompareSession) -> CameraPose {
    CameraPose {
        pitch: -0.9,
        ..session.pose
    }
}

#[test]
fn a_concave_side_keeps_its_tools_and_a_planar_side_keeps_exactly_its_planes() {
    let session = CompareSession::build(
        input(concave_design(), "concave"),
        input(round_brilliant(), "planar"),
        CompareOrigin::Snapshot,
        START_POSE,
    );
    assert!(session.before.is_solved());
    assert!(
        !session.before.stone.tools.is_empty(),
        "the groove and the dimple are placed"
    );
    assert!(!session.before.tools_dropped);
    // A planar side: no tools, and the planes are bit for bit the planar conversion's.
    let planar = &session.after;
    assert!(planar.stone.tools.is_empty() && planar.stone.placements.is_empty());
    assert!(!planar.tools_dropped);
    let solved = planar.solved.as_deref().expect("solves");
    assert_eq!(
        planar.stone.planes,
        design_to_gpu_planes_from_solved(&round_brilliant(), solved)
    );
}

#[test]
fn the_concave_stones_solid_render_differs_from_the_same_design_without_its_tiers() {
    let session = CompareSession::build(
        input(concave_design(), "concave"),
        input(concave_design_cleared(), "flat"),
        CompareOrigin::Snapshot,
        START_POSE,
    );
    // The flat planes are the same on both sides; only the tools differ.
    assert_eq!(session.before.stone.planes, session.after.stone.planes);
    assert_eq!(session.after.stone.tools.len(), 0);
    let size = (96, 96);
    let differs = [-0.9_f32, -0.5, 0.0].iter().any(|&pitch| {
        let pose = CameraPose {
            pitch,
            ..session.pose
        };
        let render = |side: &super::session::CompareSide| {
            render_solid_rgba(&side.stone, side.preform_planes, pose, size)
        };
        render(&session.before) != render(&session.after)
    });
    assert!(
        differs,
        "the groove or the dimple shows in the solid render"
    );
    // The session radius is still the stone's own: finite, positive, and no larger than the
    // flat stone's (tools only remove material).
    let carved = session.before.bounding_radius.expect("closes");
    let flat = session.after.bounding_radius.expect("closes");
    assert!(carved > 0.0 && carved <= flat + 1e-9, "{carved} vs {flat}");
}

#[test]
fn the_traced_render_carries_the_tools() {
    let session = CompareSession::build(
        input(concave_design(), "concave"),
        input(concave_design_cleared(), "flat"),
        CompareOrigin::Snapshot,
        START_POSE,
    );
    let material = session.before.material.as_ref().expect("Diamond resolves");
    let pose = below(&session);
    let size = (32, 32);
    let with_tools = render_traced_rgba(&session.before.stone, material, pose, size, 4);
    let without = render_traced_rgba(&session.after.stone, material, pose, size, 4);
    assert_eq!(with_tools.len(), 32 * 32 * 4);
    assert_ne!(with_tools, without, "the tracer subtracts the tools");
    // Same stone, same samples: deterministic.
    assert_eq!(
        with_tools,
        render_traced_rgba(&session.before.stone, material, pose, size, 4)
    );
}

#[test]
fn the_figures_of_a_concave_stone_differ_from_its_flat_twin() {
    let session = CompareSession::build(
        input(concave_design(), "concave"),
        input(concave_design_cleared(), "flat"),
        CompareOrigin::Snapshot,
        START_POSE,
    );
    let metrics = super::metrics::SessionMetrics::measure(
        &session,
        indicatrix::optics::LightingPreset::RingLights,
    );
    let strip = metrics.strip();
    assert_eq!(strip.rows.len(), 5, "both sides are measured");
    assert!(
        strip.rows.iter().any(|row| row.before != row.after),
        "the tools change at least one figure: {:?}",
        strip.rows
    );
}

#[test]
fn a_side_whose_concave_tiers_could_not_be_placed_says_so() {
    let mut session = CompareSession::build(
        input(concave_design(), "a"),
        input(round_brilliant(), "b"),
        CompareOrigin::Snapshot,
        START_POSE,
    );
    assert_eq!(
        status_text(Some(&session), Renderer::Solid, TracedProgress::Done, 48),
        "Solid",
        "a placed concave side needs no extra text"
    );
    session.before.tools_dropped = true;
    let status = status_text(Some(&session), Renderer::Solid, TracedProgress::Done, 48);
    assert!(
        status
            .contains("Before: its concave tiers could not be placed, so the flat stone is shown"),
        "{status}"
    );
    assert!(!status.contains("After:"), "{status}");
}

#[test]
fn only_a_slow_concave_side_gets_fewer_samples() {
    // A planar side always traces at full quality, however slow.
    assert_eq!(traced_spp_for(TRACED_SPP, false, Some(30.0)), TRACED_SPP);
    // A concave side does until it has been measured slow.
    assert_eq!(traced_spp_for(TRACED_SPP, true, None), TRACED_SPP);
    assert_eq!(
        traced_spp_for(TRACED_SPP, true, Some(SLOW_TRACED_SIDE_SECS)),
        TRACED_SPP,
        "exactly at the limit is not over it"
    );
    assert_eq!(
        traced_spp_for(TRACED_SPP, true, Some(SLOW_TRACED_SIDE_SECS + 0.1)),
        TRACED_SPP / 2
    );
    assert_eq!(
        traced_spp_for(1, true, Some(99.0)),
        1,
        "never below one sample"
    );
}

#[test]
fn the_status_names_a_reduced_side_and_is_silent_at_full_quality() {
    assert_eq!(reduced_spp_note([TRACED_SPP, TRACED_SPP], TRACED_SPP), None);
    let note = reduced_spp_note([TRACED_SPP, TRACED_SPP / 2], TRACED_SPP).expect("reduced");
    assert!(note.contains("After") && !note.contains("Before"), "{note}");
    assert!(note.contains(&format!("{} spp", TRACED_SPP / 2)), "{note}");
}

// --- the "Compare against" reference picker (Optimize comparison) ---

#[test]
fn the_reference_index_round_trips_and_defaults_to_the_current_design() {
    use super::origins::Reference;
    assert_eq!(Reference::default(), Reference::Current);
    assert_eq!(
        Reference::from_index(Reference::Current.index()),
        Reference::Current
    );
    assert_eq!(
        Reference::from_index(Reference::Original.index()),
        Reference::Original
    );
    assert_eq!(Reference::from_index(7), Reference::Current);
}

#[test]
fn the_before_side_is_the_original_when_picked_and_the_live_design_otherwise() {
    use super::origins::{Reference, pick_before};
    let current = round_brilliant();
    let original = round_brilliant_steeper_crown();
    let held = || Some((original.clone(), "Before retarget to Sapphire".to_string()));

    let (design, label, offer) = pick_before(&current, held(), Reference::Current);
    assert_eq!(design, current);
    assert_eq!(label, "Current design");
    assert_eq!(
        offer.original_label.as_deref(),
        Some("Before retarget to Sapphire")
    );

    let (design, label, offer) = pick_before(&current, held(), Reference::Original);
    assert_eq!(design, original);
    assert!(label.starts_with("Original"));
    assert_eq!(offer.selected, Reference::Original);
}

#[test]
fn the_picker_is_hidden_and_falls_back_to_current_when_no_original_is_held() {
    use super::origins::{Reference, pick_before, reference_options};
    let current = round_brilliant();
    let (design, label, offer) = pick_before(&current, None, Reference::Original);
    assert_eq!(design, current);
    assert_eq!(label, "Current design");
    assert!(offer.original_label.is_none());
    assert_eq!(
        reference_options(&offer),
        vec!["Current design".to_string()]
    );
}
