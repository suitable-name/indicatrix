//! Pure model-space girdle-width measurement for the "Stone size" absorption-scale
//! control ([`crate::render_setup::apply_material_overrides`]'s `stone_width_mm`
//! override).
//!
//! [`measure_model_width`] itself is cheap to call but not free -- it enumerates every
//! feasible plane-triple intersection of the active design ([`crate::geometry::
//! stone_metrics::measure_solid`]) -- so a caller that needs it every frame (the
//! desktop's live render loop) should still cache the result keyed on
//! [`crate::render_setup::hash_planes`], the way `StoneWidthCache`
//! (`apps/indicatrix-cut/src/bridge/frame_cache/stone_width.rs`) does. That cache
//! itself stays desktop-side rather than moving here: it is plain per-call state, not
//! render physics, and nothing about it needs to be wasm-specific -- this function is
//! the part every renderer built on this crate needs to agree on.

use crate::geometry::{plane::GpuFacetPlane, stone_metrics::measure_solid};
use glam::DVec3;

/// Measures the design's axis-aligned girdle width in model units.
///
/// Converts `planes` (`GpuFacetPlane { normal, d }`, whose inside half-space is
/// `n . x + d <= 0`) into `measure_solid`'s own `n . x <= m` convention (`m = -d`),
/// then measures the resulting solid. `None` when the plane arrangement doesn't bound
/// a measurable solid (e.g. an in-progress custom design missing its closing planes)
/// -- callers treat that the same as the stone-size control being off, leaving the
/// material's `absorption_path_scale` untouched.
#[must_use]
pub fn measure_model_width(planes: &[GpuFacetPlane]) -> Option<f64> {
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
    use crate::geometry::cuts::StandardGemCuts;

    #[test]
    fn measures_the_standard_round_brilliant_girdle_width() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let width = measure_model_width(&planes).expect("SRB must measure");
        assert!(width > 0.0, "width {width}");
    }

    #[test]
    fn differs_between_two_different_designs() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        assert_ne!(measure_model_width(&srb), measure_model_width(&emerald));
    }

    #[test]
    fn an_empty_plane_set_does_not_measure() {
        assert_eq!(measure_model_width(&[]), None);
    }
}
