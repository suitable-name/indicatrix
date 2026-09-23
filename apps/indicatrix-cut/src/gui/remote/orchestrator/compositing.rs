//! Turning a remote accumulator's running sum into a displayable, denoised RGBA byte
//! buffer -- [`render_merged_frame`], and the two `Copy` bundles it takes. See this
//! group's own `mod.rs` doc comment.

use crate::bridge::{
    frame_cache::guide_pass::GuideCache,
    render_thread::{
        DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame, tonemap_running_average,
    },
};
use glam::Vec3;
use indicatrix::geometry::plane::GpuFacetPlane;

/// The remote accumulator's current running sum, as [`render_merged_frame`] receives it:
/// dimensions, sample count, and the merged radiance itself. `Copy` -- every field is a
/// shared reference or a scalar, matching `render_thread::gpu_backend::BackendFrame`'s
/// identical rationale.
#[derive(Clone, Copy)]
pub(super) struct AccumSnapshot<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) samples_done: u32,
    pub(super) buffer: &'a [Vec3],
}

/// The pose (`yaw`/`pitch`/`distance`) and geometry [`render_merged_frame`] needs to key
/// and, on a cache miss, regenerate the guide-buffer prepass -- see
/// [`GuideCache::ensure`]. `Copy` for the same reason as [`AccumSnapshot`].
#[derive(Clone, Copy)]
pub(super) struct PoseAndGeometry<'a> {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) planes: &'a [GpuFacetPlane],
}

/// Turns a remote accumulator's current running sum into a displayable RGBA byte
/// buffer, applying the SAME single À-Trous denoise pass the local path applies to its
/// own readback -- never a per-source one (`buffer` here is already the merged,
/// summed-across-however-many-`FRAME`-events-arrived-so-far radiance; denoising is
/// nonlinear, so it must run once, after summing, on that whole merged buffer, exactly
/// like `render_thread::denoise_and_tonemap_frame`'s own doc comment requires of the
/// local accumulation buffer -- there is no per-contribution denoise anywhere in this
/// pipeline).
///
/// The depth/normal/facet-id guides a remote payload never carries (see
/// `bridge::remote_render`'s module docs on why they're not shipped over the wire) come
/// from `guide_cache`: a local primary-ray-only prepass over the CURRENT camera pose
/// and gem geometry (see `bridge::guide_pass`'s module docs for why that's valid for
/// ANY image of that pose, remote-sourced or not, and why caching on pose+geometry
/// rather than recomputing every call is what keeps this cheap across many `FRAME`
/// events from one in-progress render).
///
/// No Slint/GUI types in the signature -- exercised directly by this module's own unit
/// tests without a window, a socket, or a worker. This function itself still runs the
/// full (multi-second at 4K) denoise pass synchronously, so it is called only from
/// `super::generation::spawn_denoise_generation`'s background thread, never directly
/// from `super::tick::redraw_from_accumulator` (which runs on the Slint UI thread,
/// invoked from inside `handle_remote_update`'s `upgrade_in_event_loop` closure, and
/// would block it). The background thread pre-seeds a throwaway `GuideCache` via
/// [`GuideCache::adopt`] so its own internal `guide_cache.ensure` call is a guaranteed
/// cache hit rather than a synchronous regenerate -- see that function's doc comment.
pub(super) fn render_merged_frame(
    accum: AccumSnapshot<'_>,
    denoise_enabled: bool,
    pose: PoseAndGeometry<'_>,
    guide_cache: &mut GuideCache,
    scratch: &mut DenoiseScratch<'_>,
) -> Vec<u8> {
    if !denoise_enabled {
        return tonemap_running_average(
            accum.width,
            accum.height,
            accum.samples_done,
            accum.buffer,
        );
    }
    let guides = guide_cache.ensure(
        accum.width,
        accum.height,
        pose.yaw,
        pose.pitch,
        pose.distance,
        pose.planes,
    );
    denoise_and_tonemap_frame(
        FirstHitSnapshot {
            width: accum.width,
            height: accum.height,
            current_sample_count: accum.samples_done,
            accum_buffer: accum.buffer,
            first_hit_depth: &guides.depth,
            first_hit_normal: &guides.normal,
            first_hit_facet_id: &guides.facet_id,
        },
        scratch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::frame_cache::guide_pass::generate_guide_buffers;
    use indicatrix::{
        geometry::cuts::StandardGemCuts, optics::raytracer::Camera,
        renderer::denoise::AtrousDenoiser,
    };

    /// A deliberately non-uniform synthetic accumulation buffer (mirrors
    /// `render_thread`'s own `denoise_and_tonemap_frame` tests) so a bug that skips
    /// filtering, or filters the wrong data, would visibly change the output.
    fn synthetic_noisy_buffer(width: u32, height: u32) -> Vec<Vec3> {
        (0..(width * height) as usize)
            .map(|i| {
                let x = (i % width as usize) as f32;
                Vec3::new(1.0 + x, 0.5 * x, 0.1f32.mul_add(-x, 2.0))
            })
            .collect()
    }

    #[test]
    fn render_merged_frame_skips_denoising_when_disabled() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);
        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();

        let bytes = render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done: 4,
                buffer: &buffer,
            },
            false,
            PoseAndGeometry {
                yaw: 0.60,
                pitch: 0.45,
                distance: 2.4,
                planes: &planes,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );

        assert_eq!(bytes, tonemap_running_average(width, height, 4, &buffer));
        assert_eq!(
            guide_cache.generation(),
            0,
            "denoising disabled must never trigger the guide prepass at all -- the \
             user's toggle governs whether the (cheap, but not free) prepass runs, \
             exactly like it already governs the local path"
        );
    }

    #[test]
    fn render_merged_frame_reuses_guides_across_repeated_calls_with_an_unchanged_pose() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);
        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();

        // Three redraws, as if three `FRAME` events arrived from the same
        // in-progress remote render at an unchanged camera pose.
        for samples in [1u32, 2, 3] {
            render_merged_frame(
                AccumSnapshot {
                    width,
                    height,
                    samples_done: samples,
                    buffer: &buffer,
                },
                true,
                PoseAndGeometry {
                    yaw: 0.60,
                    pitch: 0.45,
                    distance: 2.4,
                    planes: &planes,
                },
                &mut guide_cache,
                &mut DenoiseScratch {
                    denoiser: &mut denoiser,
                    avg_color_buf: &mut avg,
                    filtered_buf: &mut filtered,
                },
            );
        }

        assert_eq!(
            guide_cache.generation(),
            1,
            "repeated redraws of the same in-progress remote render at an unchanged \
             pose must reuse the cached guide buffers, not regenerate them per frame"
        );
    }

    #[test]
    fn render_merged_frame_regenerates_guides_when_the_pose_changes() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);
        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();

        render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done: 1,
                buffer: &buffer,
            },
            true,
            PoseAndGeometry {
                yaw: 0.60,
                pitch: 0.45,
                distance: 2.4,
                planes: &planes,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );
        render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done: 1,
                buffer: &buffer,
            },
            true,
            PoseAndGeometry {
                yaw: 0.90,
                pitch: 0.45,
                distance: 2.4,
                planes: &planes,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );

        assert_eq!(
            guide_cache.generation(),
            2,
            "a camera-pose change (a fresh drag settling into a new remote request) \
             must invalidate the cached guides -- reusing a stale pose's guides would \
             misalign depth/normal/facet-id against the new image's geometry"
        );
    }

    /// `render_merged_frame` takes exactly one buffer and one `samples_done` -- never a
    /// per-contribution slice with its own smaller count -- and that TRUE merged total
    /// is what must drive the denoiser's convergence taper (`renderer::denoise`'s
    /// module docs on `sigma_color_effective`). This pins that the plumbing actually
    /// threads the real total through: at a converged sample count the output must be
    /// bit-identical to a plain tonemap (matching `render_thread`'s own
    /// `denoise_and_tonemap_frame_is_identity_at_high_sample_counts` guarantee for the
    /// local path). If this pipeline instead denoised a small per-contribution slice at
    /// its own (permanently small) sample count -- the "never on a partial
    /// contribution" mistake the task explicitly calls out -- this identity would never
    /// be reachable even once the real remote accumulation had converged.
    #[test]
    fn render_merged_frame_at_a_converged_sample_count_matches_a_plain_tonemap() {
        const HIGH_SAMPLE_COUNT: u32 = 50_000; // taper(50000) << taper_identity_epsilon

        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);

        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();
        let high_total_bytes = render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done: HIGH_SAMPLE_COUNT,
                buffer: &buffer,
            },
            true,
            PoseAndGeometry {
                yaw: 0.60,
                pitch: 0.45,
                distance: 2.4,
                planes: &planes,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );

        assert_eq!(
            high_total_bytes,
            tonemap_running_average(width, height, HIGH_SAMPLE_COUNT, &buffer),
            "at a converged TRUE total, denoising the merged buffer must be an exact \
             no-op, just like the local path's readback"
        );
    }

    /// At a low (unconverged) sample count, `render_merged_frame` must run the SAME
    /// `render_thread::denoise_and_tonemap_frame` the local path uses, fed the SAME
    /// guides an independent `GuideCache`/`generate_guide_buffers` call for the
    /// identical pose/geometry would produce -- not some other, silently-diverged code
    /// path. This is a direct cross-check of the actual wiring (rather than asserting
    /// the output merely "looks filtered", which for a real gem's facet geometry over a
    /// tiny test image can legitimately be a no-op if every pixel in the crop lands on
    /// a distinct facet -- the hard per-facet edge-stop is deliberately that strict, see
    /// `renderer::denoise`'s module docs).
    #[test]
    fn render_merged_frame_at_a_low_sample_count_matches_denoise_and_tonemap_frame_directly() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);
        let (yaw, pitch, distance) = (0.60, 0.45, 2.4);
        let samples_done = 1;
        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();
        let actual = render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done,
                buffer: &buffer,
            },
            true,
            PoseAndGeometry {
                yaw,
                pitch,
                distance,
                planes: &planes,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );

        // Independently reproduce what `render_merged_frame` should have done: generate
        // the guides for the same pose/geometry and denoise directly, with fresh
        // scratch state so nothing is shared with the call above.
        let camera = Camera::new(yaw, pitch, distance, 42.0);
        let guides = generate_guide_buffers(width, height, &camera, &planes);
        let mut expected_denoiser = AtrousDenoiser::new();
        let mut expected_avg = Vec::new();
        let mut expected_filtered = Vec::new();
        let expected = denoise_and_tonemap_frame(
            FirstHitSnapshot {
                width,
                height,
                current_sample_count: samples_done,
                accum_buffer: &buffer,
                first_hit_depth: &guides.depth,
                first_hit_normal: &guides.normal,
                first_hit_facet_id: &guides.facet_id,
            },
            &mut DenoiseScratch {
                denoiser: &mut expected_denoiser,
                avg_color_buf: &mut expected_avg,
                filtered_buf: &mut expected_filtered,
            },
        );

        assert_eq!(
            actual, expected,
            "render_merged_frame must denoise via the exact same guides/mechanism a \
             direct generate_guide_buffers + denoise_and_tonemap_frame call would \
             produce for the identical pose and geometry"
        );
    }
}
