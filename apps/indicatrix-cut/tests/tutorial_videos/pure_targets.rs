//! Pure (window-free) helpers of the target lookups, kept in their own file so that
//! `tests/tutorial_videos_targets.rs` can include and unit-test them: the video harness itself is
//! built with `harness = false` and therefore runs no `#[test]`.

/// The index of the candidate centre closest to `y`, or `None` when no candidate lies within
/// `tolerance` (inclusive) of `y`. Non-finite candidates never match. Ties go to the earlier one.
#[must_use]
pub fn nearest_within(candidates: &[f32], y: f32, tolerance: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (index, &center) in candidates.iter().enumerate() {
        let distance = (center - y).abs();
        if !distance.is_finite() || distance > tolerance {
            continue;
        }
        if best.is_none_or(|(_, so_far)| distance < so_far) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

/// The largest wheel delta of one tick, in logical pixels.
const WHEEL_TICK: f32 = 120.0;
/// The most ticks one wheel turn is split into.
const WHEEL_MAX_TICKS: u8 = 12;

/// The wheel ticks that add up to `delta_y`: none for a zero or non-finite delta, otherwise
/// `ceil(|delta_y| / 120)` equal ticks, at most 12 of them (a larger delta gets larger ticks).
#[must_use]
pub fn wheel_steps(delta_y: f32) -> Vec<f32> {
    if !delta_y.is_finite() || delta_y == 0.0 {
        return Vec::new();
    }
    let wanted = (delta_y.abs() / WHEEL_TICK).ceil();
    let count = (1..=WHEEL_MAX_TICKS)
        .find(|&n| f32::from(n) >= wanted)
        .unwrap_or(WHEEL_MAX_TICKS);
    vec![delta_y / f32::from(count); usize::from(count)]
}
