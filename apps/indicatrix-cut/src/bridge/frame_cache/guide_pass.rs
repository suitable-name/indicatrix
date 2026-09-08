//! Local primary-ray-only guide-buffer prepass, so a remote-sourced image (radiance
//! only, no guide buffers over the wire) can still be denoised by the same
//! edge-avoiding À-Trous filter the local path uses.
//!
//! # Why this exists
//!
//! `crates/indicatrix/src/renderer/denoise` is edge-avoiding: it needs a first-hit
//! depth/normal/facet-id per pixel to decide which neighbours may blend together. The
//! local render loop gets those for free from `trace_spectral_ray`'s `primary_hit_out`
//! parameter; a remote worker's `FRAME`/`PREVIEW` payload carries only summed XYZ
//! radiance, so guide buffers never travel over the wire. Instead this module casts one
//! un-jittered camera ray per pixel and records its first hit via the same
//! [`intersect_polyhedron`] call `trace_spectral_ray` makes at bounce 0, without paying
//! for anything downstream (wavelengths, Stokes vectors, Fresnel splitting).
//!
//! # Why caching, and on what key
//!
//! The guide buffers depend only on camera pose (`yaw`/`pitch`/`distance`) and the
//! active facet geometry -- never on which backend produced the radiance, and never on
//! light direction, material, or exposure. [`GuideCache`] recomputes only when
//! [`GuideCache::ensure`]'s key (resolution + pose + a hash of the facet planes)
//! differs from the last call, not on every `FRAME` redraw of an in-progress
//! accumulation.
//!
//! # Measured cost, and why it runs off the UI thread
//!
//! One-time cost of a single [`generate_guide_buffers`] call, parallel across
//! `thread::available_parallelism` (`StandardGemCuts::standard_round_brilliant`):
//! ~19ms at 800x600, ~82ms at 1920x1080, ~288ms at 3840x2160 -- large enough at 4K to
//! freeze the UI thread if run synchronously from the Slint event loop.
//! `gui::remote::start_remote_render` therefore kicks the prepass off on a background
//! thread as soon as the `RenderRequest` is dispatched, via
//! [`generate_guide_buffers_cancellable`], overlapping it with the network round trip.
//! Cancellation is cooperative, via an `Arc<AtomicBool>` checked between rows: if the
//! pose changes again before a background generation finishes, `gui::remote` abandons
//! it and starts a fresh one. [`GuideCache::ensure`] stays synchronous -- it's what a
//! background result gets folded into ([`GuideCache::adopt`]) once `gui::remote`
//! confirms, via [`GuideCache::key_for`]/[`GuideCache::matches_key`], that it matches
//! the pose currently on screen. A frame arriving before its background generation
//! finishes is rendered with a plain tonemap instead; a later redraw denoises once
//! ready.

use crate::bridge::render_thread::hash_planes;
use glam::Vec3;
use indicatrix::{
    geometry::plane::GpuFacetPlane,
    optics::raytracer::{Camera, intersect_polyhedron},
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

/// One pixel-per-camera-ray depth/normal/facet-id capture, row-major
/// (`index = y * width + x`) -- exactly the shape `renderer::denoise::GBuffers` expects
/// for its `depth`/`normal`/`facet_id` fields.
#[derive(Debug, Clone)]
pub struct GuideBuffers {
    pub depth: Vec<f32>,
    pub normal: Vec<Vec3>,
    pub facet_id: Vec<i32>,
}

impl GuideBuffers {
    /// An all-miss buffer of the given size: `depth = 1.0e6` (matching
    /// `render_thread::update_accumulation_state`'s own "no hit yet" sentinel),
    /// `normal = ZERO`, `facet_id = -1`.
    fn miss(width: u32, height: u32) -> Self {
        let pixel_count = (width as usize) * (height as usize);
        Self {
            depth: vec![1.0e6; pixel_count],
            normal: vec![Vec3::ZERO; pixel_count],
            facet_id: vec![-1; pixel_count],
        }
    }
}

/// Casts one un-jittered camera ray per pixel and records its first hit's
/// depth/normal/facet index. Thin wrapper over [`generate_guide_buffers_cancellable`]
/// with a cancel flag that's never set, so it always runs to completion.
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
/// `thread::available_parallelism`, chunked by row like
/// `render_thread::render_frame_scanlines`.
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

/// Everything that determines the guide buffers: resolution, camera pose, and the
/// active facet geometry (light/material/exposure are deliberately excluded -- see the
/// module doc comment). `planes_hash` reuses `render_thread::hash_planes` rather than a
/// second, parallel implementation.
///
/// `pub`, not private: `gui::remote`'s background guide-generation path tags its result
/// with this same key (via [`GuideCache::key_for`]). Fields stay private so a
/// `GuideKey` can only be constructed via `key_for`, never hand-assembled with a
/// mismatched or stale hash.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideKey {
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    distance: f32,
    planes_hash: u64,
}

/// Caches one [`GuideBuffers`], regenerating it only when [`GuideKey`] changes. See the
/// module doc comment for why pose + geometry alone (not per-frame) is the right
/// invalidation granularity.
///
/// `buffers` is a plain (never-`Option`) field, deliberately: `key` starts `None`, so
/// the very first [`Self::ensure`] call always sees a "stale" key and populates
/// `buffers` before anything ever reads it -- there is no separate empty/uninitialized
/// state to unwrap out of, which is what lets `ensure` return a plain `&GuideBuffers`
/// with no panicking accessor.
#[derive(Debug)]
pub struct GuideCache {
    key: Option<GuideKey>,
    buffers: GuideBuffers,
    /// Incremented each time [`Self::ensure`] actually regenerates the buffers. Lets
    /// tests observe cache hits/misses without inspecting buffer contents; not read by
    /// production code.
    generation: u64,
}

impl Default for GuideCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GuideCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            key: None,
            buffers: GuideBuffers::miss(0, 0),
            generation: 0,
        }
    }

    /// How many times [`Self::ensure`] has actually regenerated the guide buffers.
    /// Test-only hook -- `#[cfg(test)]` rather than plain `pub` to avoid an
    /// `#[allow(dead_code)]` where nothing outside tests calls it.
    #[cfg(test)]
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the guide buffers valid for `(width, height, yaw, pitch, distance,
    /// planes)`, regenerating the primary-ray prepass only if that key differs from the
    /// last call -- an unchanged pose/gem on a subsequent call (e.g. a later `FRAME`
    /// event from the same in-progress remote render) is a cache hit and costs nothing
    /// beyond the key comparison.
    pub fn ensure(
        &mut self,
        width: u32,
        height: u32,
        yaw: f32,
        pitch: f32,
        distance: f32,
        planes: &[GpuFacetPlane],
    ) -> &GuideBuffers {
        let key = Self::key_for(width, height, yaw, pitch, distance, planes);
        if self.key.as_ref() != Some(&key) {
            let camera = Camera::new(yaw, pitch, distance, 42.0);
            self.buffers = generate_guide_buffers(width, height, &camera, planes);
            self.key = Some(key);
            self.generation += 1;
        }
        &self.buffers
    }

    /// Computes the [`GuideKey`] for `(width, height, yaw, pitch, distance, planes)` --
    /// the same identity [`Self::ensure`] uses internally. Exposed so a caller that
    /// generates guide buffers outside this cache (`gui::remote`'s background prepass)
    /// can tag its result with the exact key [`Self::matches_key`]/[`Self::adopt`] will
    /// compare against.
    #[must_use]
    pub fn key_for(
        width: u32,
        height: u32,
        yaw: f32,
        pitch: f32,
        distance: f32,
        planes: &[GpuFacetPlane],
    ) -> GuideKey {
        GuideKey {
            width,
            height,
            yaw,
            pitch,
            distance,
            planes_hash: hash_planes(planes),
        }
    }

    /// True if `key` already matches this cache's current contents, i.e. a
    /// [`Self::ensure`] call with the same key would be a cache hit. Lets a caller
    /// confirm the cache is "ready" for a given pose/geometry without risking
    /// `ensure`'s synchronous regenerate path.
    #[must_use]
    pub fn matches_key(&self, key: &GuideKey) -> bool {
        self.key.as_ref() == Some(key)
    }

    /// Adopts externally-computed guide buffers (a background
    /// [`generate_guide_buffers_cancellable`] result) as this cache's contents for
    /// `key`, without running the prepass or touching [`Self::generation`] (that
    /// counter tracks only this cache's own prepass runs). The caller is responsible
    /// for `buffers` actually matching `key` -- `gui::remote`'s async path only calls
    /// this after confirming the background result's key matches the pose currently on
    /// screen.
    pub fn adopt(&mut self, key: GuideKey, buffers: GuideBuffers) {
        self.key = Some(key);
        self.buffers = buffers;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::cuts::StandardGemCuts;

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
        let centre = (6 * 16 + 8) as usize;
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
                assert_eq!(guides.depth[i], 1.0e6);
                assert_eq!(guides.normal[i], Vec3::ZERO);
            }
        }
    }

    #[test]
    fn guide_cache_reuses_buffers_when_the_key_is_unchanged() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();

        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(cache.generation(), 1);

        // Same width/height/pose/geometry -- must be a cache hit (generation
        // unchanged), which is the whole "recompute on pose/gem change, not per
        // frame" contract this module exists for.
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(
            cache.generation(),
            1,
            "an unchanged pose/geometry must reuse the cached guide buffers"
        );
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(cache.generation(), 1);
    }

    #[test]
    fn guide_cache_regenerates_on_yaw_change() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        cache.ensure(8, 8, 0.90, 0.45, 2.4, &planes);
        assert_eq!(
            cache.generation(),
            2,
            "a changed yaw must invalidate the cache"
        );
    }

    #[test]
    fn guide_cache_regenerates_on_pitch_or_distance_change() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        cache.ensure(8, 8, 0.60, 0.80, 2.4, &planes);
        assert_eq!(
            cache.generation(),
            2,
            "a changed pitch must invalidate the cache"
        );

        cache.ensure(8, 8, 0.60, 0.80, 3.0, &planes);
        assert_eq!(
            cache.generation(),
            3,
            "a changed distance must invalidate the cache"
        );
    }

    #[test]
    fn guide_cache_regenerates_when_the_gem_geometry_changes() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &srb);
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &emerald);
        assert_eq!(
            cache.generation(),
            2,
            "a changed cutting schedule must invalidate the cache even with an \
             unchanged camera pose"
        );
    }

    #[test]
    fn guide_cache_regenerates_on_resolution_change() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        cache.ensure(16, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(
            cache.generation(),
            2,
            "a changed output resolution must invalidate the cache"
        );
    }

    /// The cache is keyed on camera pose and geometry ONLY -- light direction is not
    /// part of the key, because a primary-ray-only prepass never samples lighting at
    /// all. This isn't something a caller could get wrong by passing a light angle in
    /// (there is no such parameter), but it's worth pinning as the documented design
    /// decision: reusing guides across a light-only change must never regenerate them.
    #[test]
    fn ensure_signature_has_no_light_parameters() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(cache.generation(), 1);
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

    #[test]
    fn guide_cache_key_for_matches_what_ensure_uses_internally() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        let key = GuideCache::key_for(8, 8, 0.60, 0.45, 2.4, &planes);

        assert!(
            !cache.matches_key(&key),
            "a freshly-constructed cache must not match any key yet"
        );
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert!(
            cache.matches_key(&key),
            "the key ensure() just populated must equal key_for()'s independently \
             computed key for the identical pose/geometry"
        );
    }

    #[test]
    fn guide_cache_adopt_installs_externally_computed_buffers_without_recomputing() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
        let buffers = generate_guide_buffers(8, 8, &camera, &planes);
        let key = GuideCache::key_for(8, 8, 0.60, 0.45, 2.4, &planes);

        let mut cache = GuideCache::new();
        cache.adopt(key.clone(), buffers.clone());

        assert_eq!(
            cache.generation(),
            0,
            "adopt() folds in an externally-computed result -- it must not be counted \
             as this cache having run its own prepass"
        );
        assert!(cache.matches_key(&key));

        // A subsequent ensure() for the identical key must be a pure cache hit: same
        // generation, same (adopted) buffers, no recompute triggered.
        let cached = cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(cached.depth, buffers.depth);
        assert_eq!(cache.generation(), 0);
    }
}
