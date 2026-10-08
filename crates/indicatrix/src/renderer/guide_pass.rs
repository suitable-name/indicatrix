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
//! # Path signature
//!
//! With a refractive index `n_d > 1` the prepass also follows the un-jittered centre ray
//! through the stone at that one index (ordinary ray, Snell refraction, total internal
//! reflection, at most [`SIGNATURE_BOUNCES`] interior hits) and hashes the facets it meets
//! into [`GuideBuffers::path_sig`]. The reflection pattern seen inside one crown facet is
//! the map of those interior facet regions, so the denoiser uses the signature as a second
//! hard edge-stop (`GBuffers::path_sig`). `n_d <= 1` (the old entry points pass `0.0`)
//! means "no signature": the buffer is all zeros. The walk uses plain `f32` operations,
//! never `mul_add`, so `wasm32` and x86 hash identical signatures.
//!
//! CPU only: no GPU program is involved, so this never takes a `GpuBackend` turn.
//!
//! Moved here unchanged from `apps/indicatrix-cut`'s `bridge::frame_cache::guide_pass`
//! (which re-exports it) so a server can reproduce the viewer's denoised picture;
//! `renderer::frame_denoise`'s pin test covers this prepass together with the denoise.

use crate::{
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::raytracer::{
        Camera, HitRecord, Ray, build_plane_soa, intersect_stone::intersect_stone_soa,
    },
    simd::PlanesSoA32,
};
use glam::Vec3;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

/// One pixel-per-camera-ray depth/normal/facet-id capture.
///
/// Row-major (`index = y * width + x`) -- exactly the shape [`crate::renderer::denoise::GBuffers`]
/// expects for its `depth`/`normal`/`facet_id` fields.
#[derive(Debug, Clone)]
pub struct GuideBuffers {
    /// First-hit ray parameter `t` per pixel; `1.0e6` for a miss.
    pub depth: Vec<f32>,
    /// First-hit facet normal per pixel; [`Vec3::ZERO`] for a miss.
    pub normal: Vec<Vec3>,
    /// First-hit facet index per pixel; `-1` for a miss.
    pub facet_id: Vec<i32>,
    /// Hash of the facets the centre ray meets inside the stone (see the module docs'
    /// "Path signature"); `0` for a miss and for every pixel when no index was given.
    pub path_sig: Vec<u32>,
}

impl GuideBuffers {
    /// An all-miss buffer of the given size: `depth = 1.0e6` (matching the GUI render
    /// loop's own "no hit yet" sentinel), `normal = ZERO`, `facet_id = -1`, `path_sig = 0`.
    #[must_use]
    pub fn miss(width: u32, height: u32) -> Self {
        let pixel_count = (width as usize) * (height as usize);
        Self {
            depth: vec![1.0e6; pixel_count],
            normal: vec![Vec3::ZERO; pixel_count],
            facet_id: vec![-1; pixel_count],
            path_sig: vec![0; pixel_count],
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
    generate_guide_buffers_with_index(width, height, camera, planes, 0.0)
}

/// [`generate_guide_buffers`] that also fills [`GuideBuffers::path_sig`] for the
/// reference refractive index `n_d` (`DispersionModel::n_d`). `n_d <= 1.0` (or NaN) leaves
/// the signature all zeros, which is exactly what [`generate_guide_buffers`] returns.
///
/// # Panics
///
/// Never in practice, for the reason [`generate_guide_buffers`] gives.
#[must_use]
pub fn generate_guide_buffers_with_index(
    width: u32,
    height: u32,
    camera: &Camera,
    planes: &[GpuFacetPlane],
    n_d: f32,
) -> GuideBuffers {
    generate_guide_buffers_cancellable_with_index(
        width,
        height,
        camera,
        planes,
        n_d,
        &AtomicBool::new(false),
    )
    .expect("a cancel flag that is never set to true never yields a cancelled result")
}

/// [`generate_guide_buffers`] for a stone with tools.
///
/// A tool hit records a facet id of `planes.len() + k`, which fits the `i32` id buffer
/// and the guide's equality-based edge test unchanged. With no tools the output is
/// [`generate_guide_buffers`]'s, bit for bit.
///
/// # Panics
///
/// Never in practice, for the reason [`generate_guide_buffers`] gives.
#[must_use]
pub fn generate_guide_buffers_geom(
    width: u32,
    height: u32,
    camera: &Camera,
    geom: StoneGeometry<'_>,
) -> GuideBuffers {
    generate_guide_buffers_geom_with_index(width, height, camera, geom, 0.0)
}

/// [`generate_guide_buffers_geom`] with the path signature of
/// [`generate_guide_buffers_with_index`].
///
/// # Panics
///
/// Never in practice, for the reason [`generate_guide_buffers`] gives.
#[must_use]
pub fn generate_guide_buffers_geom_with_index(
    width: u32,
    height: u32,
    camera: &Camera,
    geom: StoneGeometry<'_>,
    n_d: f32,
) -> GuideBuffers {
    generate_guide_buffers_cancellable_geom_with_index(
        width,
        height,
        camera,
        geom,
        n_d,
        &AtomicBool::new(false),
    )
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
    generate_guide_buffers_cancellable_with_index(width, height, camera, planes, 0.0, cancel)
}

/// [`generate_guide_buffers_cancellable`] with the path signature of
/// [`generate_guide_buffers_with_index`].
#[must_use]
pub fn generate_guide_buffers_cancellable_with_index(
    width: u32,
    height: u32,
    camera: &Camera,
    planes: &[GpuFacetPlane],
    n_d: f32,
    cancel: &AtomicBool,
) -> Option<GuideBuffers> {
    generate_guide_buffers_cancellable_geom_with_index(
        width,
        height,
        camera,
        StoneGeometry::planes_only(planes),
        n_d,
        cancel,
    )
}

/// [`generate_guide_buffers_cancellable`] for a stone with tools.
#[must_use]
pub fn generate_guide_buffers_cancellable_geom(
    width: u32,
    height: u32,
    camera: &Camera,
    geom: StoneGeometry<'_>,
    cancel: &AtomicBool,
) -> Option<GuideBuffers> {
    generate_guide_buffers_cancellable_geom_with_index(width, height, camera, geom, 0.0, cancel)
}

/// [`generate_guide_buffers_cancellable_geom`] with the path signature of
/// [`generate_guide_buffers_with_index`].
#[must_use]
pub fn generate_guide_buffers_cancellable_geom_with_index(
    width: u32,
    height: u32,
    camera: &Camera,
    geom: StoneGeometry<'_>,
    n_d: f32,
    cancel: &AtomicBool,
) -> Option<GuideBuffers> {
    let job = GuideJob {
        width,
        height,
        camera,
        planes: geom.planes,
        tools: geom.tools,
        n_d,
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
    generate_guide_buffers_into_with_index(width, height, camera, planes, 0.0, cancel, out)
}

/// [`generate_guide_buffers_into`] with the path signature of
/// [`generate_guide_buffers_with_index`].
#[must_use]
pub fn generate_guide_buffers_into_with_index(
    width: u32,
    height: u32,
    camera: &Camera,
    planes: &[GpuFacetPlane],
    n_d: f32,
    cancel: &AtomicBool,
    out: &mut GuideBuffers,
) -> bool {
    generate_guide_buffers_into_geom_with_index(
        width,
        height,
        camera,
        StoneGeometry::planes_only(planes),
        n_d,
        cancel,
        out,
    )
}

/// [`generate_guide_buffers_into`] for a stone with tools.
#[must_use]
pub fn generate_guide_buffers_into_geom(
    width: u32,
    height: u32,
    camera: &Camera,
    geom: StoneGeometry<'_>,
    cancel: &AtomicBool,
    out: &mut GuideBuffers,
) -> bool {
    generate_guide_buffers_into_geom_with_index(width, height, camera, geom, 0.0, cancel, out)
}

/// [`generate_guide_buffers_into_geom`] with the path signature of
/// [`generate_guide_buffers_with_index`].
#[must_use]
pub fn generate_guide_buffers_into_geom_with_index(
    width: u32,
    height: u32,
    camera: &Camera,
    geom: StoneGeometry<'_>,
    n_d: f32,
    cancel: &AtomicBool,
    out: &mut GuideBuffers,
) -> bool {
    let job = GuideJob {
        width,
        height,
        camera,
        planes: geom.planes,
        tools: geom.tools,
        n_d,
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
    /// Tools subtracted from the polyhedron `planes` define; empty for a planar stone.
    tools: &'a [ToolPrimitive],
    /// Reference refractive index of the path signature; `<= 1.0` disables it.
    n_d: f32,
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
        path_sig: Vec::new(),
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
        out.path_sig.clear();
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
    out.path_sig.resize(pixel_count, 0);

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
            &mut out.path_sig,
        );
    } else {
        let chunk_len = rows_per_chunk * width as usize;
        thread::scope(|s| {
            let chunks = out
                .depth
                .chunks_mut(chunk_len)
                .zip(out.normal.chunks_mut(chunk_len))
                .zip(out.facet_id.chunks_mut(chunk_len))
                .zip(out.path_sig.chunks_mut(chunk_len));
            for (chunk_idx, (((depth_chunk, normal_chunk), facet_chunk), sig_chunk)) in
                chunks.enumerate()
            {
                let start_y = chunk_idx * rows_per_chunk;
                s.spawn(move || {
                    fill_rows(
                        job,
                        soa,
                        start_y,
                        depth_chunk,
                        normal_chunk,
                        facet_chunk,
                        sig_chunk,
                    );
                });
            }
        });
    }

    !cancel.load(Ordering::Relaxed)
}

/// Interior hits the path signature follows after the entry refraction.
pub const SIGNATURE_BOUNCES: usize = 3;
/// Set on the facet word of the interior hit the ray leaves the stone through.
const SIG_EXIT_BIT: u32 = 0x8000;
/// Facet word of a ray that leaves the stone without meeting another facet.
const SIG_ESCAPE: u32 = 0xFFFF;
/// Offset along the new direction before the next interior intersection: the tracer's own
/// self-hit epsilon (`transport::bounce`).
const SIG_SELF_HIT_EPS: f32 = 1.0e-4;

/// One FNV-1a round over the four little-endian bytes of `word`.
#[inline]
const fn fnv_mix(mut hash: u32, word: u32) -> u32 {
    let mut shift = 0;
    while shift < 32 {
        hash ^= (word >> shift) & 0xFF;
        hash = hash.wrapping_mul(0x0100_0193);
        shift += 8;
    }
    hash
}

/// Plain `x*x + y*y + z*z` dot product (no fused multiply-add).
#[inline]
#[allow(clippy::suboptimal_flops)] // FMA would break wasm/x86 signature parity
fn dot3(a: Vec3, b: Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

/// Normalises `v` with plain operations, or returns it unchanged when it has no length.
#[inline]
fn unit(v: Vec3) -> Vec3 {
    let len = dot3(v, v).sqrt();
    if len > 0.0 { v / len } else { v }
}

/// The path signature of the centre ray whose first hit is `first`, or `0` when `n_d <= 1`.
///
/// Refracts into the stone at index `n_d` (Snell, ordinary ray), then follows up to
/// [`SIGNATURE_BOUNCES`] interior hits: total internal reflection continues the walk, the
/// first hit that transmits ends it with [`SIG_EXIT_BIT`] set, a ray that finds no further
/// facet ends it with [`SIG_ESCAPE`]. The facet words are folded with FNV-1a, so two pixels
/// share a signature exactly when their rays met the same facets in the same order. Plain
/// `f32` operations only (see the module docs).
#[allow(clippy::suboptimal_flops)] // FMA would break wasm/x86 signature parity
fn path_signature(
    ray: Ray,
    first: &HitRecord,
    soa: &PlanesSoA32,
    tools: &[ToolPrimitive],
    n_d: f32,
) -> u32 {
    if n_d.is_nan() || n_d <= 1.0 {
        return 0;
    }
    let mut hash = fnv_mix(0x811c_9dc5, first.facet_idx as u32);

    // Entry refraction, air -> stone.
    let incident = unit(ray.dir);
    let cos_i = (-dot3(incident, first.normal)).max(0.0);
    let eta = 1.0 / n_d;
    let k = 1.0 - eta * eta * (1.0 - cos_i * cos_i);
    if k <= 0.0 {
        return fnv_mix(hash, SIG_ESCAPE).max(1);
    }
    let mut dir = unit(incident * eta + first.normal * (eta * cos_i - k.sqrt()));
    let mut origin = ray.origin + ray.dir * first.t;

    for _ in 0..SIGNATURE_BOUNCES {
        let probe = Ray {
            origin: origin + dir * SIG_SELF_HIT_EPS,
            dir,
        };
        let Some(hit) = intersect_stone_soa(probe, soa, soa.len(), tools) else {
            hash = fnv_mix(hash, SIG_ESCAPE);
            break;
        };
        origin = probe.origin + dir * hit.t;
        let facet = (hit.facet_idx as u32) & 0x7FFF;
        let cos_t = dot3(dir, hit.normal);
        let sin_sq = n_d * n_d * (1.0 - cos_t * cos_t);
        if sin_sq > 1.0 {
            // Total internal reflection: the walk continues along the mirrored direction.
            hash = fnv_mix(hash, facet);
            dir = unit(dir - hit.normal * (2.0 * cos_t));
        } else {
            hash = fnv_mix(hash, facet | SIG_EXIT_BIT);
            break;
        }
    }
    // `0` is reserved for "no signature".
    hash.max(1)
}

/// Fills the whole rows `depth`/`normal`/`facet_id`/`path_sig` hold (each a multiple of
/// `job.width` long), the first of which is image row `start_y`. Stops early once
/// `job.cancel` is set, checked once per row. `soa` is the arena built from `job.planes`.
fn fill_rows(
    job: GuideJob<'_>,
    soa: &PlanesSoA32,
    start_y: usize,
    depth: &mut [f32],
    normal: &mut [Vec3],
    facet_id: &mut [i32],
    path_sig: &mut [u32],
) {
    let width = job.width as usize;
    let rows = depth
        .chunks_mut(width)
        .zip(normal.chunks_mut(width))
        .zip(facet_id.chunks_mut(width))
        .zip(path_sig.chunks_mut(width));
    for (local_y, (((depth_row, normal_row), facet_row), sig_row)) in rows.enumerate() {
        if job.cancel.load(Ordering::Relaxed) {
            return;
        }
        let y = start_y + local_y;
        let cells = depth_row
            .iter_mut()
            .zip(normal_row.iter_mut())
            .zip(facet_row.iter_mut())
            .zip(sig_row.iter_mut());
        for (x, (((depth_out, normal_out), facet_out), sig_out)) in cells.enumerate() {
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
            let hit = intersect_stone_soa(ray, soa, soa.len(), job.tools);

            *depth_out = hit.map_or(1.0e6, |h| h.t);
            *normal_out = hit.map_or(Vec3::ZERO, |h| h.normal);
            *facet_out = hit.map_or(-1, |h| h.facet_idx as i32);
            *sig_out = hit.map_or(0, |h| path_signature(ray, &h, soa, job.tools, job.n_d));
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
            tools: &[],
            n_d: 0.0,
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

    #[test]
    fn guide_buffers_with_no_tools_match_the_plane_only_entry_point_bit_for_bit() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let old = generate_guide_buffers(16, 12, &camera, &planes);
        let new = generate_guide_buffers_geom(16, 12, &camera, StoneGeometry::planes_only(&planes));
        assert_eq!(old.facet_id, new.facet_id);
        let bits = |b: &GuideBuffers| -> Vec<u32> { b.depth.iter().map(|d| d.to_bits()).collect() };
        assert_eq!(bits(&old), bits(&new));
    }

    /// No index (the old entry points) means no signature at all, and the other three
    /// buffers are the indexed run's, bit for bit.
    #[test]
    fn without_an_index_the_signature_is_all_zeros_and_other_buffers_are_unchanged() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let plain = generate_guide_buffers(24, 16, &camera, &planes);
        assert_eq!(plain.path_sig, vec![0; 24 * 16]);
        let zero = generate_guide_buffers_with_index(24, 16, &camera, &planes, 0.0);
        assert_eq!(zero.path_sig, plain.path_sig);
        let indexed = generate_guide_buffers_with_index(24, 16, &camera, &planes, 2.417);
        assert_eq!(indexed.facet_id, plain.facet_id);
        assert_eq!(indexed.normal, plain.normal);
        for (a, b) in indexed.depth.iter().zip(&plain.depth) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    /// A miss pixel carries signature 0 and every gem pixel a non-zero one.
    #[test]
    fn signature_is_zero_exactly_on_misses() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let guides = generate_guide_buffers_with_index(48, 32, &camera, &planes, 2.417);
        assert!(guides.facet_id.contains(&-1), "fixture needs a miss pixel");
        assert!(
            guides.facet_id.iter().any(|&id| id >= 0),
            "fixture needs a hit pixel"
        );
        for (i, &id) in guides.facet_id.iter().enumerate() {
            assert_eq!(guides.path_sig[i] == 0, id < 0, "pixel {i}, facet {id}");
        }
    }

    /// The signature, like the other buffers, does not depend on the thread count.
    #[test]
    fn signature_is_identical_for_one_two_and_eight_threads() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let cancel = AtomicBool::new(false);
        let job = GuideJob {
            width: 61,
            height: 37,
            camera: &camera,
            planes: &planes,
            tools: &[],
            n_d: 2.417,
            cancel: &cancel,
        };
        let single = generate_with_threads(job, 1).expect("never cancelled");
        let distinct: std::collections::HashSet<u32> = single.path_sig.iter().copied().collect();
        assert!(
            distinct.len() > 4,
            "a brilliant seen at this pose must show several interior regions"
        );
        for threads in [2, 8] {
            let multi = generate_with_threads(job, threads).expect("never cancelled");
            assert_eq!(multi.path_sig, single.path_sig, "{threads} threads");
        }
    }

    /// The refraction depends on the index, so the regions move when it does.
    #[test]
    fn signature_regions_change_with_the_index() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let diamond = generate_guide_buffers_with_index(64, 48, &camera, &planes, 2.417);
        let glass = generate_guide_buffers_with_index(64, 48, &camera, &planes, 1.52);
        assert_ne!(diamond.path_sig, glass.path_sig);
        assert_eq!(diamond.facet_id, glass.facet_id);
    }

    /// Reusing a buffer yields the fresh signature, including after a size change.
    #[test]
    fn signature_survives_buffer_reuse() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let cancel = AtomicBool::new(false);
        let old_camera = Camera::new(1.10, 0.20, 2.4, 42.0);
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let mut reused = generate_guide_buffers_with_index(24, 16, &old_camera, &planes, 2.417);
        assert!(generate_guide_buffers_into_with_index(
            16,
            12,
            &camera,
            &planes,
            2.417,
            &cancel,
            &mut reused
        ));
        let fresh = generate_guide_buffers_with_index(16, 12, &camera, &planes, 2.417);
        assert_eq!(reused.path_sig, fresh.path_sig);
    }

    #[test]
    fn guide_buffers_encode_tool_facet_ids_above_the_plane_count() {
        let planes = StandardGemCuts::standard_round_brilliant();
        // A groove across the table, seen from almost straight above.
        let tools = [ToolPrimitive::cylinder(
            Vec3::new(0.0, 0.44, 0.0),
            Vec3::X,
            0.3,
            2.0,
        )];
        let camera = Camera::new(0.0, 1.5, 4.0, 20.0);
        let guides = generate_guide_buffers_geom(
            24,
            24,
            &camera,
            StoneGeometry {
                planes: &planes,
                tools: &tools,
            },
        );
        let first_tool_id = planes.len() as i32;
        assert!(
            guides.facet_id.contains(&first_tool_id),
            "the groove floor must appear as tool facet {first_tool_id}"
        );
        assert!(guides.facet_id.iter().all(|&id| id <= first_tool_id));
    }
}
