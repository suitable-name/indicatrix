//! The sweep dialog's form: the default range, reading the three number fields, and the
//! time estimate and summary line shown before a run.

use super::{MAX_SWEEP_ANGLE_DEG, SweepError, SweepPlan, SweepRange};
use crate::{loading::eval_number, solve_policy::SolveCostEstimate};
use indicatrix_cut_core::design::snap_noise;

/// How far either side of the current angle the dialog's range starts out, degrees.
pub const DEFAULT_SWEEP_HALF_RANGE_DEG: f64 = 5.0;

/// The step the dialog starts out with, degrees.
pub const DEFAULT_SWEEP_STEP_DEG: f64 = 0.5;

/// The flattest end the default range reaches, degrees from flat.
const DEFAULT_RANGE_FLOOR_DEG: f64 = 0.5;

/// Seconds a row costs besides its solve: the fast score and the checks.
const ROW_BASE_SECONDS: f64 = 0.03;

/// Seconds a row's solve costs per facet plane of the design.
const SOLVE_SECONDS_PER_PLANE: f64 = 0.0004;

/// A design with tier targets solves several times over (a bootstrap and a bisection).
const TARGET_SOLVE_FACTOR: f64 = 4.0;

/// Seconds the tilt averages cost per row (about 1.4 s measured natively).
const TILT_ROW_SECONDS: f64 = 1.5;

/// How much of the worker count turns into speed.
const PARALLEL_EFFICIENCY: f64 = 0.8;

/// The range the dialog starts with for a tier at `current_deg`, as `(from, to)`.
///
/// Five degrees either side of the angle, kept between 0.5 and 89.5 degrees from flat,
/// flattest first. Magnitudes: a pavilion tier at `-41` gives `(36, 46)`, because the side
/// of the girdle comes from the tier, not from the number.
#[must_use]
pub fn default_range(current_deg: f64) -> (f64, f64) {
    let magnitude = current_deg.abs();
    let top = MAX_SWEEP_ANGLE_DEG - 0.4;
    let low = snap_noise((magnitude - DEFAULT_SWEEP_HALF_RANGE_DEG).max(DEFAULT_RANGE_FLOOR_DEG));
    let high = snap_noise((magnitude + DEFAULT_SWEEP_HALF_RANGE_DEG).min(top));
    (low, high)
}

/// An angle as the text of a number field: its magnitude, no trailing zeros, no float
/// noise.
#[must_use]
pub fn format_angle_input(angle_deg: f64) -> String {
    snap_noise(angle_deg.abs()).to_string()
}

/// Reads the three number fields of the dialog (each may be arithmetic: `41.5 - 3`).
///
/// The angles are read as typed. [`super::plan_sweep`] takes their magnitudes, so `40` and
/// `-40` mean the same for a pavilion tier and for a crown tier: the side of the girdle
/// comes from the tier.
///
/// # Errors
///
/// [`SweepError::BadNumber`] with a message naming the field (`From 'x' is not a
/// number.`).
pub fn parse_sweep_range(from: &str, to: &str, step: &str) -> Result<SweepRange, SweepError> {
    let read = |label: &str, text: &str| {
        eval_number(text, None).map_err(|error| SweepError::BadNumber(error.message(label, text)))
    };
    Ok(SweepRange {
        from_deg: read("From", from)?,
        to_deg: read("To", to)?,
        step_deg: read("Step", step)?,
    })
}

/// A rough guess of how long a sweep of `rows` angles takes, in seconds.
///
/// The solve is read off the design's plane count and tier targets, the tilt averages
/// are 1.5 s a row, and `workers` threads are credited with 80 percent of their count.
/// It is a guide for a dialog, not a promise.
#[must_use]
pub fn estimate_seconds(
    rows: usize,
    tilt_average: bool,
    workers: usize,
    cost: SolveCostEstimate,
) -> f64 {
    let mut solve = SOLVE_SECONDS_PER_PLANE * cost.planes as f64;
    if cost.has_tier_targets {
        solve *= TARGET_SOLVE_FACTOR;
    }
    let tilt = if tilt_average { TILT_ROW_SECONDS } else { 0.0 };
    let per_row = ROW_BASE_SECONDS + solve + tilt;
    let speedup = (workers.max(1) as f64 * PARALLEL_EFFICIENCY).max(1.0);
    rows as f64 * per_row / speedup
}

/// A duration for a sentence: "under a second", "about 12 seconds", "about 3 minutes".
#[must_use]
pub fn format_duration_estimate(seconds: f64) -> String {
    if seconds < 1.0 {
        return "under a second".to_owned();
    }
    if seconds < 90.0 {
        let rounded = if seconds < 10.0 {
            seconds.round()
        } else {
            (seconds / 5.0).round() * 5.0
        };
        let count = rounded as u64;
        return format!("about {count} second{}", if count == 1 { "" } else { "s" });
    }
    let minutes = (seconds / 60.0).round().max(2.0) as u64;
    format!("about {minutes} minutes")
}

/// The line under the form before a run: how many angles, over what range (magnitudes,
/// flattest first), and how long it takes (`duration` from [`format_duration_estimate`]).
#[must_use]
pub fn plan_summary(plan: &SweepPlan, duration: &str) -> String {
    let (Some(first), Some(last)) = (plan.angles.first(), plan.angles.last()) else {
        return String::new();
    };
    format!(
        "{} angles from {:.2} to {:.2} degrees. This takes {duration}.",
        plan.angles.len(),
        first.abs(),
        last.abs()
    )
}
