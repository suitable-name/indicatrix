//! Denoise + tone-map: turning a raw accumulation-buffer running sum into a displayable
//! RGBA byte buffer, with or without the À-Trous denoiser applied.
//!
//! The denoising half ([`denoise_and_tonemap_frame`], [`FirstHitSnapshot`],
//! [`DenoiseScratch`]) moved unchanged to `indicatrix::renderer::frame_denoise` so a
//! coordinator can stream the viewer's exact denoised picture; it is re-exported here so
//! every GUI call site (`display_thread`, `gui::remote::render_merged_frame`) keeps its
//! `render_thread::...` path. `tests::gui_path_matches_the_pinned_indicatrix_hash` checks
//! the GUI's own route (its `GuideCache` plus these re-exports) against the same pin.

use glam::Vec3;
use indicatrix::renderer::tonemap::tonemap_to_rgba;

pub use indicatrix::renderer::frame_denoise::{
    DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame,
};

/// Tone-maps `accum_buffer`'s running average directly, with no denoising -- what the
/// render loop uses instead of [`denoise_and_tonemap_frame`] when
/// `RenderContext::denoise_enabled` is off, and what `gui::remote::render_merged_frame`
/// uses identically for a remote-sourced image (guide buffers for that path are
/// regenerated locally by `bridge::frame_cache::guide_pass`, so this is purely the
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

    /// The GUI's own denoise route -- `GuideCache::ensure` for the guides, then the
    /// re-exported `denoise_and_tonemap_frame` -- on the fixture of
    /// `indicatrix::renderer::frame_denoise`'s pin test (24x16 round brilliant at pose
    /// `(0.60, 0.45, 2.4)`, 3 samples, the same formula-built sum) must produce that
    /// test's pinned bytes: the move changed no pixel of the GUI's live view.
    #[test]
    fn gui_path_matches_the_pinned_indicatrix_hash() {
        // The same value as the twin in `indicatrix::renderer::frame_denoise`: pinned with
        // background pixels passing through the denoiser unfiltered and the sRGB encode's
        // round-to-nearest quantisation.
        const PINNED: u64 = 0x4b0e_fd3c_897e_809c;
        let (width, height, samples) = (24_u32, 16_u32, 3_u32);
        let planes = indicatrix::geometry::cuts::StandardGemCuts::standard_round_brilliant();
        let accum: Vec<Vec3> = (0..(width * height) as usize)
            .map(|i| {
                Vec3::new(
                    ((i * 37) % 101) as f32 * 0.03,
                    ((i * 53) % 97) as f32 * 0.02,
                    ((i * 71) % 89) as f32 * 0.025,
                )
            })
            .collect();
        let mut cache = crate::bridge::frame_cache::guide_pass::GuideCache::new();
        let guides = cache.ensure(width, height, 0.60, 0.45, 2.4, &planes);
        let mut denoiser = indicatrix::renderer::denoise::AtrousDenoiser::new();
        let (mut avg_color_buf, mut filtered_buf) = (Vec::new(), Vec::new());
        let bytes = denoise_and_tonemap_frame(
            FirstHitSnapshot {
                width,
                height,
                current_sample_count: samples,
                accum_buffer: &accum,
                first_hit_depth: &guides.depth,
                first_hit_normal: &guides.normal,
                first_hit_facet_id: &guides.facet_id,
                // All zeros here (`ensure` passes no index): a constant signature filters
                // exactly like none, so `PINNED` stays the old literal.
                first_hit_path_sig: &guides.path_sig,
            },
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg_color_buf,
                filtered_buf: &mut filtered_buf,
            },
        );
        let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &b| {
            (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        assert_eq!(hash, PINNED, "GUI-path hash: {hash:#018x}");
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
