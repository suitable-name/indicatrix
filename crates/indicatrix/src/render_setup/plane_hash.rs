//! Cheap facet-plane-set identity, shared by every per-design cache that only needs to
//! know whether the active design's geometry has changed since the last call.

use crate::geometry::{plane::GpuFacetPlane, tool::StoneGeometry};
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

/// [`hash_planes`] extended to a [`StoneGeometry`].
///
/// The tool bytes are fed after the plane bytes, and only when tools exist, so a planar
/// stone keeps exactly the key [`hash_planes`] gives it and no existing cache is
/// invalidated.
///
/// Tools are `Pod` like planes, so this is again one byte-slice hash; `DefaultHasher::new()`
/// has fixed keys (no `RandomState`), which keeps the key deterministic across runs.
#[must_use]
pub fn hash_geometry(geom: StoneGeometry<'_>) -> u64 {
    let bytes: &[u8] = bytemuck::cast_slice(geom.planes);
    let mut hasher = DefaultHasher::new();
    geom.planes.len().hash(&mut hasher);
    bytes.hash(&mut hasher);
    if !geom.tools.is_empty() {
        geom.tools.len().hash(&mut hasher);
        let tool_bytes: &[u8] = bytemuck::cast_slice(geom.tools);
        tool_bytes.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{cuts::StandardGemCuts, tool::ToolPrimitive};
    use glam::Vec3;

    #[test]
    fn hash_geometry_equals_hash_planes_when_there_are_no_tools() {
        for planes in [
            StandardGemCuts::standard_round_brilliant(),
            StandardGemCuts::emerald_cut(),
            Vec::new(),
        ] {
            assert_eq!(
                hash_geometry(StoneGeometry::planes_only(&planes)),
                hash_planes(&planes)
            );
        }
    }

    #[test]
    fn hash_geometry_changes_with_the_tools() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let a = [ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), 0.1)];
        let b = [ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), 0.2)];
        let geom = |tools| StoneGeometry {
            planes: &planes,
            tools,
        };
        assert_ne!(hash_geometry(geom(&a)), hash_planes(&planes));
        assert_ne!(hash_geometry(geom(&a)), hash_geometry(geom(&b)));
        assert_eq!(hash_geometry(geom(&a)), hash_geometry(geom(&a)));
    }

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
