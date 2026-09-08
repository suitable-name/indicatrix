//! The gemological-metrics cache: recomputing `evaluate_gem_optical_metrics`/
//! `evaluate_angular_profile` is expensive (single-threaded analytical raytracing), and
//! its result depends only on a handful of inputs that don't change between
//! progressive-accumulation samples -- see [`compute_or_reuse_metrics`].

use indicatrix::{
    color::metrics::{GemOpticalMetrics, evaluate_angular_profile, evaluate_gem_optical_metrics},
    geometry::plane::GpuFacetPlane,
    optics::materials::GemMaterial,
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

/// Cheap identity for a facet-plane set: length plus a hash of the raw `Pod` bytes.
/// `GpuFacetPlane` is `bytemuck::Pod`, so this is just hashing a byte slice -- far
/// cheaper than the analytical raytracing it guards against re-running.
///
/// `pub`, not private: `bridge::guide_pass::GuideCache` reuses this exact hash as part
/// of its own cache-invalidation key rather than a second, parallel implementation.
pub fn hash_planes(planes: &[GpuFacetPlane]) -> u64 {
    let bytes: &[u8] = bytemuck::cast_slice(planes);
    let mut hasher = DefaultHasher::new();
    planes.len().hash(&mut hasher);
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Inputs that fully determine `evaluate_gem_optical_metrics` / `evaluate_angular_profile`.
/// The gemological metrics and the angular profile graphs depend only on these -- not on
/// exposure, target sample count, dimensions, distance, etc. -- so they only need
/// recomputing when one of these fields actually changes.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct MetricsCacheKey {
    yaw: f32,
    pitch: f32,
    light_yaw: f32,
    light_pitch: f32,
    /// The resolved material itself, NOT just its name -- a custom material can be
    /// edited in place and re-saved under the same name, changing the refractive index
    /// that drives `sin_crit` while leaving the name untouched. Keying on the name
    /// alone would serve stale metrics after such an edit.
    material: GemMaterial,
    planes_hash: u64,
}

/// Cached result of the (expensive, single-threaded) gemological metrics evaluation,
/// keyed on the inputs that determine it. Reused across progressive-accumulation frames
/// where the camera, light, material, and geometry haven't moved.
pub(super) struct MetricsCache {
    key: MetricsCacheKey,
    metrics: GemOpticalMetrics,
    graph_brilliance: [f32; 19],
    graph_extinction: [f32; 19],
    graph_windowing: [f32; 19],
}

/// Evaluates (or reuses, from `metrics_cache`) the gemological metrics and angular
/// profile graphs for the current frame's inputs.
pub(super) fn compute_or_reuse_metrics(
    metrics_cache: &mut Option<MetricsCache>,
    active_planes: &[GpuFacetPlane],
    current_mat: &GemMaterial,
    yaw: f32,
    pitch: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> (GemOpticalMetrics, [f32; 19], [f32; 19], [f32; 19]) {
    let cache_key = MetricsCacheKey {
        yaw,
        pitch,
        light_yaw,
        light_pitch,
        material: current_mat.clone(),
        planes_hash: hash_planes(active_planes),
    };
    match metrics_cache {
        Some(cache) if cache.key == cache_key => (
            cache.metrics,
            cache.graph_brilliance,
            cache.graph_extinction,
            cache.graph_windowing,
        ),
        _ => {
            let metrics = evaluate_gem_optical_metrics(
                active_planes,
                current_mat,
                yaw,
                pitch,
                light_yaw,
                light_pitch,
            );
            let (graph_brilliance, graph_extinction, graph_windowing) =
                evaluate_angular_profile(active_planes, current_mat, light_yaw, light_pitch);
            *metrics_cache = Some(MetricsCache {
                key: cache_key,
                metrics,
                graph_brilliance,
                graph_extinction,
                graph_windowing,
            });
            (metrics, graph_brilliance, graph_extinction, graph_windowing)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::cuts::StandardGemCuts;

    fn key_for(material: GemMaterial, planes: &[GpuFacetPlane]) -> MetricsCacheKey {
        MetricsCacheKey {
            yaw: 0.60,
            pitch: 0.45,
            light_yaw: 0.85,
            light_pitch: 0.95,
            material,
            planes_hash: hash_planes(planes),
        }
    }

    /// A custom material can be edited in place and re-saved under the SAME name. The
    /// refractive index drives `sin_crit`, so a key capturing only the material name
    /// would hit the cache and serve stale numbers after such an edit.
    #[test]
    fn metrics_cache_key_distinguishes_same_named_material_with_different_optics() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let low = GemMaterial::new_custom("MyGem", 1.50, 0.010, 0.0, [0.0, 0.0, 0.0]);
        let high = GemMaterial::new_custom("MyGem", 2.42, 0.044, 0.0, [0.0, 0.0, 0.0]);

        assert_eq!(
            low.name, high.name,
            "test premise: the names must be identical"
        );
        assert_ne!(
            key_for(low, &planes),
            key_for(high, &planes),
            "editing a custom material's refractive index under the same name must invalidate the metrics cache"
        );
    }

    /// The cache must still hit when nothing has changed, or the per-frame saving is lost.
    #[test]
    fn metrics_cache_key_is_stable_for_identical_inputs() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let a = key_for(GemMaterial::diamond(), &planes);
        let b = key_for(GemMaterial::diamond(), &planes);
        assert_eq!(a, b, "identical inputs must produce an identical cache key");
    }

    /// Changing the cut must invalidate the cache even when the material is unchanged.
    #[test]
    fn metrics_cache_key_changes_with_the_facet_geometry() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        assert_ne!(
            key_for(GemMaterial::diamond(), &srb),
            key_for(GemMaterial::diamond(), &emerald),
            "a different cutting schedule must invalidate the metrics cache"
        );
    }
}
