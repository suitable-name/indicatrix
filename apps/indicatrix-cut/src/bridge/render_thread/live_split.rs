//! The local tracer's per-frame sample claim ([`local_frame_claim`]) for the live
//! hybrid, plus the CPU-split equivalence tests that prove a local + remote split of one
//! epoch merges to the same image a single backend renders over the whole range.
//!
//! # Where local's sample indices come from
//!
//! Outside a combined settle, local owns the whole index space of its epoch and simply
//! continues from its own count (`sample_offset = accum_samples`). Inside one
//! (`LiveComputeTarget::Both` with a live epoch), local claims each frame's range from
//! the epoch's shared cursor instead, exactly like the remote lane claims its chunks,
//! so the two can never trace the same absolute sample index (invariant 1 of the
//! hybrid guide). The claimed `start` becomes `BackendFrame::sample_offset`.

use crate::bridge::sample_cursor::LiveEpoch;

/// The `(sample_offset, spp)` local should trace this frame: `local_pre_frame_count`
/// onward when there is no live epoch, otherwise a claim of up to `spp` samples from
/// `epoch` (a failed remote chunk's returned remainder first). `None` means the epoch
/// has nothing for local right now (everything is claimed; a remote chunk may still be
/// finishing). Pure apart from the claim itself.
#[must_use]
pub(super) fn local_frame_claim(
    epoch: Option<&LiveEpoch>,
    local_pre_frame_count: u32,
    spp: u32,
) -> Option<(u32, u32)> {
    epoch.map_or(Some((local_pre_frame_count, spp)), |epoch| {
        epoch.claim_local(spp)
    })
}

/// Whether this frame must RELEASE the remote epoch because it was dispatched for a
/// different scene than the one this frame is about to trace: a scene change landed
/// between the settle's dispatch (which captured the scene the remote worker renders)
/// and this frame's snapshot. Merging the two would average two different scenes
/// (invariant 2), so the epoch is released, the frame continues local-only, and the
/// orchestrator re-dispatches for the new scene once it is stable. `false` whenever
/// remote does not own the image or there is no epoch to compare.
#[must_use]
pub(super) fn epoch_scene_mismatch(
    remote_active: bool,
    epoch_scene_generation: Option<u64>,
    frame_scene_generation: u64,
) -> bool {
    remote_active && epoch_scene_generation.is_some_and(|g| g != frame_scene_generation)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            gpu_backend::{BackendFrame, FrameOutputs},
            scanline::render_frame_scanlines,
        },
        *,
    };
    use crate::bridge::remote::live_lane::{ChunkRequest, ChunkVerdict, LiveLane};
    use glam::Vec3;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{
            materials::GemMaterial,
            raytracer::{Camera, DEFAULT_FOV_DEG, DEFAULT_POSE, LightingPreset},
        },
    };
    use indicatrix_net::{
        messages::{FrameHeader, StreamEvent},
        radiance,
    };
    use std::{sync::Arc, time::Instant};

    const W: u32 = 32;
    const H: u32 = 32;
    /// Half the epoch's budget: the whole image is `2 * N` samples per pixel.
    const N: u32 = 8;
    /// Local's per-frame claim, as the live loop's small per-frame `spp` would be.
    const LOCAL_SPP: u32 = 2;

    /// Traces `[start, start + count)` of the fixed test scene with the live CPU
    /// tracer, ADDING into `accum`.
    fn trace(start: u32, count: u32, accum: &mut [Vec3]) {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let camera = Camera::new(
            DEFAULT_POSE.yaw,
            DEFAULT_POSE.pitch,
            DEFAULT_POSE.distance,
            DEFAULT_FOV_DEG,
        );
        let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let frame = BackendFrame {
            width: W,
            height: H,
            yaw: DEFAULT_POSE.yaw,
            pitch: DEFAULT_POSE.pitch,
            distance: DEFAULT_POSE.distance,
            camera: &camera,
            planes: &planes,
            tools: &[],
            fluorescence: indicatrix::optics::fluorescence::Fluorescence::none(),
            facet_finishes: &[],
            material: &material,
            max_bounces: 6,
            environment,
            spp: count,
            sample_offset: start,
        };
        let pixels = (W * H) as usize;
        let mut depth = vec![0.0; pixels];
        let mut normal = vec![Vec3::ZERO; pixels];
        let mut facet_id = vec![0; pixels];
        render_frame_scanlines(
            &frame,
            count,
            start + count,
            &mut FrameOutputs {
                accum,
                depth: &mut depth,
                normal: &mut normal,
                facet_id: &mut facet_id,
            },
        );
    }

    /// The fake worker: traces `[first, first + samples)` into its own buffer and
    /// sends it as one `FRAME` delta into the chunk's accumulator (range-checked).
    fn remote_delta(chunk: &ChunkRequest, first: u32, samples: u32) {
        let mut delta = vec![Vec3::ZERO; (W * H) as usize];
        trace(first, samples, &mut delta);
        let bytes = radiance::encode(&delta);
        let header = FrameHeader::for_payload(chunk.request_id, first, samples, &bytes);
        chunk
            .accumulator
            .lock()
            .unwrap()
            .apply(&StreamEvent::Frame(header), Some(&bytes))
            .expect("well-formed delta");
    }

    /// The single-backend reference: `[0, 2N)` in `LOCAL_SPP`-sample frames.
    fn reference() -> Vec<Vec3> {
        let mut accum = vec![Vec3::ZERO; (W * H) as usize];
        let mut start = 0;
        while start < 2 * N {
            trace(start, LOCAL_SPP, &mut accum);
            start += LOCAL_SPP;
        }
        accum
    }

    /// Local claims and traces frames until the epoch has nothing left for it.
    fn run_local(epoch: &LiveEpoch, local: &mut [Vec3], local_count: &mut u32) {
        while let Some((start, count)) = local_frame_claim(Some(epoch), *local_count, LOCAL_SPP) {
            trace(start, count, local);
            *local_count += count;
        }
    }

    fn assert_matches_reference(merged: &[Vec3], reference: &[Vec3]) {
        let mut worst = 0.0f32;
        for (m, r) in merged.iter().zip(reference) {
            for (a, b) in m.to_array().into_iter().zip(r.to_array()) {
                let scale = a.abs().max(b.abs()).max(1e-6);
                worst = worst.max((a - b).abs() / scale);
            }
        }
        assert!(
            worst <= 1e-5,
            "the split render must equal the single-backend render (worst relative \
             difference {worst:e})"
        );
    }

    /// Local/remote CPU-split equivalence: remote traces `[0, N)` (its first chunk, as two
    /// streamed deltas), local traces `[N, 2N)` claimed frame by frame from the same
    /// epoch, and the merged display sum equals a single backend over `[0, 2N)`.
    #[test]
    fn a_local_plus_remote_split_equals_a_single_backend_render() {
        let epoch = Arc::new(LiveEpoch::new(W, H, 2 * N));
        let mut lane = LiveLane::new(Arc::clone(&epoch), true);
        let chunk = lane.next_chunk(1, Instant::now()).expect("first chunk");
        assert_eq!((chunk.first_sample, chunk.samples), (0, N));

        let mut local = vec![Vec3::ZERO; (W * H) as usize];
        let mut local_count = 0;
        run_local(&epoch, &mut local, &mut local_count);
        assert_eq!(local_count, N, "local claimed exactly the other half");

        remote_delta(&chunk, 0, 3);
        remote_delta(&chunk, 3, N - 3);
        lane.chunk_done(1, Instant::now()).expect("chunk in flight");
        assert!(lane.next_chunk(2, Instant::now()).is_none());

        let mut merged = local;
        let remote_count = epoch.add_remote_into(&mut merged);
        assert_eq!(local_count + remote_count, 2 * N);
        assert_matches_reference(&merged, &reference());
    }

    /// Same, but the remote chunk fails after 3 samples: its prefix is kept, local
    /// picks up the returned remainder, and the merged image is still the reference.
    #[test]
    fn a_failed_remote_chunk_still_merges_to_the_single_backend_render() {
        let epoch = Arc::new(LiveEpoch::new(W, H, 2 * N));
        let mut lane = LiveLane::new(Arc::clone(&epoch), true);
        let chunk = lane.next_chunk(1, Instant::now()).expect("first chunk");
        remote_delta(&chunk, 0, 3);
        assert_eq!(lane.chunk_failed(1), Some(ChunkVerdict::Continue));

        let mut local = vec![Vec3::ZERO; (W * H) as usize];
        let mut local_count = 0;
        run_local(&epoch, &mut local, &mut local_count);
        assert!(lane.next_chunk(2, Instant::now()).is_none(), "all claimed");

        let mut merged = local;
        let remote_count = epoch.add_remote_into(&mut merged);
        assert_eq!(remote_count, 3);
        assert_eq!(
            local_count + remote_count,
            2 * N,
            "the final count is exact"
        );
        assert_matches_reference(&merged, &reference());
    }

    /// The scene-change race of the local/remote split. An epoch stamped for scene generation 7
    /// must be released by a frame tracing generation 8, kept by a frame tracing 7, and
    /// never matter when remote does not own the image.
    #[test]
    fn an_epoch_for_a_different_scene_is_released_and_a_matching_one_kept() {
        let epoch = LiveEpoch::new(1, 1, 16).for_scene(7);
        let generation = Some(epoch.scene_generation());
        assert!(
            epoch_scene_mismatch(true, generation, 8),
            "mismatch -> release"
        );
        assert!(!epoch_scene_mismatch(true, generation, 7), "match -> keep");
        assert!(
            !epoch_scene_mismatch(false, generation, 8),
            "not remote-owned"
        );
        assert!(!epoch_scene_mismatch(true, None, 8), "no epoch to compare");
    }

    /// End to end through the real `RenderContext`: a scene change landing after the
    /// dispatch stamped the epoch makes the next frame's generation differ.
    #[test]
    fn a_scene_change_after_the_dispatch_is_detected_by_the_next_frame() {
        use crate::bridge::render_thread::RenderContext;
        let mut ctx = RenderContext::default();
        let dispatched_for = ctx.scene_generation();
        let epoch = LiveEpoch::new(1, 1, 16).for_scene(dispatched_for);
        ctx.exposure *= 2.0; // the racing scene change
        let frame = ctx.scene_generation();
        assert!(epoch_scene_mismatch(
            true,
            Some(epoch.scene_generation()),
            frame
        ));
    }

    #[test]
    fn without_an_epoch_local_continues_from_its_own_count() {
        assert_eq!(local_frame_claim(None, 12, 4), Some((12, 4)));
    }

    #[test]
    fn with_an_epoch_local_claims_from_its_cursor_and_stops_at_the_target() {
        let epoch = LiveEpoch::new(1, 1, 5);
        assert_eq!(local_frame_claim(Some(&epoch), 0, 4), Some((0, 4)));
        assert_eq!(local_frame_claim(Some(&epoch), 4, 4), Some((4, 1)));
        assert_eq!(local_frame_claim(Some(&epoch), 5, 4), None);
    }
}
