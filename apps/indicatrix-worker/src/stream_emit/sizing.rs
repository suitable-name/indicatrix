//! Adaptive sub-batch sizing: [`next_batch_size`] adapts the tracer's sub-batch sample
//! count toward [`TARGET_SUBBATCH`], given how long the previous sub-batch actually took.

use std::time::Duration;

/// The wall-clock duration [`next_batch_size`] adapts sub-batch sizes toward. Bounds
/// both cancellation latency (the tracer only checks its cancel flag between
/// sub-batches) and scheduling granularity, on hardware ranging from an A100 on a LAN
/// to a 2060 over hotel wifi, without hardcoding a sample count wrong for either end.
pub(super) const TARGET_SUBBATCH: Duration = Duration::from_millis(100);

/// Absolute ceiling on a single sub-batch, regardless of how favorable the timing that
/// fed [`next_batch_size`] looked. Defense in depth: the relative 4x-per-step clamp
/// below only bounds growth relative to the PREVIOUS result, so nothing stops many
/// steps compounding if measurements keep landing well under [`TARGET_SUBBATCH`] (e.g.
/// dispatch overhead dominating a small sample count on faster hardware). Comfortably
/// below `crate::validate::MAX_SAMPLES_PER_REQUEST` so it can bind before a single
/// sub-batch swallows a whole request's remaining range.
pub(super) const MAX_SUBBATCH: u32 = 2048;

/// Adapts the next sub-batch's sample count toward [`TARGET_SUBBATCH`], given how long
/// `prev` samples actually took to trace. Grows (up to 4x, never past [`MAX_SUBBATCH`])
/// when the previous batch finished well under budget, shrinks toward 1 when it ran
/// over, and never returns 0 -- converges on a sub-batch size that fits the worker's
/// own actual throughput rather than a single hardcoded sample count.
#[must_use]
pub(super) fn next_batch_size(prev: u32, elapsed: Duration) -> u32 {
    if elapsed.is_zero() {
        return prev.saturating_mul(4).clamp(1, MAX_SUBBATCH);
    }
    let ratio = TARGET_SUBBATCH.as_secs_f64() / elapsed.as_secs_f64();
    let scaled = f64::from(prev) * ratio;
    let max_growth = f64::from(prev.saturating_mul(4).clamp(1, MAX_SUBBATCH));
    scaled.clamp(1.0, max_growth).round() as u32
}
