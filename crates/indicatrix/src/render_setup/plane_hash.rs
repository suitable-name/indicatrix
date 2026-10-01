//! Cheap facet-plane-set identity, shared by every per-design cache that only needs to
//! know whether the active design's geometry has changed since the last call.

use crate::geometry::plane::GpuFacetPlane;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

/// Cheap identity for a facet-plane set: length plus a hash of the raw `Pod` bytes.
///
/// `GpuFacetPlane` is `bytemuck::Pod`, so this is just hashing a byte slice -- far
/// cheaper than whatever expensive per-design computation a caller guards with it
/// (gemological metrics, girdle-facet classification, girdle-width measurement).
#[must_use]
pub fn hash_planes(planes: &[GpuFacetPlane]) -> u64 {
    let bytes: &[u8] = bytemuck::cast_slice(planes);
    let mut hasher = DefaultHasher::new();
    planes.len().hash(&mut hasher);
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::cuts::StandardGemCuts;

    #[test]
    fn hash_planes_is_stable_for_identical_inputs() {
        let planes = StandardGemCuts::standard_round_brilliant();
        assert_eq!(hash_planes(&planes), hash_planes(&planes));
    }

    #[test]
    fn hash_planes_changes_with_the_facet_geometry() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        assert_ne!(hash_planes(&srb), hash_planes(&emerald));
    }

    #[test]
    fn hash_planes_of_an_empty_set_does_not_panic() {
        let empty: Vec<GpuFacetPlane> = Vec::new();
        let _ = hash_planes(&empty);
    }
}
