//! Environment/lighting sources a ray can sample when it misses the gemstone.
//!
//! The analytic gemological studio rig ([`LightingPreset`], [`sample_studio_environment`])
//! and the loaded-HDR-panorama alternative ([`EnvironmentSource::HdrMap`]).
//!
//! # Lighting models
//!
//! Presets map to one of four [`LightingModel`] variants:
//! - `Studio`: four classic studio setups (`Daylight`, `Incandescent`, `RingLights`, `DarkSpotlight`),
//!   bit-identical to the original analytic rig.
//! - `IsoHemisphere`: uniform lit upper hemisphere with zenith cosine gradient (0.70-1.0), observer head shadow, 4° soft horizon, dark below.
//! - `SoftDome`: soft hemisphere dome (0.02 max, 0.005 ground) with directional key, fill, and ring pinpoints.
//! - `DaylightDome`: sky hemisphere (0.05 max, 0.005 ground) with sun disc (16.0) and solar aureole (1.5).

use super::color::illuminant_white_balance;
use crate::renderer::env_map::EnvironmentMap;
use glam::Vec3;

/// Tabulated CIE Standard Illuminant D65 relative spectral power distribution,
/// 380-780nm at 10nm intervals (41 values), as published in CIE 15:2004 "Colorimetry,"
/// 3rd ed., Table T.3. D65 is the standard's own measurement of "average daylight," NOT
/// a 6500K Planckian blackbody curve -- it has real irregularities (most visibly a
/// broad peak/dip through the blue-green region) no smooth Planckian curve reproduces.
/// See [`d65_relative_spectral_power`], used by the `"D65 Daylight"` [`LightingPreset`]
/// instead of [`blackbody_spectrum`] at 6500K.
const CIE_D65_SPD_380_780_10NM: [f32; 41] = [
    49.9755, 54.6482, 82.7549, 91.4860, 93.4318, 86.6823, 104.865, 117.008, 117.812, 114.861,
    115.923, 108.811, 109.354, 107.802, 104.790, 107.689, 104.405, 104.046, 100.000, 96.3342,
    95.7880, 88.6856, 90.0062, 89.5991, 87.6987, 83.2886, 83.6992, 80.0268, 80.2146, 82.2778,
    78.2842, 69.7213, 71.6091, 74.3496, 61.6045, 69.8856, 75.0870, 63.5928, 46.4182, 66.8054,
    63.3828,
];

/// Linearly-interpolated CIE D65 relative spectral power at `lambda_nm`.
///
/// Normalized so that 560nm (the table's own `100.000` entry) reads exactly `1.0`,
/// matching [`blackbody_spectrum`]'s own normalization point -- both curves agree at
/// 560nm by construction, keeping the "D65 Daylight" preset's overall exposure/white
/// point unchanged relative to the old blackbody approximation.
///
/// Wavelengths outside the tabulated 380-780nm range clamp to the nearest table edge
/// (a defensive bound: this renderer's own channel sampling never leaves 380-780nm).
#[must_use]
pub fn d65_relative_spectral_power(lambda_nm: f32) -> f32 {
    const START_NM: f32 = 380.0;
    const STEP_NM: f32 = 10.0;
    const LAST_INDEX: usize = CIE_D65_SPD_380_780_10NM.len() - 1;

    let clamped = lambda_nm.clamp(START_NM, STEP_NM.mul_add(LAST_INDEX as f32, START_NM));
    let position = (clamped - START_NM) / STEP_NM;
    let index0 = (position.floor() as usize).min(LAST_INDEX - 1);
    let index1 = index0 + 1;
    let frac = position - index0 as f32;

    let v0 = CIE_D65_SPD_380_780_10NM[index0];
    let v1 = CIE_D65_SPD_380_780_10NM[index1];
    frac.mul_add(v1 - v0, v0) / 100.0
}

/// Physical Planck Blackbody Spectral Radiance S(lambda, T) normalized to 1.0 at 560nm
#[must_use]
pub fn blackbody_spectrum(lambda_nm: f32, temp_k: f32) -> f32 {
    let t_k = temp_k.max(1000.0);
    let h_c_k = 14_388_000.0_f32; // hc / k_B in nm * K
    let exp_val = (h_c_k / (lambda_nm * t_k)).min(80.0).exp();
    let exp_560 = (h_c_k / (560.0 * t_k)).min(80.0).exp();
    let denom = (exp_val - 1.0).max(1e-6);
    let denom_560 = (exp_560 - 1.0).max(1e-6);
    let ratio = denom_560 / denom;
    ((560.0 / lambda_nm).powi(5) * ratio).clamp(0.01, 20.0)
}

/// Colour temperature and rig-intensity parameters for one named studio lighting preset.
///
/// Returned by [`LightingPreset::params`] -- the single lookup both
/// `sample_studio_environment` (which lights the traced image) and
/// `illuminant_temperature_k` (which derives the von-Kries white balance for that same
/// image) share, so the two cannot independently drift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingRigParams {
    /// Blackbody colour temperature in Kelvin, fed to [`blackbody_spectrum`].
    pub temp_k: f32,
    /// Multiplier on the key-softbox and ring-emitter intensity terms (does not affect
    /// the fill light or the ambient backdrop).
    pub spot_mult: f32,
}

/// The gemological studio lighting rig presets, as a closed, exhaustively-matched set
/// of variants.
///
/// An unrecognised preset is not representable, unlike a `&str`-keyed lookup where a
/// caller could pass a string that silently falls through to a default.
///
/// `Daylight` is index `0` / the [`Default`], and is what any legacy or unrecognised
/// persisted label -- including the old, mislabelled `"D65 Daylight (5500K)"` string --
/// migrates to via [`Self::from_label`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LightingPreset {
    #[default]
    Daylight,
    Incandescent,
    RingLights,
    DarkSpotlight,
    IsoHemisphere,
    SoftDome,
    DaylightDome,
}

/// Which environment the preset samples -- see this module's "Lighting models" doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LightingModel {
    Studio,
    IsoHemisphere,
    SoftDome,
    DaylightDome,
}

impl LightingModel {
    /// The `u32` discriminant bound in `GpuTransportParams::studio_model`.
    #[must_use]
    pub const fn gpu_id(self) -> u32 {
        match self {
            Self::Studio => 0,
            Self::IsoHemisphere => 1,
            Self::SoftDome => 2,
            Self::DaylightDome => 3,
        }
    }
}

impl LightingPreset {
    /// All seven presets, in the same order as their UI index / the `lighting_options`
    /// combo box list (`app.slint`).
    pub const ALL: [Self; 7] = [
        Self::Daylight,
        Self::Incandescent,
        Self::RingLights,
        Self::DarkSpotlight,
        Self::IsoHemisphere,
        Self::SoftDome,
        Self::DaylightDome,
    ];

    /// This preset's colour temperature and rig-intensity multiplier -- the single
    /// source of truth both `sample_studio_environment` and `illuminant_temperature_k`
    /// read from. See the type's doc comment for why that matters.
    #[must_use]
    pub const fn params(self) -> LightingRigParams {
        match self {
            Self::Incandescent => LightingRigParams {
                temp_k: 3200.0,
                spot_mult: 1.2,
            },
            Self::RingLights => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.6,
            },
            Self::DarkSpotlight => LightingRigParams {
                temp_k: 6000.0,
                spot_mult: 2.4,
            },
            Self::SoftDome => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.0,
            },
            Self::Daylight | Self::IsoHemisphere | Self::DaylightDome => LightingRigParams {
                temp_k: 6500.0,
                spot_mult: 1.0,
            },
        }
    }

    /// The corrected user-facing display label: D65 daylight is 6500K, not the
    /// `"5500K"` the UI previously (and inconsistently with the actually-rendered
    /// colour) displayed.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Daylight => "D65 Daylight (6500K)",
            Self::Incandescent => "Incandescent (3200K)",
            Self::RingLights => "Gem Studio Ring Lights",
            Self::DarkSpotlight => "Dramatic Dark Spotlight",
            Self::IsoHemisphere => "ISO hemisphere",
            Self::SoftDome => "Soft dome + ring lights",
            Self::DaylightDome => "Daylight dome + sun",
        }
    }

    /// Parses a persisted or UI-supplied label back into a preset. Falls back to
    /// [`Self::Daylight`] for anything unrecognised -- including the legacy
    /// `"D65 Daylight (5500K)"` label an older settings file may still contain, which
    /// already resolved to D65 6500K, so migration is silent.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label {
            "Incandescent (3200K)" => Self::Incandescent,
            "Gem Studio Ring Lights" => Self::RingLights,
            "Dramatic Dark Spotlight" => Self::DarkSpotlight,
            "ISO hemisphere" => Self::IsoHemisphere,
            "Soft dome + ring lights" => Self::SoftDome,
            "Daylight dome + sun" => Self::DaylightDome,
            _ => Self::Daylight,
        }
    }

    /// The index into [`Self::ALL`] / the UI combo box's `lighting_options` list.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Daylight => 0,
            Self::Incandescent => 1,
            Self::RingLights => 2,
            Self::DarkSpotlight => 3,
            Self::IsoHemisphere => 4,
            Self::SoftDome => 5,
            Self::DaylightDome => 6,
        }
    }

    /// Inverse of [`Self::index`]; out-of-range indices fall back to [`Self::Daylight`]
    /// (index 0), matching [`Self::from_label`]'s fallback.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Incandescent,
            2 => Self::RingLights,
            3 => Self::DarkSpotlight,
            4 => Self::IsoHemisphere,
            5 => Self::SoftDome,
            6 => Self::DaylightDome,
            _ => Self::Daylight,
        }
    }

    /// Which environment model this preset samples.
    #[must_use]
    pub const fn model(self) -> LightingModel {
        match self {
            Self::Daylight | Self::Incandescent | Self::RingLights | Self::DarkSpotlight => {
                LightingModel::Studio
            }
            Self::IsoHemisphere => LightingModel::IsoHemisphere,
            Self::SoftDome => LightingModel::SoftDome,
            Self::DaylightDome => LightingModel::DaylightDome,
        }
    }

    /// Whether the illuminant is the tabulated CIE D65 curve rather than a Planckian fit.
    #[must_use]
    pub const fn uses_d65(self) -> bool {
        matches!(
            self,
            Self::Daylight | Self::IsoHemisphere | Self::DaylightDome
        )
    }

    /// Convenience constructor for the common case of tracing against the analytic
    /// studio rig: `LightingPreset::RingLights.studio(1.0, 0.85, 0.95)` reads at the
    /// call site much like the old positional `&str` argument list did.
    #[must_use]
    pub const fn studio(
        self,
        exposure: f32,
        light_yaw: f32,
        light_pitch: f32,
    ) -> EnvironmentSource<'static> {
        EnvironmentSource::Studio {
            preset: self,
            exposure,
            light_yaw,
            light_pitch,
        }
    }
}

/// Selects what `trace_spectral_ray` samples when a ray misses the gemstone.
///
/// Either the analytic studio rig (`Studio`, the default -- see
/// `sample_studio_environment`) or a loaded HDR equirectangular panorama (`HdrMap`, via
/// [`crate::renderer::env_map::EnvironmentMap`]). The analytic rig stays useful for
/// controlled comparisons where a real photograph would introduce variables (its own
/// exposure, white balance, capture noise) a study wants held constant.
#[derive(Clone, Copy)]
pub enum EnvironmentSource<'a> {
    Studio {
        preset: LightingPreset,
        exposure: f32,
        light_yaw: f32,
        light_pitch: f32,
    },
    HdrMap(&'a EnvironmentMap),
}

/// Looks up channel `lambda_nm`'s spectral radiance, in direction `dir`, for a ray that
/// missed the gemstone and is now sampling `environment`. Pulled out of
/// `trace_spectral_ray`'s miss branch so the `HdrMap` arm doesn't grow that
/// already-oversized function.
///
/// Takes the `Studio` variant's [`StudioRig`](crate::optics::studio_rig::StudioRig)
/// pre-built (`studio_rig`) rather than reconstructing it from `light_yaw`/
/// `light_pitch` on every call -- `accumulate_miss_radiance`, the only caller, builds it
/// once per ray and reuses it across all `NUM_CHANNELS` channels. Unused for `HdrMap`.
#[inline]
pub(super) fn sample_environment_channel(
    environment: EnvironmentSource<'_>,
    dir: Vec3,
    lambda_nm: f32,
    studio_rig: Option<&crate::optics::studio_rig::StudioRig>,
) -> f32 {
    match environment {
        EnvironmentSource::Studio {
            preset, exposure, ..
        } => {
            let rig = studio_rig
                .expect("sample_environment_channel: Studio environment needs a pre-built rig");
            sample_studio_environment_with_rig(dir, lambda_nm, preset, exposure, rig)
        }
        EnvironmentSource::HdrMap(map) => map.radiance_at(dir, lambda_nm),
    }
}

/// One next-event-estimation draw toward the environment: the sampled
/// direction, its solid-angle-measure pdf under [`EnvironmentMap::sample`]'s own
/// importance distribution, and the raw RGB radiance in that direction (from which a
/// caller lifts each channel's own spectral radiance via
/// [`crate::renderer::env_map::rgb_to_spectral_radiance`] -- the same lift
/// [`EnvironmentMap::radiance_at`] itself uses).
pub(super) struct EnvNeeSample {
    pub(super) dir: Vec3,
    pub(super) pdf: f32,
    pub(super) rgb: [f32; 3],
}

/// Draws one NEE direction from `environment`'s own importance distribution, from two
/// independent uniform `[0, 1)` randoms.
///
/// `None` for [`EnvironmentSource::Studio`] (the analytic rig has no such distribution to
/// sample -- NEE is HDR-map-only, see `scattering::NeeContext`'s doc comment) or for a
/// degenerate [`EnvironmentSource::HdrMap`] draw (`pdf <= 0.0`, e.g. a direction whose
/// row/column solid angle collapsed to nothing at a pole) -- either way the caller should
/// simply skip this NEE contribution for the current event, not fabricate one.
#[must_use]
pub(super) fn sample_environment_for_nee(
    environment: EnvironmentSource<'_>,
    u0: f32,
    u1: f32,
) -> Option<EnvNeeSample> {
    match environment {
        EnvironmentSource::Studio { .. } => None,
        EnvironmentSource::HdrMap(map) => {
            let (dir, rgb, pdf) = map.sample(u0, u1);
            (pdf > 0.0).then_some(EnvNeeSample { dir, pdf, rgb })
        }
    }
}

/// The solid-angle-measure pdf [`sample_environment_for_nee`] would assign to `dir`,
/// computed independently of any particular sample -- the OTHER half of a balance-heuristic
/// MIS weight: evaluating the light-sampling technique's own density at a direction the
/// COMPETING (BSDF/phase) technique produced. `0.0` for [`EnvironmentSource::Studio`]
/// (matching [`sample_environment_for_nee`]'s `None`: no light-sampling technique exists
/// to compete against there).
#[must_use]
pub(super) fn environment_nee_pdf(environment: EnvironmentSource<'_>, dir: Vec3) -> f32 {
    match environment {
        EnvironmentSource::Studio { .. } => 0.0,
        EnvironmentSource::HdrMap(map) => map.pdf(dir),
    }
}

/// The von-Kries white-balance scale (Bradford LMS-space, per-cone -- see
/// [`compute_illuminant_white_balance`]) [`trace_spectral_ray`] applies, via
/// [`apply_von_kries_white_balance`], to its final XYZ integration for `environment`.
/// Only the analytic studio rig has a single well-defined illuminant colour temperature
/// to neutralize against -- a loaded HDR panorama has no one blackbody temperature
/// standing in for it, so this applies no correction (`Vec3::ONE`) for `HdrMap`.
#[inline]
pub(super) fn environment_white_balance(environment: EnvironmentSource<'_>) -> Vec3 {
    match environment {
        EnvironmentSource::Studio { preset, .. } => illuminant_white_balance(preset),
        EnvironmentSource::HdrMap(_) => Vec3::ONE,
    }
}

/// Evaluates high-dynamic-range gemological studio lighting at a specific continuous
/// wavelength `lambda_nm`.
///
/// Builds a fresh [`StudioRig`](crate::optics::studio_rig::StudioRig) every call, right
/// for a single ad-hoc lookup (this function's public callers, and
/// `color::metrics::evaluate_gem_optical_metrics`). `trace_spectral_ray`'s per-bounce
/// environment lookups instead use [`sample_studio_environment_with_rig`] (which this
/// delegates to) via `accumulate_miss_radiance`, which builds the rig once per ray and
/// reuses it across all `NUM_CHANNELS` channels.
#[must_use]
pub fn sample_studio_environment(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> f32 {
    // Key/fill/ring directions come from the shared `StudioRig` (see its module doc
    // for why this is not recomputed inline here) -- the SAME construction
    // `color::metrics::evaluate_gem_optical_metrics` uses to score the image this
    // function lights, so the two can never silently drift apart.
    let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
    sample_studio_environment_with_rig(dir, lambda_nm, lighting_preset, exposure, &rig)
}

pub const RING_CONE_OUTER_COS: f32 = 0.965_925_8;
pub const RING_CONE_INNER_COS: f32 = 0.996_194_7;
pub const SUN_OUTER_COS: f32 = 0.970_295_7;
pub const SUN_INNER_COS: f32 = 0.997_564_1;

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (-2.0f32).mul_add(t, 3.0)
}

fn sample_iso_hemisphere(d: Vec3, spec_power: f32, exposure: f32, key_dir: Vec3) -> f32 {
    let obs_dot = d.dot(key_dir);
    let shadow_factor = 1.0 - smoothstep(0.93, 0.97, obs_dot);
    let dome = if d.y > 0.0 {
        (0.30f32.mul_add(d.y, 0.70) - 0.005).mul_add(shadow_factor, 0.005)
    } else {
        0.005
    };
    let horizon = smoothstep(-0.02, 0.05, d.y);
    (dome * horizon) * (spec_power * exposure)
}

fn sample_soft_dome(
    d: Vec3,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
) -> f32 {
    let dome = if d.y >= 0.0 {
        0.02 * 0.5f32.mul_add(d.y, 0.5)
    } else {
        0.005
    };
    let mut radiance = dome;

    let key_dot = d.dot(rig.key_dir).max(0.0);
    if key_dot > 0.0 {
        let key = key_dot.powi(16) * (3.5 * spot_mult);
        radiance += key;
    }

    let fill_dot = d.dot(rig.fill_dir).max(0.0);
    if fill_dot > 0.0 {
        let fill = fill_dot.powi(12) * 1.0;
        radiance += fill;
    }

    let ring_scale = 0.8 * spot_mult;
    for ring_dir in rig.ring_dirs {
        let ring_dot = d.dot(ring_dir);
        let ring = smoothstep(RING_CONE_OUTER_COS, RING_CONE_INNER_COS, ring_dot) * ring_scale;
        radiance += ring;
    }

    radiance * (exposure * spec_power)
}

fn sample_daylight_dome(
    d: Vec3,
    spec_power: f32,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
) -> f32 {
    let dome = if d.y >= 0.0 {
        0.05 * 0.4f32.mul_add(d.y, 0.6)
    } else {
        0.005
    };
    let key_dot = d.dot(rig.key_dir);
    let sun = smoothstep(SUN_OUTER_COS, SUN_INNER_COS, key_dot) * 16.0;
    let aureole = key_dot.max(0.0).powi(16) * 1.5;
    (dome + sun + aureole) * (exposure * spec_power)
}

fn sample_studio_rig(
    d: Vec3,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
) -> f32 {
    // 1. Ambient luxury studio backdrop (pure neutral dark charcoal velvet)
    let bg_val = 0.012f32.mul_add(d.y.mul_add(0.5, 0.5), 0.015).max(0.005) * exposure;
    let mut radiance = bg_val * spec_power;

    // 2. Main Key Softbox Light
    let key_dot = d.dot(rig.key_dir).max(0.0);
    if key_dot > 0.0 {
        let softbox = key_dot.powi(28) * 12.0 * spot_mult * exposure;
        radiance = softbox.mul_add(spec_power, radiance);
    }

    // 3. Fill Softbox Light (side reflector offset by 140 deg)
    let fill_dot = d.dot(rig.fill_dir).max(0.0);
    if fill_dot > 0.0 {
        let fill = fill_dot.powi(18) * 4.5 * exposure;
        radiance = fill.mul_add(spec_power, radiance);
    }

    // 4. Circular Ring Scintillation Lights (16 sparkling pinpoint sources rotating with lighting rig)
    for ring_dir in rig.ring_dirs {
        let ring_dot = d.dot(ring_dir).max(0.0);
        if ring_dot > 0.96 {
            let spark = (ring_dot - 0.96) / 0.04;
            let intensity = spark.powi(6) * 22.0 * spot_mult * exposure;
            radiance = intensity.mul_add(spec_power, radiance);
        }
    }

    radiance
}

/// The rig-independent body of [`sample_studio_environment`]: identical arithmetic, in
/// the identical order, just reading `key_dir`/`fill_dir`/`ring_dirs`/`sin_light_pitch`
/// off an already-built `rig` instead of constructing one from `(light_yaw,
/// light_pitch)` itself. A direct extraction -- see that function's doc comment for
/// why.
#[must_use]
fn sample_studio_environment_with_rig(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
) -> f32 {
    let d = dir.normalize();

    let LightingRigParams { temp_k, spot_mult } = lighting_preset.params();
    let spec_power = if lighting_preset.uses_d65() {
        d65_relative_spectral_power(lambda_nm)
    } else {
        blackbody_spectrum(lambda_nm, temp_k)
    };

    match lighting_preset.model() {
        LightingModel::Studio => sample_studio_rig(d, spec_power, spot_mult, exposure, rig),
        LightingModel::IsoHemisphere => sample_iso_hemisphere(d, spec_power, exposure, rig.key_dir),
        LightingModel::SoftDome => sample_soft_dome(d, spec_power, spot_mult, exposure, rig),
        LightingModel::DaylightDome => sample_daylight_dome(d, spec_power, exposure, rig),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins [`d65_relative_spectral_power`] against the tabulated CIE D65 values
    /// directly, and against a genuine Planckian 6500K curve to confirm this is not
    /// just a relabelled blackbody -- D65's real, measured blue-green irregularity
    /// means the two must disagree at some wavelengths.
    #[test]
    fn d65_relative_spectral_power_matches_the_cie_table_at_450_and_550nm() {
        // Exact table entries, normalized by /100.0 (560nm's entry) like the function
        // under test does.
        let expected_450 = 117.008 / 100.0;
        let expected_550 = 104.046 / 100.0;

        let got_450 = d65_relative_spectral_power(450.0);
        let got_550 = d65_relative_spectral_power(550.0);
        assert!(
            (got_450 - expected_450).abs() < 1e-4,
            "450nm: got {got_450}, expected {expected_450} from the CIE D65 table"
        );
        assert!(
            (got_550 - expected_550).abs() < 1e-4,
            "550nm: got {got_550}, expected {expected_550} from the CIE D65 table"
        );

        // The real CIE table has 450nm's power exceeding 550nm's (a measured
        // irregularity, not something a smooth blackbody curve produces at 6500K).
        assert!(
            got_450 > got_550,
            "D65's real spectrum has more relative power at 450nm than 550nm; \
             got 450nm={got_450}, 550nm={got_550}"
        );

        // Distinguishes this from a plain 6500K Planckian: a genuine blackbody curve
        // is smooth (450nm/380nm ratio close to 1, both on the same gently-rising
        // Wien tail), while the real D65 table rises much more steeply there.
        let d65_ratio = d65_relative_spectral_power(450.0) / d65_relative_spectral_power(380.0);
        let blackbody_ratio = blackbody_spectrum(450.0, 6500.0) / blackbody_spectrum(380.0, 6500.0);
        assert!(
            d65_ratio > blackbody_ratio * 1.5,
            "test premise: the real D65 table's 450nm/380nm rise ({d65_ratio:.3}) must \
             be much steeper than a smooth 6500K Planckian's ({blackbody_ratio:.3}), \
             confirming the Daylight preset is no longer just a relabelled blackbody"
        );
    }

    /// `sample_studio_environment`'s Daylight preset must route through the D65 table,
    /// not `blackbody_spectrum` -- exercised end to end through the full lighting-rig
    /// function, not just the standalone table lookup above.
    #[test]
    fn daylight_preset_studio_environment_uses_the_d65_table_not_a_blackbody() {
        // Straight down from above (`Vec3::Y`) misses every directional light term
        // (key/fill/ring all `.max(0.0)`-clamped dot products that can legitimately
        // land at/near zero here), leaving only the ambient backdrop term -- which is
        // exactly `bg_val * spec_power`, isolating `spec_power` cleanly.
        let dir = Vec3::new(0.0, -1.0, 0.0);
        let exposure = 1.0;

        let daylight_450 =
            sample_studio_environment(dir, 450.0, LightingPreset::Daylight, exposure, 0.0, 0.0);
        let daylight_550 =
            sample_studio_environment(dir, 550.0, LightingPreset::Daylight, exposure, 0.0, 0.0);
        // A genuine Planckian 6500K curve has 450nm < 550nm (see the standalone table
        // test above); the real D65 table has the opposite ordering. If this preset
        // were still silently using `blackbody_spectrum`, this assertion would fail.
        assert!(
            daylight_450 > daylight_550,
            "Daylight preset must reflect D65's own 450nm > 550nm ordering end to \
             end, got 450nm={daylight_450}, 550nm={daylight_550}"
        );
    }

    #[test]
    fn label_and_index_round_trip_for_every_preset() {
        for (pos, &p) in LightingPreset::ALL.iter().enumerate() {
            assert_eq!(LightingPreset::from_label(p.label()), p);
            assert_eq!(LightingPreset::from_index(p.index()), p);
            assert_eq!(p.index(), pos as i32);
        }
    }

    #[test]
    fn iso_hemisphere_is_one_above_and_zero_below() {
        let val_up =
            sample_studio_environment(Vec3::Y, 550.0, LightingPreset::IsoHemisphere, 1.0, 0.0, 0.0);
        let expected = d65_relative_spectral_power(550.0);
        assert!(
            (val_up - expected).abs() < 1e-4,
            "up direction should match d65_relative_spectral_power(550): got {val_up}, expected {expected}"
        );

        let val_down = sample_studio_environment(
            -Vec3::Y,
            550.0,
            LightingPreset::IsoHemisphere,
            1.0,
            0.0,
            0.0,
        );
        assert_eq!(val_down, 0.0, "down direction should be 0");
    }

    #[test]
    fn new_models_never_exceed_their_documented_peak() {
        let mut max_iso = 0.0f32;
        let mut max_soft = 0.0f32;
        let mut max_daylight = 0.0f32;

        for i in 0..2000 {
            let phi = (i as f32 + 0.5) * (std::f32::consts::PI * (3.0 - 5.0f32.sqrt()));
            let y = (i as f32 + 0.5).mul_add(-(2.0 / 2000.0), 1.0);
            let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
            let dir = Vec3::new(r * phi.cos(), y, r * phi.sin());

            let v_iso = sample_studio_environment(
                dir,
                560.0,
                LightingPreset::IsoHemisphere,
                1.0,
                0.4,
                0.35,
            );
            let v_soft =
                sample_studio_environment(dir, 560.0, LightingPreset::SoftDome, 1.0, 0.4, 0.35);
            let v_daylight =
                sample_studio_environment(dir, 560.0, LightingPreset::DaylightDome, 1.0, 0.4, 0.35);

            max_iso = max_iso.max(v_iso);
            max_soft = max_soft.max(v_soft);
            max_daylight = max_daylight.max(v_daylight);
        }

        assert!(
            max_iso <= 1.0 + 1e-5,
            "ISO peak must not exceed 1.0, got {max_iso}"
        );
        // Key (3.5) + fill (1.0) + ring (0.8) + dome (0.02) can overlap along a ring light direction,
        // peaking around ~5.3.
        assert!(
            max_soft <= 6.0,
            "Soft dome peak must not exceed 6.0, got {max_soft}"
        );
        // Sun (16.0) + aureole (1.5) + sky dome (0.05) can overlap at the sun center, peaking around ~17.55.
        assert!(
            max_daylight <= 19.0,
            "Daylight dome peak must not exceed 19.0, got {max_daylight}"
        );
    }

    #[test]
    #[allow(clippy::unreadable_literal)]
    fn studio_presets_are_unchanged_by_the_split() {
        const BASELINE_BITS: [u32; 192] = [
            1021303258, 1023655294, 1023453223, 1021200980, 1023595102, 1023378703, 1021099049,
            1023535114, 1023261534, 1020997253, 1023475205, 1023144520, 1021568336, 1023811298,
            1023605575, 1022776057, 1024522062, 1024299704, 1020689822, 1023178377, 1022791133,
            1027755127, 1030009411, 1029658622, 1044895793, 1047214420, 1046853619, 1020392899,
            1022828889, 1022449825, 1020281301, 1022697534, 1022321544, 1020178664, 1022576726,
            1022203564, 1061137584, 1063361422, 1063015371, 1083905861, 1085705250, 1085425247,
            1019902579, 1022251765, 1021886209, 1046107587, 1048608372, 1048246560, 1076756649,
            1078775455, 1078461309, 1093086489, 1095026093, 1094724272, 1100967472, 1102817210,
            1102529373, 1102501900, 1104623284, 1104293176, 1100418366, 1102170893, 1101898183,
            1101296236, 1103204175, 1102907281, 1072082398, 1074250299, 1074042064, 1075709089,
            1077542440, 1077257152, 1081883524, 1083470199, 1083242507, 1084303182, 1086172911,
            1085881963, 1078120144, 1080380336, 1080028628, 1050792594, 1052670085, 1052377929,
            1047922901, 1049676716, 1049454620, 1044118990, 1046300096, 1045960695, 1018488179,
            1020586967, 1020260376, 1018134305, 1020170445, 1019853601, 1018031797, 1020049789,
            1019735769, 1075090799, 1076814691, 1076546437, 1068234283, 1070229409, 1069918948,
            1017725102, 1019688798, 1019383227, 1017689159, 1019646492, 1019341912, 1017521023,
            1019448590, 1019148641, 1028692337, 1031112541, 1030735933, 1017420802, 1019330627,
            1019033440, 1017213944, 1019087147, 1018795658, 1039204672, 1041094120, 1040876564,
            1018976767, 1021162052, 1020822000, 1016907249, 1018726157, 1018443117, 1016805017,
            1018605826, 1018325602, 1016702784, 1018485494, 1018208087, 1026205460, 1028185399,
            1027877301, 1016519388, 1018269632, 1017997277, 1016396089, 1018124504, 1017855546,
            1016303883, 1018015974, 1017749556, 1016191626, 1017883843, 1017620518, 1016089640,
            1017763802, 1017503286, 1015987162, 1017643183, 1017385490, 1015884931, 1017522852,
            1017267976, 1015807868, 1017432147, 1017179394, 1015680468, 1017282193, 1017032949,
            1015578235, 1017161861, 1016915434, 1015476004, 1017041531, 1016797920, 1015373773,
            1016921202, 1016680407, 1015271540, 1016800870, 1016562892, 1015169308, 1016680540,
            1016445378, 1015067077, 1016560210, 1016327864, 1014908123, 1016439880, 1016210351,
            1014703658, 1016319549, 1016092836,
        ];
        let mut idx = 0;
        for k in 0..64 {
            let phi = (k as f32 + 0.5) * (std::f32::consts::PI * (3.0 - 5.0f32.sqrt()));
            let y = (k as f32 + 0.5).mul_add(-(2.0 / 64.0), 1.0);
            let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
            let dir = Vec3::new(r * phi.cos(), y, r * phi.sin());
            for &lambda in &[450.0f32, 550.0, 650.0] {
                let val = sample_studio_environment(
                    dir,
                    lambda,
                    LightingPreset::RingLights,
                    1.2,
                    0.4,
                    0.35,
                );
                assert_eq!(
                    val.to_bits(),
                    BASELINE_BITS[idx],
                    "divergence at dir {k}, lambda {lambda}: got {val} (bits {}), expected bits {}",
                    val.to_bits(),
                    BASELINE_BITS[idx]
                );
                idx += 1;
            }
        }
    }
}
