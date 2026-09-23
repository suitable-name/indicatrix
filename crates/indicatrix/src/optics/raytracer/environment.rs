//! Environment/lighting sources a ray can sample when it misses the gemstone.
//!
//! The analytic gemological studio rig ([`LightingPreset`], [`sample_studio_environment`])
//! and the loaded-HDR-panorama alternative ([`EnvironmentSource::HdrMap`]).
//!
//! # Lighting models
//!
//! Every [`LightingPreset`] samples one [`LightingModel`]:
//! - `Studio` (`Daylight`, `Incandescent`, `RingLights`, `DarkSpotlight`): the analytic
//!   studio rig -- a charcoal backdrop, one key softbox, one fill and sixteen ring
//!   pinpoints. Its arithmetic is pinned by golden images and never changes.
//! - `IsoHemisphere`: a uniformly radiant upper hemisphere at radiance 1, nothing below
//!   the girdle plane -- the ISO-standard viewing geometry.
//! - `LightTent`: a jewellery light tent -- dim tent walls, one broad overhead softbox,
//!   three black cards on the ring positions away from the key (the contrast
//!   photographers add so a diamond reads as a facet pattern rather than a white blur),
//!   one small hard spark light for scintillation, black velvet below the girdle.
//! - `DaylightDome`: a clear sky, brighter at the horizon than at the zenith with an
//!   aureole around the sun, a 2 degree sun disc, dark ground.
//!
//! The three lit models also darken every exit direction inside the observer's
//! head-shadow cone (`HEAD_SHADOW_*`), the term that gives a face-up stone its dark
//! table reflections; `Studio` ignores the observer. Radiances are chosen so that at
//! exposure 1 the ambient terms land near middle grey after the ACES curve and only a
//! direct reflection of a light source clips to white.

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
    LightTent,
    DaylightDome,
}

/// Which environment the preset samples -- see this module's "Lighting models" doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LightingModel {
    Studio,
    IsoHemisphere,
    LightTent,
    DaylightDome,
}

impl LightingModel {
    /// The `u32` discriminant bound in `GpuTransportParams::studio_model`.
    #[must_use]
    pub const fn gpu_id(self) -> u32 {
        match self {
            Self::Studio => 0,
            Self::IsoHemisphere => 1,
            Self::LightTent => 2,
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
        Self::LightTent,
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
            Self::LightTent => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.0,
            },
            Self::Daylight | Self::IsoHemisphere | Self::DaylightDome => LightingRigParams {
                temp_k: 6500.0,
                spot_mult: 1.0,
            },
        }
    }

    /// The user-facing display label.
    ///
    /// D65 daylight is 6500K, so this must read `"6500K"`, not `"5500K"`, to stay
    /// consistent with the actually-rendered colour.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Daylight => "D65 Daylight (6500K)",
            Self::Incandescent => "Incandescent (3200K)",
            Self::RingLights => "Gem Studio Ring Lights",
            Self::DarkSpotlight => "Dramatic Dark Spotlight",
            Self::IsoHemisphere => "ISO hemisphere",
            Self::LightTent => "Light tent + black cards",
            Self::DaylightDome => "Daylight sky + sun",
        }
    }

    /// Parses a persisted or UI-supplied label back into a preset. Falls back to
    /// [`Self::Daylight`] for anything unrecognised -- including the legacy
    /// `"D65 Daylight (5500K)"` label an older settings file may still contain, which
    /// already resolved to D65 6500K, so migration is silent. The lit models' first
    /// labels (`"ISO hemisphere (GemRay-style)"`, `"Soft dome + ring lights"`,
    /// `"Daylight dome + sun"`) resolve to their current presets the same way.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label {
            "Incandescent (3200K)" => Self::Incandescent,
            "Gem Studio Ring Lights" => Self::RingLights,
            "Dramatic Dark Spotlight" => Self::DarkSpotlight,
            "ISO hemisphere" | "ISO hemisphere (GemRay-style)" => Self::IsoHemisphere,
            "Light tent + black cards" | "Soft dome + ring lights" => Self::LightTent,
            "Daylight sky + sun" | "Daylight dome + sun" => Self::DaylightDome,
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
            Self::LightTent => 5,
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
            5 => Self::LightTent,
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
            Self::LightTent => LightingModel::LightTent,
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

    /// Relative spectral power of this preset's illuminant at `lambda_nm`: the
    /// tabulated CIE D65 curve where [`Self::uses_d65`], else a Planckian fit at the
    /// preset's colour temperature.
    #[must_use]
    pub fn spectral_power(self, lambda_nm: f32) -> f32 {
        if self.uses_d65() {
            d65_relative_spectral_power(lambda_nm)
        } else {
            blackbody_spectrum(lambda_nm, self.params().temp_k)
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
            backdrop: 0.0,
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
        /// Radiance of the backdrop card a camera ray sees where it misses the stone,
        /// in the preset's own spectral-power units (so it renders neutral after white
        /// balance) and independent of `exposure`. `0.0` shows the environment itself.
        /// The stone's optics never see the card -- only the primary ray does -- so
        /// leakage and windows stay as dark as the real ground, behind a neutral grey
        /// backdrop card. See [`BACKDROP_GREY`].
        backdrop: f32,
    },
    HdrMap(&'a EnvironmentMap),
}

/// Backdrop radiance that tone-maps to a neutral grey backdrop card (about sRGB 160).
pub const BACKDROP_GREY: f32 = 0.23;
/// Backdrop radiance that tone-maps to white: a light box behind the stone.
pub const BACKDROP_WHITE: f32 = 8.0;

impl EnvironmentSource<'_> {
    /// Puts a backdrop card of radiance `backdrop` behind the stone (see the `Studio`
    /// variant's field); an HDR map is its own backdrop and is returned unchanged.
    #[must_use]
    pub const fn with_backdrop(self, backdrop: f32) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                ..
            } => Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
            },
            hdr @ Self::HdrMap(_) => hdr,
        }
    }
}

/// Looks up channel `lambda_nm`'s spectral radiance, in direction `dir`, for a ray that
/// missed the gemstone and is now sampling `environment`. Pulled out of
/// `trace_spectral_ray`'s miss branch so the `HdrMap` arm doesn't grow that
/// already-oversized function.
///
/// Takes the `Studio` variant's [`StudioRig`](crate::optics::studio_rig::StudioRig)
/// pre-built (`studio_rig`) rather than reconstructing it from `light_yaw`/
/// `light_pitch` on every call -- `accumulate_miss_radiance` and the exit-split probe
/// build it once per ray and reuse it across all `NUM_CHANNELS` channels. `observer` is
/// the unit direction from the stone towards the eye (see
/// [`sample_studio_environment_observed`]). Both are unused for `HdrMap`.
#[inline]
pub(super) fn sample_environment_channel(
    environment: EnvironmentSource<'_>,
    dir: Vec3,
    lambda_nm: f32,
    studio_rig: Option<&crate::optics::studio_rig::StudioRig>,
    observer: Vec3,
) -> f32 {
    match environment {
        EnvironmentSource::Studio {
            preset, exposure, ..
        } => {
            let rig = studio_rig
                .expect("sample_environment_channel: Studio environment needs a pre-built rig");
            sample_studio_environment_with_rig(dir, lambda_nm, preset, exposure, rig, observer)
        }
        EnvironmentSource::HdrMap(map) => map.radiance_at(dir, lambda_nm),
    }
}

/// Fills every channel of `radiance` with the backdrop card's radiance if the scene
/// has one, and reports whether it did. For the camera ray only (bounce 0: unit Stokes
/// intensity, no NEE weight), so the assignment is that ray's whole contribution.
pub(super) fn fill_backdrop<const N: usize>(
    environment: EnvironmentSource<'_>,
    lambdas: &[f32; N],
    radiance: &mut [f32; N],
) -> bool {
    match environment {
        EnvironmentSource::Studio {
            preset, backdrop, ..
        } if backdrop > 0.0 => {
            for (out, &lambda_nm) in radiance.iter_mut().zip(lambdas) {
                *out = backdrop * preset.spectral_power(lambda_nm);
            }
            true
        }
        _ => false,
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
/// [`apply_von_kries_white_balance`], to its final XYZ integration for a `Studio`
/// `environment`. Only the analytic studio rig has a single well-defined illuminant
/// colour temperature to neutralize against -- a loaded HDR panorama has no one
/// blackbody temperature standing in for it, so this returns `Vec3::ONE` for `HdrMap`
/// (a mathematical no-op scale), but `trace_spectral_ray`'s own `HdrMap` arm does not
/// even call [`apply_von_kries_white_balance`] with it: the full
/// XYZ->LMS->XYZ round trip is not quite the identity at `Vec3::ONE` in f32 (the two
/// published Bradford matrices are not exact inverses), so skipping the call entirely
/// for `HdrMap` is exact, matching `transport_bounce.wgsl`'s own `params.env_mode == 1u`
/// (`Studio`-only) gate on the identical transform, instead of merely close to it.
#[inline]
pub(crate) fn environment_white_balance(environment: EnvironmentSource<'_>) -> Vec3 {
    match environment {
        EnvironmentSource::Studio { preset, .. } => illuminant_white_balance(preset),
        EnvironmentSource::HdrMap(_) => Vec3::ONE,
    }
}

/// Evaluates high-dynamic-range gemological studio lighting at a specific continuous
/// wavelength `lambda_nm`, with no observer in the scene.
///
/// The lit models' head shadow is off here; see
/// [`sample_studio_environment_observed`].
/// Builds a fresh [`StudioRig`](crate::optics::studio_rig::StudioRig) every call, right
/// for a single ad-hoc lookup (this function's public callers, and
/// `color::metrics::evaluate_gem_optical_metrics`). `trace_spectral_ray`'s per-bounce
/// environment lookups instead use [`sample_studio_environment_with_rig`] via
/// `accumulate_miss_radiance`, which builds the rig once per ray and reuses it across
/// all `NUM_CHANNELS` channels.
#[must_use]
pub fn sample_studio_environment(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> f32 {
    sample_studio_environment_observed(
        dir,
        lambda_nm,
        lighting_preset,
        exposure,
        light_yaw,
        light_pitch,
        Vec3::ZERO,
    )
}

/// [`sample_studio_environment`] with an observer in the scene.
///
/// `observer` is the unit direction from the stone towards the eye (the reverse of the
/// pixel's primary ray), and the lit models darken every exit direction inside the
/// head-shadow cone around it -- the dark table reflections a real face-up stone
/// shows. `Studio` presets ignore it, and `Vec3::ZERO` disables the shadow for every
/// model (every dot product is then `0.0`, outside the cone).
#[must_use]
pub fn sample_studio_environment_observed(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    observer: Vec3,
) -> f32 {
    // Key/fill/ring directions come from the shared `StudioRig` (see its module doc
    // for why this is not recomputed inline here) -- the SAME construction
    // `color::metrics::evaluate_gem_optical_metrics` uses to score the image this
    // function lights, so the two can never silently drift apart.
    let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
    sample_studio_environment_with_rig(dir, lambda_nm, lighting_preset, exposure, &rig, observer)
}

/// Cosines of the cone half-angles the lit models are built from. Literal values,
/// never computed, so the CPU and the WGSL twins (`transport_physics.wgsl`,
/// `transport_bounce.wgsl`, `environment.wgsl`) use identical bits.
///
/// Observer head shadow: fully dark within 14 degrees of the eye direction, gone by 18
/// (the metrics' own 16 degree cone, softened so its edge never aliases).
const HEAD_SHADOW_OUTER_COS: f32 = 0.951_056_5;
const HEAD_SHADOW_INNER_COS: f32 = 0.970_295_7;
/// Sun disc: full radiance within 2 degrees of the key direction, gone by 4.
const SUN_OUTER_COS: f32 = 0.997_564_1;
const SUN_INNER_COS: f32 = 0.999_390_8;
/// Light tent overhead softbox: full within 20 degrees of the key, gone by 40.
const TENT_KEY_OUTER_COS: f32 = 0.766_044_4;
const TENT_KEY_INNER_COS: f32 = 0.939_692_6;
/// Light tent spark light (a bare bulb at the fill position): full within 2 degrees,
/// gone by 5.
const SPARK_OUTER_COS: f32 = 0.996_194_7;
const SPARK_INNER_COS: f32 = 0.999_390_8;
/// Light tent black cards: fully black within 16 degrees of a card centre, gone by 26.
const CARD_OUTER_COS: f32 = 0.898_794;
const CARD_INNER_COS: f32 = 0.961_261_7;
/// The ring positions (see `StudioRig::ring_dirs`) carrying the three black cards: 90,
/// 180 and 270 degrees around from the key light, at the ring's own elevation.
const CARD_RING_SLOTS: [usize; 3] = [4, 8, 12];

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (-2.0f32).mul_add(t, 3.0)
}

/// `1.0` where direction `d` sees past the observer, falling to `0.0` inside the
/// head-shadow cone around `observer` (see [`sample_studio_environment_observed`]).
fn observer_visibility(d: Vec3, observer: Vec3) -> f32 {
    1.0 - smoothstep(
        HEAD_SHADOW_OUTER_COS,
        HEAD_SHADOW_INNER_COS,
        d.dot(observer),
    )
}

/// `0.0` below the girdle plane, `1.0` above, blended over `-0.05..0.05` in `d.y` so
/// the horizon never aliases.
fn horizon_blend(d: Vec3) -> f32 {
    smoothstep(-0.05, 0.05, d.y)
}

fn sample_iso_hemisphere(d: Vec3, spec_power: f32, exposure: f32, observer: Vec3) -> f32 {
    (horizon_blend(d) * observer_visibility(d, observer)) * (spec_power * exposure)
}

fn sample_light_tent(
    d: Vec3,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
) -> f32 {
    let horizon = horizon_blend(d);
    // Tent walls: 0.14 at the girdle plane rising to 0.22 at the zenith -- middle grey
    // after the ACES curve, so a facet that sees nothing but the tent is grey, not
    // white. The black cards cut that to a tenth.
    let mut walls = 0.08f32.mul_add(d.y.max(0.0), 0.14);
    let mut card = 0.0f32;
    for slot in CARD_RING_SLOTS {
        card = card.max(smoothstep(
            CARD_OUTER_COS,
            CARD_INNER_COS,
            d.dot(rig.ring_dirs[slot]),
        ));
    }
    walls *= card.mul_add(-0.9, 1.0);
    let key =
        smoothstep(TENT_KEY_OUTER_COS, TENT_KEY_INNER_COS, d.dot(rig.key_dir)) * (1.4 * spot_mult);
    let spark =
        smoothstep(SPARK_OUTER_COS, SPARK_INNER_COS, d.dot(rig.fill_dir)) * (5.0 * spot_mult);
    let above = ((walls + key) + spark) * (horizon * observer_visibility(d, observer));
    let ground = 0.02 * (1.0 - horizon);
    (above + ground) * (spec_power * exposure)
}

fn sample_daylight_dome(
    d: Vec3,
    spec_power: f32,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
) -> f32 {
    let horizon = horizon_blend(d);
    let sun_dot = d.dot(rig.key_dir);
    // A clear sky is brightest at the horizon and around the sun, darkest at the zenith.
    let sky = 0.08f32.mul_add(1.0 - d.y.max(0.0), 0.10);
    // `sun_dot^8`, written as three squarings so the GPU twin multiplies identically.
    let glow = sun_dot.max(0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let sun = smoothstep(SUN_OUTER_COS, SUN_INNER_COS, sun_dot) * 10.0;
    let above = ((sky + aureole) + sun) * (horizon * observer_visibility(d, observer));
    let ground = 0.04 * (1.0 - horizon);
    (above + ground) * (spec_power * exposure)
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

/// The rig-independent body of [`sample_studio_environment_observed`]: identical
/// arithmetic, in the identical order, just reading `key_dir`/`fill_dir`/`ring_dirs`/
/// `sin_light_pitch` off an already-built `rig` instead of constructing one from
/// `(light_yaw, light_pitch)` itself. Dispatches on the preset's [`LightingModel`];
/// the `Studio` arm is the original rig body, untouched.
#[must_use]
fn sample_studio_environment_with_rig(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
) -> f32 {
    let d = dir.normalize();

    let LightingRigParams { spot_mult, .. } = lighting_preset.params();
    let spec_power = lighting_preset.spectral_power(lambda_nm);

    match lighting_preset.model() {
        LightingModel::Studio => sample_studio_rig(d, spec_power, spot_mult, exposure, rig),
        LightingModel::IsoHemisphere => sample_iso_hemisphere(d, spec_power, exposure, observer),
        LightingModel::LightTent => {
            sample_light_tent(d, spec_power, spot_mult, exposure, rig, observer)
        }
        LightingModel::DaylightDome => sample_daylight_dome(d, spec_power, exposure, rig, observer),
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
    fn backdrop_fills_the_camera_ray_only_when_set() {
        let lambdas = [450.0f32, 550.0, 650.0];
        let mut radiance = [0.0f32; 3];
        let plain = LightingPreset::LightTent.studio(1.0, 0.4, 0.35);
        assert!(!fill_backdrop(plain, &lambdas, &mut radiance));
        assert_eq!(radiance, [0.0; 3]);

        let carded = plain.with_backdrop(BACKDROP_GREY);
        assert!(fill_backdrop(carded, &lambdas, &mut radiance));
        for (&value, &lambda_nm) in radiance.iter().zip(&lambdas) {
            let expected = BACKDROP_GREY * LightingPreset::LightTent.spectral_power(lambda_nm);
            assert!(
                (value - expected).abs() < 1e-6,
                "backdrop at {lambda_nm} nm: got {value}, expected {expected}"
            );
        }
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

    /// The `Studio` arm ignores the observer; the lit models go fully dark exactly at
    /// the eye direction and are untouched 20 degrees away from it.
    #[test]
    fn observer_head_shadow_darkens_only_the_lit_models() {
        let observer = Vec3::new(0.2, 0.9, -0.3).normalize();
        // 20 degrees away from the observer, outside the 18 degree cone.
        let perp = observer.cross(Vec3::X).normalize();
        let (sin_a, cos_a) = 20.0f32.to_radians().sin_cos();
        let outside = observer.mul_add(Vec3::splat(cos_a), perp * sin_a);

        let ring_with = sample_studio_environment_observed(
            observer,
            560.0,
            LightingPreset::RingLights,
            1.0,
            0.4,
            0.35,
            observer,
        );
        let ring_without =
            sample_studio_environment(observer, 560.0, LightingPreset::RingLights, 1.0, 0.4, 0.35);
        assert_eq!(
            ring_with.to_bits(),
            ring_without.to_bits(),
            "the studio rig must ignore the observer"
        );

        for preset in [
            LightingPreset::IsoHemisphere,
            LightingPreset::LightTent,
            LightingPreset::DaylightDome,
        ] {
            let at_eye = sample_studio_environment_observed(
                observer, 560.0, preset, 1.0, 0.4, 0.35, observer,
            );
            assert_eq!(
                at_eye, 0.0,
                "{preset:?}: the eye direction must be fully shadowed"
            );
            let clear = sample_studio_environment_observed(
                outside, 560.0, preset, 1.0, 0.4, 0.35, observer,
            );
            let unobserved = sample_studio_environment(outside, 560.0, preset, 1.0, 0.4, 0.35);
            assert_eq!(
                clear.to_bits(),
                unobserved.to_bits(),
                "{preset:?}: outside the cone the observer must not matter"
            );
            assert!(
                unobserved > 0.0,
                "{preset:?}: a lit direction above the girdle must be lit"
            );
        }
    }

    #[test]
    fn legacy_lit_model_labels_resolve_to_the_renamed_presets() {
        assert_eq!(
            LightingPreset::from_label("ISO hemisphere"),
            LightingPreset::IsoHemisphere
        );
        assert_eq!(
            LightingPreset::from_label("ISO hemisphere (GemRay-style)"),
            LightingPreset::IsoHemisphere,
            "a settings file saved before the GemRay-style label was reworded must still \
             load as IsoHemisphere"
        );
        assert_eq!(
            LightingPreset::from_label("Soft dome + ring lights"),
            LightingPreset::LightTent
        );
        assert_eq!(
            LightingPreset::from_label("Daylight dome + sun"),
            LightingPreset::DaylightDome
        );
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
                sample_studio_environment(dir, 560.0, LightingPreset::LightTent, 1.0, 0.4, 0.35);
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
        // Spark (5.0) on the tent walls (<= 0.22) is the tent's brightest direction; the
        // key softbox (1.4) never coincides with it.
        assert!(
            max_soft <= 5.5,
            "Light tent peak must not exceed 5.5, got {max_soft}"
        );
        // Sun (10.0) + aureole (0.30) + sky (<= 0.18) at the sun centre.
        assert!(
            max_daylight <= 10.5,
            "Daylight dome peak must not exceed 10.5, got {max_daylight}"
        );
    }

    /// Regression pin for the rig-sharing split (`sample_studio_environment_with_rig`,
    /// which both the ad-hoc callers here and `accumulate_miss_radiance`'s per-bounce
    /// lookup share, instead of each duplicating the studio rig arithmetic inline):
    /// tracing 64 directions x 3 wavelengths through `RingLights` must reproduce
    /// [`BASELINE_BITS`], to within a small ULP tolerance.
    ///
    /// # Regenerating the baseline
    ///
    /// [`BASELINE_BITS`] is captured from this test's OWN current output (a temporary
    /// `eprintln!` of `got_bits` per sample, run once via `cargo
    /// test -p indicatrix --lib -- studio_presets_are_unchanged_by_the_split
    /// --nocapture`, then removed). [`MAX_ULP_DIFF`] must stay tight: a budget wide
    /// enough to absorb reassociation noise is ALSO wide enough to absorb a small
    /// coefficient change on the ambient term without the test ever failing, which would
    /// mean it isn't actually pinning the formula.
    ///
    /// The CPU/GPU HDR white-balance step does not touch
    /// `sample_studio_rig`/`sample_studio_environment` at all -- it lives entirely in
    /// `trace_spectral_ray_inner`'s post-integration white-balance step -- so nothing on
    /// the Studio-rig sampling path this test exercises is affected by it. Before
    /// accepting a freshly captured baseline, diff it against the prior one: deltas of a
    /// few ULP concentrated in the falloff-cone-edge cluster named below (e.g.
    /// 28/29/38/46) match the `key_dot.powi(28)` reassociation-noise mechanism analyzed
    /// below and can be folded into the new baseline; a diff that is large, widespread,
    /// or inconsistent with that mechanism is a real regression and should be reported
    /// rather than pasted over.
    ///
    /// # Why a tolerance at all, not bit-for-bit equality
    ///
    /// Requiring exact equality only ever surfaces the FIRST mismatch (`assert_eq!`
    /// aborts the loop immediately), which is what hid the true shape of this the first
    /// time: an exhaustive sweep of all 192 samples' bit-pattern deltas (not just the
    /// first failure) is needed to tell "reassociation noise" apart from "a real
    /// regression". Every non-ambient term in `sample_studio_rig` (key softbox, fill,
    /// ring) is accumulated via `softbox.mul_add(spec_power, radiance)`-style fused
    /// multiply-adds, and `key_dot.powi(28)` in particular amplifies a sub-ULP
    /// perturbation in `key_dot` into a much larger one in the softbox term for
    /// directions near the falloff cone's edge -- exactly where the largest deltas
    /// (dir 28/29/38/46) sit. Floating-point multiplication and fused multiply-add are
    /// not associative, so evaluating the identical formula through a differently-shaped
    /// call chain, or a differently-optimized build, can legitimately round
    /// intermediate bits either way, with a `^28` term turning "either way" into
    /// "either way, times a large derivative" -- with no change in the actual light
    /// transport (10 ULP here is a relative difference near `1e-6`, many orders below
    /// anything a path tracer's own sample noise could ever resolve). [`MAX_ULP_DIFF`]'s
    /// `2` leaves headroom for exactly that kind of single-ULP-scale noise on the
    /// majority of samples while a difference of many thousands of ULP -- what a
    /// genuine behavioural regression (a changed exponent, a changed coefficient, a
    /// dropped term) would actually produce -- still fails loudly; a handful of the
    /// falloff-edge directions may need re-diffing the same way if a future legitimate
    /// change (not a regression) shifts them past `2` again.
    #[test]
    #[allow(clippy::unreadable_literal)]
    fn studio_presets_are_unchanged_by_the_split() {
        /// Largest tolerated bit-pattern distance between a freshly computed value and
        /// its golden. Every baseline value here is positive and finite, so `u32` bit
        /// patterns increase monotonically with the represented value exactly like ULP
        /// distance does -- a signed difference of the raw bit patterns IS the ULP
        /// distance, no separate distance function needed. See the doc comment above
        /// for why this must stay tight rather than a wider, looser bound.
        const MAX_ULP_DIFF: i64 = 2;
        const BASELINE_BITS: [u32; 192] = [
            1021303259, 1023655294, 1023453223, 1021200981, 1023595102, 1023378703, 1021099050,
            1023535114, 1023261534, 1020997254, 1023475205, 1023144520, 1021568337, 1023811298,
            1023605575, 1022776056, 1024522061, 1024299703, 1020689823, 1023178377, 1022791133,
            1027755127, 1030009410, 1029658621, 1044895794, 1047214421, 1046853620, 1020392900,
            1022828889, 1022449825, 1020281302, 1022697534, 1022321544, 1020178665, 1022576726,
            1022203564, 1061137586, 1063361424, 1063015373, 1083905863, 1085705252, 1085425249,
            1019902580, 1022251765, 1021886209, 1046107587, 1048608372, 1048246559, 1076756650,
            1078775455, 1078461309, 1093086491, 1095026095, 1094724274, 1100967474, 1102817211,
            1102529373, 1102501898, 1104623281, 1104293173, 1100418369, 1102170895, 1101898185,
            1101296235, 1103204173, 1102907279, 1072082398, 1074250299, 1074042063, 1075709089,
            1077542440, 1077257152, 1081883525, 1083470199, 1083242507, 1084303182, 1086172910,
            1085881962, 1078120145, 1080380336, 1080028628, 1050792595, 1052670085, 1052377929,
            1047922906, 1049676718, 1049454622, 1044118982, 1046300086, 1045960685, 1018488180,
            1020586967, 1020260376, 1018134306, 1020170445, 1019853601, 1018031798, 1020049789,
            1019735769, 1075090800, 1076814691, 1076546437, 1068234284, 1070229409, 1069918948,
            1017725102, 1019688798, 1019383227, 1017689160, 1019646492, 1019341912, 1017521024,
            1019448590, 1019148641, 1028692342, 1031112546, 1030735938, 1017420804, 1019330627,
            1019033440, 1017213944, 1019087147, 1018795658, 1039204674, 1041094121, 1040876565,
            1018976767, 1021162051, 1020821999, 1016907250, 1018726157, 1018443117, 1016805017,
            1018605826, 1018325602, 1016702785, 1018485494, 1018208087, 1026205465, 1028185404,
            1027877306, 1016519389, 1018269632, 1017997277, 1016396090, 1018124504, 1017855546,
            1016303883, 1018015974, 1017749556, 1016191627, 1017883843, 1017620518, 1016089641,
            1017763802, 1017503286, 1015987163, 1017643183, 1017385490, 1015884931, 1017522852,
            1017267976, 1015807869, 1017432147, 1017179394, 1015680469, 1017282193, 1017032949,
            1015578236, 1017161861, 1016915434, 1015476004, 1017041531, 1016797920, 1015373773,
            1016921202, 1016680407, 1015271541, 1016800870, 1016562892, 1015169309, 1016680540,
            1016445378, 1015067077, 1016560210, 1016327864, 1014908125, 1016439880, 1016210351,
            1014703659, 1016319549, 1016092836,
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
                let got_bits = val.to_bits();
                let expected_bits = BASELINE_BITS[idx];
                let ulp_diff = (i64::from(got_bits) - i64::from(expected_bits)).abs();
                assert!(
                    ulp_diff <= MAX_ULP_DIFF,
                    "divergence at dir {k}, lambda {lambda}: got {val} (bits {got_bits}), \
                     expected bits {expected_bits} ({ulp_diff} ULP away, tolerance \
                     {MAX_ULP_DIFF})"
                );
                idx += 1;
            }
        }
    }
}
