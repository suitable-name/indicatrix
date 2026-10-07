//! The face-up tone: the CIELAB colour of the light a stone returns to the observer at one
//! pose.
//!
//! Each visibly returned ray carries a Fresnel weight `w_i` and an internal path `L_i`
//! (model units). The mean transmittance spectrum of the returned light is the mixture
//! `T(lambda) = sum_i w_i exp(-alpha(lambda) s L_i) / sum_i w_i`, not Beer-Lambert of the
//! mean path (the mixture is lighter and less saturated, which is physically right: short
//! paths dominate the brightness). `s` is [`GemMaterial::absorption_path_scale`], the same
//! size rule the renderer uses.
//!
//! The mixture is linear in the rays, so [`ToneAccumulator`] only keeps a fixed
//! [`PATH_BINS`]-bin histogram of the weights over `0 ..= MAX_PATH_HALF_WIDTHS` girdle
//! half-widths (a longer path lands in the last bin); the spectrum is then summed over the
//! bin centres, independent of the ray count. The colorimetry runs under the lighting
//! preset's own light ([`ToneIlluminant`]).

use glam::Vec3;

use crate::{
    color::body_color::body_color_from_spectra,
    optics::{
        absorption::AbsorptionBand,
        materials::GemMaterial,
        raytracer::{
            EnvironmentSource, LightingPreset,
            environment::{d65_relative_spectral_power, environment_white_balance},
        },
    },
};

/// Number of path-length bins of the tone histogram.
pub(super) const PATH_BINS: usize = 128;

/// The histogram spans `0 ..= MAX_PATH_HALF_WIDTHS` girdle half-widths of internal path;
/// a ray that travels further lands in the last bin.
pub(super) const MAX_PATH_HALF_WIDTHS: f32 = 12.0;

/// The face-up colour of the light a stone returns to the eye at one pose.
///
/// Lab values are relative to the illuminant's own white, so a colourless stone is
/// neutral (`l_star` 100, `chroma` 0) under every lighting preset; the `srgb` swatch is
/// adapted to the screen's D65 the way the Live Render white-balances that preset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceUpTone {
    /// CIELAB lightness: 100 = colourless, 0 = black (no returned light at all).
    pub l_star: f32,
    /// CIELAB chroma `C*ab = sqrt(a*^2 + b*^2)`: the colour strength.
    pub chroma: f32,
    /// CIELAB hue angle in degrees, `0 ..= 360`; for display only (0 when `chroma < 0.5`).
    pub hue_deg: f32,
    /// The swatch: display sRGB of the returned light.
    pub srgb: [u8; 3],
    /// `sum w_i L_i / sum w_i`, in model units: how far the returned light travels
    /// through the stone on average (independent of the material and the size scale).
    pub mean_path_units: f32,
    /// `returned / total` rays, the same figure as `brilliance_pct / 100`.
    pub returned_fraction: f32,
}

impl FaceUpTone {
    /// No returned ray at all: black, every field zero.
    pub const NONE: Self = Self {
        l_star: 0.0,
        chroma: 0.0,
        hue_deg: 0.0,
        srgb: [0, 0, 0],
        mean_path_units: 0.0,
        returned_fraction: 0.0,
    };
}

/// The light the tone's colorimetry is taken under.
///
/// A lighting preset's own spectrum and white point, with a documented fall-back to D65
/// where the preset has no visible white point (the UV lamps) or no preset at all (an HDR
/// map).
///
/// Everything here is a pure function of the preset, so the same preset always gives the
/// same bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToneIlluminant {
    /// The preset whose light the tone is measured under.
    pub preset: LightingPreset,
    /// The tone is measured under D65 instead of the preset's own light (UV lamps, HDR
    /// maps: no visible white point).
    pub fallback_to_d65: bool,
}

impl ToneIlluminant {
    /// The illuminant of an environment: a studio preset is used as it is (UV lamps fall
    /// back to D65); an HDR map has no single white point and falls back to D65.
    #[must_use]
    pub const fn for_environment(environment: EnvironmentSource<'_>) -> Self {
        match environment {
            EnvironmentSource::Studio { preset, .. } => Self {
                preset,
                fallback_to_d65: preset.is_uv_lamp(),
            },
            EnvironmentSource::HdrMap(_) => Self {
                preset: LightingPreset::Daylight,
                fallback_to_d65: true,
            },
        }
    }

    /// Whether the tone is under the preset's own light (`false` when it fell back to
    /// D65); for the UI note.
    #[must_use]
    pub const fn uses_preset_light(&self) -> bool {
        !self.fallback_to_d65
    }

    /// Relative spectral power at `lambda_nm`: the D65 table when falling back or when the
    /// preset itself is a D65 one, else the preset's own Planckian fit.
    #[must_use]
    pub fn spectral_power(&self, lambda_nm: f64) -> f64 {
        if self.fallback_to_d65 || self.preset.uses_d65() {
            f64::from(d65_relative_spectral_power(lambda_nm as f32))
        } else {
            f64::from(self.preset.spectral_power(lambda_nm as f32))
        }
    }

    /// The per-cone von Kries scale that adapts the preset's white to the screen's D65, the
    /// renderer's own cached Bradford scale; `None` where the renderer applies none (the
    /// D65 presets) or the tone fell back to D65.
    #[must_use]
    pub fn display_adaptation(&self) -> Option<Vec3> {
        if !self.fallback_to_d65 && self.preset.uses_white_balance() {
            Some(environment_white_balance(self.preset.studio(1.0, 0.0, 0.0)))
        } else {
            None
        }
    }
}

/// Collects the returned rays of one evaluation into the path histogram, in grid order.
pub(super) struct ToneAccumulator {
    /// Fresnel weight summed per path-length bin.
    bins: [f32; PATH_BINS],
    /// `sum w_i L_i`.
    weighted_path: f32,
    /// `sum w_i`.
    weight_sum: f32,
    /// Visibly returned rays offered.
    returned: u32,
    /// Every traced ray (the denominator of the returned fraction).
    total: u32,
    /// Path length per bin, model units.
    bin_width: f32,
}

impl ToneAccumulator {
    /// An empty accumulator for a stone with the given girdle half-width (model units).
    pub(super) fn new(half_width: f32) -> Self {
        Self {
            bins: [0.0; PATH_BINS],
            weighted_path: 0.0,
            weight_sum: 0.0,
            returned: 0,
            total: 0,
            bin_width: MAX_PATH_HALF_WIDTHS * half_width / PATH_BINS as f32,
        }
    }

    /// Counts one traced ray (every classified ray, returned or not).
    pub(super) const fn count_total(&mut self) {
        self.total += 1;
    }

    /// Adds one returned ray with Fresnel weight `transmittance` and internal path
    /// `path_len` (model units).
    pub(super) fn offer(&mut self, transmittance: f32, path_len: f32) {
        let bin = ((path_len / self.bin_width) as usize).min(PATH_BINS - 1);
        self.bins[bin] += transmittance;
        self.weighted_path = path_len.mul_add(transmittance, self.weighted_path);
        self.weight_sum += transmittance;
        self.returned += 1;
    }

    /// Turns the histogram into the tone of `material` under `illuminant`.
    pub(super) fn finish(&self, material: &GemMaterial, illuminant: ToneIlluminant) -> FaceUpTone {
        let returned_fraction = self.returned as f32 / self.total.max(1) as f32;
        if self.weight_sum <= 0.0 {
            return FaceUpTone {
                returned_fraction,
                ..FaceUpTone::NONE
            };
        }

        // Occupied bins only: (normalised weight, bin-centre path scaled to absorption units).
        let scale = f64::from(material.absorption_path_scale.max(0.0));
        let weight_sum = f64::from(self.weight_sum);
        let occupied: Vec<(f64, f64)> = self
            .bins
            .iter()
            .enumerate()
            .filter(|&(_, &w)| w > 0.0)
            .map(|(b, &w)| {
                let centre = (b as f32 + 0.5) * self.bin_width;
                (f64::from(w) / weight_sum, f64::from(centre) * scale)
            })
            .collect();

        let alpha_of = |bands: &[AbsorptionBand], lambda: f64| -> f64 {
            f64::from(
                bands
                    .iter()
                    .map(|band| band.evaluate(lambda as f32))
                    .sum::<f32>(),
            )
            .max(0.0)
        };
        let mixture = |alpha: f64| -> f64 {
            occupied
                .iter()
                .fold(0.0, |acc, &(w, path)| w.mul_add((-alpha * path).exp(), acc))
        };

        let tensor = &material.absorption;
        // The unpolarised rule of `body_color::body_colors`.
        let t_fn = |lambda: f64| -> f64 {
            let t_o = mixture(alpha_of(&tensor.o_ray, lambda));
            tensor.beta_ray.as_ref().map_or_else(
                || {
                    if tensor.is_pleochroic {
                        let t_e = mixture(alpha_of(&tensor.e_ray, lambda));
                        2.0f64.mul_add(t_o, t_e) / 3.0
                    } else {
                        t_o
                    }
                },
                |beta_bands| {
                    let t_b = mixture(alpha_of(beta_bands, lambda));
                    let t_e = mixture(alpha_of(&tensor.e_ray, lambda));
                    (t_o + t_b + t_e) / 3.0
                },
            )
        };

        let colour = body_color_from_spectra(
            t_fn,
            |lambda| illuminant.spectral_power(lambda),
            illuminant.display_adaptation(),
        );

        let [l, a, b] = colour.lab;
        let chroma = a.hypot(b);
        let hue_deg = if chroma < 0.5 {
            0.0
        } else {
            let deg = b.atan2(a).to_degrees();
            if deg < 0.0 { deg + 360.0 } else { deg }
        };

        FaceUpTone {
            l_star: l as f32,
            chroma: chroma as f32,
            hue_deg: hue_deg as f32,
            srgb: colour.srgb,
            mean_path_units: self.weighted_path / self.weight_sum,
            returned_fraction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::body_color::{Illuminant, body_color},
        optics::materials::body_color::BODY_COLOR_PRESETS,
    };

    fn bits3(v: Vec3) -> [u32; 3] {
        v.to_array().map(f32::to_bits)
    }

    #[test]
    fn the_mixture_is_lighter_than_the_mean_path() {
        let material = GemMaterial::diamond().with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb);
        let mut acc = ToneAccumulator::new(1.0);
        for path in [0.5, 4.0] {
            acc.count_total();
            acc.offer(1.0, path);
        }
        let illuminant =
            ToneIlluminant::for_environment(LightingPreset::Daylight.studio(1.0, 0.85, 0.95));
        let tone = acc.finish(&material, illuminant);

        let alpha = |lambda: f64| -> f64 {
            f64::from(
                material
                    .absorption
                    .o_ray
                    .iter()
                    .map(|band| band.evaluate(lambda as f32))
                    .sum::<f32>(),
            )
        };
        let mean_path = body_color(alpha, 2.25, Illuminant::D65);
        assert!(
            f64::from(tone.l_star) > mean_path.lab[0],
            "mixture L* {} must exceed Beer-Lambert of the mean path {}",
            tone.l_star,
            mean_path.lab[0]
        );
        assert!((tone.mean_path_units - 2.25).abs() < 1e-5);
    }

    #[test]
    fn uv_lamps_and_hdr_maps_fall_back_to_d65() {
        let uv = ToneIlluminant::for_environment(LightingPreset::UvLamp365.studio(1.0, 0.85, 0.95));
        assert!(uv.fallback_to_d65);
        assert!(!uv.uses_preset_light());
        assert_eq!(
            uv.spectral_power(550.0).to_bits(),
            f64::from(d65_relative_spectral_power(550.0)).to_bits()
        );
        assert!(uv.display_adaptation().is_none());

        for preset in [LightingPreset::Daylight, LightingPreset::Incandescent] {
            let ill = ToneIlluminant::for_environment(preset.studio(1.0, 0.85, 0.95));
            assert!(!ill.fallback_to_d65);
            assert!(ill.uses_preset_light());
            assert_eq!(
                ill.spectral_power(550.0).to_bits(),
                f64::from(preset.spectral_power(550.0)).to_bits()
            );
        }

        for preset in [
            LightingPreset::Daylight,
            LightingPreset::IsoHemisphere,
            LightingPreset::DaylightDome,
            LightingPreset::UvLamp365,
            LightingPreset::UvLamp395,
        ] {
            let ill = ToneIlluminant::for_environment(preset.studio(1.0, 0.85, 0.95));
            assert!(ill.display_adaptation().is_none(), "{preset:?}");
        }
        for preset in [
            LightingPreset::Incandescent,
            LightingPreset::RingLights,
            LightingPreset::DarkSpotlight,
            LightingPreset::LightTent,
        ] {
            let ill = ToneIlluminant::for_environment(preset.studio(1.0, 0.85, 0.95));
            let expected = environment_white_balance(preset.studio(1.0, 0.85, 0.95));
            let got = ill.display_adaptation().expect("Planckian preset adapts");
            assert_eq!(bits3(got), bits3(expected), "{preset:?}");
        }
    }
}
