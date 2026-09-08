//! Cached model-space girdle width for the "Stone size" absorption-scale control.
//!
//! `apply_material_overrides` needs the design's real girdle width in model units
//! every time `stone_width_mm` (see `RenderContext::stone_width_mm`) is dialled in, so
//! it can turn a physical millimetre width into `GemMaterial::absorption_path_scale`.
//! `indicatrix::geometry::stone_metrics::measure_solid` is not free (it enumerates every
//! feasible plane-triple intersection of the active design), and the render loop calls
//! `apply_material_overrides` every frame, so [`StoneWidthCache`] recomputes only when
//! `render_thread::hash_planes`'s key actually differs from the last call -- the exact
//! same cheap `Pod`-bytes identity scheme `GirdleFinishCache` (see that module's doc
//! comment) already uses for the identical reason.

use crate::bridge::render_thread::hash_planes;
use glam::DVec3;
use indicatrix::geometry::{plane::GpuFacetPlane, stone_metrics::measure_solid};

/// `key` starts `None`, so the very first [`Self::ensure`] call always sees a "stale"
/// key and measures before anything reads `width` -- same never-empty-but-still-
/// correct shape `GirdleFinishCache` uses for the identical reason (see that type's
/// doc comment).
#[derive(Debug, Default)]
pub struct StoneWidthCache {
    key: Option<u64>,
    width: Option<f64>,
}

impl StoneWidthCache {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            key: None,
            width: None,
        }
    }

    /// Returns the design's girdle width in model units (`SolidMetrics::width_axis`
    /// of the solid bounded by `planes`), or `None` when the plane arrangement doesn't
    /// bound a measurable solid (e.g. an in-progress custom design missing its closing
    /// planes) -- `apply_material_overrides` treats `None` the same as the control
    /// being off, leaving the material's `absorption_path_scale` untouched. Reclassifies
    /// only when `planes`'s hash differs from the last call, matching
    /// `GirdleFinishCache::ensure`.
    pub fn ensure(&mut self, planes: &[GpuFacetPlane]) -> Option<f64> {
        let key = hash_planes(planes);
        if self.key != Some(key) {
            self.width = measure_model_width(planes);
            self.key = Some(key);
        }
        self.width
    }
}

/// Converts `planes` (`GpuFacetPlane { normal, d }`, whose inside half-space is
/// `n . x + d <= 0`) into `measure_solid`'s own `n . x <= m` convention (`m = -d`),
/// then measures the resulting solid's axis-aligned girdle width.
fn measure_model_width(planes: &[GpuFacetPlane]) -> Option<f64> {
    let converted: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|p| {
            (
                DVec3::new(
                    f64::from(p.normal[0]),
                    f64::from(p.normal[1]),
                    f64::from(p.normal[2]),
                ),
                -f64::from(p.d),
            )
        })
        .collect();
    measure_solid(&converted).map(|m| m.width_axis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::cuts::StandardGemCuts;

    #[test]
    fn ensure_measures_the_standard_round_brilliant_girdle_width() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = StoneWidthCache::new();
        let width = cache.ensure(&planes).expect("SRB must measure");
        assert!(width > 0.0, "width {width}");
    }

    /// A second `ensure` call with the SAME planes must not remeasure -- verified
    /// indirectly (no observable generation counter, same treatment
    /// `GirdleFinishCache`'s own repeat-call test gives this) by confirming the result
    /// is unchanged and still correct after a repeat call.
    #[test]
    fn ensure_is_stable_across_repeated_calls_with_unchanged_planes() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = StoneWidthCache::new();
        let first = cache.ensure(&planes);
        let second = cache.ensure(&planes);
        assert_eq!(first, second);
    }

    /// A design change (different plane set) must remeasure, not keep serving the
    /// previous design's stale width.
    #[test]
    fn ensure_remeasures_when_the_design_changes() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        let mut cache = StoneWidthCache::new();
        let srb_width = cache.ensure(&srb);
        let emerald_width = cache.ensure(&emerald);
        assert_eq!(emerald_width, measure_model_width(&emerald));
        assert_ne!(
            srb_width, emerald_width,
            "SRB and emerald_cut have different girdle widths, so a stale \
             (un-remeasured) result would show up as an equal width"
        );
    }
}
