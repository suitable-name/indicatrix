//! The GUI's cache over the primary-ray-only guide-buffer prepass, so a remote-sourced
//! image (radiance only, no guide buffers over the wire) can still be denoised by the
//! same edge-avoiding À-Trous filter the local path uses.
//!
//! # Where the prepass lives
//!
//! The prepass itself ([`generate_guide_buffers`], [`generate_guide_buffers_cancellable`],
//! [`GuideBuffers`]) is `indicatrix::renderer::guide_pass` -- moved there unchanged so a
//! coordinator can compute the identical guides for its denoised `DISPLAY_FRAME`s -- and
//! re-exported here, so every GUI call site keeps this module's path. Only the cache
//! stays GUI-side: its key hashes the planes with `render_thread::hash_planes`, which the
//! GUI's other frame caches share.
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
//! # Why it runs off the UI thread
//!
//! One prepass costs ~82ms at 1920x1080 and ~288ms at 3840x2160 (see the `indicatrix`
//! module) -- large enough at 4K to freeze the UI thread if run synchronously from the
//! Slint event loop. `gui::remote::start_remote_render` therefore kicks the prepass off
//! on a background thread as soon as the `RenderRequest` is dispatched, via
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
use indicatrix::{geometry::plane::GpuFacetPlane, optics::raytracer::Camera};

// The pure prepass moved to `indicatrix::renderer::guide_pass` (so a coordinator can
// compute the same guides for its denoised display frames); re-exported so every GUI
// call site keeps its `bridge::frame_cache::guide_pass::...` path.
pub use indicatrix::renderer::guide_pass::{
    GuideBuffers, generate_guide_buffers, generate_guide_buffers_cancellable,
};

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
