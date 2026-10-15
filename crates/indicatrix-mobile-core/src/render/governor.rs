//! The performance and battery governor: a pure function from the device's condition to
//! how many render threads may run and whether tracing runs at all (plan section 4.10).
//!
//! Rules, in priority order:
//! 1. In the background everything is paused.
//! 2. A thermal headroom above 1.0 (the device is throttling) pauses tracing: "Device Is
//!    Hot".
//! 3. Headroom of 0.95 or more leaves one worker; 0.85 or more halves the workers.
//! 4. Battery saver caps the workers at two and asks for Draft quality.
//!
//! Solve, metrics and tilt threads are exempt: they are short and user-initiated.

use super::PauseReason;

/// The most render threads the pool ever runs.
pub const MAX_WORKERS: usize = 6;

/// Cores the app leaves to the UI and the system.
const RESERVED_CORES: usize = 2;

/// The worker cap under battery saver.
pub const BATTERY_SAVER_WORKERS: usize = 2;

/// Thermal headroom from which the workers are halved.
const HALVE_AT: f32 = 0.85;
/// Thermal headroom from which one worker is left.
const SINGLE_AT: f32 = 0.95;
/// Thermal headroom above which tracing pauses.
const PAUSE_ABOVE: f32 = 1.0;

/// What the governor looks at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GovernorInputs {
    /// The device's CPU core count.
    pub cores: usize,
    /// The device's thermal headroom forecast (0.0 cool, 1.0 at the throttling threshold);
    /// `None` when the device does not report it. NaN counts as `None`.
    pub thermal_headroom: Option<f32>,
    /// Whether the app is in the foreground.
    pub foreground: bool,
    /// The battery-saver preference.
    pub battery_saver: bool,
}

/// What the governor decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GovernorDecision {
    /// How many render threads may trace (0 while paused).
    pub workers: usize,
    /// Why tracing is paused, if it is.
    pub paused: Option<PauseReason>,
    /// Whether the next render should use Draft quality (battery saver).
    pub draft_only: bool,
}

/// The render threads an unconstrained device runs: two cores fewer than it has, at least
/// one and at most [`MAX_WORKERS`].
#[must_use]
pub const fn base_workers(cores: usize) -> usize {
    let free = cores.saturating_sub(RESERVED_CORES);
    if free < 1 {
        1
    } else if free > MAX_WORKERS {
        MAX_WORKERS
    } else {
        free
    }
}

/// Applies the governor rules to `inputs`. Pure.
#[must_use]
pub fn govern(inputs: &GovernorInputs) -> GovernorDecision {
    let base = base_workers(inputs.cores);
    let headroom = inputs.thermal_headroom.filter(|value| value.is_finite());
    let mut workers = match headroom {
        Some(value) if value >= SINGLE_AT => 1,
        Some(value) if value >= HALVE_AT => (base / 2).max(1),
        _ => base,
    };
    if inputs.battery_saver {
        workers = workers.min(BATTERY_SAVER_WORKERS);
    }
    let paused = if !inputs.foreground {
        Some(PauseReason::Background)
    } else if headroom.is_some_and(|value| value > PAUSE_ABOVE) {
        Some(PauseReason::Hot)
    } else {
        None
    };
    GovernorDecision {
        workers: if paused.is_some() { 0 } else { workers },
        paused,
        draft_only: inputs.battery_saver,
    }
}
