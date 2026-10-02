//! Adaptive sub-batch sizing: [`next_batch_size`] adapts the tracer's sub-batch sample
//! count toward [`TARGET_SUBBATCH`], given how long the previous sub-batch actually took.
//!
//! # Carrying the converged size across jobs
//!
//! Every job's ramp restarts at `1` sample and climbs (at most 4x per step) toward
//! whatever size actually fills [`TARGET_SUBBATCH`] on this machine -- cheap for one job,
//! but `stream_emit::tracer::run_tracer` runs one job per `RenderRequest`, per
//! coordinator lane chunk, and per live Direct request, so a long-running stream pays
//! that ramp-up repeatedly for a size that only depends on this process's own resource
//! profile. [`seeded_batch_size`]/[`remember_converged_subbatch`] cache the last
//! converged size per [`crate::render_core::hybrid::JobKey`] (the same key
//! `render_core::hybrid::calibrate_cached` uses, since both are asking "how fast does
//! this profile actually go"), so a new job starts near steady-state instead of at `1`.

use crate::render_core::hybrid::JobKey;
use std::{
    collections::BTreeMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

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

/// Process-wide cache of the last converged sub-batch size for a given
/// [`JobKey`] -- see this module's top doc comment. A `BTreeMap`, not a hash map, purely
/// to keep iteration/debug output deterministic; lookups are by exact key equality
/// either way.
static CONVERGED_SUBBATCH: LazyLock<Mutex<BTreeMap<JobKey, u32>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// The size a job's adaptive ramp should start from for `key`: `1` (the original
/// cold-start value) if this resource profile has never converged before, otherwise the
/// last size [`remember_converged_subbatch`] recorded for it.
#[must_use]
pub(super) fn seeded_batch_size(key: JobKey) -> u32 {
    CONVERGED_SUBBATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied()
        .unwrap_or(1)
}

/// Records `size` -- the sub-batch size a job's adaptive ramp last reached, whether the
/// job finished, was cancelled, or is simply between sub-batches -- as `key`'s new seed,
/// so the next job/chunk with the same resource profile starts near steady-state instead
/// of ramping from `1` again.
pub(super) fn remember_converged_subbatch(key: JobKey, size: u32) {
    CONVERGED_SUBBATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, size);
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::{cli::ComputeMode, render_core::hybrid::job_key};
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
        renderer::gpu_backend::GpuBackend,
    };
    use indicatrix_net::SceneState;

    /// A minimal valid scene -- this module only needs `job_key` to produce distinct,
    /// real keys (varying `threads` below), not a scene that traces to anything
    /// meaningful.
    fn tiny_scene() -> SceneState {
        SceneState {
            width: 8,
            height: 8,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: indicatrix_net::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
        }
    }

    #[test]
    fn seeded_batch_size_defaults_to_one_before_anything_is_recorded() {
        let gpu = GpuBackend::disabled();
        let key = job_key(&gpu, &tiny_scene(), 11, ComputeMode::Hybrid);
        assert_eq!(seeded_batch_size(key), 1);
    }

    #[test]
    fn remember_then_seed_round_trips_the_converged_size() {
        let gpu = GpuBackend::disabled();
        let key = job_key(&gpu, &tiny_scene(), 12, ComputeMode::Hybrid);
        remember_converged_subbatch(key, 512);
        assert_eq!(seeded_batch_size(key), 512);

        // A later job for the SAME key starts where this one left off, not at 1.
        remember_converged_subbatch(key, 640);
        assert_eq!(seeded_batch_size(key), 640);
    }

    #[test]
    fn different_keys_never_share_a_converged_size() {
        let gpu = GpuBackend::disabled();
        let scene = tiny_scene();
        let a = job_key(&gpu, &scene, 13, ComputeMode::Hybrid);
        let b = job_key(&gpu, &scene, 14, ComputeMode::Hybrid);
        remember_converged_subbatch(a, 900);
        assert_eq!(
            seeded_batch_size(b),
            1,
            "an unrelated key must not see `a`'s size"
        );
        assert_eq!(seeded_batch_size(a), 900);
    }
}
