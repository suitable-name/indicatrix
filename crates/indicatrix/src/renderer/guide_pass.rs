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
//! [`intersect_polyhedron`] call `trace_spectral_ray` makes at bounce 0, without paying
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
//! CPU only: no GPU program is involved, so this never takes a `GpuBackend` turn.
//!
//! Moved here unchanged from `apps/indicatrix-cut`'s `bridge::frame_cache::guide_pass`
//! (which re-exports it) so a server can reproduce the viewer's denoised picture;
//! `renderer::frame_denoise`'s pin test covers this prepass together with the denoise.

use crate::{
    geometry::plane::GpuFacetPlane,
    optics::raytracer::{Camera, intersect_polyhedron},
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

/// The cancellable core [`generate_guide_buffers`] wraps: casts one un-jittered camera
/// ray per pixel and records its first hit. Parallel across
/// `thread::available_parallelism`, chunked by row.
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
    if width == 0 || height == 0 {
        return Some(GuideBuffers::miss(width, height));
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let mut buffers = GuideBuffers::miss(width, height);
    let num_threads = thread::available_parallelism().map_or(8, std::num::NonZero::get);
    let rows_per_chunk = (height as usize).div_ceil(num_threads);

    thread::scope(|s| {
        let chunks_depth: Vec<&mut [f32]> = buffers
            .depth
            .chunks_mut(rows_per_chunk * width as usize)
            .collect();
        let chunks_normal: Vec<&mut [Vec3]> = buffers
            .normal
            .chunks_mut(rows_per_chunk * width as usize)
            .collect();
        let chunks_facet: Vec<&mut [i32]> = buffers
            .facet_id
            .chunks_mut(rows_per_chunk * width as usize)
            .collect();

        let chunks = chunks_depth
            .into_iter()
            .zip(chunks_normal)
            .zip(chunks_facet);

        for (chunk_idx, ((depth_chunk, normal_chunk), facet_chunk)) in chunks.enumerate() {
            let start_y = chunk_idx * rows_per_chunk;
            let end_y = (start_y + rows_per_chunk).min(height as usize);

            s.spawn(move || {
                for y in start_y..end_y {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let local_y = y - start_y;
                    let row_offset = local_y * width as usize;

                    for x in 0..(width as usize) {
                        let local_idx = row_offset + x;
                        // No jitter: this is a single deterministic prepass, not an
                        // accumulated sample -- there is nothing to anti-alias against.
                        let ray = camera.generate_ray(
                            x as f32,
                            y as f32,
                            width as f32,
                            height as f32,
                            0.0,
                            0.0,
                        );
                        let hit = intersect_polyhedron(ray, planes);

                        depth_chunk[local_idx] = hit.map_or(1.0e6, |h| h.t);
                        normal_chunk[local_idx] = hit.map_or(Vec3::ZERO, |h| h.normal);
                        facet_chunk[local_idx] = hit.map_or(-1, |h| h.facet_idx as i32);
                    }
                }
            });
        }
    });

    if cancel.load(Ordering::Relaxed) {
        None
    } else {
        Some(buffers)
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
