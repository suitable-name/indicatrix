//! CPU+GPU hybrid frame rendering: the GPU and every CPU core trace disjoint sample
//! ranges of the SAME frame at the same time, and their per-pixel radiance sums are
//! merged into one image.
//!
//! Ports no physics of its own. The GPU side is exactly
//! [`GpuFrameRenderer::accumulate`]; the CPU side uses the same seed hash, stratified
//! jitter, Cranley-Patterson rotation, and hero-wavelength draw as `estimator_check`'s
//! CPU reference loop, via `optics::raytracer::sampling::{pixel_rotations,
//! sample_draws}` -- the one place that formula lives (`estimator_check` still carries
//! an independent copy, so a formula change must land in both places).
//!
//! The split is disjoint sample RANGES, not disjoint pixels:
//! `GpuFrameRenderer::accumulate`'s `sample_offset` lets workers own disjoint ranges of
//! one frame (the shader derives seed/jitter from the absolute sample index). GPU
//! renders `[0, gpu_samples)`, CPU renders `[gpu_samples, gpu_samples + cpu_samples)`,
//! concurrently.
//!
//! # Merge convention: sum, never average
//!
//! Every accumulation buffer in this crate is a running SUM of per-sample radiance,
//! divided by sample count only once, by the caller, at display time. [`render_hybrid`]
//! adds the GPU and CPU per-pixel sums together; the caller divides by
//! `split.total_spp()` as for a single-engine render.
//!
//! # Biaxial routing
//!
//! [`render_hybrid`] overrides any material with `GemMaterial::gpu_supported() ==
//! false` to an all-CPU split (`gpu_samples = 0`, reported via
//! [`HybridStats::forced_cpu_only`]). Unreachable today since `gpu_supported()` is
//! unconditionally `true` -- kept as a real predicate for any future regression.
//!
//! # Determinism
//!
//! Two [`render_hybrid`] calls with the same explicit [`HybridSplit`] against the same
//! scene produce byte-identical summed buffers: the GPU side already has this property,
//! the CPU side by construction (seed/jitter are pure functions of pixel and absolute
//! sample index). [`HybridSplit::calibrated`] itself is NOT required to be reproducible
//! (it measures noisy wall-clock throughput) -- only a render given an already-chosen
//! split is.

use std::time::{Duration, Instant};

use glam::Vec3;

use crate::optics::raytracer::{
    build_plane_soa, pixel_rotations, sample_draws, trace_spectral_ray_with_finish_soa,
};

use super::frame::{GpuFrameError, GpuFrameRenderer, GpuFrameScene};

/// A deterministic, explicit static split of one frame's total samples-per-pixel budget
/// between the GPU and the CPU. Total spp is `gpu_samples + cpu_samples`.
///
/// Deliberately NOT chosen inside [`render_hybrid`] itself -- the render always takes
/// an explicit split, even one from [`Self::calibrated`] a moment earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HybridSplit {
    pub gpu_samples: u32,
    pub cpu_samples: u32,
}

impl HybridSplit {
    /// Total samples per pixel this split covers.
    #[must_use]
    pub const fn total_spp(&self) -> u32 {
        self.gpu_samples + self.cpu_samples
    }

    /// A split that sends every sample to the CPU -- what [`render_hybrid`] falls back
    /// to for a material whose `gpu_supported()` is `false`, and a reasonable default
    /// when no GPU adapter exists at all.
    #[must_use]
    pub const fn cpu_only(total_spp: u32) -> Self {
        Self {
            gpu_samples: 0,
            cpu_samples: total_spp,
        }
    }

    /// Measures a small warmup frame's GPU-only and CPU-only throughput (samples/sec, at
    /// `warmup_spp` each) and picks a deterministic `total_spp`-sample split
    /// proportional to the two measured rates.
    ///
    /// The warmup renders are thrown away -- only wall-clock cost is used, purely to
    /// pick a reasonable starting split; [`render_hybrid`]'s determinism doesn't depend
    /// on this measurement being stable across runs.
    ///
    /// A `scene.material` whose `gpu_supported()` is `false` short-circuits to
    /// [`Self::cpu_only`] without touching the GPU. `total_spp == 0` or `warmup_spp == 0`
    /// also short-circuits rather than dividing by a zero-sample warmup.
    ///
    /// # Errors
    ///
    /// Whatever [`GpuFrameRenderer::accumulate`] returns for the GPU warmup dispatch.
    pub fn calibrated(
        renderer: &mut GpuFrameRenderer,
        scene: &GpuFrameScene<'_>,
        total_spp: u32,
        warmup_spp: u32,
    ) -> Result<Self, GpuFrameError> {
        if !scene.material.gpu_supported() {
            return Ok(Self::cpu_only(total_spp));
        }
        if total_spp == 0 || warmup_spp == 0 {
            return Ok(Self {
                gpu_samples: 0,
                cpu_samples: total_spp,
            });
        }

        let num_pixels = scene.width as usize * scene.height as usize;

        // Throwaway dispatch first: a GPU dispatch's first call includes output-buffer
        // allocation and driver warm-up, which would otherwise skew gpu_rate pessimistic.
        let mut gpu_throwaway = vec![Vec3::ZERO; num_pixels];
        renderer.accumulate(scene, 0, warmup_spp, &mut gpu_throwaway)?;

        let mut gpu_warmup = vec![Vec3::ZERO; num_pixels];
        let gpu_start = Instant::now();
        renderer.accumulate(scene, 0, warmup_spp, &mut gpu_warmup)?;
        let gpu_rate = samples_per_sec(num_pixels, warmup_spp, gpu_start.elapsed());

        let cpu_start = Instant::now();
        let _cpu_warmup = cpu_trace_range(scene, 0, warmup_spp);
        let cpu_rate = samples_per_sec(num_pixels, warmup_spp, cpu_start.elapsed());

        Ok(Self::from_rates(total_spp, gpu_rate, cpu_rate))
    }

    /// Deterministic split (round-half-up on the measured share, given fixed rates) --
    /// pulled out of [`Self::calibrated`] so the arithmetic is testable without a GPU.
    fn from_rates(total_spp: u32, gpu_rate: f64, cpu_rate: f64) -> Self {
        let total_rate = gpu_rate + cpu_rate;
        if total_rate <= 0.0 {
            // Neither engine produced a measurable rate -- split down the middle.
            let gpu_samples = total_spp / 2;
            return Self {
                gpu_samples,
                cpu_samples: total_spp - gpu_samples,
            };
        }
        let gpu_share = gpu_rate / total_rate;
        let gpu_samples = f64::from(total_spp)
            .mul_add(gpu_share, 0.5)
            .floor()
            .min(f64::from(total_spp)) as u32;
        Self {
            gpu_samples,
            cpu_samples: total_spp - gpu_samples,
        }
    }
}

/// What one [`render_hybrid`] call measured. Purely informational -- correctness never
/// depends on any field here, only on the merged buffer `render_hybrid` writes into.
#[derive(Debug, Clone, Copy)]
pub struct HybridStats {
    /// Wall-clock time for the whole hybrid render, from just before the GPU dispatch
    /// and CPU tracing both start to just after their results are merged.
    pub wall_time: Duration,
    pub gpu_samples: u32,
    pub cpu_samples: u32,
    /// `0.0` when `gpu_samples == 0` (no dispatch happened, so no rate was measured).
    pub gpu_samples_per_sec: f64,
    /// `0.0` when `cpu_samples == 0`.
    pub cpu_samples_per_sec: f64,
    /// `true` when the requested split's `gpu_samples` was overridden to `0` -- see
    /// "Biaxial routing" in the module doc. Always `false` today since `gpu_supported()`
    /// is unconditional, but stays observable for any future material that returns
    /// `false`.
    pub forced_cpu_only: bool,
}

/// Renders one frame with the GPU and CPU tracing disjoint sample ranges at the same
/// time, then sums their per-pixel radiance into `accum`.
///
/// `split.gpu_samples` samples per pixel run on the GPU (`sample_offset = 0`) while
/// `split.cpu_samples` run on the CPU (`sample_offset = split.gpu_samples`, spread
/// across every available core).
///
/// Concurrency: the GPU dispatch runs on its own thread (`accumulate` needs `&mut
/// renderer` exclusively) while the calling thread and its workers trace the CPU's
/// range -- see [`cpu_trace_range`]. Outputs land in separate buffers, combined only
/// after both finish.
///
/// A biaxial `scene.material` (`gpu_supported() == false`) routes EVERY sample to the
/// CPU regardless of `requested_split` -- see "Biaxial routing" and
/// [`HybridStats::forced_cpu_only`].
///
/// `accum` is ADDED into, like [`GpuFrameRenderer::accumulate`]'s own `accum`.
///
/// # Errors
///
/// Whatever [`GpuFrameRenderer::accumulate`] returns for the GPU dispatch. Never
/// reached when `scene.material` is biaxial.
///
/// # Panics
///
/// Panics if `accum.len()` is not `scene.width * scene.height`, or if the internal GPU
/// dispatch thread itself panics.
pub fn render_hybrid(
    renderer: &mut GpuFrameRenderer,
    scene: &GpuFrameScene<'_>,
    requested_split: HybridSplit,
    accum: &mut [Vec3],
) -> Result<HybridStats, GpuFrameError> {
    let num_pixels = scene.width as usize * scene.height as usize;
    assert_eq!(
        accum.len(),
        num_pixels,
        "accumulation buffer must have one entry per pixel"
    );

    let forced_cpu_only = !scene.material.gpu_supported();
    let split = if forced_cpu_only {
        HybridSplit::cpu_only(requested_split.total_spp())
    } else {
        requested_split
    };

    let wall_start = Instant::now();

    // GPU dispatch runs on its own thread for the whole CPU-tracing duration below --
    // renderer.accumulate blocks on GPU readback, so overlapping it with CPU work is the
    // point of "hybrid".
    let (gpu_result, cpu_buf, cpu_elapsed) = std::thread::scope(|s| {
        // Proving this closure Send walks wgpu's internal buffer/registry nesting --
        // see the crate root's #![recursion_limit = "256"].
        let gpu_handle = s.spawn(move || -> Result<(Vec<Vec3>, Duration), GpuFrameError> {
            let mut buf = vec![Vec3::ZERO; num_pixels];
            if split.gpu_samples == 0 {
                return Ok((buf, Duration::ZERO));
            }
            let start = Instant::now();
            renderer.accumulate(scene, 0, split.gpu_samples, &mut buf)?;
            Ok((buf, start.elapsed()))
        });

        let cpu_start = Instant::now();
        let cpu_buf = if split.cpu_samples == 0 {
            vec![Vec3::ZERO; num_pixels]
        } else {
            cpu_trace_range(scene, split.gpu_samples, split.cpu_samples)
        };
        let cpu_elapsed = cpu_start.elapsed();

        let gpu_result = gpu_handle
            .join()
            .expect("hybrid GPU dispatch thread panicked");
        (gpu_result, cpu_buf, cpu_elapsed)
    });

    let (gpu_buf, gpu_elapsed) = gpu_result?;

    for ((dst, gpu_sample), cpu_sample) in accum.iter_mut().zip(gpu_buf).zip(cpu_buf) {
        *dst += gpu_sample + cpu_sample;
    }

    Ok(HybridStats {
        wall_time: wall_start.elapsed(),
        gpu_samples: split.gpu_samples,
        cpu_samples: split.cpu_samples,
        gpu_samples_per_sec: samples_per_sec(num_pixels, split.gpu_samples, gpu_elapsed),
        cpu_samples_per_sec: samples_per_sec(num_pixels, split.cpu_samples, cpu_elapsed),
        forced_cpu_only,
    })
}

/// Samples/sec for one engine, `0.0` if it traced no samples at all (rather than
/// reporting a meaningless rate for zero work).
fn samples_per_sec(num_pixels: usize, samples: u32, elapsed: Duration) -> f64 {
    if samples == 0 {
        return 0.0;
    }
    let total_samples = num_pixels as f64 * f64::from(samples);
    total_samples / elapsed.as_secs_f64().max(f64::EPSILON)
}

/// The CPU-engine mirror of [`GpuFrameRenderer::accumulate`].
///
/// Traces `spp` samples per pixel starting at sample index `sample_offset`, using the
/// exact per-sample seed/jitter construction the GPU shader draws from for the same
/// indices (see the module doc), and ADDS each pixel's summed radiance into `accum` --
/// same accumulate-not-overwrite convention, same `sample_offset` meaning, as its GPU
/// counterpart.
///
/// [`render_hybrid`]'s own CPU tracing, and its biaxial "route everything to CPU" case,
/// both go through this exact function (via [`cpu_trace_range`]), so an isolated
/// CPU-only measurement built to check against a hybrid render exercises the same code
/// path, not a lookalike that could drift from it.
///
/// # Panics
///
/// Panics if `accum.len()` is not `scene.width * scene.height`.
pub fn cpu_accumulate(scene: &GpuFrameScene<'_>, sample_offset: u32, spp: u32, accum: &mut [Vec3]) {
    let num_pixels = scene.width as usize * scene.height as usize;
    assert_eq!(
        accum.len(),
        num_pixels,
        "accumulation buffer must have one entry per pixel"
    );
    let sums = cpu_trace_range(scene, sample_offset, spp);
    for (dst, src) in accum.iter_mut().zip(sums) {
        *dst += src;
    }
}

// CPU sample construction -- mirrors renderer::gpu::estimator_check's own CPU reference
// loop exactly, both drawn from optics::raytracer::sampling::{pixel_rotations,
// sample_draws} rather than each hand-copying the arithmetic inline.

/// One `(pixel, sample_num)` sample, traced through the real
/// `optics::raytracer::trace_spectral_ray_with_finish_soa` -- never a reimplementation
/// of the estimator, only of the per-sample seed/jitter construction around it.
///
/// Takes the batch's `plane_soa` arena (built once by [`cpu_trace_range`]) rather than
/// rebuilding it from `scene.planes` on every one of the many calls this batch makes.
fn cpu_sample_xyz(
    scene: &GpuFrameScene<'_>,
    plane_soa: &crate::simd::PlanesSoA32,
    pixel: u32,
    sample_num: u32,
) -> Vec3 {
    let width = scene.width;
    let x = pixel % width;
    let y = pixel / width;

    let rot = pixel_rotations(pixel);
    let draws = sample_draws(pixel, sample_num, &rot);

    let ray = scene.camera.generate_ray(
        x as f32,
        y as f32,
        width as f32,
        scene.height as f32,
        draws.jitter_x,
        draws.jitter_y,
    );
    trace_spectral_ray_with_finish_soa(
        ray,
        scene.planes,
        plane_soa,
        scene.facet_finishes,
        scene.material,
        scene.max_bounces,
        scene.environment,
        draws.seed,
        draws.hero_rand,
        None,
    )
}

/// Traces `spp` samples per pixel on the CPU, sample indices `[sample_offset,
/// sample_offset + spp)`, across every available core, and returns one SUMMED `Vec3` per
/// pixel (never an average -- see the module doc's "Merge convention" section).
///
/// Pixels are partitioned across threads INTERLEAVED (thread `t` owns pixels `t`, `t +
/// num_threads`, ...) rather than in contiguous blocks, so a spatially clustered cost
/// difference doesn't pile all its extra work onto one thread.
fn cpu_trace_range(scene: &GpuFrameScene<'_>, sample_offset: u32, spp: u32) -> Vec<Vec3> {
    let num_pixels = scene.width as usize * scene.height as usize;
    let mut out = vec![Vec3::ZERO; num_pixels];
    if spp == 0 || num_pixels == 0 {
        return out;
    }

    let num_threads = std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(num_pixels);

    // Built ONCE for this batch and shared by reference across every worker thread --
    // std::thread::scope guarantees every thread joins before plane_soa goes out of scope.
    let plane_soa = build_plane_soa(scene.planes);

    let partials: Vec<Vec<(usize, Vec3)>> = std::thread::scope(|s| {
        // Collected into a `Vec` deliberately: every thread must be SPAWNED before any
        // is joined, or the trace would silently serialize into spawn, join, spawn, ...
        let handles: Vec<_> = (0..num_threads)
            .map(|thread_idx| {
                let plane_soa = &plane_soa;
                s.spawn(move || {
                    let mut local = Vec::with_capacity(num_pixels / num_threads + 1);
                    let mut pixel = thread_idx;
                    while pixel < num_pixels {
                        let mut sum = Vec3::ZERO;
                        for local_sample in 0..spp {
                            let sample_num = sample_offset + local_sample;
                            sum += cpu_sample_xyz(scene, plane_soa, pixel as u32, sample_num);
                        }
                        local.push((pixel, sum));
                        pixel += num_threads;
                    }
                    local
                })
            })
            .collect();

        let mut partials = Vec::with_capacity(handles.len());
        for handle in handles {
            partials.push(
                handle
                    .join()
                    .expect("hybrid CPU trace worker thread panicked"),
            );
        }
        partials
    });

    for part in partials {
        for (pixel, sum) in part {
            out[pixel] = sum;
        }
    }
    out
}
