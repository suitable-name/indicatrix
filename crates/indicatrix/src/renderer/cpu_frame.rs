//! Wasm-safe, thread-free "trace these pixels x samples" tracer over
//! [`FrameScene`] -- the shared core both the desktop's threaded
//! `renderer::gpu::hybrid::cpu_trace_range` and a browser's per-Worker CPU tracer call.
//!
//! No threads, no [`std::time::Instant`], no filesystem access: every function here can
//! run inside a wasm32 Web Worker as well as a native thread.
//!
//! # Determinism and partitioning
//!
//! A pixel's sample stream is a pure function of `(pixel, sample_num)` alone (see
//! [`trace_pixels_interleaved`]'s own doc), so ANY partition of a frame's pixels across
//! however many callers -- one thread, one Worker, any `first_pixel`/`stride` choice --
//! produces per-pixel bit-identical summed radiance, as long as every pixel is covered
//! by exactly one partition. [`scatter_interleaved`] adds a partition's results back
//! into a full-frame buffer using the same `first_pixel`/`stride` convention, so summing
//! several partitions' scatters reproduces a single unpartitioned trace exactly.

use glam::Vec3;

use crate::{
    geometry::tool::{StoneGeometry, ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            PixelRotations, add_finite_sample, pixel_rotations, sample_draws,
            trace_spectral_ray_with_finish_soa_geom,
        },
    },
    simd::PlanesSoA32,
};

use super::frame_scene::FrameScene;

/// One `(pixel, sample_num)` sample, traced through the real
/// [`trace_spectral_ray_with_finish_soa`] -- never a reimplementation of the estimator,
/// only of the per-sample seed/jitter construction around it.
///
/// Takes the batch's `plane_soa` arena (built once by the caller) rather than
/// rebuilding it from `scene.planes` on every one of the many calls a batch makes.
///
/// `rot` is `pixel`'s [`PixelRotations`], computed once per pixel by the caller: it is a
/// pure function of `pixel` alone, so passing the precomputed value to
/// [`sample_draws`] yields the same draws as recomputing it for every sample.
fn cpu_sample_xyz(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    fluorescence: &Fluorescence,
    plane_soa: &PlanesSoA32,
    pixel: u32,
    rot: &PixelRotations,
    sample_num: u32,
) -> Vec3 {
    let width = scene.width;
    let x = pixel % width;
    let y = pixel / width;

    let draws = sample_draws(pixel, sample_num, rot);

    let ray = scene.camera.generate_ray(
        x as f32,
        y as f32,
        width as f32,
        scene.height as f32,
        draws.jitter_x,
        draws.jitter_y,
    );
    trace_spectral_ray_with_finish_soa_geom(
        ray,
        StoneGeometry {
            planes: scene.planes,
            tools,
        },
        plane_soa,
        scene.facet_finishes,
        scene.material,
        fluorescence,
        scene.max_bounces,
        scene.environment,
        draws.seed,
        draws.hero_rand,
        None,
    )
}

/// Traces one interleaved partition of the frame's pixels and returns one SUMMED
/// [`Vec3`] per owned pixel.
///
/// Sample indices `[sample_offset, sample_offset + spp)` are traced for pixels
/// `first_pixel, first_pixel + stride, first_pixel + 2 * stride, …` (stopping once a
/// pixel index would reach `scene.width * scene.height`), in that same ascending
/// order. A non-finite sample is dropped but still counted, like the GPU reduction it
/// merges with (see [`add_finite_sample`]).
///
/// Because a pixel's sample stream depends only on `(pixel, sample_num)` -- never on
/// `first_pixel`, `stride`, or which caller happens to own that pixel -- any partition
/// of a frame's pixels into disjoint interleaved sets, traced independently and
/// recombined with [`scatter_interleaved`], reproduces the exact same per-pixel sums a
/// single unpartitioned call over the whole frame would produce. That is what lets a
/// desktop thread pool and a browser's Web Worker pool call this same function with
/// different `first_pixel`/`stride` splits and still agree bit-for-bit with each other
/// and with a single-threaded reference trace.
#[must_use]
pub fn trace_pixels_interleaved(
    scene: &FrameScene<'_>,
    plane_soa: &PlanesSoA32,
    first_pixel: u32,
    stride: u32,
    sample_offset: u32,
    spp: u32,
) -> Vec<Vec3> {
    trace_pixels_interleaved_geom(
        scene,
        &[],
        Fluorescence::none(),
        plane_soa,
        first_pixel,
        stride,
        sample_offset,
        spp,
    )
}

/// [`trace_pixels_interleaved`] for a stone with tools.
///
/// `scene.planes` stay the polyhedron and `tools` are subtracted from it; `plane_soa` is
/// the arena built from `scene.planes` alone. `FrameScene` carries no tools field of its
/// own because it is constructed by struct literal in the desktop, worker and web
/// crates, so the tools ride beside it here instead; the `fluorescence` of the material
/// rides beside the tools the same way. With empty `tools` and an empty `fluorescence`
/// the sums are [`trace_pixels_interleaved`]'s, bit for bit. A tool-bearing or
/// fluorescent frame never reaches the GPU (see `gpu_backend::scene_routes_to_gpu`), so
/// this CPU path is the one that traces it.
#[expect(
    clippy::too_many_arguments,
    reason = "the tools and the fluorescence ride beside the scene, as separate inputs, so \
              no caller has to build a bundle struct"
)]
#[must_use]
pub fn trace_pixels_interleaved_geom(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    fluorescence: &Fluorescence,
    plane_soa: &PlanesSoA32,
    first_pixel: u32,
    stride: u32,
    sample_offset: u32,
    spp: u32,
) -> Vec<Vec3> {
    let num_pixels = scene.width as usize * scene.height as usize;
    if stride == 0 || num_pixels == 0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut pixel = first_pixel;
    while (pixel as usize) < num_pixels {
        let mut sum = Vec3::ZERO;
        // Per pixel, not per sample: the rotations depend on `pixel` alone (see
        // `PixelRotations`), so hoisting them leaves every sample's draws unchanged.
        let rot = pixel_rotations(pixel);
        for local_sample in 0..spp {
            let sample_num = sample_offset + local_sample;
            add_finite_sample(
                &mut sum,
                cpu_sample_xyz(
                    scene,
                    tools,
                    fluorescence,
                    plane_soa,
                    pixel,
                    &rot,
                    sample_num,
                ),
            );
        }
        out.push(sum);
        pixel += stride;
    }
    out
}

/// Adds one partition's `sums` back into the full-frame buffer `dst`.
///
/// `sums` must be in the same `first_pixel, first_pixel + stride, …` order
/// [`trace_pixels_interleaved`] produced them in -- this scatters them into `dst` at
/// those same pixel indices. ADDS rather than overwrites, matching every other
/// accumulation buffer in this crate (see `renderer::gpu::hybrid`'s "Merge convention"
/// doc): summing several partitions' scatters into the same zero-initialised buffer
/// reproduces a single unpartitioned trace's sums exactly.
///
/// # Panics
///
/// Panics if `sums` has more entries than `dst` has pixels starting at `first_pixel`
/// and stepping by `stride` -- i.e. if it was not produced by a
/// [`trace_pixels_interleaved`] call against a `dst`-shaped frame with the same
/// `first_pixel`/`stride`.
pub fn scatter_interleaved(dst: &mut [Vec3], first_pixel: u32, stride: u32, sums: &[Vec3]) {
    let mut pixel = first_pixel as usize;
    for &sum in sums {
        dst[pixel] += sum;
        pixel += stride as usize;
    }
}
