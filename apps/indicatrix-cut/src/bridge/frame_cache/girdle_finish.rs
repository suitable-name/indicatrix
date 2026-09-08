//! Cached girdle-facet classification for the frosted (bruted) girdle toggle.
//!
//! `indicatrix::geometry::girdle::girdle_facet_finishes` walks every plane in the active
//! design to decide which ones are the girdle band -- cheap once, but the render loop
//! calls into whichever finish set is active many times per second, and the classified
//! band only ever changes when the design's own geometry does. [`GirdleFinishCache`]
//! recomputes only when `render_thread::hash_planes`'s key actually differs from the
//! last call -- the same cheap `Pod`-bytes identity `bridge::guide_pass::GuideCache`
//! already keys part of its own invalidation on (see that module's doc comment),
//! reused here rather than a third, parallel geometry-identity scheme.

use crate::bridge::render_thread::hash_planes;
use indicatrix::{
    geometry::{girdle::girdle_facet_finishes, plane::GpuFacetPlane},
    optics::raytracer::FacetFinish,
};

/// `key` starts `None`, so the very first [`Self::ensure`] call always sees a "stale"
/// key and populates `finishes` before anything reads it -- same never-empty-but-still-
/// correct shape `bridge::guide_pass::GuideCache` uses for the identical reason (see
/// that type's doc comment).
#[derive(Debug, Default)]
pub struct GirdleFinishCache {
    key: Option<u64>,
    finishes: Vec<FacetFinish>,
}

impl GirdleFinishCache {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            key: None,
            finishes: Vec::new(),
        }
    }

    /// Returns the per-facet finishes for `planes` (girdle band `Frosted`, everything
    /// else `Polished`), reclassifying only when `planes`'s hash differs from the last
    /// call -- an unchanged design is a cache hit, costing nothing beyond the hash
    /// comparison.
    pub fn ensure(&mut self, planes: &[GpuFacetPlane]) -> &[FacetFinish] {
        let key = hash_planes(planes);
        if self.key != Some(key) {
            self.finishes = girdle_facet_finishes(planes);
            self.key = Some(key);
        }
        &self.finishes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::cuts::{STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, StandardGemCuts};

    #[test]
    fn ensure_matches_the_uncached_classification() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GirdleFinishCache::new();
        let cached = cache.ensure(&planes).to_vec();
        let direct = girdle_facet_finishes(&planes);
        assert_eq!(cached, direct);
        for i in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
            assert_eq!(cached[i], FacetFinish::Frosted, "index {i}");
        }
    }

    /// A second `ensure` call with the SAME planes must not reclassify -- verified
    /// indirectly here (no observable generation counter, unlike `GuideCache`, since
    /// there is nothing here expensive enough to be worth instrumenting) by confirming
    /// the result is unchanged and still correct after a repeat call.
    #[test]
    fn ensure_is_stable_across_repeated_calls_with_unchanged_planes() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GirdleFinishCache::new();
        let first = cache.ensure(&planes).to_vec();
        let second = cache.ensure(&planes).to_vec();
        assert_eq!(first, second);
    }

    /// A design change (different plane set) must reclassify, not keep serving the
    /// previous design's stale finishes.
    #[test]
    fn ensure_reclassifies_when_the_design_changes() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        let mut cache = GirdleFinishCache::new();
        let srb_finishes = cache.ensure(&srb).to_vec();
        let emerald_finishes = cache.ensure(&emerald).to_vec();
        assert_eq!(emerald_finishes, girdle_facet_finishes(&emerald));
        assert_ne!(
            srb_finishes.len(),
            emerald_finishes.len(),
            "SRB and emerald_cut have different plane counts, so a stale (un-reclassified) \
             result would show up as a length mismatch"
        );
    }
}
