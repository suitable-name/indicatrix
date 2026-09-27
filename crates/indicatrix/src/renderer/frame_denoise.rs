//! Denoise + tone-map one displayed frame: a raw accumulation-buffer running sum plus its
//! first-hit guide buffers in, a displayable RGBA byte buffer out.
//!
//! This is the viewer's live-view picture pipeline (average, À-Trous denoise, tone-map).
//! It moved here unchanged from `apps/indicatrix-cut`'s `bridge::render_thread::denoise`
//! (which re-exports it, so every GUI call site is untouched) so a coordinator streaming
//! finished 8-bit `DISPLAY_FRAME`s can produce exactly the picture the viewer would have
//! produced from the same merged sum -- guides from [`crate::renderer::guide_pass`], the
//! denoiser from [`crate::renderer::denoise`], the tone curve from
//! [`crate::renderer::tonemap`]. `tests::denoised_output_is_pinned_to_the_pre_move_gui_code`
//! pins the bytes.
//!
//! CPU only (the denoiser and tone-mapper are `std::thread::scope` loops): no GPU program
//! is involved, so a caller never needs a `GpuBackend` turn for it.

use crate::renderer::{
    denoise::{AtrousDenoiser, AtrousParams, GBuffers},
    tonemap::tonemap_to_rgba,
};
use glam::Vec3;

/// Denoises and tone-maps one frame into a fresh `width * height * 4` RGBA byte buffer.
///
/// Runs the À-Trous denoiser over `frame.accum_buffer`'s running average -- NEVER over
/// `accum_buffer` itself, which stays the raw unfiltered sum so filtered output is
/// never fed back into the progressive-accumulation estimator (that would bias it) --
/// and tone-maps the result.
///
/// `scratch`'s denoiser and buffers are owned by the caller and passed by mutable
/// reference so steady-state use does no per-frame heap allocation beyond the returned
/// byte buffer.
///
/// Denoising is nonlinear: call this once, on the whole merged sum, never per source.
/// The guide buffers depend only on pose and geometry, so they may come from traced
/// samples (the GUI's local loop) or from [`crate::renderer::guide_pass`]'s prepass (a
/// remote-sourced or coordinator-merged image) with the same result for the same guides.
pub fn denoise_and_tonemap_frame(
    frame: FirstHitSnapshot<'_>,
    scratch: &mut DenoiseScratch<'_>,
) -> Vec<u8> {
    let inv_samples = 1.0 / frame.current_sample_count as f32;
    scratch.avg_color_buf.clear();
    scratch
        .avg_color_buf
        .extend(frame.accum_buffer.iter().map(|v| *v * inv_samples));

    let gbuffers = GBuffers {
        color: scratch.avg_color_buf,
        depth: frame.first_hit_depth,
        normal: frame.first_hit_normal,
        facet_id: frame.first_hit_facet_id,
        width: frame.width as usize,
        height: frame.height as usize,
        spp: frame.current_sample_count,
    };
    scratch
        .denoiser
        .denoise_into(&gbuffers, &AtrousParams::default(), scratch.filtered_buf);

    // `filtered_buf` is already averaged and filtered, so no further scaling.
    tonemap_to_rgba(scratch.filtered_buf, 1.0)
}

/// One frame's accumulated radiance plus its first-hit guide buffers.
///
/// Everything [`denoise_and_tonemap_frame`] reads (never mutates) to build that call's
/// `GBuffers`. `Copy`: every field is a shared reference or scalar, so by-value is as
/// cheap as by-reference and reads better.
#[derive(Clone, Copy)]
pub struct FirstHitSnapshot<'a> {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Samples per pixel summed into `accum_buffer` (the running-average divisor).
    pub current_sample_count: u32,
    /// Summed XYZ radiance, `width * height` long, row-major.
    pub accum_buffer: &'a [Vec3],
    /// First-hit depth per pixel (`1.0e6` for a miss).
    pub first_hit_depth: &'a [f32],
    /// First-hit normal per pixel ([`Vec3::ZERO`] for a miss).
    pub first_hit_normal: &'a [Vec3],
    /// First-hit facet index per pixel (`-1` for a miss).
    pub first_hit_facet_id: &'a [i32],
}

/// The mutable denoise scratch state a call reuses across frames to avoid per-frame
/// heap allocation -- see [`denoise_and_tonemap_frame`].
pub struct DenoiseScratch<'a> {
    /// The persistent denoiser (its ping-pong buffers are resized only on a size change).
    pub denoiser: &'a mut AtrousDenoiser,
    /// Receives the running average (`accum_buffer / current_sample_count`).
    pub avg_color_buf: &'a mut Vec<Vec3>,
    /// Receives the filtered average, then tone-mapped.
    pub filtered_buf: &'a mut Vec<Vec3>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane},
        optics::raytracer::{Camera, intersect_polyhedron, xyz_to_srgb_gamma},
        renderer::guide_pass::generate_guide_buffers,
    };

    /// FNV-1a 64 over `bytes` -- a dependency-free fingerprint for the pin below.
    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &b| {
            (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// The pin fixture: a 24x16 view of the standard round brilliant at a fixed pose,
    /// and a deterministic, deliberately noisy 3-sample accumulation sum (low enough
    /// that the denoiser really filters). `apps/indicatrix-cut`'s
    /// `render_thread::denoise` tests rebuild this exact fixture through the GUI's own
    /// `GuideCache` and re-exports and assert the same hash.
    const PIN_SIZE: (u32, u32) = (24, 16);
    const PIN_SAMPLES: u32 = 3;
    const PIN_POSE: (f32, f32, f32) = (0.60, 0.45, 2.4);

    fn pin_accum() -> Vec<Vec3> {
        (0..(PIN_SIZE.0 * PIN_SIZE.1) as usize)
            .map(|i| {
                Vec3::new(
                    ((i * 37) % 101) as f32 * 0.03,
                    ((i * 53) % 97) as f32 * 0.02,
                    ((i * 71) % 89) as f32 * 0.025,
                )
            })
            .collect()
    }

    /// `bridge::frame_cache::guide_pass::generate_guide_buffers` exactly as the GUI
    /// computed each pixel before the move, run serially (every pixel is a pure function
    /// of its own ray, so row chunking cannot change a value): the "before" side.
    fn pre_move_guides(
        (width, height): (u32, u32),
        camera: &Camera,
        planes: &[GpuFacetPlane],
    ) -> (Vec<f32>, Vec<Vec3>, Vec<i32>) {
        let mut depth = Vec::new();
        let mut normal = Vec::new();
        let mut facet_id = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let ray =
                    camera.generate_ray(x as f32, y as f32, width as f32, height as f32, 0.0, 0.0);
                let hit = intersect_polyhedron(ray, planes);
                depth.push(hit.map_or(1.0e6, |h| h.t));
                normal.push(hit.map_or(Vec3::ZERO, |h| h.normal));
                facet_id.push(hit.map_or(-1, |h| h.facet_idx as i32));
            }
        }
        (depth, normal, facet_id)
    }

    /// `bridge::render_thread::denoise::denoise_and_tonemap_frame` exactly as the GUI
    /// wrote it before the move (fresh buffers instead of caller scratch).
    fn pre_move_denoise_and_tonemap(
        (width, height): (u32, u32),
        samples: u32,
        accum: &[Vec3],
        (depth, normal, facet_id): &(Vec<f32>, Vec<Vec3>, Vec<i32>),
    ) -> Vec<u8> {
        let inv_samples = 1.0 / samples as f32;
        let avg: Vec<Vec3> = accum.iter().map(|v| *v * inv_samples).collect();
        let gbuffers = GBuffers {
            color: &avg,
            depth,
            normal,
            facet_id,
            width: width as usize,
            height: height as usize,
            spp: samples,
        };
        let mut filtered = Vec::new();
        AtrousDenoiser::new().denoise_into(&gbuffers, &AtrousParams::default(), &mut filtered);
        tonemap_to_rgba(&filtered, 1.0)
    }

    /// Byte identity of the moved pipeline (guide prepass + denoise + tone-map) with the
    /// pre-move GUI code, plus a pinned hash so a later change to any of the three fails
    /// here first. The hash is the pre-move code's output.
    #[test]
    fn denoised_output_is_pinned_to_the_pre_move_gui_code() {
        const PINNED: u64 = 0xca39_c692_af52_3c71;
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(PIN_POSE.0, PIN_POSE.1, PIN_POSE.2, 42.0);
        let accum = pin_accum();

        let before = pre_move_denoise_and_tonemap(
            PIN_SIZE,
            PIN_SAMPLES,
            &accum,
            &pre_move_guides(PIN_SIZE, &camera, &planes),
        );

        let guides = generate_guide_buffers(PIN_SIZE.0, PIN_SIZE.1, &camera, &planes);
        let mut denoiser = AtrousDenoiser::new();
        let (mut avg_color_buf, mut filtered_buf) = (Vec::new(), Vec::new());
        let after = denoise_and_tonemap_frame(
            FirstHitSnapshot {
                width: PIN_SIZE.0,
                height: PIN_SIZE.1,
                current_sample_count: PIN_SAMPLES,
                accum_buffer: &accum,
                first_hit_depth: &guides.depth,
                first_hit_normal: &guides.normal,
                first_hit_facet_id: &guides.facet_id,
            },
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg_color_buf,
                filtered_buf: &mut filtered_buf,
            },
        );

        assert_eq!(after, before, "the moved pipeline differs from the GUI's");
        assert_ne!(
            after,
            tonemap_to_rgba(&accum, 1.0 / PIN_SAMPLES as f32),
            "the fixture must actually be filtered, not the taper's identity copy"
        );
        let hash = fnv1a64(&before);
        assert_eq!(hash, PINNED, "pre-move hash: {hash:#018x}");
    }

    /// Convergence requirement: at a high enough sample count the À-Trous filter's
    /// taper curve drives its colour sigma below `taper_identity_epsilon`, so
    /// `denoise_into` short-circuits to an exact copy of its input. Pins that
    /// guarantee at the integration point: once converged, the displayed image must be
    /// bit-identical to tone-mapping the raw accumulation average.
    #[test]
    fn denoise_and_tonemap_frame_is_identity_at_high_sample_counts() {
        // taper(50000) ~= 0.009, comfortably under the 0.02 identity threshold.
        const HIGH_SAMPLE_COUNT: u32 = 50_000;

        let width = 6u32;
        let height = 5u32;
        let pixel_count = (width * height) as usize;
        // Non-uniform, edge-having synthetic frame: a bug that filters anyway (rather
        // than taking the identity short-circuit) would visibly smear it.
        let accum_buffer: Vec<Vec3> = (0..pixel_count)
            .map(|i| {
                let x = i % width as usize;
                Vec3::new(
                    1.0 + x as f32,
                    0.5 * x as f32,
                    0.1f32.mul_add(-(x as f32), 2.0),
                )
            })
            .collect();
        let first_hit_depth: Vec<f32> = (0..pixel_count)
            .map(|i| (i as f32).mul_add(0.01, 1.0))
            .collect();
        let first_hit_normal: Vec<Vec3> = vec![Vec3::Y; pixel_count];
        let first_hit_facet_id: Vec<i32> = (0..pixel_count)
            .map(|i| (i % width as usize) as i32)
            .collect();

        let mut denoiser = AtrousDenoiser::new();
        let mut avg_color_buf = Vec::new();
        let mut filtered_buf = Vec::new();
        let denoised_bytes = denoise_and_tonemap_frame(
            FirstHitSnapshot {
                width,
                height,
                current_sample_count: HIGH_SAMPLE_COUNT,
                accum_buffer: &accum_buffer,
                first_hit_depth: &first_hit_depth,
                first_hit_normal: &first_hit_normal,
                first_hit_facet_id: &first_hit_facet_id,
            },
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg_color_buf,
                filtered_buf: &mut filtered_buf,
            },
        );

        // Ground truth: tone-map the raw accumulation average with no denoiser.
        let mut expected_bytes = vec![0u8; pixel_count * 4];
        for (i, xyz) in accum_buffer.iter().enumerate() {
            let rgba = xyz_to_srgb_gamma(*xyz / HIGH_SAMPLE_COUNT as f32);
            expected_bytes[i * 4..i * 4 + 4].copy_from_slice(&rgba);
        }

        assert_eq!(
            denoised_bytes, expected_bytes,
            "at a converged (high) sample count, denoise_and_tonemap_frame's output must be bit-identical to tone-mapping the raw accumulation average with no filtering applied"
        );
    }
}
