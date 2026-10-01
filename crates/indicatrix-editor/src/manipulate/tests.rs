//! Tests for the GUI-free direct-manipulation math: projection round trips, the facet
//! frame's bit-exact normals, handle layout and hit-testing, drag values and snapping,
//! slice planes and provisional tiers, dependents and wording.

use super::*;
use glam::Vec3;
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier},
    optics::raytracer::Camera,
};
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta, expected_orbit};

const SIZE: ScreenSize = ScreenSize::new(800.0, 600.0);

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

fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

/// `facet_map/build.rs`'s per-facet normal, copied line for line.
fn reference_normal(angle_deg: f64, idx: f64, gear_teeth: f32, is_crown: bool) -> Vec3 {
    let theta = (angle_deg.abs() as f32).to_radians();
    let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
    let phi = 2.0 * std::f32::consts::PI * (idx as f32) / gear_teeth;
    let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
    let normal = if is_crown {
        Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
    } else {
        Vec3::new(sin_theta * cos_phi, -cos_theta, sin_theta * sin_phi)
    };
    normal.normalize()
}

// --- projection ---

#[test]
fn project_then_unproject_round_trips_a_world_point() {
    let point = Vec3::new(0.3, 0.2, -0.1);
    // The last pose has pitch 1.9 rad, past the pole at pi / 2.
    for (yaw, pitch) in [(0.0_f32, 0.0_f32), (0.6, 0.4), (0.3, 1.9)] {
        let camera = Camera::new(yaw, pitch, 3.0, 42.0);
        let screen = project(&camera, point, SIZE).expect("the point is in front of the camera");
        let ray = unproject(&camera, screen, SIZE);
        let miss = (point - ray.origin).cross(ray.dir).length();
        assert!(miss < 1e-4, "pose ({yaw}, {pitch}): ray misses by {miss}");
    }
}

#[test]
fn project_rejects_points_behind_the_camera_and_empty_frames() {
    let camera = Camera::new(0.0, 0.0, 4.0, 42.0);
    assert!(project(&camera, Vec3::new(0.0, 0.0, 10.0), SIZE).is_none());
    assert!(project(&camera, Vec3::ZERO, ScreenSize::new(0.0, 600.0)).is_none());
    let centre = project(&camera, Vec3::ZERO, SIZE).expect("origin is in view");
    assert!((centre.x - 400.0).abs() < 1e-3 && (centre.y - 300.0).abs() < 1e-3);
}

// --- facet frame ---

#[test]
fn facet_frame_normal_is_bit_identical_to_the_facet_map_construction() {
    for (angle, is_crown) in [(34.5, true), (-41.0, false)] {
        let t = tier("X", angle, &[0.0, 24.0], MeetConstraint::MeetExisting);
        for idx in [0.0, 24.0] {
            let frame = FacetFrame::from_tier(&t, idx, 96, Vec3::ZERO);
            let expected = reference_normal(angle, idx, 96.0, is_crown);
            assert_eq!(
                bits(frame.normal),
                bits(expected),
                "angle {angle} idx {idx}"
            );
            assert_eq!(frame.is_crown(), is_crown);
        }
    }
}

/// `facet_map/build.rs` places a tier with NO index-wheel positions at
/// `(0, +-cos t, sin t)` -- an exact zero `x`, not `cos(pi/2)`'s residue -- and reports
/// its index as `0`; the frame must reproduce that, not the index-0 normal.
#[test]
fn facet_frame_of_an_indexless_tier_matches_the_facet_map_special_case() {
    for (angle, sign) in [(34.5_f64, 1.0_f32), (-41.0, -1.0)] {
        let t = tier("X", angle, &[], MeetConstraint::MeetExisting);
        let frame = FacetFrame::from_tier(&t, 0.0, 96, Vec3::ZERO);
        let theta = (angle.abs() as f32).to_radians();
        let expected = Vec3::new(0.0, sign * theta.cos(), theta.sin()).normalize();
        assert_eq!(bits(frame.normal), bits(expected), "angle {angle}");
        assert!(frame.is_indexless());
        assert!(frame.tangent_theta().dot(frame.normal).abs() < 1e-5);
        assert_eq!(bits(frame.tangent_phi()), bits(Vec3::new(-1.0, 0.0, 0.0)));
    }
    let t = tier("X", 34.5, &[0.0], MeetConstraint::MeetExisting);
    assert!(!FacetFrame::from_tier(&t, 0.0, 96, Vec3::ZERO).is_indexless());
}

#[test]
fn facet_frame_tangents_are_unit_and_orthogonal_to_the_normal() {
    for angle in [34.5, -41.0] {
        let t = tier("X", angle, &[6.0], MeetConstraint::MeetExisting);
        let frame = FacetFrame::from_tier(&t, 6.0, 96, Vec3::ZERO);
        for tangent in [frame.tangent_theta(), frame.tangent_phi()] {
            assert!((tangent.length() - 1.0).abs() < 1e-5);
            assert!(tangent.dot(frame.normal).abs() < 1e-5, "angle {angle}");
        }
        // The angle tangent really is d normal / d theta: a small step along it agrees
        // with the normal of a slightly steeper facet.
        let steeper = angle + 0.01_f64.copysign(angle);
        let stepped = reference_normal(steeper, 6.0, 96.0, frame.is_crown());
        let predicted = frame.normal + frame.tangent_theta() * 0.01_f32.to_radians();
        assert!((stepped - predicted).length() < 1e-5, "angle {angle}");
    }
}

// --- handles ---

fn facing_x_frame() -> FacetFrame {
    // A vertical facet (90 deg) at index 0: its normal is +X.
    let t = tier("G1", 90.0, &[0.0], MeetConstraint::MeetExisting);
    FacetFrame::from_tier(&t, 0.0, 96, Vec3::new(0.5, 0.0, 0.0))
}

#[test]
fn handle_layout_points_the_depth_handle_screen_right_for_an_x_facing_facet() {
    let camera = Camera::new(0.0, 0.0, 4.0, 42.0);
    let layout = handle_layout(&facing_x_frame(), &camera, SIZE, 0.3).expect("in view");
    assert!(
        layout.depth_dir.0 > 0.99,
        "depth_dir {:?}",
        layout.depth_dir
    );
    assert!(layout.depth_tip.x > layout.anchor.x);
    // The angle tangent of a vertical crown facet is straight down (-Y), and screen y
    // grows downward.
    assert!(
        layout.angle_dir.1 > 0.99,
        "angle_dir {:?}",
        layout.angle_dir
    );
    assert!(layout.pixels_per_mast_unit > 0.0);
    assert!(layout.pixels_per_degree > 0.0);
    assert!(layout.pixels_per_tooth > 0.0);
    for dir in [layout.angle_dir, layout.depth_dir, layout.index_dir] {
        assert!((dir.0.hypot(dir.1) - 1.0).abs() < 1e-4);
    }
}

#[test]
fn handle_layout_is_none_for_a_facet_behind_the_camera_or_a_bad_length() {
    let camera = Camera::new(0.0, 0.0, 4.0, 42.0);
    let t = tier("G1", 90.0, &[0.0], MeetConstraint::MeetExisting);
    let behind = FacetFrame::from_tier(&t, 0.0, 96, Vec3::new(0.0, 0.0, 10.0));
    assert!(handle_layout(&behind, &camera, SIZE, 0.3).is_none());
    assert!(handle_layout(&facing_x_frame(), &camera, SIZE, 0.0).is_none());
    assert!(handle_layout(&facing_x_frame(), &camera, SIZE, f32::NAN).is_none());
}

#[test]
fn hit_test_picks_the_nearest_tip_inside_the_radius() {
    let camera = Camera::new(0.0, 0.0, 4.0, 42.0);
    let layout = handle_layout(&facing_x_frame(), &camera, SIZE, 0.3).expect("in view");
    let near_depth = ScreenPoint::new(layout.depth_tip.x + 3.0, layout.depth_tip.y);
    assert_eq!(
        hit_test(&layout, near_depth, HANDLE_HIT_RADIUS_PX),
        Some(HandleKind::Depth)
    );
    let near_angle = ScreenPoint::new(layout.angle_tip.x, layout.angle_tip.y - 4.0);
    assert_eq!(
        hit_test(&layout, near_angle, HANDLE_HIT_RADIUS_PX),
        Some(HandleKind::Angle)
    );
    let far = ScreenPoint::new(5.0, 5.0);
    assert_eq!(hit_test(&layout, far, HANDLE_HIT_RADIUS_PX), None);
}

// --- drag values ---

fn synthetic_layout() -> HandleLayout {
    HandleLayout {
        anchor: ScreenPoint::new(100.0, 100.0),
        angle_tip: ScreenPoint::new(140.0, 100.0),
        depth_tip: ScreenPoint::new(100.0, 60.0),
        index_tip: ScreenPoint::new(140.0, 100.0),
        angle_dir: (1.0, 0.0),
        depth_dir: (0.0, -1.0),
        index_dir: (1.0, 0.0),
        pixels_per_degree: 10.0,
        pixels_per_mast_unit: 100.0,
        pixels_per_tooth: 20.0,
    }
}

fn start(kind: HandleKind, angle_deg: f64, mast: f64) -> DragStart {
    DragStart {
        kind,
        start_angle_deg: angle_deg,
        start_mast: mast,
        pointer: ScreenPoint::new(100.0, 100.0),
        layout: synthetic_layout(),
    }
}

fn at(dx: f32, dy: f32) -> ScreenPoint {
    ScreenPoint::new(100.0 + dx, 100.0 + dy)
}

fn angle_of(value: DragValue) -> f64 {
    match value {
        DragValue::AngleDeg(a) => a,
        other => panic!("expected an angle, got {other:?}"),
    }
}

fn mast_of(value: DragValue) -> f64 {
    match value {
        DragValue::Mast(m) => m,
        other => panic!("expected a mast, got {other:?}"),
    }
}

#[test]
fn drag_angle_snaps_to_a_tenth_coarse_a_hundredth_fine_and_not_at_all_off() {
    let s = start(HandleKind::Angle, 40.0, 0.5);
    // 30.4 px at 10 px/deg is 3.04 degrees.
    let pointer = at(30.4, 0.0);
    assert_eq!(angle_of(drag_value(&s, pointer, SnapMode::Coarse)), 43.0);
    assert_eq!(angle_of(drag_value(&s, pointer, SnapMode::Fine)), 43.04);
    let off = angle_of(drag_value(&s, pointer, SnapMode::Off));
    assert!(
        (off - 43.04).abs() < 1e-4 && off != 43.04,
        "unsnapped: {off}"
    );
}

#[test]
fn drag_angle_clamps_at_zero_on_the_tiers_own_side() {
    let crown = start(HandleKind::Angle, 1.0, 0.5);
    let past = angle_of(drag_value(&crown, at(-50.0, 0.0), SnapMode::Coarse));
    assert!(
        past == 0.0 && past.is_sign_positive(),
        "crown clamps at +0.0: {past}"
    );

    // The angle handle points toward a steeper facet: on the pavilion that is a MORE
    // negative angle, so dragging against it heads for the culet.
    let pavilion = start(HandleKind::Angle, -1.0, 0.5);
    let past = angle_of(drag_value(&pavilion, at(-50.0, 0.0), SnapMode::Coarse));
    assert!(
        past == 0.0 && past.is_sign_negative(),
        "pavilion clamps at -0.0: {past}"
    );
    let steeper = angle_of(drag_value(&pavilion, at(50.0, 0.0), SnapMode::Coarse));
    assert_eq!(steeper, -6.0);
    let vertical = angle_of(drag_value(&crown, at(5000.0, 0.0), SnapMode::Off));
    assert_eq!(vertical, 90.0, "a facet is never steeper than vertical");
}

#[test]
fn drag_mast_grows_along_depth_dir_keeps_its_sign_and_never_goes_below_zero() {
    let out = start(HandleKind::Depth, 40.0, 0.5);
    assert_eq!(
        mast_of(drag_value(&out, at(0.0, -50.0), SnapMode::Coarse)),
        1.0
    );
    let negative = start(HandleKind::Depth, 40.0, -0.5);
    assert_eq!(
        mast_of(drag_value(&negative, at(0.0, -50.0), SnapMode::Coarse)),
        -1.0
    );
    let inward = mast_of(drag_value(&out, at(0.0, 200.0), SnapMode::Coarse));
    assert!(inward == 0.0 && inward >= 0.0, "clamped at zero: {inward}");
}

#[test]
fn drag_mast_snaps_to_a_hundredth_coarse_a_thousandth_fine() {
    let s = start(HandleKind::Depth, 40.0, 0.5);
    let pointer = at(0.0, -12.34);
    assert_eq!(mast_of(drag_value(&s, pointer, SnapMode::Coarse)), 0.62);
    assert_eq!(mast_of(drag_value(&s, pointer, SnapMode::Fine)), 0.623);
    let off = mast_of(drag_value(&s, pointer, SnapMode::Off));
    assert!((off - 0.6234).abs() < 1e-5, "unsnapped: {off}");
}

#[test]
fn drag_index_rounds_to_whole_teeth_in_every_mode() {
    let s = start(HandleKind::Index, 40.0, 0.5);
    for snap in [SnapMode::Coarse, SnapMode::Fine, SnapMode::Off] {
        assert_eq!(
            drag_value(&s, at(47.0, 0.0), snap),
            DragValue::IndexTeeth(2)
        );
        assert_eq!(
            drag_value(&s, at(-47.0, 0.0), snap),
            DragValue::IndexTeeth(-2)
        );
        assert_eq!(drag_value(&s, at(3.0, 0.0), snap), DragValue::IndexTeeth(0));
    }
}

#[test]
fn an_edge_on_handle_keeps_its_start_value() {
    let mut s = start(HandleKind::Angle, 40.0, 0.5);
    s.layout.pixels_per_degree = 0.0;
    assert_eq!(
        angle_of(drag_value(&s, at(80.0, 0.0), SnapMode::Coarse)),
        40.0
    );
}

#[test]
fn drag_coalesce_keys_separate_tiers_and_handles() {
    let a = drag_coalesce_key(3, HandleKind::Angle);
    assert_eq!(a, drag_coalesce_key(3, HandleKind::Angle));
    assert_ne!(a, drag_coalesce_key(4, HandleKind::Angle));
    assert_ne!(a, drag_coalesce_key(3, HandleKind::Depth));
    assert_ne!(
        drag_coalesce_key(3, HandleKind::Depth),
        drag_coalesce_key(3, HandleKind::Index)
    );
}

// --- slicing ---

#[test]
fn slice_normal_of_a_horizontal_line_is_vertical_and_left_negates_right() {
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let a = ScreenPoint::new(200.0, 300.0);
    let b = ScreenPoint::new(600.0, 300.0);
    let right = slice_normal(&camera, a, b, SIZE, SliceSide::Right).expect("a real line");
    assert!(
        (right - Vec3::NEG_Y).length() < 1e-3,
        "dragging left to right cuts away what is below: {right:?}"
    );
    let left = slice_normal(&camera, a, b, SIZE, SliceSide::Left).expect("a real line");
    assert_eq!(left, -right);
    assert_eq!(SliceSide::Right.flipped(), SliceSide::Left);
}

#[test]
fn slice_normal_needs_at_least_four_pixels_of_drag() {
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let a = ScreenPoint::new(200.0, 300.0);
    assert!(
        slice_normal(
            &camera,
            a,
            ScreenPoint::new(203.0, 300.0),
            SIZE,
            SliceSide::Right
        )
        .is_none()
    );
    assert!(slice_normal(&camera, a, a, SIZE, SliceSide::Right).is_none());
    assert!(
        slice_normal(
            &camera,
            a,
            ScreenPoint::new(204.5, 300.0),
            SIZE,
            SliceSide::Right
        )
        .is_some()
    );
}

#[test]
fn snap_to_gear_finds_the_nearest_tooth_and_angle_step() {
    // 41 degrees crown, 0.7 degrees of azimuth off tooth 24 (a tooth is 3.75 degrees).
    let theta = 41.0_f32.to_radians();
    let phi = 90.7_f32.to_radians();
    let n = Vec3::new(
        theta.sin() * phi.cos(),
        theta.cos(),
        theta.sin() * phi.sin(),
    );
    let snapped = snap_to_gear(n, 96, 0.1);
    assert_eq!(snapped.index, 24.0);
    assert_eq!(snapped.angle_deg, 41.0);
    assert_eq!(
        bits(snapped.normal),
        bits(reference_normal(41.0, 24.0, 96.0, true))
    );

    let theta = 41.34_f32.to_radians();
    let n = Vec3::new(theta.sin(), theta.cos(), 0.0);
    assert!((snap_to_gear(n, 96, 0.1).angle_deg - 41.3).abs() < 1e-9);
    assert!((snap_to_gear(n, 96, 0.5).angle_deg - 41.5).abs() < 1e-9);
}

#[test]
fn snap_to_gear_wraps_the_index_and_signs_the_angle_by_hemisphere() {
    let theta = 40.0_f32.to_radians();
    let phi = (-3.75_f32).to_radians();
    let n = Vec3::new(
        theta.sin() * phi.cos(),
        -theta.cos(),
        theta.sin() * phi.sin(),
    );
    let pavilion = snap_to_gear(n, 96, 0.1);
    assert_eq!(pavilion.index, 95.0);
    assert_eq!(pavilion.angle_deg, -40.0);

    let culet = snap_to_gear(Vec3::new(1e-4, -1.0, 0.0).normalize(), 96, 0.1);
    assert!(culet.angle_deg == 0.0 && culet.angle_deg.is_sign_negative());
    let table = snap_to_gear(Vec3::new(1e-4, 1.0, 0.0).normalize(), 96, 0.1);
    assert!(table.angle_deg == 0.0 && table.angle_deg.is_sign_positive());
    assert!(table.index.is_sign_positive());
}

#[test]
fn tangency_mast_is_the_farthest_corner_along_the_normal() {
    let mut corners = Vec::new();
    for x in [-0.5_f32, 0.5] {
        for y in [-0.5_f32, 0.5] {
            for z in [-0.5_f32, 0.5] {
                corners.push(Vec3::new(x, y, z));
            }
        }
    }
    assert_eq!(tangency_mast(Vec3::X, &corners), 0.5);
    assert_eq!(tangency_mast(Vec3::X, &[]), 1.0);
}

#[test]
fn slice_tier_expands_the_symmetric_orbit_or_keeps_one_index() {
    let meta = ScheduleMeta::standard_round_brilliant();
    let snapped = SnappedFacet {
        angle_deg: 41.0,
        index: 6.0,
        normal: Vec3::Y,
    };
    let existing = vec!["C1".to_string()];

    let symmetric = slice_tier(&snapped, 0.8, &meta, true, &existing);
    assert_eq!(symmetric.indices, expected_orbit(6.0, 8, true, 96));
    assert!(symmetric.indices.len() > 1);
    assert_eq!(symmetric.name, "C2");
    assert_eq!(symmetric.angle_deg, 41.0);
    assert_eq!(symmetric.constraint, MeetConstraint::ScaleReference(0.8));
    assert!(symmetric.imported_meet.is_none() && symmetric.original_notes.is_none());
    assert_eq!(symmetric.detached, Vec::<f64>::new());

    let single = slice_tier(&snapped, 0.8, &meta, false, &existing);
    assert_eq!(single.indices, vec![6.0]);
}

// --- dependents ---

fn design_of(tiers: Vec<ConstraintTier>) -> Design {
    let mut design = Design::fresh(PreformSpec::cylinder(96, 1.5, 1.0, 1.5), 96, 8, 1.54);
    design.tiers = tiers;
    design
}

fn solved(mast: f64) -> SolvedTier {
    SolvedTier {
        mast,
        strategy: SolveStrategy::DependencyOrder,
        detail: String::new(),
    }
}

#[test]
fn tiers_meeting_finds_named_meets_and_ignores_meet_existing() {
    let design = design_of(vec![
        tier("P1", -40.0, &[0.0], MeetConstraint::ScaleReference(0.5)),
        tier(
            "P2",
            -42.0,
            &[0.0],
            MeetConstraint::MeetNamed(vec!["p1".to_string()]),
        ),
        tier("P3", -44.0, &[0.0], MeetConstraint::MeetExisting),
        tier(
            "C1",
            35.0,
            &[0.0],
            MeetConstraint::MeetNamed(vec!["Z9".to_string()]),
        ),
        tier(
            "C2",
            38.0,
            &[0.0],
            MeetConstraint::MeetNamed(vec!["P1".to_string(), "P3".to_string()]),
        ),
    ]);
    assert_eq!(dependents::tiers_meeting(&design, 0), vec![1, 4]);
    assert_eq!(dependents::tiers_meeting(&design, 2), vec![4]);
    assert_eq!(dependents::tiers_meeting(&design, 3), Vec::<usize>::new());
    assert_eq!(dependents::tiers_meeting(&design, 99), Vec::<usize>::new());
}

#[test]
fn moved_tiers_reports_only_real_movers_and_excludes_the_dragged_tier() {
    let before = [solved(0.5), solved(0.6), solved(0.7), solved(0.8)];
    let after = [solved(0.9), solved(0.6), solved(0.71), solved(0.800_000_1)];
    assert_eq!(dependents::moved_tiers(&before, &after, 0, 1e-3), vec![2]);
    assert_eq!(dependents::moved_tiers(&before, &after, 2, 1e-3), vec![0]);
    assert_eq!(
        dependents::moved_tiers(&before, &after[..3], 0, 1e-3),
        Vec::<usize>::new()
    );
}

// --- wording ---

#[test]
fn the_live_hint_reads_like_the_spec_example() {
    let hint = text::drag_live_hint(HandleKind::Angle, "P1", &DragValue::AngleDeg(41.3), 3);
    assert_eq!(hint, "P1 -> 41.3 deg, 3 other tiers follow");
    let one = text::drag_live_hint(HandleKind::Angle, "P1", &DragValue::AngleDeg(-41.0), 1);
    assert_eq!(one, "P1 -> -41.0 deg, 1 other tier follows");
    let alone = text::drag_live_hint(HandleKind::Depth, "P1", &DragValue::Mast(0.673), 0);
    assert_eq!(alone, "P1 -> mast 0.673");
    let turned = text::drag_live_hint(HandleKind::Index, "P1", &DragValue::IndexTeeth(-2), 0);
    assert_eq!(turned, "P1 -> 2 teeth back");
}

#[test]
fn hover_hints_count_the_tiers_that_meet_by_name() {
    let none = text::handle_hover_hint(HandleKind::Depth, "P1", 0, SnapMode::Coarse);
    assert!(!none.contains("meet"), "{none}");
    let two = text::handle_hover_hint(HandleKind::Depth, "P1", 2, SnapMode::Coarse);
    assert!(two.contains("2 tiers meet it by name"), "{two}");
    let off = text::handle_hover_hint(HandleKind::Angle, "P1", 0, SnapMode::Off);
    assert!(off.contains("Snapping is off"), "{off}");
}

#[test]
fn every_toast_mentions_undo_and_a_replaced_meet_is_named() {
    let angle = text::drag_done_toast(HandleKind::Angle, "P1", &DragValue::AngleDeg(41.3), None);
    assert!(angle.contains("Undo") && angle.contains("41.3"), "{angle}");
    let named = MeetConstraint::MeetNamed(vec!["P2".to_string()]);
    let depth = text::drag_done_toast(HandleKind::Depth, "P1", &DragValue::Mast(0.7), Some(&named));
    assert!(
        depth.contains("Undo") && depth.contains("met P2"),
        "{depth}"
    );
    let index = text::drag_done_toast(HandleKind::Index, "P1", &DragValue::IndexTeeth(3), None);
    assert!(
        index.contains("Undo") && index.contains("3 teeth forward"),
        "{index}"
    );
    let kept = text::slice_kept_toast("C3", 8, 41.0);
    assert!(kept.contains("Undo") && kept.contains("8 facets"), "{kept}");
    assert!(text::slice_mode_hint(true).contains("whole symmetric set"));
    assert!(text::slice_mode_hint(false).contains("single index"));
    let provisional = text::slice_provisional_hint("C3", 1, 41.0, 6.0);
    assert!(
        provisional.contains("1 facet at 41.0 deg, index 6.0"),
        "{provisional}"
    );
}
