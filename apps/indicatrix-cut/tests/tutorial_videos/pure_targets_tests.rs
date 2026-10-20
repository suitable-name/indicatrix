//! Unit tests of [`crate::pure_targets`]. They live in their own file, included only by
//! `tests/tutorial_videos_targets.rs`: the video harness (`main.rs`) has `harness = false`, so a
//! `#[test]` there never runs, and a test module inside the helper file would only be compiled
//! (with its imports unused) into the harness.

use crate::pure_targets::{nearest_within, wheel_steps};

#[test]
fn a_wheel_turn_is_split_into_ticks_of_at_most_120() {
    assert_eq!(wheel_steps(100.0), vec![100.0]);
    assert_eq!(wheel_steps(-240.0), vec![-120.0, -120.0]);
    assert_eq!(wheel_steps(-300.0), vec![-100.0, -100.0, -100.0]);
}

#[test]
fn a_big_wheel_turn_is_capped_at_twelve_ticks() {
    let steps = wheel_steps(3000.0);
    assert_eq!(steps.len(), 12);
    assert!((steps.iter().sum::<f32>() - 3000.0).abs() < 1e-3);
}

#[test]
fn no_wheel_ticks_for_zero_or_non_finite_deltas() {
    assert_eq!(wheel_steps(0.0), [] as [f32; 0]);
    assert_eq!(wheel_steps(f32::NAN), [] as [f32; 0]);
    assert_eq!(wheel_steps(f32::INFINITY), [] as [f32; 0]);
}

#[test]
fn the_nearest_candidate_wins() {
    let centres = [10.0, 52.0, 94.0];
    assert_eq!(nearest_within(&centres, 50.0, 20.0), Some(1));
    assert_eq!(nearest_within(&centres, 12.0, 20.0), Some(0));
    assert_eq!(nearest_within(&centres, 100.0, 20.0), Some(2));
}

#[test]
fn nothing_within_the_tolerance_is_none() {
    let centres = [10.0, 52.0, 94.0];
    assert_eq!(nearest_within(&centres, 70.0, 10.0), None);
    assert_eq!(nearest_within(&[], 70.0, 10.0), None);
}

#[test]
fn the_tolerance_is_inclusive_and_a_tie_goes_to_the_earlier_candidate() {
    assert_eq!(nearest_within(&[40.0], 50.0, 10.0), Some(0));
    assert_eq!(nearest_within(&[40.0, 60.0], 50.0, 10.0), Some(0));
}

#[test]
fn non_finite_candidates_never_match() {
    assert_eq!(
        nearest_within(&[f32::NAN, f32::INFINITY], 0.0, f32::MAX),
        None
    );
    assert_eq!(nearest_within(&[f32::NAN, 3.0], 0.0, 5.0), Some(1));
}
