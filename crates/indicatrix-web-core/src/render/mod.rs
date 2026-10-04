//! The CPU render path, split the way the browser runs it.
//!
//! - A render Worker traces one chunk: [`handle_trace_chunk`], an interleaved pixel
//!   partition (`first_pixel`, `stride`) over a sample range, through the desktop's own
//!   CPU estimator (`renderer::cpu_frame::trace_pixels_interleaved`).
//! - The page merges chunks into the full frame: [`Accumulator`].
//! - The page decides what each Worker traces next: [`ChunkPlanner`].
//!
//! # Passes and partitions
//!
//! A frame is split into `P` interleaved partitions, one per render Worker (partition
//! `i` is pixels `i, i + P, i + 2P, ...`, so `first_pixel = i` and `stride = P`). A
//! *pass* is a sample range `[sample_offset, sample_offset + spp)` that every partition
//! traces. A pass is complete once all `P` partitions' chunks for it have arrived; only
//! complete passes are merged into the displayed sum, in pass order. So on one target
//! every pixel's running sum is exactly the desktop's `cpu_accumulate` called once per
//! pass, in the same order, bit for bit (see this module's tests), and the displayed
//! image never mixes pixels with different sample counts. The same code runs in the
//! browser, but bitwise identity is only claimed on the same target: the tracer's
//! transcendental functions come from the platform's libm, which differs between wasm
//! and native.
//!
//! # Row groups
//!
//! A chunk is traced in row groups (finer interleaved partitions of its own pixels, see
//! [`trace_chunk_in_slices`]) so a Worker can look at its cancel URL between them and stop
//! a chunk whose scene was replaced. Every pixel's sum depends on the pixel index and
//! the sample range alone, so grouping changes no bit.

mod accumulator;
mod planner;
#[cfg(test)]
mod tests;

pub use accumulator::{Accumulator, ChunkOutcome, ChunkRejection};
pub use planner::{
    ChunkAssignment, ChunkPlanner, DEFAULT_LIVE_SPP, EXPORT_MAX_SPP, EXPORT_MIN_SPP, LIVE_MAX_SPP,
    LIVE_MIN_SPP, LOOKAHEAD_PASSES, MAX_CHUNK_SPP, TARGET_CHUNK_MS, clamp_export_spp,
    clamp_live_spp,
};

use glam::Vec3;
use indicatrix::{renderer::cpu_frame::trace_pixels_interleaved_geom, simd::PlanesSoA32};

use crate::scene::OwnedScene;

/// Traces one chunk: sample indices `[sample_offset, sample_offset + spp)` for pixels
/// `first_pixel, first_pixel + stride, ...` of `scene`, returning one SUMMED radiance
/// per owned pixel, in that pixel order.
///
/// A thin call into `renderer::cpu_frame::trace_pixels_interleaved_geom`, the same
/// thread-free core the desktop's `hybrid::cpu_trace_range` runs per thread, with the scene's
/// fluorescence beside it (empty for a non-fluorescent material, which then is
/// `trace_pixels_interleaved` bit for bit). `plane_soa`
/// is normally [`OwnedScene::plane_soa`]; it is a parameter so a caller can reuse its
/// own arena.
#[must_use]
pub fn handle_trace_chunk(
    scene: &OwnedScene,
    plane_soa: &PlanesSoA32,
    first_pixel: u32,
    stride: u32,
    sample_offset: u32,
    spp: u32,
) -> Vec<Vec3> {
    trace_pixels_interleaved_geom(
        &scene.frame_scene(),
        &[],
        scene.fluorescence(),
        plane_soa,
        first_pixel,
        stride,
        sample_offset,
        spp,
    )
}

/// Which pixels and samples one chunk traces: `ToWorker::TraceChunk` without the scene id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRange {
    /// The partition (first pixel).
    pub first_pixel: u32,
    /// The partition count.
    pub stride: u32,
    /// First sample index.
    pub sample_offset: u32,
    /// Samples per pixel.
    pub spp: u32,
}

/// Pixel-samples one row group of an abortable chunk holds, about.
///
/// Small enough that a revoked cancel URL is noticed within a few tens of milliseconds,
/// large enough that the URL is not asked for more often than it is worth.
const SLICE_PIXEL_SAMPLES: u64 = 1024;

/// The most row groups a chunk is split into.
const MAX_SLICES: u32 = 256;

/// How many row groups a chunk of `pixels` pixels at `spp` samples per pixel is traced in.
///
/// One per [`SLICE_PIXEL_SAMPLES`] of work, at least one, at most [`MAX_SLICES`] and never
/// more than the chunk has pixels.
#[must_use]
pub fn slice_count(pixels: u32, spp: u32) -> u32 {
    let work = u64::from(pixels) * u64::from(spp);
    let slices = (work / SLICE_PIXEL_SAMPLES).clamp(1, u64::from(MAX_SLICES));
    u32::try_from(slices)
        .unwrap_or(MAX_SLICES)
        .min(pixels.max(1))
}

/// [`handle_trace_chunk`] for `range`, stopping early: `aborted` is asked before every row
/// group, and the first `true` ends the chunk with `None`.
///
/// Returns exactly what [`handle_trace_chunk`] returns when `aborted` never says so.
#[must_use]
pub fn trace_chunk_abortable(
    scene: &OwnedScene,
    range: ChunkRange,
    aborted: &dyn Fn() -> bool,
) -> Option<Vec<Vec3>> {
    let frame_pixels = scene.width().saturating_mul(scene.height());
    let pixels = partition_len(frame_pixels, range.first_pixel, range.stride);
    trace_chunk_in_slices(scene, range, slice_count(pixels, range.spp), aborted)
}

/// [`trace_chunk_abortable`] with an explicit row-group count.
///
/// Group `g` of `slices` traces the chunk's pixels `g, g + slices, ...` -- the interleaved
/// partition (`first_pixel + g * stride`, `stride * slices`) -- and its sums are put back
/// in the chunk's own pixel order.
#[must_use]
pub fn trace_chunk_in_slices(
    scene: &OwnedScene,
    range: ChunkRange,
    slices: u32,
    aborted: &dyn Fn() -> bool,
) -> Option<Vec<Vec3>> {
    let slices = slices.max(1);
    let plane_soa = scene.plane_soa();
    let trace = |first_pixel: u32, stride: u32| {
        handle_trace_chunk(
            scene,
            plane_soa,
            first_pixel,
            stride,
            range.sample_offset,
            range.spp,
        )
    };
    if slices == 1 {
        return (!aborted()).then(|| trace(range.first_pixel, range.stride));
    }
    let frame_pixels = scene.width().saturating_mul(scene.height());
    let pixels = partition_len(frame_pixels, range.first_pixel, range.stride) as usize;
    let mut out = vec![Vec3::ZERO; pixels];
    let fine_stride = range.stride.saturating_mul(slices);
    for group in 0..slices {
        if aborted() {
            return None;
        }
        let first = range
            .first_pixel
            .saturating_add(group.saturating_mul(range.stride));
        for (n, sum) in trace(first, fine_stride).into_iter().enumerate() {
            if let Some(slot) = out.get_mut(group as usize + n * slices as usize) {
                *slot = sum;
            }
        }
    }
    Some(out)
}

/// How many pixels partition `first_pixel` of a `pixel_count`-pixel frame split
/// `stride` ways owns.
#[must_use]
pub const fn partition_len(pixel_count: u32, first_pixel: u32, stride: u32) -> u32 {
    if stride == 0 || first_pixel >= pixel_count {
        0
    } else {
        (pixel_count - first_pixel).div_ceil(stride)
    }
}
