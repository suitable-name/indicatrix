//! The mm-per-unit anchor ([`mm_per_unit`]) and the two figures downstream of
//! it/of geometry alone ([`volume_mm3`], [`volumetric_yield`],
//! [`carat_weight`]) -- see the parent module's doc comment for why `mast`
//! needed binding at all, and "Two numbers -- never conflate them" for the
//! exact/estimate distinction between the last two.

/// A model-unit measurement too small to trust as a real linear extent -- guards
/// [`mm_per_unit`]/[`super::fit::exceeds_preform`] against dividing by (or comparing
/// against) a collapsing/degenerate solid's near-zero width. Mast units are of order 1
/// (see `crate::preform`'s own module doc comment), so this is many orders of
/// magnitude below anything a real design would ever measure.
pub(super) const MIN_TRUSTED_WIDTH: f64 = 1e-9;

/// Millimetres per model ("mast") unit, anchored on the girdle diameter.
///
/// See this module's doc comment. `width_axis` is the FINISHED, solved design's own
/// measured
/// [`width_axis`](indicatrix::geometry::stone_metrics::SolidMetrics::width_axis) (not the
/// preform's) -- the model-unit girdle width `girdle_diameter_mm` is a real-world
/// measurement of.
///
/// `None` when there is nothing sane to compute from: a non-finite or non-positive
/// `girdle_diameter_mm`, or a `width_axis` too small to trust ([`MIN_TRUSTED_WIDTH`] --
/// a collapsing/degenerate solid has no meaningful width to anchor against).
#[must_use]
pub fn mm_per_unit(girdle_diameter_mm: f64, width_axis: f64) -> Option<f64> {
    if girdle_diameter_mm.is_finite() && girdle_diameter_mm > 0.0 && width_axis > MIN_TRUSTED_WIDTH
    {
        Some(girdle_diameter_mm / width_axis)
    } else {
        None
    }
}

/// Converts a mast-unit volume to mm^3 given a `mm_per_unit` scale factor.
///
/// Volume scales as the **cube** of a linear factor -- this is `volume_units *
/// mm_per_unit.powi(3)`, never `volume_units * mm_per_unit`. See this crate's
/// acceptance gate 3 (changing the girdle diameter must scale volume by the cube of
/// the ratio) for the property this specific exponent exists to satisfy.
#[must_use]
pub fn volume_mm3(volume_units: f64, mm_per_unit: f64) -> f64 {
    volume_units * mm_per_unit.powi(3)
}

/// Volumetric yield: finished solid volume over preform volume.
///
/// **Exact**, and independent of any mm scale or material data at all -- both
/// volumes live in the SAME model-unit coordinate system `crate::preform`'s own doc
/// comment describes, so
/// converting either (or both) to mm^3 would multiply numerator and denominator by
/// the identical `mm_per_unit^3` factor, which cancels in the ratio. This function
/// therefore takes plain, consistent-unit volumes (mast-unit^3 or mm^3, it does not
/// matter which, as long as both arguments use the same one) and never touches
/// [`mm_per_unit`] or any specific gravity -- see this module's doc comment ("Two
/// numbers -- never conflate them") and this crate's acceptance gate 2.
///
/// `None` when `preform_volume` is not positive (nothing to divide by).
#[must_use]
pub fn volumetric_yield(finished_volume: f64, preform_volume: f64) -> Option<f64> {
    (preform_volume > 0.0).then_some(finished_volume / preform_volume)
}

/// Estimated carat weight from a real mm^3 volume and a specific gravity (g/cm^3).
///
/// 1 carat = 0.2 g exactly, and SG is grams per **cm^3**, not mm^3 (1 cm^3 = 1000
/// mm^3), so `mass_g = (volume_mm3 / 1000) * specific_gravity` and `carat_weight =
/// mass_g / 0.2`, i.e. `volume_mm3 * specific_gravity / 200`.
///
/// **An estimate, never more precise than `specific_gravity` itself** -- see this
/// module's doc comment and `crate::material`'s own doc comment for how wide a
/// species' real SG range can be. Kept as a free function taking a plain
/// `specific_gravity: f64` (not a [`crate::material::MaterialSelection`]) so it stays
/// obviously independent of *which* material supplied that number -- exactly what
/// this crate's acceptance gate 2 (volumetric yield must not depend on SG)
/// needs [`volumetric_yield`] to prove by never calling this function at all.
#[must_use]
pub fn carat_weight(volume_mm3: f64, specific_gravity: f64) -> f64 {
    volume_mm3 * specific_gravity / 200.0
}
