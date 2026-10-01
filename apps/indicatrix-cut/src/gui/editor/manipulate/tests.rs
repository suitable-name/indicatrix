//! Slint-free tests of the manipulation module's arithmetic and decisions: the
//! pick-frame/logical conversions, facet selection, the "hide when misaligned"
//! predicates, the drain's apply-only-on-change rule and the small pure helpers.

use super::{
    PROVISIONAL_GENERATION,
    drag::{AppliedEdit, DragProgress, Step, drain_value, start_value},
    frame_updates_mast_cache,
    handles::{
        LogicalHandles, hit_kind, ids_aligned, layout_to_logical, masts_aligned, pick_facet,
    },
    kind_from_int, kind_to_int,
    slice::{
        LandedAction, SliceLine, cut_hides_tier, keep_allowed, landed_action, plan_slice,
        replan_chain, session_outlives, surviving_facets, wheel_turn, with_provisional_tier,
    },
    snap_mode, tier_label,
};
use crate::gui::{
    editor::{
        callbacks::{letterbox_margin, logical_to_pick_pixels, pick_pixels_to_logical},
        state::EditorState,
    },
    render::camera_lighting::contained_request_size,
};
use glam::Vec3;
use indicatrix::geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier};
use indicatrix_cut_core::{ConstraintTier, Edit};
use indicatrix_editor::manipulate::{
    DragValue, HandleKind, HandleLayout, ScreenPoint, ScreenSize, SliceSide, SnapMode,
};
use std::collections::BTreeSet;

/// Asserts two pixel pairs agree to well under a hundredth of a pixel.
fn assert_close(actual: (f32, f32), expected: (f32, f32)) {
    assert!(
        (actual.0 - expected.0).abs() < 1e-3 && (actual.1 - expected.1).abs() < 1e-3,
        "{actual:?} != {expected:?}"
    );
}

/// A handle layout with the anchor at `(100, 100)` and the tips a fixed distance away.
fn layout() -> HandleLayout {
    HandleLayout {
        anchor: ScreenPoint::new(100.0, 100.0),
        angle_tip: ScreenPoint::new(100.0, 40.0),
        depth_tip: ScreenPoint::new(160.0, 100.0),
        index_tip: ScreenPoint::new(40.0, 100.0),
        angle_dir: (0.0, -1.0),
        depth_dir: (1.0, 0.0),
        index_dir: (-1.0, 0.0),
        pixels_per_degree: 2.0,
        pixels_per_mast_unit: 100.0,
        pixels_per_tooth: 10.0,
    }
}

fn tier(name: &str) -> ConstraintTier {
    ConstraintTier {
        angle_deg: 41.0,
        name: name.to_string(),
        indices: vec![0.0, 12.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn applied(generation: u64) -> AppliedEdit {
    AppliedEdit {
        generation,
        replaced_meet: None,
    }
}

// --- pick frame <-> logical -------------------------------------------------------

#[test]
fn pick_to_logical_round_trips_the_pointer_mapping_at_2x() {
    // Solid mode at 2x: no letterbox bars, so the margin is (0, 0).
    let margin = (0.0, 0.0);
    for logical in [(0.0, 0.0), (400.0, 300.0), (123.5, 77.25)] {
        let pick = logical_to_pick_pixels(logical, 2.0, margin);
        assert_close(pick, (logical.0 * 2.0, logical.1 * 2.0));
        assert_close(pick_pixels_to_logical(pick, 2.0, margin), logical);
    }
}

#[test]
fn pick_to_logical_round_trips_in_a_letterboxed_half_mode() {
    // An 800x600 logical viewport at 2x is 1600x1200 physical; a square 1000x1000
    // trace fitted into it is 1200x1200 with 200px bars left and right.
    let viewport_physical = (1600, 1200);
    for view_mode in [1u8, 2u8] {
        let contained = contained_request_size(view_mode, viewport_physical, (1000, 1000));
        assert_eq!(contained, (1200, 1200));
        let margin = letterbox_margin(viewport_physical, contained);
        assert_close(margin, (200.0, 0.0));

        // The pick-frame origin sits at logical x = 100 (200 physical px in).
        assert_close(
            pick_pixels_to_logical((0.0, 0.0), 2.0, margin),
            (100.0, 0.0),
        );
        // A logical click at (150, 50) is pick pixel (100, 100), and back.
        let pick = logical_to_pick_pixels((150.0, 50.0), 2.0, margin);
        assert_close(pick, (100.0, 100.0));
        assert_close(pick_pixels_to_logical(pick, 2.0, margin), (150.0, 50.0));
    }
}

#[test]
fn solid_mode_has_no_letterbox_margin() {
    let viewport_physical = (1600, 1200);
    let contained = contained_request_size(0, viewport_physical, (1000, 1000));
    assert_eq!(contained, viewport_physical);
    assert_close(letterbox_margin(viewport_physical, contained), (0.0, 0.0));
}

#[test]
fn layout_to_logical_converts_every_point_with_the_inverse_mapping() {
    let logical = layout_to_logical(&layout(), 2.0, (200.0, 0.0));
    let expected = LogicalHandles {
        anchor: (150.0, 50.0),
        angle_tip: (150.0, 20.0),
        depth_tip: (180.0, 50.0),
        index_tip: (120.0, 50.0),
    };
    assert_close(logical.anchor, expected.anchor);
    assert_close(logical.angle_tip, expected.angle_tip);
    assert_close(logical.depth_tip, expected.depth_tip);
    assert_close(logical.index_tip, expected.index_tip);
}

// --- which facet carries the handles ----------------------------------------------

#[test]
fn pick_facet_prefers_the_remembered_facet_of_the_selected_tier() {
    let facets = [4, 5, 6];
    let picked = pick_facet(
        Some(5),
        2,
        &facets,
        |id| Some(if facets.contains(&id) { 2 } else { 9 }),
        |_| true,
    );
    assert_eq!(picked, Some(5));
}

#[test]
fn pick_facet_falls_back_to_the_first_facet_with_a_centroid() {
    let facets = [4, 5, 6];
    // A tier-table selection remembers nothing.
    let no_memory = pick_facet(None, 2, &facets, |_| Some(2), |id| id >= 5);
    assert_eq!(no_memory, Some(5));
    // A remembered facet of ANOTHER tier is ignored.
    let other_tier = pick_facet(
        Some(40),
        2,
        &facets,
        |id| Some(usize::from(id == 40) + 2),
        |_| true,
    );
    // Facet 40 reports tier 3, not the selected tier 2, so the fallback wins.
    assert_eq!(other_tier, Some(4));
    // A remembered facet without a centroid is ignored too.
    let no_centroid = pick_facet(Some(4), 2, &facets, |_| Some(2), |id| id != 4);
    assert_eq!(no_centroid, Some(5));
}

#[test]
fn pick_facet_is_none_when_no_facet_of_the_tier_has_a_centroid() {
    assert_eq!(
        pick_facet(Some(4), 2, &[4, 5], |_| Some(2), |_| false),
        None
    );
    assert_eq!(pick_facet(None, 2, &[], |_| Some(2), |_| true), None);
}

// --- hide when misaligned ---------------------------------------------------------

#[test]
fn handles_hide_when_the_solved_masts_do_not_describe_the_design() {
    assert!(masts_aligned(12, 12));
    assert!(!masts_aligned(13, 12), "a tier was just added");
    assert!(!masts_aligned(11, 12), "a tier was just removed");
    assert!(!masts_aligned(3, 0), "nothing solved yet");
}

#[test]
fn handles_hide_when_the_frame_holds_another_facet_id_space() {
    assert!(ids_aligned(210, 210));
    assert!(
        !ids_aligned(210, 96),
        "the Cut slider shows only the first tiers"
    );
    assert!(!ids_aligned(96, 210), "the frame is ahead of the design");
}

// --- hit-testing ------------------------------------------------------------------

#[test]
fn hit_kind_finds_the_tip_under_the_pointer() {
    let layout = layout();
    let near = |p: ScreenPoint| hit_kind(&layout, false, p, 12.0);
    assert_eq!(near(ScreenPoint::new(102.0, 44.0)), Some(HandleKind::Angle));
    assert_eq!(
        near(ScreenPoint::new(155.0, 103.0)),
        Some(HandleKind::Depth)
    );
    assert_eq!(near(ScreenPoint::new(45.0, 96.0)), Some(HandleKind::Index));
    assert_eq!(
        near(ScreenPoint::new(100.0, 100.0)),
        None,
        "the anchor grabs nothing"
    );
}

#[test]
fn a_tier_without_index_positions_has_no_index_handle_to_grab() {
    let layout = layout();
    let on_index_tip = ScreenPoint::new(40.0, 100.0);
    assert_eq!(
        hit_kind(&layout, false, on_index_tip, 12.0),
        Some(HandleKind::Index)
    );
    assert_eq!(hit_kind(&layout, true, on_index_tip, 12.0), None);
    // The other two handles are unaffected.
    assert_eq!(
        hit_kind(&layout, true, ScreenPoint::new(100.0, 40.0), 12.0),
        Some(HandleKind::Angle)
    );
}

// --- the drain applies only what changed ------------------------------------------

/// Runs `values` through [`drain_value`] with a hook that counts applications and
/// records the steps it was handed.
fn drain_all(values: &[DragValue]) -> (DragProgress, Vec<Step>) {
    let mut progress = DragProgress::default();
    let mut steps = Vec::new();
    for &value in values {
        drain_value(&mut progress, value, |step| {
            steps.push(step);
            Some(applied(steps.len() as u64))
        });
    }
    (progress, steps)
}

#[test]
fn the_drain_skips_a_value_equal_to_the_last_one() {
    let (progress, steps) = drain_all(&[
        DragValue::AngleDeg(41.3),
        DragValue::AngleDeg(41.3),
        DragValue::AngleDeg(41.3),
        DragValue::AngleDeg(41.4),
        DragValue::AngleDeg(41.4),
    ]);
    assert_eq!(steps, vec![Step::Angle(41.3), Step::Angle(41.4)]);
    assert!(progress.applied_any);
    assert_eq!(progress.applied_generation, Some(2));
}

#[test]
fn the_drain_calls_the_apply_hook_once_per_changed_value() {
    let mut progress = DragProgress::default();
    let mut calls = 0_u32;
    for value in [1.0, 1.0, 2.0, 2.0, 2.0, 3.0, 2.0] {
        drain_value(&mut progress, DragValue::Mast(value), |_| {
            calls += 1;
            Some(applied(u64::from(calls)))
        });
    }
    // 1, 2, 3, 2: the repeated values never reach the hook.
    assert_eq!(calls, 4);
}

#[test]
fn the_drain_turns_index_running_totals_into_relative_steps() {
    let (progress, steps) = drain_all(&[
        DragValue::IndexTeeth(2),
        DragValue::IndexTeeth(3),
        DragValue::IndexTeeth(1),
        DragValue::IndexTeeth(1),
        DragValue::IndexTeeth(0),
    ]);
    // Totals 2, 3, 1, (1 again: skipped), 0 -> turns of +2, +1, -2, -1.
    assert_eq!(
        steps,
        vec![
            Step::Teeth(2),
            Step::Teeth(1),
            Step::Teeth(-2),
            Step::Teeth(-1)
        ]
    );
    assert_eq!(progress.teeth_applied, 0);
    assert!(progress.applied_any);
}

#[test]
fn an_index_total_of_zero_at_the_start_asks_for_no_turn() {
    let (progress, steps) = drain_all(&[DragValue::IndexTeeth(0)]);
    assert_eq!(steps, Vec::<Step>::new());
    assert!(!progress.applied_any);
}

#[test]
fn a_hook_that_applies_nothing_leaves_the_gesture_unapplied() {
    let mut progress = DragProgress::default();
    let applied_now = drain_value(&mut progress, DragValue::AngleDeg(41.0), |_| None);
    assert!(!applied_now);
    assert!(!progress.applied_any);
    assert_eq!(progress.applied_generation, None);
    // The value was still consumed: asking for it again does not re-run the hook.
    let mut calls = 0;
    drain_value(&mut progress, DragValue::AngleDeg(41.0), |_| {
        calls += 1;
        None
    });
    assert_eq!(calls, 0);
}

#[test]
fn the_toast_keeps_the_first_meet_a_depth_pin_replaced() {
    let mut progress = DragProgress::default();
    drain_value(&mut progress, DragValue::Mast(0.60), |_| {
        Some(AppliedEdit {
            generation: 1,
            replaced_meet: Some(MeetConstraint::MeetNamed(vec!["P2".to_string()])),
        })
    });
    // Later pins of the same gesture find the tier already pinned: nothing replaced.
    drain_value(&mut progress, DragValue::Mast(0.62), |_| Some(applied(2)));
    assert_eq!(
        progress.replaced_meet,
        Some(MeetConstraint::MeetNamed(vec!["P2".to_string()]))
    );
    assert_eq!(progress.applied_generation, Some(2));
}

// --- small pure helpers -----------------------------------------------------------

#[test]
fn the_handle_int_round_trips_and_rejects_unknown_values() {
    for kind in [HandleKind::Angle, HandleKind::Depth, HandleKind::Index] {
        assert_eq!(kind_from_int(kind_to_int(kind)), Some(kind));
    }
    assert_eq!(kind_from_int(-1), None);
    assert_eq!(kind_from_int(3), None);
}

#[test]
fn shift_selects_fine_snapping_and_the_pill_turns_it_off() {
    assert_eq!(snap_mode(false, false), SnapMode::Coarse);
    assert_eq!(snap_mode(true, false), SnapMode::Fine);
    assert_eq!(snap_mode(false, true), SnapMode::Off);
    assert_eq!(snap_mode(true, true), SnapMode::Off);
}

#[test]
fn a_tier_is_labelled_by_name_else_by_its_one_based_number() {
    assert_eq!(tier_label(&tier("P1"), 0), "P1");
    assert_eq!(tier_label(&tier(""), 4), "tier 5");
}

#[test]
fn a_handle_starts_at_the_value_it_already_has() {
    assert_eq!(
        start_value(HandleKind::Angle, 41.0, 0.7),
        DragValue::AngleDeg(41.0)
    );
    assert_eq!(
        start_value(HandleKind::Depth, 41.0, 0.7),
        DragValue::Mast(0.7)
    );
    assert_eq!(
        start_value(HandleKind::Index, 41.0, 0.7),
        DragValue::IndexTeeth(0)
    );
}

// --- the Slice tool ---------------------------------------------------------------

/// A slice line across the middle of a 800x600 frame, seen from a fixed camera.
fn slice_line() -> SliceLine {
    SliceLine {
        a: ScreenPoint::new(200.0, 300.0),
        b: ScreenPoint::new(600.0, 320.0),
        size: ScreenSize::new(800.0, 600.0),
        pose: (0.5, 0.3, 4.0),
    }
}

/// The eight corners of a unit cube: a stand-in for the committed stone.
fn cube_corners() -> Vec<Vec3> {
    let mut corners = Vec::new();
    for x in [-1.0_f32, 1.0] {
        for y in [-1.0_f32, 1.0] {
            for z in [-1.0_f32, 1.0] {
                corners.push(Vec3::new(x, y, z));
            }
        }
    }
    corners
}

#[test]
fn the_generation_guard_keeps_a_session_only_while_the_design_is_untouched() {
    assert!(session_outlives(7, 7));
    assert!(!session_outlives(7, 8), "an edit landed");
    assert!(!session_outlives(8, 7), "an undo went back");

    let mut state = EditorState::fresh();
    let base = state.current_generation();
    assert!(session_outlives(base, state.current_generation()));
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("P1"),
        })
        .expect("add must apply");
    assert!(
        !session_outlives(base, state.current_generation()),
        "a real edit of the committed design must end the session"
    );
}

#[test]
fn the_provisional_generation_is_above_any_reachable_generation() {
    // Read through `black_box` so the comparisons run rather than fold to constants.
    let generation = std::hint::black_box(PROVISIONAL_GENERATION);
    assert!(generation > 1 << 62);
    assert_ne!(generation, u64::MAX);
    assert!(generation > 1_000_000_000_000);
}

#[test]
fn only_the_provisional_generation_is_kept_out_of_the_mast_cache() {
    assert!(!frame_updates_mast_cache(PROVISIONAL_GENERATION));
    for generation in [0, 1, 12_345, u64::MAX] {
        assert!(frame_updates_mast_cache(generation), "{generation}");
    }
}

#[test]
fn flip_on_the_stored_endpoints_negates_the_normal() {
    let line = slice_line();
    let right = line
        .normal(SliceSide::Right)
        .expect("a long line has a plane");
    let left = line
        .normal(SliceSide::Left)
        .expect("a long line has a plane");
    assert!(
        left.abs_diff_eq(-right, 1e-6),
        "right {right:?} left {left:?}"
    );
    assert_eq!(SliceSide::Right.flipped(), SliceSide::Left);
    assert_eq!(SliceSide::Left.flipped(), SliceSide::Right);
}

#[test]
fn a_line_shorter_than_a_few_pixels_defines_no_plane() {
    let mut line = slice_line();
    line.b = ScreenPoint::new(201.0, 300.5);
    assert_eq!(line.normal(SliceSide::Right), None);
}

#[test]
fn flipping_a_plan_keeps_the_index_family_and_mirrors_the_angle_side() {
    let design = EditorState::fresh().design.clone();
    let corners = cube_corners();
    let right = plan_slice(&slice_line(), SliceSide::Right, &corners, &design, true)
        .expect("plan for the right side");
    let left = plan_slice(&slice_line(), SliceSide::Left, &corners, &design, true)
        .expect("plan for the left side");
    // The two planes are exact opposites, so |n.y| (the angle) agrees to the step and
    // the crown/pavilion side is swapped.
    assert!((right.snapped.angle_deg.abs() - left.snapped.angle_deg.abs()).abs() < 1e-9);
    assert!(
        right.snapped.angle_deg.abs() < 1e-9
            || right.snapped.angle_deg.is_sign_positive()
                != left.snapped.angle_deg.is_sign_positive()
    );
    // A tangency mast: the plane just touches the cube, so it is never negative here.
    assert!(matches!(
        right.tier.constraint,
        MeetConstraint::ScaleReference(mast) if mast > 0.0
    ));
    assert_ne!(right.tier.indices.len(), 0);
}

#[test]
fn the_provisional_tier_is_appended_at_the_end_and_the_committed_design_is_untouched() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("P1"),
        })
        .expect("add must apply");
    state
        .apply(Edit::AddTier {
            index: 1,
            tier: tier("P2"),
        })
        .expect("add must apply");
    let committed = state.design.clone();
    let (provisional, index) =
        with_provisional_tier(&committed, tier("C1")).expect("append must apply");
    assert_eq!(index, committed.tiers.len());
    assert_eq!(provisional.tiers.len(), committed.tiers.len() + 1);
    assert_eq!(
        provisional.tiers.last().map(|t| t.name.as_str()),
        Some("C1")
    );
    // Every committed tier keeps its index, so facet ids and selections stay valid.
    for (kept, original) in provisional.tiers.iter().zip(&committed.tiers) {
        assert_eq!(kept.name, original.name);
    }
    assert_eq!(
        state.design.tiers.len(),
        committed.tiers.len(),
        "the source is a clone"
    );
}

// --- keeping the provisional picture on screen ------------------------------------

/// A solved tier at `mast` (the strategy is irrelevant to these tests).
fn solved(mast: f64) -> SolvedTier {
    SolvedTier {
        mast,
        strategy: SolveStrategy::ScaleReference,
        detail: String::new(),
    }
}

#[test]
fn a_committed_frame_over_a_live_session_resubmits_and_never_the_other_way_round() {
    let base = 7;
    // The three generations a landed frame can carry: the session's base, another one
    // (the editor moved on) and the provisional stamp.
    assert_eq!(
        landed_action(Some(base), false, base, base),
        LandedAction::Resubmit,
        "a committed frame of the session's own design replaced the provisional picture"
    );
    assert_eq!(
        landed_action(Some(base), false, base + 1, base + 1),
        LandedAction::Discard,
        "the editor's generation moved: the session is stale whatever the frame is"
    );
    assert_eq!(
        landed_action(Some(base), false, base + 1, PROVISIONAL_GENERATION),
        LandedAction::Discard
    );
    for awaiting in [false, true] {
        assert_eq!(
            landed_action(Some(base), awaiting, base, PROVISIONAL_GENERATION),
            LandedAction::Ignore,
            "a provisional frame never asks for another replan (no ping-pong)"
        );
    }
}

#[test]
fn a_provisional_replan_already_on_its_way_is_not_asked_for_twice() {
    let base = 7;
    assert_eq!(
        landed_action(Some(base), true, base, base),
        LandedAction::Ignore
    );
    // An older committed frame (a reproject can carry one): re-render, do not discard.
    assert_eq!(
        landed_action(Some(base), false, base, base - 1),
        LandedAction::Resubmit
    );
}

#[test]
fn without_a_session_no_frame_asks_for_anything() {
    for frame in [0, 7, PROVISIONAL_GENERATION] {
        for awaiting in [false, true] {
            assert_eq!(
                landed_action(None, awaiting, 7, frame),
                LandedAction::Ignore
            );
        }
    }
}

#[test]
fn the_provisional_replan_chains_its_own_masts_with_the_tier_dirty() {
    let masts = [solved(0.4), solved(0.5), solved(0.6)];
    // Once a provisional frame has landed: those masts, and only the provisional tier
    // (the last one) is dirty, so the plan worker re-solves a subgraph.
    let (last_solved, dirty) = replan_chain(Some(masts.as_slice()), 3, 2);
    let chained: Vec<f64> = last_solved
        .expect("aligned masts are chained")
        .iter()
        .map(|tier| tier.mast)
        .collect();
    assert_eq!(chained, vec![0.4, 0.5, 0.6]);
    assert_eq!(dirty, BTreeSet::from([2]));

    // Before the first frame: a full solve, nothing dirty.
    let (last_solved, dirty) = replan_chain(None, 3, 2);
    assert!(last_solved.is_none());
    assert!(dirty.is_empty());
}

#[test]
fn masts_of_another_tier_count_are_never_chained() {
    let masts = [solved(0.4), solved(0.5)];
    let (last_solved, dirty) = replan_chain(Some(masts.as_slice()), 3, 2);
    assert!(last_solved.is_none(), "a tier was added since those masts");
    assert!(dirty.is_empty());
}

#[test]
fn keep_is_refused_while_no_facet_touches_the_stone_and_allowed_after_the_depth_changed() {
    // A fresh slice sits at the tangency mast: the plane exists but no facet is on
    // the stone yet, so no plane of the tier has a centroid in the frame.
    let tier_facets = [4, 5];
    let tangent = [None; 6];
    let at_zero_depth = surviving_facets(&tier_facets, &tangent, 6);
    assert_eq!(at_zero_depth, Some(0));
    assert!(
        !keep_allowed(at_zero_depth),
        "Keep would add a tier that cuts nothing"
    );

    // Dragging the depth handle inward makes both planes cut a facet.
    let mut cut = [None; 6];
    cut[4] = Some(Vec3::new(0.2, 0.3, 0.1));
    cut[5] = Some(Vec3::new(-0.2, 0.3, 0.1));
    let after_depth = surviving_facets(&tier_facets, &cut, 6);
    assert_eq!(after_depth, Some(2));
    assert!(keep_allowed(after_depth));

    // A single surviving facet (the other still tangent) is enough to keep.
    cut[5] = None;
    assert!(keep_allowed(surviving_facets(&tier_facets, &cut, 6)));
}

#[test]
fn keep_waits_for_the_picture_and_a_foreign_facet_id_space_counts_nothing() {
    assert!(
        !keep_allowed(None),
        "no frame has described the provisional design yet"
    );
    // The frame's centroid table belongs to another design (a committed frame, or the
    // Cut slider): its ids say nothing about this tier.
    assert_eq!(surviving_facets(&[4], &[Some(Vec3::X); 5], 6), None);
    // An id past the end has no centroid.
    assert_eq!(surviving_facets(&[9], &[None, Some(Vec3::X)], 2), Some(0));
}

#[test]
fn the_cut_slider_hides_a_provisional_tier_it_stops_short_of() {
    assert!(!cut_hides_tier(-1, 5), "-1 shows the whole design");
    assert!(
        cut_hides_tier(4, 5),
        "through tier 4 ends before the tier at index 5"
    );
    assert!(!cut_hides_tier(5, 5), "through tier 5 includes it");
    assert!(!cut_hides_tier(9, 5));
}

#[test]
fn a_turn_never_falls_back_to_no_turn_however_large_it_is() {
    assert_eq!(wheel_turn(0, 96), None);
    assert_eq!(wheel_turn(96, 96), None, "a whole revolution moves nothing");
    assert_eq!(wheel_turn(-192, 96), None);
    assert_eq!(wheel_turn(1, 96), Some(1));
    assert_eq!(wheel_turn(-1, 96), Some(95));
    assert_eq!(wheel_turn(97, 96), Some(1));
    // Beyond an i32: reduced modulo the wheel instead of dropped.
    let far = i64::from(i32::MAX) + 5;
    assert_eq!(
        wheel_turn(far, 96),
        Some(u32::try_from(far % 96).expect("fits"))
    );
    assert_eq!(wheel_turn(i64::MAX, 96), Some(31));
    assert_eq!(wheel_turn(i64::MIN, 96), Some(64));
    assert_eq!(wheel_turn(5, 0), None, "a wheel with no teeth cannot turn");
}
