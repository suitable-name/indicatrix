//! Many poses of one stone, evaluated across threads.
//!
//! A tilt sweep is 724 independent pure evaluations (4 axes x 181 poses) of the same
//! prepared stone ([`PreparedEval`]); measured serially that is ~1.36 s. The functions here
//! spread them over `std::thread::scope` workers with a fixed split and write every result
//! to the slot its pose owns, so the output arrays are bit-identical to the serial loop's
//! whatever the thread count. Nothing is reduced across threads: the callers' f64 means
//! still run serially, in the original order, over the finished arrays.
//!
//! Only the Fire-less pass is used ([`MetricsWanted::CONSUMED_ONLY`]): the sweeps read
//! brilliance, extinction and windowing only.

use std::num::NonZeroUsize;

use super::{
    evaluate::{MetricsWanted, evaluate_pose, placeholder_metrics},
    prepared::PreparedEval,
    profile::{EVALUATIONS_PER_AXIS, HALF_AXIS_PITCH_DEG},
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{materials::GemMaterial, raytracer::EnvironmentSource},
};

/// Most worker threads one sweep uses. Past ~16 the 181-pose axes run out of work to split
/// and the fixed cost of a scoped thread stops paying back.
pub(super) const MAX_SWEEP_THREADS: usize = 16;

/// Gate checks made ahead of one batch of evaluations, per worker thread, when a stepped
/// sweep runs in parallel: a cancel is honoured within about this many evaluations per
/// thread (a few milliseconds), and the progress callback runs on the calling thread only.
const GATE_CHUNK_PER_THREAD: usize = 4;

/// One axis's curves, `[brilliance, extinction, windowing]`, each indexed like
/// `TILT_ANGLES_DEG` (181 points, -90 to +90 degrees).
pub(super) type AxisCurves = [[f32; EVALUATIONS_PER_AXIS]; 3];

/// How many threads a sweep uses: `max_threads` (or the machine's parallelism when `None`),
/// at least 1 and at most [`MAX_SWEEP_THREADS`]; always 1 on `wasm32`, which cannot spawn.
pub(super) fn resolve_threads(max_threads: Option<usize>) -> usize {
    if cfg!(target_arch = "wasm32") {
        return 1;
    }
    let available = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    max_threads.unwrap_or(available).clamp(1, MAX_SWEEP_THREADS)
}

/// `[brilliance, extinction, windowing]` of one pose, Fire-less. `None` (no planes) reports
/// the neutral placeholder values for every pose, like the full entry points.
fn consumed_at(prepared: Option<&PreparedEval<'_>>, cam_yaw: f32, cam_pitch: f32) -> [f32; 3] {
    let metrics = prepared.map_or_else(placeholder_metrics, |prepared| {
        evaluate_pose(
            prepared,
            cam_yaw,
            cam_pitch,
            MetricsWanted::CONSUMED_ONLY,
            None,
        )
    });
    [
        metrics.brilliance_pct,
        metrics.extinction_pct,
        metrics.windowing_pct,
    ]
}

/// Evaluates every `(yaw, pitch)` pose (radians) and returns `[brilliance, extinction,
/// windowing]` for each, in pose order.
///
/// With `threads > 1` worker `t` takes poses `t, t + threads, t + 2 * threads, ...`: a
/// static interleaved split (the cost of a pose varies with how much of the stone it
/// sees, so interleaving balances better than contiguous ranges) with no shared state and
/// no scheduling dependence. The worker results are scattered back to their pose slots by
/// the calling thread.
pub(super) fn run_poses(
    prepared: Option<&PreparedEval<'_>>,
    poses: &[(f32, f32)],
    threads: usize,
) -> Vec<[f32; 3]> {
    let threads = threads.clamp(1, poses.len().max(1));
    if threads == 1 {
        return poses
            .iter()
            .map(|&(yaw, pitch)| consumed_at(prepared, yaw, pitch))
            .collect();
    }
    let per_thread: Vec<Vec<[f32; 3]>> = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for first in 0..threads {
            handles.push(scope.spawn(move || {
                poses
                    .iter()
                    .skip(first)
                    .step_by(threads)
                    .map(|&(yaw, pitch)| consumed_at(prepared, yaw, pitch))
                    .collect::<Vec<_>>()
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    let mut out = vec![[0.0f32; 3]; poses.len()];
    for (first, values) in per_thread.iter().enumerate() {
        for (k, value) in values.iter().enumerate() {
            out[first + k * threads] = *value;
        }
    }
    out
}

/// The 181 `(yaw, pitch)` poses of one axis in evaluation order: the shared table-up pole,
/// then pitches `0..=89` at the positive azimuth, then at `positive + 180` degrees (see
/// `evaluate_full_axis_profile_at_azimuth` for the geometry).
fn axis_poses(positive_azimuth_deg: f32) -> [(f32, f32); EVALUATIONS_PER_AXIS] {
    let positive = positive_azimuth_deg.to_radians();
    let negative = (positive_azimuth_deg + 180.0).to_radians();
    let mut poses = [(positive, 90.0f32.to_radians()); EVALUATIONS_PER_AXIS];
    for (index, &deg) in HALF_AXIS_PITCH_DEG.iter().enumerate() {
        let pitch = deg.to_radians();
        poses[1 + index] = (positive, pitch);
        poses[1 + HALF_AXIS_PITCH_DEG.len() + index] = (negative, pitch);
    }
    poses
}

/// Where pose `job` of [`axis_poses`] lands in the 181-point tilt-indexed curve: the pole
/// at tilt 0 (index 90), the positive half reversed into tilt `1..=90` (pitch `p` at index
/// `180 - p`), the negative half laid down directly at tilt `-90..=-1` (pitch `p` at `p`).
const fn tilt_slot(job: usize) -> usize {
    let half = HALF_AXIS_PITCH_DEG.len();
    if job == 0 {
        half
    } else if job <= half {
        2 * half - (job - 1)
    } else {
        job - 1 - half
    }
}

/// Lays one axis's `EVALUATIONS_PER_AXIS` per-pose results out as its three curves.
fn assemble_axis(triples: &[[f32; 3]]) -> AxisCurves {
    let mut curves = [[0.0f32; EVALUATIONS_PER_AXIS]; 3];
    for (job, triple) in triples.iter().enumerate() {
        let slot = tilt_slot(job);
        for (curve, &value) in curves.iter_mut().zip(triple) {
            curve[slot] = value;
        }
    }
    curves
}

/// Full-axis profiles for every azimuth of `azimuths_deg`, evaluated `chunk` poses at a
/// time. Before each pose `gate(axis)` is asked on the calling thread; the first `false`
/// stops the whole sweep (`None`, nothing further is asked), exactly the cancel semantics
/// of the serial stepped sweep. The `chunk` poses after a batch of approvals are then
/// evaluated in parallel, so progress runs at most `chunk` poses ahead of the work.
fn evaluate_axes(
    prepared: Option<&PreparedEval<'_>>,
    azimuths_deg: &[f32],
    threads: usize,
    chunk: usize,
    gate: &mut dyn FnMut(usize) -> bool,
) -> Option<Vec<AxisCurves>> {
    let poses: Vec<(f32, f32)> = azimuths_deg
        .iter()
        .flat_map(|&azimuth_deg| axis_poses(azimuth_deg))
        .collect();
    let total = poses.len();
    let chunk = chunk.clamp(1, total.max(1));
    let mut triples = vec![[0.0f32; 3]; total];
    let mut start = 0;
    while start < total {
        let end = (start + chunk).min(total);
        for job in start..end {
            if !gate(job / EVALUATIONS_PER_AXIS) {
                return None;
            }
        }
        triples[start..end].copy_from_slice(&run_poses(prepared, &poses[start..end], threads));
        start = end;
    }
    Some(
        triples
            .as_chunks::<EVALUATIONS_PER_AXIS>()
            .0
            .iter()
            .map(|chunk| assemble_axis(chunk))
            .collect(),
    )
}

/// The full-axis profile of each azimuth in `azimuths_deg`, with no cancel hook: one
/// prepared stone, every pose in one parallel batch. `max_threads` caps the workers
/// (`None` = the machine's parallelism, at most [`MAX_SWEEP_THREADS`]).
pub(super) fn sweep_axes(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
    azimuths_deg: &[f32],
    max_threads: Option<usize>,
) -> Vec<AxisCurves> {
    let prepared = PreparedEval::new(geom, material, environment);
    // The gate never answers `false`, so the sweep is never stopped.
    evaluate_axes(
        prepared.as_ref(),
        azimuths_deg,
        resolve_threads(max_threads),
        usize::MAX,
        &mut |_| true,
    )
    .unwrap_or_default()
}

/// [`sweep_axes`] with `gate(axis)` asked before every pose (see [`evaluate_axes`]); `None`
/// when the gate stopped the sweep.
pub(super) fn sweep_axes_gated(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
    azimuths_deg: &[f32],
    max_threads: Option<usize>,
    gate: &mut dyn FnMut(usize) -> bool,
) -> Option<Vec<AxisCurves>> {
    let prepared = PreparedEval::new(geom, material, environment);
    let threads = resolve_threads(max_threads);
    // Serial (wasm, or one thread): ask, evaluate, ask, ... exactly as before. Parallel:
    // ask a few evaluations per thread ahead, then run that batch across the workers.
    let chunk = if threads > 1 {
        threads * GATE_CHUNK_PER_THREAD
    } else {
        1
    };
    evaluate_axes(prepared.as_ref(), azimuths_deg, threads, chunk, gate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot map is a bijection onto the 181 tilt indices, with the pole at 90 and the
    /// two edge-on poses at the ends.
    #[test]
    fn every_pose_lands_in_its_own_tilt_slot() {
        let mut seen = [false; EVALUATIONS_PER_AXIS];
        for job in 0..EVALUATIONS_PER_AXIS {
            let slot = tilt_slot(job);
            assert!(!seen[slot], "slot {slot} taken twice");
            seen[slot] = true;
        }
        assert!(seen.iter().all(|&taken| taken));
        assert_eq!(tilt_slot(0), 90);
        // Pitch 0 at the positive azimuth is tilt +90 (index 180), at the negative -90.
        assert_eq!(tilt_slot(1), 180);
        assert_eq!(tilt_slot(91), 0);
        // Pitch 89 is one degree off table-up: tilt +1 / -1.
        assert_eq!(tilt_slot(90), 91);
        assert_eq!(tilt_slot(180), 89);
    }

    /// The threaded split returns what the serial loop does, bit for bit, for any thread
    /// count (including more threads than poses).
    #[test]
    fn the_threaded_batch_is_the_serial_batch() {
        use crate::{geometry::cuts::StandardGemCuts, optics::raytracer::LightingPreset};
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let prepared =
            PreparedEval::new(StoneGeometry::planes_only(&planes), &material, environment);
        let poses: Vec<(f32, f32)> = (0..23u8)
            .map(|i| (f32::from(i) * 0.3, f32::from(i) * 0.06))
            .collect();
        let serial = run_poses(prepared.as_ref(), &poses, 1);
        for threads in [2, 3, 7, 64] {
            let threaded = run_poses(prepared.as_ref(), &poses, threads);
            let bits = |rows: &[[f32; 3]]| -> Vec<u32> {
                rows.iter().flatten().map(|v| v.to_bits()).collect()
            };
            assert_eq!(bits(&serial), bits(&threaded), "threads = {threads}");
        }
    }
}
