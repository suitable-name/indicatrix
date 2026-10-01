//! Tests for the pieces both apps share around a handle drag and the Slice tool: the
//! gesture's decisions, the provisional slice's in-place edits, and the pointer <->
//! pick-frame mapping the web's letterboxed Solid view relies on.

use super::{
    ActiveDrag, DiscardReason, DragValue, FacetFrame, GestureInputs, HANDLE_HIT_RADIUS_PX,
    HandleKind, HandleTarget, ProvisionalSlice, ScreenPoint, ScreenSize, SliceLine, SliceSide,
    Step,
    gesture::{apply_step, kind_from_int, kind_to_int, snap_mode, tier_label},
    handle_layout, hit_test,
    provisional::{
        PROVISIONAL_GENERATION, cut_hides_tier, frame_updates_mast_cache, keep_allowed,
        resting_hint, session_outlives, surviving_facets, wheel_turn,
    },
    text,
};
use crate::session::EditorSession;
use glam::Vec3;
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier},
    optics::raytracer::Camera,
};
use indicatrix_cut_core::{ConstraintTier, Design, Edit, PreformSpec};
use indicatrix_solid::preview::view::contain_fit;
use std::{sync::Arc, time::Duration};

fn tier(name: &str, angle_deg: f64, indices: &[f64], constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn solved(mast: f64) -> SolvedTier {
    SolvedTier {
        mast,
        strategy: SolveStrategy::DependencyOrder,
        detail: String::new(),
    }
}

/// A vertical facet facing +X seen from the front, with its three handles laid out on an
/// 800x600 pick frame.
fn target() -> HandleTarget {
    let t = tier("G1", 90.0, &[0.0], MeetConstraint::ScaleReference(0.5));
    let frame = FacetFrame::from_tier(&t, 0.0, 96, Vec3::new(0.5, 0.0, 0.0));
    let camera = Camera::new(0.0, 0.0, 4.0, 42.0);
    let size = ScreenSize::new(800.0, 600.0);
    let layout = handle_layout(&frame, &camera, size, 0.3).expect("in view");
    HandleTarget {
        tier: 1,
        frame,
        layout,
        label: "G1".to_string(),
        provisional: false,
    }
}

fn inputs(masts: Option<Vec<SolvedTier>>) -> GestureInputs {
    GestureInputs {
        start_angle_deg: 90.0,
        masts,
        restore: None,
    }
}

// --- pointer <-> pick frame in a letterboxed view ----------------------------------

#[test]
fn a_handle_marker_drawn_in_a_letterboxed_view_is_grabbed_by_a_pointer_on_it() {
    // An 800x600 raster shown in a 1000x500 logical view: scale 5/6, bars left and right.
    let t = target();
    let fit = contain_fit(1000.0, 500.0, 800, 600).expect("a real view");
    assert!(fit.offset_x > 100.0 && fit.offset_y.abs() < 1e-3);
    // The grab radius keeps its on-screen size: 12 logical px are 14.4 raster px here.
    let radius = fit.length_to_image(HANDLE_HIT_RADIUS_PX);
    assert!((radius - 14.4).abs() < 1e-3);
    for kind in [HandleKind::Angle, HandleKind::Depth, HandleKind::Index] {
        let tip = t.layout.tip(kind);
        // Where the overlay draws the marker, and a click exactly there.
        let (logical_x, logical_y) = fit.to_view(tip.x, tip.y);
        let (px, py) = fit.to_image(logical_x, logical_y);
        assert!(
            (px - tip.x).abs() < 1e-3 && (py - tip.y).abs() < 1e-3,
            "{kind:?}: tip {tip:?} came back as ({px}, {py})"
        );
        let click = ScreenPoint::new(px, py);
        assert_eq!(hit_test(&t.layout, click, radius), Some(kind));
        assert_eq!(t.hit(click, radius), Some(kind));
        // Ten logical px beside the marker still grabs; twenty do not.
        let near = fit.to_image(logical_x + 10.0, logical_y);
        let far = fit.to_image(logical_x + 20.0, logical_y);
        assert_eq!(
            t.hit(ScreenPoint::new(near.0, near.1), radius),
            Some(kind),
            "{kind:?} 10 px away"
        );
        assert_ne!(
            t.hit(ScreenPoint::new(far.0, far.1), radius),
            Some(kind),
            "{kind:?} 20 px away"
        );
    }
}

// --- the gesture ---------------------------------------------------------------------

#[test]
fn a_depth_drag_needs_solved_masts_but_the_other_handles_do_not() {
    let t = target();
    let pointer = ScreenPoint::new(300.0, 200.0);
    let now = Duration::from_secs(3);
    assert!(ActiveDrag::begin(HandleKind::Depth, &t, inputs(None), pointer, now).is_none());
    for kind in [HandleKind::Angle, HandleKind::Index] {
        let drag =
            ActiveDrag::begin(kind, &t, inputs(None), pointer, now).expect("no masts needed");
        assert_eq!(drag.start.start_mast, 0.0);
        assert_eq!(drag.start.pointer, pointer);
        assert_eq!(drag.gesture_now, now);
    }
    let masts = vec![solved(0.4), solved(0.62)];
    let drag = ActiveDrag::begin(HandleKind::Depth, &t, inputs(Some(masts)), pointer, now)
        .expect("solved masts");
    assert_eq!(drag.start.start_mast, 0.62, "the dragged tier's own mast");
    assert_eq!(drag.opening_hint(None), "G1 -> mast 0.62");
}

#[test]
fn live_feedback_counts_the_tiers_whose_mast_followed_the_drag() {
    let t = target();
    let masts = vec![solved(0.4), solved(0.62), solved(0.8)];
    let mut drag = ActiveDrag::begin(
        HandleKind::Angle,
        &t,
        inputs(Some(masts)),
        ScreenPoint::default(),
        Duration::ZERO,
    )
    .expect("begins");
    drag.requested = Some(DragValue::AngleDeg(41.3));
    let now = [solved(0.5), solved(0.7), solved(0.9)];
    let (hint, moved) = drag.live_feedback(Some(&now));
    assert_eq!(moved, vec![0, 2], "tier 1 is the dragged one");
    assert_eq!(hint, "G1 -> 41.3 deg, 2 other tiers follow");
    let (hint, moved) = drag.live_feedback(None);
    assert_eq!(moved, Vec::<usize>::new());
    assert_eq!(hint, "G1 -> 41.3 deg");
}

#[test]
fn only_a_committed_gesture_that_changed_something_toasts() {
    let t = target();
    let mut drag = ActiveDrag::begin(
        HandleKind::Angle,
        &t,
        inputs(None),
        ScreenPoint::default(),
        Duration::ZERO,
    )
    .expect("begins");
    assert_eq!(drag.done_toast(), None, "nothing applied yet");
    drag.requested = Some(DragValue::AngleDeg(41.3));
    drag.progress.applied_any = true;
    let toast = drag.done_toast().expect("a committed drag toasts");
    assert!(toast.contains("41.3") && toast.contains("Undo"), "{toast}");
    drag.provisional = true;
    assert_eq!(
        drag.done_toast(),
        None,
        "a provisional drag has no history entry"
    );
}

#[test]
fn one_gesture_of_session_edits_is_one_undo_step() {
    let mut session = EditorSession::fresh();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("P1", 41.0, &[0.0, 12.0], MeetConstraint::MeetExisting),
        })
        .expect("add applies");
    session.history.end_coalesce_run();
    let now = Duration::from_secs(10);
    // The pointer sweeps through several values; every one is stamped with the
    // gesture's clock, so a pause longer than the coalescing window cannot split it.
    for deg in [41.3, 41.7, 42.4] {
        let applied = apply_step(&mut session, 0, Step::Angle(deg), now).expect("no error");
        assert!(applied.is_some());
    }
    assert_eq!(session.design.tiers[0].angle_deg, 42.4);
    session.history.end_coalesce_run();
    session
        .undo()
        .expect("undo applies")
        .expect("something to undo");
    assert_eq!(
        session.design.tiers[0].angle_deg, 41.0,
        "the whole sweep is one step"
    );
    assert_eq!(
        session.design.tiers.len(),
        1,
        "the earlier AddTier is still there"
    );
}

#[test]
fn a_depth_pin_reports_the_meet_it_replaced_only_once() {
    let mut session = EditorSession::fresh();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("P1", 41.0, &[0.0], MeetConstraint::ScaleReference(0.5)),
        })
        .expect("add applies");
    session
        .apply(Edit::AddTier {
            index: 1,
            tier: tier(
                "P2",
                43.0,
                &[0.0],
                MeetConstraint::MeetNamed(vec!["P1".to_string()]),
            ),
        })
        .expect("add applies");
    session.history.end_coalesce_run();
    let now = Duration::from_secs(1);
    let first = apply_step(&mut session, 1, Step::Mast(0.6), now)
        .expect("no error")
        .expect("applied");
    assert_eq!(
        first.replaced_meet,
        Some(MeetConstraint::MeetNamed(vec!["P1".to_string()]))
    );
    let second = apply_step(&mut session, 1, Step::Mast(0.62), now)
        .expect("no error")
        .expect("applied");
    assert_eq!(second.replaced_meet, None, "the tier is already pinned");
}

#[test]
fn the_handle_ints_the_ui_uses_round_trip_and_snap_follows_shift_and_the_pill() {
    for kind in [HandleKind::Angle, HandleKind::Depth, HandleKind::Index] {
        assert_eq!(kind_from_int(kind_to_int(kind)), Some(kind));
    }
    assert_eq!(kind_from_int(-1), None);
    assert_eq!(snap_mode(false, false), super::SnapMode::Coarse);
    assert_eq!(snap_mode(true, false), super::SnapMode::Fine);
    assert_eq!(snap_mode(true, true), super::SnapMode::Off);
    assert_eq!(
        tier_label(&tier("", 1.0, &[], MeetConstraint::MeetExisting), 4),
        "tier 5"
    );
}

// --- the provisional slice -----------------------------------------------------------

fn committed_design() -> Design {
    let mut design = Design::fresh(PreformSpec::cylinder(96, 1.5, 1.0, 1.5), 96, 8, 1.54);
    design.tiers = vec![tier(
        "P1",
        -41.0,
        &[0.0, 12.0, 24.0, 36.0],
        MeetConstraint::ScaleReference(0.7),
    )];
    design
}

fn slice_line() -> SliceLine {
    SliceLine {
        a: ScreenPoint::new(200.0, 300.0),
        b: ScreenPoint::new(600.0, 320.0),
        size: ScreenSize::new(800.0, 600.0),
        pose: (0.5, 0.3, 4.0),
    }
}

fn corners() -> Arc<Vec<Vec3>> {
    let mut points = Vec::new();
    for x in [-1.0_f32, 1.0] {
        for y in [-1.0_f32, 1.0] {
            for z in [-1.0_f32, 1.0] {
                points.push(Vec3::new(x, y, z));
            }
        }
    }
    Arc::new(points)
}

fn build(design: &Design) -> ProvisionalSlice {
    ProvisionalSlice::build(slice_line(), SliceSide::Right, corners(), design, 7, true)
        .expect("a long line has a plane")
}

#[test]
fn a_provisional_slice_appends_its_tier_and_leaves_the_committed_design_alone() {
    let committed = committed_design();
    let mut slice = build(&committed);
    assert_eq!(slice.tier_index, committed.tiers.len());
    assert_eq!(slice.design.tiers.len(), committed.tiers.len() + 1);
    assert_eq!(committed.tiers.len(), 1, "the source is untouched");
    assert_eq!(slice.base_generation, 7);
    let before = slice.tier().expect("the provisional tier").angle_deg;

    // Edits happen in place on the clone.
    let target_angle = before + 5.0;
    assert!(slice.apply_step(Step::Angle(target_angle)).is_some());
    assert_eq!(slice.tier().map(|t| t.angle_deg), Some(target_angle));
    assert_eq!(slice.snapped.angle_deg, target_angle);
    assert!(
        slice.apply_step(Step::Angle(target_angle)).is_none(),
        "no change, no edit"
    );
    assert!(slice.apply_step(Step::Mast(0.9)).is_some());
    assert!(matches!(
        slice.tier().map(|t| &t.constraint),
        Some(MeetConstraint::ScaleReference(m)) if (*m - 0.9).abs() < 1e-12
    ));
    assert!(slice.apply_step(Step::Teeth(2)).is_some());
    assert!(slice.apply_step(Step::Teeth(0)).is_none());
    // The committed tier never changed.
    assert_eq!(
        slice.design.tiers[0].angle_deg,
        committed.tiers[0].angle_deg
    );
}

#[test]
fn escape_restores_the_snapshot_a_press_took_and_flip_negates_the_normal() {
    let committed = committed_design();
    let mut slice = build(&committed);
    let snapshot = slice.snapshot().expect("a tier to snapshot");
    let angle = snapshot.tier.angle_deg;
    let inputs = slice.gesture_inputs().expect("inputs");
    assert_eq!(inputs.start_angle_deg, angle);
    assert!(inputs.restore.is_some());
    slice.apply_step(Step::Angle(angle + 3.0)).expect("changed");
    slice.apply_step(Step::Teeth(1)).expect("changed");
    assert!(slice.restore(snapshot));
    assert_eq!(slice.tier().map(|t| t.angle_deg), Some(angle));

    let right = slice.line.normal(SliceSide::Right).expect("a plane");
    let left = slice.line.normal(SliceSide::Left).expect("a plane");
    assert!(left.abs_diff_eq(-right, 1e-6));
}

#[test]
fn symmetric_toggling_rebuilds_the_orbit_from_the_snapped_index() {
    let committed = committed_design();
    let mut slice = build(&committed);
    let orbit = slice.tier().expect("tier").indices.len();
    assert!(orbit > 1, "symmetric by default");
    assert!(slice.rebuild_indices(false));
    assert_eq!(
        slice.tier().expect("tier").indices,
        vec![slice.snapped.index]
    );
    assert!(slice.rebuild_indices(true));
    assert_eq!(slice.tier().expect("tier").indices.len(), orbit);
}

#[test]
fn landed_masts_become_a_one_shot_outline_of_the_provisional_facets() {
    let committed = committed_design();
    let mut slice = build(&committed);
    assert!(slice.take_outline_update(None).is_none(), "no masts yet");
    assert!(
        slice.place(&frame_geometry()).is_none(),
        "no masts, no handles"
    );
    // Masts that do not describe the design are ignored.
    slice.note_masts(vec![solved(0.7)]);
    assert!(slice.take_outline_update(None).is_none());
    slice.note_masts(vec![solved(0.7), solved(0.5)]);
    let (changed, ids) = slice.take_outline_update(None).expect("new masts");
    assert!(changed && !ids.is_empty());
    assert_eq!(slice.facet_count(), ids.len());
    assert!(
        slice.take_outline_update(None).is_none(),
        "nothing new the second time"
    );
    slice.note_masts(vec![solved(0.7), solved(0.5)]);
    let (changed, again) = slice.take_outline_update(None).expect("new masts again");
    assert!(
        !changed && again == ids,
        "the same facets: nothing to redraw"
    );
    assert!(slice.hint().starts_with("New tier "), "{}", slice.hint());
}

#[test]
fn a_slice_that_touches_no_facet_of_the_stone_cannot_be_kept() {
    let committed = committed_design();
    let mut slice = build(&committed);
    assert!(!slice.may_keep(), "no frame has landed yet");
    slice.note_masts(vec![solved(0.7), solved(0.5)]);
    let map = slice.facet_map().expect("the masts describe the design");
    let (map_facets, ids) = (map.facet_count(), map.facets_of_tier(1).to_vec());
    assert!(ids.len() >= 2, "a symmetric orbit has several facets");

    // A frame whose geometry has no centroid for any of the tier's ids: nothing
    // survives on the stone, so the facet has no area -- Keep is refused and the hint
    // says what to do.
    let mut geometry = frame_geometry();
    geometry.facet_centroids = Arc::new(vec![None; map_facets]);
    slice
        .take_outline_update(Some(&geometry))
        .expect("new masts");
    assert!(!slice.may_keep());
    assert_eq!(slice.facet_count(), 0);
    assert!(
        slice.hint().contains("depth handle inward"),
        "{}",
        slice.hint()
    );

    // Centroids for two of the ids: those survive, and Keep is allowed.
    let mut centroids = vec![None; map_facets];
    centroids[ids[0] as usize] = Some(Vec3::ZERO);
    centroids[ids[1] as usize] = Some(Vec3::X);
    geometry.facet_centroids = Arc::new(centroids.clone());
    slice.note_masts(vec![solved(0.7), solved(0.5)]);
    slice
        .take_outline_update(Some(&geometry))
        .expect("new masts");
    assert!(slice.may_keep());
    assert_eq!(slice.facet_count(), 2);
    assert_eq!(surviving_facets(&ids, &centroids, map_facets), Some(2));
    // A frame in another facet-id space says nothing.
    assert_eq!(surviving_facets(&ids, &centroids, map_facets + 3), None);
    assert!(keep_allowed(Some(1)) && !keep_allowed(Some(0)) && !keep_allowed(None));
}

#[test]
fn a_turn_of_whole_revolutions_or_a_toothless_wheel_moves_nothing() {
    assert_eq!(wheel_turn(3, 96), Some(3));
    assert_eq!(wheel_turn(-1, 96), Some(95));
    assert_eq!(wheel_turn(96, 96), None);
    assert_eq!(wheel_turn(-192, 96), None);
    assert_eq!(
        wheel_turn(i64::MAX, 96),
        u32::try_from(i64::MAX.rem_euclid(96)).ok()
    );
    assert_eq!(wheel_turn(5, 0), None);
    let committed = committed_design();
    let mut slice = build(&committed);
    assert!(slice.apply_step(Step::Teeth(96)).is_none());
    assert!(slice.apply_step(Step::Teeth(-1)).is_some());
}

#[test]
fn the_first_provisional_replan_chains_from_the_committed_masts() {
    let committed = committed_design();
    let mut slice = build(&committed);
    let committed_masts = [solved(0.7)];
    // Nothing known: a full solve.
    let (last, dirty) = slice.replan_chain(None);
    assert!(last.is_none() && dirty.is_empty());
    // The committed masts plus the pinned mast of the new tier.
    let (last, dirty) = slice.replan_chain(Some(&committed_masts));
    let last = last.expect("chains");
    assert_eq!(last.len(), 2);
    assert_eq!(last[0].mast, 0.7);
    assert!(matches!(
        slice.tier().map(|t| &t.constraint),
        Some(MeetConstraint::ScaleReference(m)) if *m == last[1].mast
    ));
    assert_eq!(dirty, std::collections::BTreeSet::from([1]));
    // Committed masts of another length do not align.
    let (last, _) = slice.replan_chain(Some(&[solved(0.7), solved(0.7)]));
    assert!(last.is_none());
    // Once the session holds a landed frame's masts, they win.
    slice.note_masts(vec![solved(0.7), solved(0.55)]);
    let (last, dirty) = slice.replan_chain(None);
    assert_eq!(last.expect("own masts")[1].mast, 0.55);
    assert_eq!(dirty, std::collections::BTreeSet::from([1]));
}

#[test]
fn the_resting_hint_prefers_the_cut_slider_warning_then_the_tier_then_the_mode() {
    let committed = committed_design();
    let slice = build(&committed);
    assert_eq!(resting_hint(None, -1, false, true), "");
    assert_eq!(
        resting_hint(None, -1, true, true),
        text::slice_mode_hint(true)
    );
    assert_eq!(
        resting_hint(None, -1, true, false),
        text::slice_mode_hint(false)
    );
    assert_eq!(resting_hint(Some(&slice), -1, true, true), slice.hint());
    assert_eq!(
        resting_hint(Some(&slice), 0, true, true),
        text::SLICE_CUT_SLIDER_HINT,
        "the slider stops before the new tier"
    );
    assert!(cut_hides_tier(0, 1) && !cut_hides_tier(1, 1) && !cut_hides_tier(-1, 1));
    assert!(frame_updates_mast_cache(0) && !frame_updates_mast_cache(PROVISIONAL_GENERATION));
}

/// A frame geometry with no centroids: enough to prove `place` needs masts first.
fn frame_geometry() -> indicatrix_solid::preview::FrameGeometry {
    indicatrix_solid::preview::FrameGeometry {
        corner_points: corners(),
        facet_centroids: Arc::new(Vec::new()),
        bounding_radius: 1.5,
        camera: indicatrix_solid::preview::CameraPose {
            yaw: 0.5,
            pitch: 0.3,
            distance: 4.0,
        },
        size: (800, 600),
    }
}

#[test]
fn a_discard_says_why_and_only_a_selection_change_skips_the_replan() {
    assert!(DiscardReason::User.replans());
    assert!(DiscardReason::DesignChanged.replans());
    assert!(!DiscardReason::SelectionChanged.replans());
    assert_eq!(
        DiscardReason::User.toast("C3"),
        text::slice_discarded_toast("C3")
    );
    assert!(
        DiscardReason::DesignChanged
            .toast("C3")
            .contains("design changed")
    );
    assert!(
        DiscardReason::SelectionChanged
            .toast("C3")
            .contains("another tier")
    );
    assert!(session_outlives(3, 3) && !session_outlives(3, 4));
}
