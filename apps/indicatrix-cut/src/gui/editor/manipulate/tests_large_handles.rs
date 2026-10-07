//! Slint-free tests of the "Larger handles" preference: the hit radius grows with the
//! drawn size, and the selection hint shows only for a new committed selection.

use super::handles::{hit_kind, hit_radius_px, selection_hint_due};
use indicatrix_editor::manipulate::{HANDLE_HIT_RADIUS_PX, HandleKind, HandleLayout, ScreenPoint};

/// The angle tip sits at `(100, 40)`; the other tips are far away.
fn layout() -> HandleLayout {
    HandleLayout {
        anchor: ScreenPoint::new(100.0, 100.0),
        angle_tip: ScreenPoint::new(100.0, 40.0),
        depth_tip: ScreenPoint::new(400.0, 100.0),
        index_tip: ScreenPoint::new(-200.0, 100.0),
        angle_dir: (0.0, -1.0),
        depth_dir: (1.0, 0.0),
        index_dir: (-1.0, 0.0),
        pixels_per_degree: 2.0,
        pixels_per_mast_unit: 100.0,
        pixels_per_tooth: 10.0,
    }
}

#[test]
fn normal_handles_keep_the_original_hit_radius() {
    assert!((hit_radius_px(1.0, 1.0) - HANDLE_HIT_RADIUS_PX).abs() < 1e-6);
    // The window's scale factor still applies on its own.
    let doubled = 2.0_f32.mul_add(-HANDLE_HIT_RADIUS_PX, hit_radius_px(2.0, 1.0));
    assert!(doubled.abs() < 1e-6);
}

#[test]
fn larger_handles_multiply_the_hit_radius_by_the_drawn_scale() {
    let radius = hit_radius_px(1.5, 1.6);
    let error = (HANDLE_HIT_RADIUS_PX * 1.5).mul_add(-1.6, radius);
    assert!(error.abs() < 1e-4);
    assert!(radius > hit_radius_px(1.5, 1.0));
}

#[test]
fn a_press_that_misses_a_normal_handle_still_grabs_a_larger_one() {
    let layout = layout();
    // 18 px above the angle tip: outside 12 px, inside 12 * 1.6 = 19.2 px.
    let press = ScreenPoint::new(100.0, 22.0);
    assert_eq!(
        hit_kind(&layout, false, press, hit_radius_px(1.0, 1.0)),
        None
    );
    assert_eq!(
        hit_kind(&layout, false, press, hit_radius_px(1.0, 1.6)),
        Some(HandleKind::Angle)
    );
    // Still not unlimited: 25 px away misses even the larger radius.
    let far = ScreenPoint::new(100.0, 15.0);
    assert_eq!(hit_kind(&layout, false, far, hit_radius_px(1.0, 1.6)), None);
}

#[test]
fn the_selection_hint_shows_for_a_new_committed_selection_with_large_handles() {
    // Nothing was selected before, then tier 3.
    assert!(selection_hint_due(true, None, (3, false)));
    // Another tier selected.
    assert!(selection_hint_due(true, Some((2, false)), (3, false)));
    // Coming back from the provisional slice tier to a committed one.
    assert!(selection_hint_due(true, Some((3, true)), (3, false)));
}

#[test]
fn the_selection_hint_stays_quiet_otherwise() {
    // Normal handles have hover; they never need it.
    assert!(!selection_hint_due(false, None, (3, false)));
    // The same selection re-placed by the next frame (an orbit makes dozens a second).
    assert!(!selection_hint_due(true, Some((3, false)), (3, false)));
    // The provisional slice tier brings its own hint.
    assert!(!selection_hint_due(true, None, (3, true)));
    assert!(!selection_hint_due(true, Some((2, false)), (3, true)));
}
