//! Environment/lighting sources a ray can sample when it misses the gemstone.
//!
//! The analytic gemological studio rig ([`LightingPreset`], [`sample_studio_environment`])
//! and the loaded-HDR-panorama alternative ([`EnvironmentSource::HdrMap`]).

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
}

impl LightingPreset {
    /// All four presets, in the same order as their UI index / the `lighting_options`
    /// combo box list (`app.slint`).
    pub const ALL: [Self; 4] = [
        Self::Daylight,
        Self::Incandescent,
        Self::RingLights,
        Self::DarkSpotlight,
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
            Self::Daylight => LightingRigParams {
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
            _ => Self::Daylight,
        }
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
    // "D65 Daylight" samples the real tabulated CIE D65 spectrum instead of a smooth
    // Planckian approximation; every other preset keeps the Planckian model.
    //
    // Ported to the GPU backend: `shaders/spectral_transport.wgsl`'s own
    // `d65_relative_spectral_power`, gated by its `studio_use_d65` param, mirrors this
    // branch exactly (see that shader's own doc comment citing this function), and
    // `shaders/environment.wgsl`'s `sample_studio_environment` carries the same
    // `use_d65`-gated branch -- a GPU-routed Daylight render uses the real tabulated
    // D65 curve, not the old 6500K Planckian fallback.
    let spec_power = if matches!(lighting_preset, LightingPreset::Daylight) {
        d65_relative_spectral_power(lambda_nm)
    } else {
        blackbody_spectrum(lambda_nm, temp_k)
    };

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
}
