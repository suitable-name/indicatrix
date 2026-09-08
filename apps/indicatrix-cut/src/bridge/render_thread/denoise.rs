//! Denoise + tone-map: turning a raw accumulation-buffer running sum into a displayable
//! RGBA byte buffer, with or without the À-Trous denoiser applied.

use glam::Vec3;
use indicatrix::renderer::{
    denoise::{AtrousDenoiser, AtrousParams, GBuffers},
    tonemap::tonemap_to_rgba,
};

/// Runs the À-Trous denoiser over `accum_buffer`'s running average -- NEVER over
/// `accum_buffer` itself, which stays the raw unfiltered sum so filtered output is
/// never fed back into the progressive-accumulation estimator (that would bias it) --
/// and tone-maps the result into a fresh `width * height * 4` RGBA byte buffer.
/// `denoiser`/`avg_color_buf`/`filtered_buf` are owned by the render loop and passed
/// by mutable reference so steady-state use does no per-frame heap allocation beyond
/// the returned byte buffer.
///
/// Public (not private): `gui::remote::render_merged_frame` calls this directly too,
/// so a remote-sourced image is denoised by the exact same code as a local one -- the
/// only difference is where the guide buffers came from (traced samples locally,
/// `bridge::guide_pass`'s primary-ray prepass remotely).
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

/// One frame's accumulated radiance plus its first-hit guide buffers -- everything
/// [`denoise_and_tonemap_frame`] reads (never mutates) to build that call's `GBuffers`.
/// Bundled because the function has two call sites with differently-shaped sources for
/// these seven values. `Copy`, like `gpu_backend::BackendFrame`: every field is a
/// shared reference or scalar, so by-value is as cheap as by-reference and reads better.
#[derive(Clone, Copy)]
pub struct FirstHitSnapshot<'a> {
    pub width: u32,
    pub height: u32,
    pub current_sample_count: u32,
    pub accum_buffer: &'a [Vec3],
    pub first_hit_depth: &'a [f32],
    pub first_hit_normal: &'a [Vec3],
    pub first_hit_facet_id: &'a [i32],
}

/// The mutable denoise scratch state a call reuses across frames to avoid per-frame
/// heap allocation -- see [`denoise_and_tonemap_frame`].
pub struct DenoiseScratch<'a> {
    pub denoiser: &'a mut AtrousDenoiser,
    pub avg_color_buf: &'a mut Vec<Vec3>,
    pub filtered_buf: &'a mut Vec<Vec3>,
}

/// Tone-maps `accum_buffer`'s running average directly, with no denoising -- what the
/// render loop uses instead of [`denoise_and_tonemap_frame`] when
/// `RenderContext::denoise_enabled` is off, and what `gui::remote::render_merged_frame`
/// uses identically for a remote-sourced image (guide buffers for that path are
/// regenerated locally by `bridge::guide_pass`, so this is purely the
/// `denoise_enabled == false` path for both backends, not a fallback one is stuck
/// with). Mirrors `export_thread::tonemap_to_rgba`'s logic (kept separate since that
/// one is `export_thread`-private with a `total_samples` divisor).
#[must_use]
pub fn tonemap_running_average(
    width: u32,
    height: u32,
    current_sample_count: u32,
    accum_buffer: &[Vec3],
) -> Vec<u8> {
    debug_assert_eq!(
        accum_buffer.len(),
        (width * height) as usize,
        "accum_buffer must hold exactly width*height pixels"
    );
    let inv_samples = 1.0 / current_sample_count.max(1) as f32;
    tonemap_to_rgba(accum_buffer, inv_samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::optics::raytracer::xyz_to_srgb_gamma;

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

    /// `tonemap_running_average` must match a direct tone-map of the raw accumulation
    /// average exactly, at any sample count (not just the high-sample-count identity
    /// case above).
    #[test]
    fn tonemap_running_average_matches_a_direct_tonemap_at_any_sample_count() {
        let width = 4u32;
        let height = 3u32;
        let pixel_count = (width * height) as usize;
        let sample_count = 7u32;
        let accum_buffer: Vec<Vec3> = (0..pixel_count)
            .map(|i| Vec3::new(i as f32 * 0.3, i as f32 * 0.1, 1.0))
            .collect();

        let actual = tonemap_running_average(width, height, sample_count, &accum_buffer);

        let mut expected = vec![0u8; pixel_count * 4];
        for (i, xyz) in accum_buffer.iter().enumerate() {
            let rgba = xyz_to_srgb_gamma(*xyz / sample_count as f32);
            expected[i * 4..i * 4 + 4].copy_from_slice(&rgba);
        }

        assert_eq!(actual, expected);
    }

    /// A `current_sample_count` of 0 (the first poll before any sample is traced)
    /// must not divide by zero or panic.
    #[test]
    fn tonemap_running_average_handles_zero_samples_without_panicking() {
        let buf = vec![Vec3::ZERO; 4];
        let out = tonemap_running_average(2, 2, 0, &buf);
        assert_eq!(out.len(), 16);
    }
}
