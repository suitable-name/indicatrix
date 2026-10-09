//! Zoned-absorption helpers of [`GemMaterial`] (`zoning` feature only): the builder that installs
//! zones and the per-zone swatches the inspector shows.

use super::{AbsorptionUnit, GemMaterial};
use crate::{
    color::{Illuminant, body_colors},
    optics::zoning::ZonedAbsorption,
    render_setup::{MODEL_UNIT_FACE_UP_PATH, effective_stone_width_mm},
};

/// The girdle width in model units assumed by [`zone_swatches`] for a material that has not been
/// sized yet (`absorption_path_scale == 1.0`).
///
/// The built-in cuts have a girdle radius of order one model unit (`GemMaterial::scattering_sigma_s`'s "useful range" note), i.e. a width of about 2.
/// Only the swatch of an unsized material depends on it; a sized material (the render path) uses
/// its own `absorption_path_scale`.
pub const ZONE_SWATCH_FALLBACK_MODEL_WIDTH: f64 = 2.0;

impl GemMaterial {
    /// Installs `zoning` on this material.
    ///
    /// The material becomes [`AbsorptionUnit::PerMm`] and its plain
    /// [`absorption`](Self::absorption) is set to the base zone's tensor, so a build or consumer
    /// that ignores zones still shows the base zone (the adopt path relies on this). The tracer
    /// itself ignores `absorption` for a zoned material and uses the zones.
    #[must_use]
    pub fn with_zoning(mut self, zoning: ZonedAbsorption) -> Self {
        self.absorption = zoning.base.tensor.clone();
        self.absorption_unit = AbsorptionUnit::PerMm;
        self.zoning = Some(zoning);
        self
    }
}

/// The sRGB swatches (each channel in `[0, 1]`, D65) of a zoned material's zones at the face-up
/// path length: the base zone first, then the shaped zones in order. Empty for a material
/// without zones.
///
/// The path is the render's own rule (`MODEL_UNIT_FACE_UP_PATH` model units times the material's
/// `absorption_path_scale`, millimetres per model unit for a `PerMm` material), so the swatch is
/// the colour the stone shows face-up. A material that has not been sized yet
/// (`absorption_path_scale` exactly `1.0`) takes its millimetres per model unit from `width_mm`
/// instead (7 mm when `0.0`), assuming [`ZONE_SWATCH_FALLBACK_MODEL_WIDTH`].
#[must_use]
pub fn zone_swatches(material: &GemMaterial, width_mm: f32) -> Vec<[f32; 3]> {
    let Some(zoning) = material.zoning.as_ref() else {
        return Vec::new();
    };
    let scale = material.absorption_path_scale;
    let sized = scale.is_finite() && scale > 0.0 && scale.to_bits() != 1.0_f32.to_bits();
    let mm_per_unit = if sized {
        f64::from(scale)
    } else {
        f64::from(effective_stone_width_mm(width_mm)) / ZONE_SWATCH_FALLBACK_MODEL_WIDTH
    };
    let path_mm = f64::from(MODEL_UNIT_FACE_UP_PATH) * mm_per_unit;
    std::iter::once(&zoning.base)
        .chain(zoning.zones.iter().map(|zone| &zone.absorption))
        .map(|zone| {
            body_colors(&zone.tensor, path_mm, Illuminant::D65)
                .unpolarised
                .srgb
                .map(|v| f32::from(v) / 255.0)
        })
        .collect()
}
