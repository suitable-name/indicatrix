//! The two off-thread background generations the orchestrator keeps in flight: the
//! guide-buffer prepass ([`PendingGuideGeneration`]/[`spawn_guide_generation`]/
//! [`adopt_ready_guides`]) and the full denoise-and-tonemap pass
//! ([`PendingDenoiseGeneration`]/[`spawn_denoise_generation`]/[`adopt_ready_denoise`]).
//! See this group's own `mod.rs` doc comment.

use super::compositing::{AccumSnapshot, PoseAndGeometry, render_merged_frame};
use crate::bridge::{
    frame_cache::guide_pass::{
        GuideBuffers, GuideCache, GuideKey, generate_guide_buffers_cancellable,
    },
    render_thread::DenoiseScratch,
};
use glam::Vec3;
use indicatrix::{
    geometry::plane::GpuFacetPlane, optics::raytracer::Camera, renderer::denoise::AtrousDenoiser,
};
use std::sync::{Arc, Mutex, PoisonError, atomic::AtomicBool};

/// A background primary-ray-only guide-buffer prepass in flight for one dispatched
/// `RenderRequest` -- started at dispatch time (`super::tick::start_remote_render`),
/// not on the first redraw that needs guides, so the work overlaps the network round
/// trip and the remote render itself instead of stalling the first post-dispatch
/// redraw.
///
/// Cancellation mirrors `bridge::export_thread::ExportHandle`'s cooperative
/// `Arc<AtomicBool>` pattern: setting `cancel` doesn't tear down the background thread
/// forcibly, it just asks `generate_guide_buffers_cancellable`'s per-row check to stop
/// early.
///
/// A superseded generation can never overwrite a newer one's result: every call to
/// [`spawn_guide_generation`] allocates a brand new `result` `Arc`, never reuses the
/// previous generation's. So even if a cancelled (or simply slow) background thread
/// finishes after `Orchestrator::pending_guide_gen` has already moved on to a fresh
/// [`PendingGuideGeneration`], its write lands in an `Arc` nothing still reachable from
/// `Orchestrator` ever reads from again.
pub(super) struct PendingGuideGeneration {
    /// The pose/geometry/resolution this generation is for -- the same [`GuideKey`]
    /// identity `GuideCache` keys its own cache on. Compared against the current
    /// desired key on every redraw (`adopt_ready_guides`) so a result is only ever
    /// adopted when it matches the image currently being displayed.
    pub(super) key: GuideKey,
    /// Cooperative cancellation flag: set when the pose changes again (the render this
    /// generation was for gets cancelled/superseded), so the background thread abandons
    /// a now-pointless computation instead of running it to completion.
    pub(super) cancel: Arc<AtomicBool>,
    /// Filled in by the background thread once, and only once, if it finishes without
    /// observing `cancel`. `None` while generation is still in flight (or was
    /// abandoned) -- callers must treat a `None` read as "not ready yet", never block
    /// waiting for it.
    result: Arc<Mutex<Option<GuideBuffers>>>,
}

/// Kicks off the guide-buffer prepass for `key` on a background thread. Does NOT cancel
/// or replace any previous [`PendingGuideGeneration`] itself -- the caller
/// (`super::tick::start_remote_render`) does that first, since only it knows whether a
/// previous generation exists at all.
pub(super) fn spawn_guide_generation(
    key: GuideKey,
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    width: u32,
    height: u32,
) -> PendingGuideGeneration {
    let cancel = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    let cancel_worker = Arc::clone(&cancel);
    let result_worker = Arc::clone(&result);

    std::thread::spawn(move || {
        if let Some(buffers) =
            generate_guide_buffers_cancellable(width, height, &camera, &planes, &cancel_worker)
        {
            *result_worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(buffers);
        }
        // A `None` (cancelled) result is simply dropped: `pending_guide_gen` has
        // already moved on to a different generation by the time cancellation is
        // observed, so nothing would read this result even if it were written.
    });

    PendingGuideGeneration {
        key,
        cancel,
        result,
    }
}

/// Checks whether guide buffers are ready and correct for `desired_key` -- the
/// pose/geometry identity (see `GuideCache::key_for`) of the image about to be redrawn
/// -- adopting a background generation's result into `guide_cache` when it matches, and
/// reporting "not ready" otherwise so the caller can fall back to a plain tonemap for
/// this frame rather than blocking the UI thread to regenerate guides synchronously.
///
/// Pure aside from the one `Mutex` lock on `pending`'s result slot, so it's directly
/// unit-testable with a hand-built [`PendingGuideGeneration`] and no real background
/// thread, socket, or timer -- see this module's tests.
pub(super) fn adopt_ready_guides(
    desired_key: &GuideKey,
    guide_cache: &mut GuideCache,
    pending: Option<&PendingGuideGeneration>,
) -> bool {
    if guide_cache.matches_key(desired_key) {
        // Already installed, or `ensure` was called for this exact key before.
        return true;
    }

    let Some(pending) = pending else {
        return false;
    };
    if pending.key != *desired_key {
        // In-flight (or finished) generation is for a different pose -- reject it.
        return false;
    }

    let mut slot = pending
        .result
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let Some(buffers) = slot.take() else {
        // Still in flight.
        return false;
    };
    drop(slot);
    guide_cache.adopt(desired_key.clone(), buffers);
    true
}

/// The full À-Trous denoise-and-tonemap pass (`render_thread::denoise_and_tonemap_frame`)
/// running on a background thread for one pose/buffer snapshot, mirroring
/// [`PendingGuideGeneration`]'s cancellation-by-orphaning: a brand new `result` `Arc`
/// every dispatch, so a stale generation's eventual write lands somewhere nothing
/// still reachable ever reads.
///
/// Unlike guide generation (dispatched once per `RenderRequest`, since guides depend
/// only on pose+geometry), a denoise generation is redispatched every time the
/// previous one completes and is adopted, because its input (the accumulation buffer)
/// keeps growing as more `FRAME` events arrive. There is deliberately no cooperative
/// `cancel` flag here: `AtrousDenoiser` has no early-exit hook, so an abandoned
/// generation simply runs to completion on its own thread, consuming CPU but touching
/// nothing else -- correctness comes entirely from the structural key check in
/// [`adopt_ready_denoise`] below, never from timing.
pub(super) struct PendingDenoiseGeneration {
    /// The pose/geometry/resolution this generation's output is valid for; a result is
    /// only adopted once [`adopt_ready_denoise`] confirms this still equals the pose
    /// currently on screen. The buffer's sample count is deliberately not part of the
    /// key: a denoised frame a few samples behind the latest accumulation is still a
    /// valid (if slightly stale) image of the same pose.
    pub(super) key: GuideKey,
    /// Filled in by the background thread once, and only once, with the finished RGBA
    /// bytes. `None` while still in flight; callers must treat that as "not ready yet",
    /// never block waiting for it.
    result: Arc<Mutex<Option<Vec<u8>>>>,
}

/// Everything [`spawn_denoise_generation`] needs on its background thread, owned rather
/// than borrowed since that thread outlives this function's own call frame.
pub(super) struct DenoiseGenerationJob {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) samples_done: u32,
    pub(super) buffer: Vec<Vec3>,
    pub(super) guides: GuideBuffers,
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) planes: Vec<GpuFacetPlane>,
}

/// Kicks off one full denoise-and-tonemap pass on a background thread for `key`'s pose,
/// over the given `buffer`/`samples_done` snapshot and `guides` (already known-good for
/// `key` -- the caller confirms this via [`adopt_ready_guides`] before calling, and
/// clones the buffers out of `guide_cache` since that cache is otherwise only touched
/// from the Slint event-loop thread).
///
/// Runs via [`render_merged_frame`], fed a throwaway [`GuideCache`] pre-seeded with
/// `guides` so its internal `guide_cache.ensure` call is a guaranteed cache hit rather
/// than a synchronous regenerate. Builds its own fresh, throwaway [`AtrousDenoiser`]
/// and scratch buffers too -- this background thread owns nothing anyone else touches,
/// and one extra allocation every ~3.5s (4K) is immaterial next to the pass itself.
pub(super) fn spawn_denoise_generation(
    key: GuideKey,
    job: DenoiseGenerationJob,
) -> PendingDenoiseGeneration {
    let result = Arc::new(Mutex::new(None));
    let result_worker = Arc::clone(&result);
    let key_for_thread = key.clone();

    std::thread::spawn(move || {
        let mut guide_cache = GuideCache::new();
        guide_cache.adopt(key_for_thread, job.guides);
        let mut denoiser = AtrousDenoiser::new();
        let mut avg_color_buf = Vec::new();
        let mut filtered_buf = Vec::new();
        let bytes = render_merged_frame(
            AccumSnapshot {
                width: job.width,
                height: job.height,
                samples_done: job.samples_done,
                buffer: &job.buffer,
            },
            true,
            PoseAndGeometry {
                yaw: job.yaw,
                pitch: job.pitch,
                distance: job.distance,
                planes: &job.planes,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg_color_buf,
                filtered_buf: &mut filtered_buf,
            },
        );
        *result_worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(bytes);
    });

    PendingDenoiseGeneration { key, result }
}

/// Checks whether a background denoise pass is finished and still valid for
/// `desired_key`, taking (and returning) its bytes if so. `None` covers "nothing in
/// flight", "in flight for a different pose", and "in flight but not done yet" -- the
/// caller doesn't need to distinguish, since every fallback (show the last-adopted
/// frame, or a plain tonemap) is the same regardless.
///
/// Pure aside from the one `Mutex` lock on `pending`'s result slot, so it's directly
/// unit-testable with a hand-built [`PendingDenoiseGeneration`] and no real background
/// thread.
pub(super) fn adopt_ready_denoise(
    desired_key: &GuideKey,
    pending: Option<&PendingDenoiseGeneration>,
) -> Option<Vec<u8>> {
    let pending = pending?;
    if pending.key != *desired_key {
        return None;
    }
    let mut slot = pending
        .result
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    slot.take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::frame_cache::guide_pass::generate_guide_buffers;
    use indicatrix::geometry::cuts::StandardGemCuts;

    /// Builds a [`PendingGuideGeneration`] whose result is already sitting in its slot
    /// (as if the background thread had already finished), for `key`.
    fn ready_pending(key: GuideKey, buffers: GuideBuffers) -> PendingGuideGeneration {
        PendingGuideGeneration {
            key,
            cancel: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(Some(buffers))),
        }
    }

    /// Builds a [`PendingDenoiseGeneration`] whose result is already sitting in its slot
    /// (as if the background thread had already finished), for `key`.
    fn ready_denoise_pending(key: GuideKey, bytes: Vec<u8>) -> PendingDenoiseGeneration {
        PendingDenoiseGeneration {
            key,
            result: Arc::new(Mutex::new(Some(bytes))),
        }
    }

    /// Builds a [`PendingDenoiseGeneration`] that is still in flight -- no result yet --
    /// for `key`.
    fn in_flight_denoise_pending(key: GuideKey) -> PendingDenoiseGeneration {
        PendingDenoiseGeneration {
            key,
            result: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn adopt_ready_denoise_returns_none_when_nothing_is_pending() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let key = GuideCache::key_for(6, 5, 0.60, 0.45, 2.4, &planes);
        assert_eq!(adopt_ready_denoise(&key, None), None);
    }

    #[test]
    fn adopt_ready_denoise_returns_none_while_still_in_flight() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let key = GuideCache::key_for(6, 5, 0.60, 0.45, 2.4, &planes);
        let pending = in_flight_denoise_pending(key.clone());

        assert_eq!(
            adopt_ready_denoise(&key, Some(&pending)),
            None,
            "a generation that hasn't produced a result yet is 'not ready', not an error"
        );
    }

    #[test]
    fn adopt_ready_denoise_takes_the_result_once_ready_and_matching() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let key = GuideCache::key_for(6, 5, 0.60, 0.45, 2.4, &planes);
        let bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let pending = ready_denoise_pending(key.clone(), bytes.clone());

        assert_eq!(
            adopt_ready_denoise(&key, Some(&pending)),
            Some(bytes),
            "a finished generation for the exact pose on screen must be adopted"
        );
        assert_eq!(
            adopt_ready_denoise(&key, Some(&pending)),
            None,
            "the result is taken, not cloned -- a second read must not resurrect it"
        );
    }

    #[test]
    fn adopt_ready_denoise_rejects_a_result_for_a_different_pose() {
        let planes = StandardGemCuts::standard_round_brilliant();
        // Finished, but for a different yaw than the pose being rendered -- e.g. it
        // settled on the previous pose right as a new drag started.
        let stale_key = GuideCache::key_for(6, 5, 0.10, 0.45, 2.4, &planes);
        let pending = ready_denoise_pending(stale_key, vec![9u8, 9, 9, 9]);

        let current_key = GuideCache::key_for(6, 5, 0.60, 0.45, 2.4, &planes);

        assert_eq!(
            adopt_ready_denoise(&current_key, Some(&pending)),
            None,
            "a background result for a pose other than the one displayed must never be \
             adopted -- rejection is structural (a key comparison), not timing-dependent"
        );
    }

    #[test]
    fn a_superseded_generations_late_result_does_not_overwrite_newer_guides() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);

        // The cache already holds the current (newer) pose's guides, as if an earlier
        // redraw had already adopted them.
        let current_key = GuideCache::key_for(width, height, 0.90, 0.45, 2.4, &planes);
        let current_camera = Camera::new(0.90, 0.45, 2.4, 42.0);
        let current_buffers = generate_guide_buffers(width, height, &current_camera, &planes);
        let mut guide_cache = GuideCache::new();
        guide_cache.adopt(current_key.clone(), current_buffers.clone());

        // A generation for an older pose finally finishes late, after being superseded
        // (its cancel flag was set, but it raced past the last check anyway).
        let old_key = GuideCache::key_for(width, height, 0.10, 0.45, 2.4, &planes);
        let old_camera = Camera::new(0.10, 0.45, 2.4, 42.0);
        let old_buffers = generate_guide_buffers(width, height, &old_camera, &planes);
        let superseded = ready_pending(old_key, old_buffers);

        let adopted = adopt_ready_guides(&current_key, &mut guide_cache, Some(&superseded));

        assert!(
            adopted,
            "the cache already had the correct guides for the current pose"
        );
        assert!(
            guide_cache.matches_key(&current_key),
            "a superseded generation's late result for an older pose must never \
             overwrite the current pose's already-adopted guides"
        );
        assert_eq!(
            guide_cache
                .ensure(width, height, 0.90, 0.45, 2.4, &planes)
                .depth,
            current_buffers.depth,
            "the buffers in the cache must still be the current pose's, not the stale \
             generation's"
        );
    }
}
