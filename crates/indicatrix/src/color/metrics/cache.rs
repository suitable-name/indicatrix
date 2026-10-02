//! The gemological-metrics cache: recomputing [`evaluate_gem_optical_metrics`] /
//! [`evaluate_angular_profile`] is expensive (single-threaded analytical raytracing), and
//! their results depend only on a handful of inputs that do not change between
//! progressive-accumulation samples -- see [`compute_or_reuse_metrics`].
//!
//! Shared by the desktop's render thread and the browser app's solve Worker, so both
//! recompute on exactly the same changes.

use super::{
    GemOpticalMetrics, evaluate_angular_profile, evaluate_gem_optical_metrics,
    lighting::{HEMISPHERE_DIRECTIONS, hemisphere_direction},
};
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, LightingPreset},
    },
    render_setup::hash_planes,
    renderer::env_map::EnvironmentMap,
};
use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write as _,
    hash::{Hash, Hasher},
};

/// A 64-bit fingerprint of everything a [`GemMaterial`] holds, so a cache key can stand for
/// the material without owning a copy of it.
///
/// The fingerprint hashes the material's `Debug` rendering, which prints every field
/// (floats in their shortest round-trip form, so two distinct `f32` values never print
/// alike) and therefore keeps covering a field added to the struct later. The hasher is
/// `DefaultHasher::new()`, whose keys are fixed, so the value is stable within a process.
/// A collision (about 2^-64 per pair of materials) would serve one material's metrics for
/// another.
fn material_fingerprint(material: &GemMaterial) -> u64 {
    /// Streams formatted text into a hasher without building the `String` first.
    struct HashWriter(DefaultHasher);

    impl std::fmt::Write for HashWriter {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0.write(text.as_bytes());
            Ok(())
        }
    }

    let mut writer = HashWriter(DefaultHasher::new());
    // Writing into a hasher cannot fail; the `Result` is only the `fmt::Write` contract.
    let _ = write!(writer, "{material:?}");
    writer.0.finish()
}

/// The part of an [`EnvironmentSource`] the metrics depend on.
///
/// The exposure and backdrop of a studio environment are left out: neither changes which
/// directions count as illuminated, so changing them must not recompute anything.
#[derive(Clone, Copy, PartialEq, Debug)]
enum EnvironmentKey {
    /// An analytic studio rig: the preset selects the radiance model, the pose places its
    /// lights.
    Studio {
        preset: LightingPreset,
        light_yaw: f32,
        light_pitch: f32,
    },
    /// A loaded HDR panorama, by [`map_fingerprint`].
    Hdr(u64),
}

impl EnvironmentKey {
    fn new(environment: EnvironmentSource<'_>) -> Self {
        match environment {
            EnvironmentSource::Studio {
                preset,
                light_yaw,
                light_pitch,
                ..
            } => Self::Studio {
                preset,
                light_yaw,
                light_pitch,
            },
            EnvironmentSource::HdrMap(map) => Self::Hdr(map_fingerprint(map)),
        }
    }
}

/// A 64-bit fingerprint of an HDR map: its size and the radiance it returns along the
/// upper-hemisphere directions the metrics probe it with, which is all of the map the
/// metrics read. Two maps differing only away from those directions would collide.
fn map_fingerprint(map: &EnvironmentMap) -> u64 {
    let mut hasher = DefaultHasher::new();
    map.width().hash(&mut hasher);
    map.height().hash(&mut hasher);
    for index in 0..HEMISPHERE_DIRECTIONS {
        for channel in map.radiance_rgb(hemisphere_direction(index)) {
            channel.to_bits().hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Inputs that fully determine `evaluate_gem_optical_metrics`.
///
/// The gemological metrics depend only on these -- not on exposure, target sample count,
/// dimensions, distance, etc. -- so they only need recomputing when one of these fields
/// actually changes. The lighting is part of the key: the same stone scores differently
/// under a different preset, light pose or HDR map.
#[derive(Clone, PartialEq, Debug)]
pub struct MetricsCacheKey {
    yaw: f32,
    pitch: f32,
    /// The environment the metrics were scored under (preset and light pose, or the map).
    environment: EnvironmentKey,
    /// [`material_fingerprint`] of the resolved material itself, NOT just its name -- a
    /// custom material can be edited in place and re-saved under the same name, changing
    /// the refractive index that drives `sin_crit` while leaving the name untouched.
    /// Keying on the name alone would serve stale metrics after such an edit. A
    /// fingerprint rather than a clone keeps the key (and each comparison of keys) small.
    material_fingerprint: u64,
    planes_hash: u64,
}

impl MetricsCacheKey {
    /// The key for one camera pose of a stone under `environment`.
    #[must_use]
    pub fn new(
        active_planes: &[GpuFacetPlane],
        material: &GemMaterial,
        yaw: f32,
        pitch: f32,
        environment: EnvironmentSource<'_>,
    ) -> Self {
        Self {
            yaw,
            pitch,
            environment: EnvironmentKey::new(environment),
            material_fingerprint: material_fingerprint(material),
            planes_hash: hash_planes(active_planes),
        }
    }

    /// The part of the key the angular profile depends on: the camera does not enter it
    /// (the profile sweeps the camera itself).
    fn same_profile_inputs(&self, other: &Self) -> bool {
        self.environment == other.environment
            && self.planes_hash == other.planes_hash
            && self.material_fingerprint == other.material_fingerprint
    }
}

/// The 19-point (5 degree step) angular profile: brilliance, extinction, windowing.
type Profile = ([f32; 19], [f32; 19], [f32; 19]);

/// Cached result of the (expensive, single-threaded) gemological metrics evaluation.
///
/// Keyed on the inputs that determine it, and reused across progressive-accumulation
/// frames where the camera, light, material, and geometry haven't moved.
/// The angular profile is cached separately, under the key it was computed for: it does
/// not depend on the camera, so orbiting recomputes only the pose metrics.
pub struct MetricsCache {
    key: MetricsCacheKey,
    metrics: GemOpticalMetrics,
    /// The profile and the key it was computed for; `None` until a caller asks for one.
    profile: Option<(MetricsCacheKey, Profile)>,
}

/// The cache entry for this pose, evaluating the pose metrics only when the entry on
/// hand was computed for other inputs. A profile already cached is kept either way: it is
/// only used when its own key still matches.
fn ensure_pose<'a>(
    slot: &'a mut Option<MetricsCache>,
    active_planes: &[GpuFacetPlane],
    current_mat: &GemMaterial,
    pose: [f32; 2],
    environment: EnvironmentSource<'_>,
) -> &'a mut MetricsCache {
    let [yaw, pitch] = pose;
    let key = MetricsCacheKey::new(active_planes, current_mat, yaw, pitch, environment);
    // An entry for other inputs is dropped first (keeping its profile), so the entry is
    // then either the matching one or empty, and one `get_or_insert_with` serves both.
    let kept_profile = if slot.as_ref().is_some_and(|cache| cache.key != key) {
        slot.take().and_then(|cache| cache.profile)
    } else {
        None
    };
    slot.get_or_insert_with(|| MetricsCache {
        metrics: evaluate_gem_optical_metrics(active_planes, current_mat, yaw, pitch, environment),
        key,
        profile: kept_profile,
    })
}

/// Evaluates (or reuses, from `metrics_cache`) the gemological metrics for one pose under
/// `environment`.
///
/// The angular profile is not touched; see [`compute_or_reuse_metrics`] for both.
pub fn compute_or_reuse_pose_metrics(
    metrics_cache: &mut Option<MetricsCache>,
    active_planes: &[GpuFacetPlane],
    current_mat: &GemMaterial,
    yaw: f32,
    pitch: f32,
    environment: EnvironmentSource<'_>,
) -> GemOpticalMetrics {
    ensure_pose(
        metrics_cache,
        active_planes,
        current_mat,
        [yaw, pitch],
        environment,
    )
    .metrics
}

/// Evaluates (or reuses, from `metrics_cache`) the gemological metrics and the angular
/// profile graphs for the current frame's inputs, scored under `environment`.
pub fn compute_or_reuse_metrics(
    metrics_cache: &mut Option<MetricsCache>,
    active_planes: &[GpuFacetPlane],
    current_mat: &GemMaterial,
    yaw: f32,
    pitch: f32,
    environment: EnvironmentSource<'_>,
) -> (GemOpticalMetrics, [f32; 19], [f32; 19], [f32; 19]) {
    let cache = ensure_pose(
        metrics_cache,
        active_planes,
        current_mat,
        [yaw, pitch],
        environment,
    );
    let profile: Profile = match &cache.profile {
        Some((key, profile)) if key.same_profile_inputs(&cache.key) => *profile,
        _ => {
            let profile = evaluate_angular_profile(active_planes, current_mat, environment);
            cache.profile = Some((cache.key.clone(), profile));
            profile
        }
    };
    (cache.metrics, profile.0, profile.1, profile.2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::cuts::StandardGemCuts;

    /// The environment the cache tests score under unless a test varies it.
    const fn studio() -> EnvironmentSource<'static> {
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
    }

    fn key_for(material: &GemMaterial, planes: &[GpuFacetPlane]) -> MetricsCacheKey {
        MetricsCacheKey::new(planes, material, 0.60, 0.45, studio())
    }

    /// The same stone and pose under another preset scores differently, so it must not
    /// be served from the entry of the first; exposure and backdrop change no score and
    /// must not invalidate it.
    #[test]
    fn metrics_cache_key_follows_the_lighting_preset_but_not_exposure_or_backdrop() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let diamond = GemMaterial::diamond();
        let key = |environment| MetricsCacheKey::new(&planes, &diamond, 0.60, 0.45, environment);

        assert_ne!(
            key(LightingPreset::RingLights.studio(1.0, 0.85, 0.95)),
            key(LightingPreset::LightTent.studio(1.0, 0.85, 0.95)),
            "a different lighting preset must invalidate the metrics cache"
        );
        assert_ne!(
            key(LightingPreset::RingLights.studio(1.0, 0.85, 0.95)),
            key(LightingPreset::RingLights.studio(1.0, 0.80, 0.95)),
            "a moved light must invalidate the metrics cache"
        );
        assert_eq!(
            key(LightingPreset::RingLights.studio(1.0, 0.85, 0.95)),
            key(LightingPreset::RingLights
                .studio(3.0, 0.85, 0.95)
                .with_backdrop(0.23)
                .with_surface_glare(0.0)),
            "exposure, backdrop and surface glare do not change the metrics"
        );
    }

    /// An HDR map is part of the key by content, and is not mistaken for a studio rig.
    #[test]
    fn metrics_cache_key_follows_the_hdr_map() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let diamond = GemMaterial::diamond();
        let dim = EnvironmentMap::uniform(8, 4, [0.1, 0.1, 0.1]);
        let bright = EnvironmentMap::uniform(8, 4, [2.0, 2.0, 2.0]);
        let key = |environment| MetricsCacheKey::new(&planes, &diamond, 0.60, 0.45, environment);

        let same_content = dim.clone();
        assert_eq!(
            key(EnvironmentSource::HdrMap(&dim)),
            key(EnvironmentSource::HdrMap(&same_content))
        );
        assert_ne!(
            key(EnvironmentSource::HdrMap(&dim)),
            key(EnvironmentSource::HdrMap(&bright))
        );
        assert_ne!(
            key(EnvironmentSource::HdrMap(&dim)),
            key_for(&diamond, &planes)
        );
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
            key_for(&low, &planes),
            key_for(&high, &planes),
            "editing a custom material's refractive index under the same name must invalidate the metrics cache"
        );
    }

    /// The cache must still hit when nothing has changed, or the per-frame saving is lost.
    #[test]
    fn metrics_cache_key_is_stable_for_identical_inputs() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let a = key_for(&GemMaterial::diamond(), &planes);
        let b = key_for(&GemMaterial::diamond(), &planes);
        assert_eq!(a, b, "identical inputs must produce an identical cache key");
    }

    /// Changing the cut must invalidate the cache even when the material is unchanged.
    #[test]
    fn metrics_cache_key_changes_with_the_facet_geometry() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        assert_ne!(
            key_for(&GemMaterial::diamond(), &srb),
            key_for(&GemMaterial::diamond(), &emerald),
            "different cutting instructions must invalidate the metrics cache"
        );
    }

    fn metric_bits(m: &GemOpticalMetrics) -> [u32; 5] {
        [
            m.brilliance_pct.to_bits(),
            m.fire_index.to_bits(),
            m.scintillation_pct.to_bits(),
            m.windowing_pct.to_bits(),
            m.extinction_pct.to_bits(),
        ]
    }

    fn profile_bits(p: &Profile) -> Vec<u32> {
        p.0.iter()
            .chain(&p.1)
            .chain(&p.2)
            .map(|v| v.to_bits())
            .collect()
    }

    /// Whatever the cache reuses, the answer is what the direct calls give -- across an
    /// orbit (profile reused), a light move (profile recomputed) and a repeated pose.
    #[test]
    fn cached_answers_equal_the_direct_calls() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let mut cache = None;
        for &(yaw, pitch, preset, light_yaw, light_pitch) in &[
            (0.60, 0.45, LightingPreset::RingLights, 0.85, 0.95),
            (0.61, 0.46, LightingPreset::RingLights, 0.85, 0.95),
            (0.61, 0.46, LightingPreset::RingLights, 0.80, 0.90),
            (0.61, 0.46, LightingPreset::RingLights, 0.80, 0.90),
            (0.61, 0.46, LightingPreset::LightTent, 0.80, 0.90),
            (0.60, 0.45, LightingPreset::RingLights, 0.85, 0.95),
        ] {
            let environment = preset.studio(1.0, light_yaw, light_pitch);
            let (m, b, e, w) =
                compute_or_reuse_metrics(&mut cache, &planes, &material, yaw, pitch, environment);
            let direct = evaluate_gem_optical_metrics(&planes, &material, yaw, pitch, environment);
            let profile = evaluate_angular_profile(&planes, &material, environment);
            assert_eq!(metric_bits(&m), metric_bits(&direct));
            assert_eq!(profile_bits(&(b, e, w)), profile_bits(&profile));
        }
    }

    /// The pose-only call leaves the profile out, and a later full call fills it in
    /// without redoing the pose.
    #[test]
    fn the_pose_only_call_and_the_full_call_share_one_cache() {
        let planes = StandardGemCuts::emerald_cut();
        let material = GemMaterial::diamond();
        let mut cache = None;
        let environment = LightingPreset::RingLights.studio(1.0, 0.8, 0.9);
        let pose =
            compute_or_reuse_pose_metrics(&mut cache, &planes, &material, 0.6, 0.4, environment);
        assert!(cache.as_ref().is_some_and(|c| c.profile.is_none()));
        let (m, ..) =
            compute_or_reuse_metrics(&mut cache, &planes, &material, 0.6, 0.4, environment);
        assert_eq!(metric_bits(&pose), metric_bits(&m));
        assert!(cache.as_ref().is_some_and(|c| c.profile.is_some()));
    }
}
