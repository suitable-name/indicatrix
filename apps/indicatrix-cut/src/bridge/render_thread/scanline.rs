//! The CPU scanline tracer: renders one full frame in parallel across
//! `thread::available_parallelism` CPU threads.

use super::gpu_backend::{BackendFrame, FrameOutputs};
use glam::Vec3;
use indicatrix::optics::raytracer::{
    HitRecord, build_plane_soa, pixel_rotations, sample_draws, trace_spectral_ray_with_finish_soa,
};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

/// One row's worth of the four output buffers, handed out to whichever thread's atomic
/// counter claims that row -- see [`render_frame_scanlines`].
struct RowSlices<'a> {
    acc: &'a mut [Vec3],
    depth: &'a mut [f32],
    normal: &'a mut [Vec3],
    facet: &'a mut [i32],
}

/// Renders one full frame's scanlines in parallel across `thread::available_parallelism`
/// CPU threads, accumulating each pixel's radiance into `accum_buffer` and each pixel's
/// PRIMARY-ray first-hit depth/normal/facet-index into the three `first_hit_*` guide
/// buffers (these feed the À-Trous denoiser). Per-pixel tone-mapping is not done here --
/// that's `denoise_and_tonemap_frame`'s job, run once over the whole frame afterward
/// since the denoiser needs every pixel's guide data at once.
///
/// # Work distribution: a shared atomic row counter, not contiguous row bands
///
/// Rows through the stone cost far more than background rows, so splitting the image
/// into `num_threads` contiguous row bands starves threads that land entirely on cheap
/// background rows while stone-covering threads are still working -- measured on a
/// 480x360/2spp/16-thread render: 256ms wall time at 49% thread utilization for
/// contiguous bands, 145ms at 99% for the dynamic counter below. Each thread instead
/// repeatedly claims "the next unclaimed row" via `next_row.fetch_add(1, ..)`, so a
/// thread finishing a cheap row picks up the next one immediately rather than idling.
///
/// Each row's four output slices are handed out exactly once: `rows` pre-splits every
/// buffer into per-row slices via `chunks_mut(width)`, wrapped in a [`Mutex`]. A thread
/// claiming row `y` locks `rows` just long enough to `Option::take` that row's slices
/// (held per ROW, not per pixel), then processes the whole row without the lock held.
/// `next_row.fetch_add` hands out each index exactly once, so results stay
/// bit-identical to a contiguous-band split -- only which thread renders which
/// row changes run to run.
pub(super) fn render_frame_scanlines(
    frame: &BackendFrame<'_>,
    spp: u32,
    current_sample_count: u32,
    outputs: &mut FrameOutputs<'_>,
) {
    let width = frame.width;
    let height = frame.height;
    let num_threads = thread::available_parallelism().map_or(8, std::num::NonZero::get);
    let width_usize = width as usize;

    // Reborrowed as four disjoint fields so the simultaneous `zip` below borrows each
    // independently.
    let FrameOutputs {
        accum,
        depth,
        normal,
        facet_id,
    } = outputs;
    let rows: Vec<Option<RowSlices<'_>>> = accum
        .chunks_mut(width_usize)
        .zip(depth.chunks_mut(width_usize))
        .zip(normal.chunks_mut(width_usize))
        .zip(facet_id.chunks_mut(width_usize))
        .map(|(((acc, depth), normal), facet)| {
            Some(RowSlices {
                acc,
                depth,
                normal,
                facet,
            })
        })
        .collect();
    let rows = Mutex::new(rows);
    let next_row = AtomicUsize::new(0);

    // Built ONCE per call rather than once per traced sample: the non-SoA
    // `trace_spectral_ray_with_finish` rebuilds this SIMD arena from `frame.planes` on
    // every invocation -- a heap allocation per (pixel, sample) tuple. `build_plane_soa`/
    // `trace_spectral_ray_with_finish_soa` are the caller-builds-the-arena-once pair
    // documented on `trace_spectral_ray_with_finish_soa` for exactly this case; results
    // stay bit-identical since the arena's contents depend only on `frame.planes`, which
    // is fixed for the whole frame.
    let plane_soa = build_plane_soa(frame.planes);

    thread::scope(|s| {
        for _ in 0..num_threads {
            let rows = &rows;
            let next_row = &next_row;
            let plane_soa = &plane_soa;

            s.spawn(move || {
                loop {
                    let y = next_row.fetch_add(1, Ordering::Relaxed);
                    if y >= height as usize {
                        break;
                    }

                    let RowSlices {
                        acc: acc_row,
                        depth: depth_row,
                        normal: normal_row,
                        facet: facet_row,
                    } = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("each row index is claimed by exactly one thread via fetch_add");

                    for x in 0..width_usize {
                        let global_pixel_idx = (y * width_usize + x) as u32;
                        let mut sample_sum = Vec3::ZERO;
                        let mut primary_hit: Option<HitRecord> = None;

                        // Per-pixel Cranley-Patterson rotations for the stratified
                        // pixel-jitter/hero-wavelength draws below -- pure functions of
                        // `global_pixel_idx` alone. Must stay in sync with
                        // `indicatrix-worker/src/render_core.rs::trace_into`, which
                        // computes through the same shared functions.
                        let rot = pixel_rotations(global_pixel_idx);

                        for s_idx in 0..spp {
                            let sample_num = current_sample_count - spp + s_idx;
                            let draws = sample_draws(global_pixel_idx, sample_num, &rot);

                            let ray = frame.camera.generate_ray(
                                x as f32,
                                y as f32,
                                width as f32,
                                height as f32,
                                draws.jitter_x,
                                draws.jitter_y,
                            );

                            // Frosted girdle: `facet_finishes` is `&[]` when
                            // `RenderContext::girdle_frosted` is off, equivalent to
                            // `trace_spectral_ray` (every facet looks up
                            // `FacetFinish::default() == Polished`).
                            let sample_xyz = trace_spectral_ray_with_finish_soa(
                                ray,
                                frame.planes,
                                plane_soa,
                                frame.facet_finishes,
                                frame.material,
                                frame.max_bounces,
                                frame.environment,
                                draws.seed,
                                draws.hero_rand,
                                Some(&mut primary_hit),
                            );

                            // Defensive: no reachable NaN/Inf producer is
                            // known in the CPU path today, but this accumulator adds
                            // unconditionally into a buffer that never resets except on
                            // a full scene change -- one non-finite sample would poison
                            // every pixel's running average for the rest of the session
                            // rather than just corrupting the one frame it came from.
                            if sample_xyz.is_finite() {
                                sample_sum += sample_xyz;
                            }
                        }

                        acc_row[x] += sample_sum;
                        // `primary_hit` is captured from the LAST sample traced this
                        // call (the hero channel's own first hit). AA jitter means
                        // consecutive samples can land on different facets at a
                        // silhouette edge; the denoiser's facet-identity guide term is
                        // a hard Kronecker delta, so a stale-by-one-sample id there
                        // costs at most a conservative edge-stop, never a wrong blend.
                        depth_row[x] = primary_hit.map_or(1.0e6, |h| h.t);
                        normal_row[x] = primary_hit.map_or(Vec3::ZERO, |h| h.normal);
                        facet_row[x] = primary_hit.map_or(-1, |h| h.facet_idx as i32);
                    }
                }
            });
        }
    });
}
