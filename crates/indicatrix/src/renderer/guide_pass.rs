//! Primary-ray-only guide-buffer prepass for denoising radiance that came without guides.
//!
//! An image whose radiance arrived without guide buffers (a remote worker's summed XYZ,
//! or a coordinator's merged lane sum) can then still be denoised by the same
//! edge-avoiding À-Trous filter the local render loop uses.
//!
//! # Why this exists
//!
//! [`crate::renderer::denoise`] is edge-avoiding: it needs a first-hit depth/normal/
//! facet-id per pixel to decide which neighbours may blend together. The GUI's local
//! render loop gets those for free from `trace_spectral_ray`'s `primary_hit_out`
//! parameter; a remote worker's `FRAME`/`PREVIEW` payload carries only summed XYZ
//! radiance, so guide buffers never travel over the wire. Instead this module casts one
//! un-jittered camera ray per pixel and records its first hit via the same
//! intersection `trace_spectral_ray` makes at bounce 0 (through the bit-identical SIMD
//! twin of `intersect_polyhedron`, with the plane arena built once per call), without paying
//! for anything downstream (wavelengths, Stokes vectors, Fresnel splitting).
//!
//! The guides depend only on camera pose and the active facet geometry -- never on which
//! backend produced the radiance, and never on light direction, material or exposure --
//! so a caller computes them once per pose/geometry and reuses them for every frame of
//! that accumulation (the GUI's `bridge::frame_cache::guide_pass::GuideCache`, or a
//! coordinator's per-request display denoiser).
//!
//! # Measured cost
//!
//! One [`generate_guide_buffers`] call, parallel across `thread::available_parallelism`
//! (`StandardGemCuts::standard_round_brilliant`): ~19ms at 800x600, ~82ms at 1920x1080,
//! ~288ms at 3840x2160 -- large enough at 4K that a UI or socket thread runs it on a
//! background thread via [`generate_guide_buffers_cancellable`]. Cancellation is
//! cooperative, via an `AtomicBool` checked between rows.
//!
//! On `wasm32` the prepass runs on the calling thread: `std::thread::Scope::spawn`
//! panics there, so the thread count is pinned to 1 exactly like
//! `renderer::tonemap`/`renderer::denoise` do. Every pixel is a pure function of the
//! camera and planes, so the single-thread output is bit-identical to the threaded one.
//!
//! CPU only: no GPU program is involved, so this never takes a `GpuBackend` turn.
//!
//! Moved here unchanged from `apps/indicatrix-cut`'s `bridge::frame_cache::guide_pass`
//! (which re-exports it) so a server can reproduce the viewer's denoised picture;
//! `renderer::frame_denoise`'s pin test covers this prepass together with the denoise.

use crate::{
    geometry::plane::GpuFacetPlane,
    optics::raytracer::{Camera, build_plane_soa, intersect_polyhedron_soa},
    simd::PlanesSoA32,
};
use glam::Vec3;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

/// One pixel-per-camera-ray depth/normal/facet-id capture, row-major
/// (`index = y * width + x`) -- exactly the shape [`crate::renderer::denoise::GBuffers`]
/// expects for its `depth`/`normal`/`facet_id` fields.
#[derive(Debug, Clone)]
pub struct GuideBuffers {
    /// First-hit ray parameter `t` per pixel; `1.0e6` for a miss.
    pub depth: Vec<f32>,
    /// First-hit facet normal per pixel; [`Vec3::ZERO`] for a miss.
    pub normal: Vec<Vec3>,
    /// First-hit facet index per pixel; `-1` for a miss.
    pub facet_id: Vec<i32>,
}

impl GuideBuffers {
    /// An all-miss buffer of the given size: `depth = 1.0e6` (matching the GUI render
    /// loop's own "no hit yet" sentinel), `normal = ZERO`, `facet_id = -1`.
    #[must_use]
    pub fn miss(width: u32, height: u32) -> Self {
        let pixel_count = (width as usize) * (height as usize);
        Self {
            depth: vec![1.0e6; pixel_count],
            normal: vec![Vec3::ZERO; pixel_count],
            facet_id: vec![-1; pixel_count],
        }
    }
}

/// Casts one un-jittered camera ray per pixel and records its first hit.
///
/// Thin wrapper over [`generate_guide_buffers_cancellable`] with a cancel flag that's
/// never set, so it always runs to completion.
///
/// # Panics
///
/// Never in practice: the wrapped call only returns `None` once its cancel flag is set,
/// and this one's never is.
#[must_use]
pub fn generate_guide_buffers(
    width: u32,
    height: u32,
    camera: &Camera,
    planes: &[GpuFacetPlane],
) -> GuideBuffers {
    generate_guide_buffers_cancellable(width, height, camera, planes, &AtomicBool::new(false))
        .expect("a cancel flag that is never set to true never yields a cancelled result")
}

/// The cancellable core [`generate_guide_buffers`] wraps.
///
/// Casts one un-jittered camera ray per pixel and records its first hit. Parallel across
/// `thread::available_parallelism` natively, chunked by row; single-threaded on
/// `wasm32` (see the module doc comment), with bit-identical output either way.
///
/// `cancel` is checked once per row, so a generation abandoned mid-flight (pose changed
/// again before it finished) stops within roughly one row's work. Returns `None` if
/// cancellation was observed at any point -- the caller must discard any partial
/// buffers rather than publish them.
///
/// Returns an all-miss [`GuideBuffers`] (still correctly sized) for a zero-area image
/// rather than panicking, even if `cancel` is already set.
#[must_use]
pub fn generate_guide_buffers_cancellable(
    width: u32,
    height: u32,
    camera: &Camera,
    planes: &[GpuFacetPlane],
    cancel: &AtomicBool,
) -> Option<GuideBuffers> {
    let job = GuideJob {
        width,
        height,
        camera,
        planes,
        cancel,
    };
    generate_with_threads(job, auto_thread_count())
}

/// [`generate_guide_buffers_cancellable`] writing into caller-owned buffers.
///
/// A caller that regenerates guides repeatedly (a pose that keeps moving) reuses one
/// allocation instead of paying three fresh full-frame vectors per call.
///
/// `out` is resized to `width * height` pixels; every pixel of a completed run is
/// overwritten, so its previous contents are irrelevant. Returns `false` if cancellation
/// was observed, in which case `out` holds a partial mix of old and new rows and must be
/// discarded rather than published. The result of a `true` return is bit-identical to
/// the buffers [`generate_guide_buffers_cancellable`] would allocate.
#[must_use]
pub fn generate_guide_buffers_into(
    width: u32,
    height: u32,
    camera: &Camera,
    planes: &[GpuFacetPlane],
    cancel: &AtomicBool,
    out: &mut GuideBuffers,
) -> bool {
    let job = GuideJob {
        width,
        height,
        camera,
        planes,
        cancel,
    };
    fill_with_threads(job, auto_thread_count(), out)
}

/// The thread count [`generate_guide_buffers_cancellable`] splits its rows across.
// wasm32-unknown-unknown has no OS thread to spawn (`std::thread::Scope::spawn` panics
// at runtime there), so this is pinned to 1, which makes `generate_with_threads` take its
// inline single-chunk path and never reach `thread::scope`. Two cfg-gated definitions,
// like `renderer::tonemap::effective_thread_count`, so the wasm32 arm can be `const fn`.
#[cfg(target_arch = "wasm32")]
const fn auto_thread_count() -> usize {
    1
}

#[cfg(not(target_arch = "wasm32"))]
fn auto_thread_count() -> usize {
    crate::renderer::tonemap::effective_thread_count(0)
}

/// The per-call inputs every row of the prepass shares, bundled so the row worker
/// ([`fill_rows`]) stays under clippy's argument-count limit.
#[derive(Clone, Copy)]
struct GuideJob<'a> {
    width: u32,
    height: u32,
    camera: &'a Camera,
    planes: &'a [GpuFacetPlane],
    cancel: &'a AtomicBool,
}

/// [`generate_guide_buffers_cancellable`] with an explicit thread count (`0` is treated
/// as 1). A count that leaves every row in one chunk runs inline on the calling thread
/// without `thread::scope`; the output does not depend on the count.
fn generate_with_threads(job: GuideJob<'_>, num_threads: usize) -> Option<GuideBuffers> {
    let mut buffers = GuideBuffers {
        depth: Vec::new(),
        normal: Vec::new(),
        facet_id: Vec::new(),
    };
    fill_with_threads(job, num_threads, &mut buffers).then_some(buffers)
}

/// The shared body of [`generate_with_threads`] and [`generate_guide_buffers_into`]:
/// fits `out` to the image and fills every pixel of it, returning `false` on cancellation.
///
/// The plane arena is built once here and shared by every row worker; the `SoA` scan is the
/// bit-identical twin of the scalar `intersect_polyhedron` loop, so the output does not
/// depend on which one runs.
fn fill_with_threads(job: GuideJob<'_>, num_threads: usize, out: &mut GuideBuffers) -> bool {
    let GuideJob {
        width,
        height,
        cancel,
        planes,
        ..
    } = job;
    let pixel_count = (width as usize) * (height as usize);
    if pixel_count == 0 {
        out.depth.clear();
        out.normal.clear();
        out.facet_id.clear();
        return true;
    }
    if cancel.load(Ordering::Relaxed) {
        return false;
    }

    // Every pixel is written below, so a reused buffer needs no re-initialisation: only
    // growth fills, with the miss sentinels.
    out.depth.resize(pixel_count, 1.0e6);
    out.normal.resize(pixel_count, Vec3::ZERO);
    out.facet_id.resize(pixel_count, -1);

    let soa = build_plane_soa(planes);
    let soa = &soa;
    let rows_per_chunk = (height as usize).div_ceil(num_threads.max(1));

    if rows_per_chunk >= height as usize {
        fill_rows(
            job,
            soa,
            0,
            &mut out.depth,
            &mut out.normal,
            &mut out.facet_id,
        );
    } else {
        let chunk_len = rows_per_chunk * width as usize;
        thread::scope(|s| {
            let chunks = out
                .depth
                .chunks_mut(chunk_len)
                .zip(out.normal.chunks_mut(chunk_len))
                .zip(out.facet_id.chunks_mut(chunk_len));
            for (chunk_idx, ((depth_chunk, normal_chunk), facet_chunk)) in chunks.enumerate() {
                let start_y = chunk_idx * rows_per_chunk;
                s.spawn(move || {
                    fill_rows(job, soa, start_y, depth_chunk, normal_chunk, facet_chunk);
                });
            }
        });
    }

    !cancel.load(Ordering::Relaxed)
}

/// Fills the whole rows `depth`/`normal`/`facet_id` hold (each a multiple of
/// `job.width` long), the first of which is image row `start_y`. Stops early once
/// `job.cancel` is set, checked once per row. `soa` is the arena built from `job.planes`.
fn fill_rows(
    job: GuideJob<'_>,
    soa: &PlanesSoA32,
    start_y: usize,
    depth: &mut [f32],
    normal: &mut [Vec3],
    facet_id: &mut [i32],
) {
    let width = job.width as usize;
    let rows = depth
        .chunks_mut(width)
        .zip(normal.chunks_mut(width))
        .zip(facet_id.chunks_mut(width));
    for (local_y, ((depth_row, normal_row), facet_row)) in rows.enumerate() {
        if job.cancel.load(Ordering::Relaxed) {
            return;
        }
        let y = start_y + local_y;
        let cells = depth_row
            .iter_mut()
            .zip(normal_row.iter_mut())
            .zip(facet_row.iter_mut());
        for (x, ((depth_out, normal_out), facet_out)) in cells.enumerate() {
            // No jitter: this is a single deterministic prepass, not an accumulated
            // sample -- there is nothing to anti-alias against.
            let ray = job.camera.generate_ray(
                x as f32,
                y as f32,
                job.width as f32,
                job.height as f32,
                0.0,
                0.0,
            );
            let hit = intersect_polyhedron_soa(ray, soa);

            *depth_out = hit.map_or(1.0e6, |h| h.t);
            *normal_out = hit.map_or(Vec3::ZERO, |h| h.normal);
            *facet_out = hit.map_or(-1, |h| h.facet_idx as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::cuts::StandardGemCuts;

    #[test]
    fn generate_guide_buffers_is_correctly_sized_and_facet_bounded() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let guides = generate_guide_buffers(16, 12, &camera, &planes);

        assert_eq!(guides.depth.len(), 16 * 12);
        assert_eq!(guides.normal.len(), 16 * 12);
        assert_eq!(guides.facet_id.len(), 16 * 12);

        // Looking straight at a centred gem from a reasonable distance, the centre
        // pixel must hit *some* facet, and every recorded facet id must be a valid
        // index into `planes` (or the -1 "miss" sentinel).
        let centre = 6 * 16 + 8;
        assert!(
            guides.facet_id[centre] >= 0,
            "centre pixel should hit the gem"
        );
        for &id in &guides.facet_id {
            assert!(
                id == -1 || (id as usize) < planes.len(),
                "facet id {id} out of range for {} planes",
                planes.len()
            );
        }
    }

    #[test]
    fn generate_guide_buffers_handles_zero_area_without_panicking() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.0, 0.0, 2.4, 42.0);
        let guides = generate_guide_buffers(0, 0, &camera, &planes);
        assert_eq!(guides.depth.len(), 0);
        assert_eq!(guides.normal.len(), 0);
        assert_eq!(guides.facet_id.len(), 0);
    }

    #[test]
    fn a_miss_pixel_gets_the_sentinel_depth_and_facet_id() {
        // A ray aimed far off the gem's silhouette misses every plane.
        let planes = StandardGemCuts::standard_round_brilliant();
        // Pull the camera far back and look at a corner of a tiny image so at least
        // the corner pixels miss.
        let camera = Camera::new(0.0, 1.5, 50.0, 5.0);
        let guides = generate_guide_buffers(4, 4, &camera, &planes);
        assert!(
            guides.facet_id.contains(&-1),
            "expected at least one miss pixel at this camera distance/fov"
        );
        for (i, &id) in guides.facet_id.iter().enumerate() {
            if id == -1 {
                assert_eq!(guides.depth[i].to_bits(), 1.0e6_f32.to_bits());
                assert_eq!(guides.normal[i], Vec3::ZERO);
            }
        }
    }

    #[test]
    fn generate_guide_buffers_cancellable_matches_the_non_cancellable_version_when_never_cancelled()
    {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let cancel = AtomicBool::new(false);

        let expected = generate_guide_buffers(16, 12, &camera, &planes);
        let actual = generate_guide_buffers_cancellable(16, 12, &camera, &planes, &cancel)
            .expect("an AtomicBool that's never set true must never yield a cancelled result");

        assert_eq!(actual.depth, expected.depth);
        assert_eq!(actual.facet_id, expected.facet_id);
    }

    /// The single-thread path (the only one `wasm32` ever takes) must produce exactly
    /// the buffers the threaded native path does, for thread counts that split the rows
    /// unevenly and for one larger than the row count.
    #[test]
    fn single_thread_and_multi_thread_paths_produce_identical_guides() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let cancel = AtomicBool::new(false);
        let job = GuideJob {
            width: 21,
            height: 13,
            camera: &camera,
            planes: &planes,
            cancel: &cancel,
        };
        let single = generate_with_threads(job, 1).expect("never cancelled");
        assert!(
            single.facet_id.iter().any(|&id| id >= 0),
            "fixture must hit the gem somewhere"
        );
        let bits = |g: &GuideBuffers| -> Vec<u32> {
            g.depth
                .iter()
                .copied()
                .map(f32::to_bits)
                .chain(g.normal.iter().flat_map(|n| n.to_array().map(f32::to_bits)))
                .collect()
        };
        for threads in [2, 3, 7, 64] {
            let multi = generate_with_threads(job, threads).expect("never cancelled");
            assert_eq!(bits(&multi), bits(&single), "{threads} threads");
            assert_eq!(multi.facet_id, single.facet_id, "{threads} threads");
        }
    }

    /// Regenerating into a buffer that still holds another pose's (and another size's)
    /// guides yields exactly the freshly allocated result.
    #[test]
    fn generating_into_a_reused_buffer_matches_a_fresh_allocation() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let cancel = AtomicBool::new(false);
        let old_camera = Camera::new(1.10, 0.20, 2.4, 42.0);
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);

        let mut reused = generate_guide_buffers(24, 16, &old_camera, &planes);
        assert!(generate_guide_buffers_into(
            16,
            12,
            &camera,
            &planes,
            &cancel,
            &mut reused
        ));
        let fresh = generate_guide_buffers(16, 12, &camera, &planes);

        assert_eq!(reused.facet_id, fresh.facet_id);
        assert_eq!(reused.depth.len(), fresh.depth.len());
        for (a, b) in reused.depth.iter().zip(&fresh.depth) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        assert_eq!(reused.normal, fresh.normal);
    }

    #[test]
    fn generate_guide_buffers_cancellable_returns_none_when_pre_cancelled() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let cancel = AtomicBool::new(true);

        let result = generate_guide_buffers_cancellable(64, 64, &camera, &planes, &cancel);
        assert!(
            result.is_none(),
            "a generation cancelled before it starts must not produce buffers"
        );
    }
}
