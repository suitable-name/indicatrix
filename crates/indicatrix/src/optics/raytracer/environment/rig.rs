//! Numerically evaluates radiance for a sampled direction.
//!
//! The analytic studio rig's per-model falloff shapes
//! ([`sample_studio_environment_with_rig`] and its
//! `Studio`/`IsoHemisphere`/`LightTent`/`DaylightDome` arms), the observer head-shadow
//! and horizon blends they share, the backdrop-card fill for a camera ray that misses
//! the stone, and next-event-estimation sampling/pdf lookups against an `HdrMap`.

use super::{
    DEFAULT_HEAD_SHADOW_COSINES, EnvironmentSource, LightingModel, LightingPreset,
    LightingRigParams, head_shadow_cosines,
};
use crate::optics::studio_rig::RING_LIGHT_COUNT;
use glam::Vec3;

/// Looks up channel `lambda_nm`'s spectral radiance, in direction `dir`, for a ray that
/// missed the gemstone and is now sampling `environment`. Pulled out of
/// `trace_spectral_ray`'s miss branch so the `HdrMap` arm doesn't grow that
/// already-oversized function.
///
/// Takes the `Studio` variant's [`StudioRig`](crate::optics::studio_rig::StudioRig)
/// pre-built (`studio_rig`) rather than reconstructing it from `light_yaw`/
/// `light_pitch` on every call -- the trace builds it once per ray and both
/// `accumulate_miss_radiance` and the exit-split probe borrow it. `observer` is
/// the unit direction from the stone towards the eye (see
/// [`sample_studio_environment_observed`](super::sample_studio_environment_observed)).
/// Both are unused for `HdrMap`.
#[inline]
pub(in super::super) fn sample_environment_channel(
    environment: EnvironmentSource<'_>,
    dir: Vec3,
    lambda_nm: f32,
    studio_rig: Option<&crate::optics::studio_rig::StudioRig>,
    observer: Vec3,
) -> f32 {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            head_shadow_deg,
            ..
        } => {
            let rig = studio_rig
                .expect("sample_environment_channel: Studio environment needs a pre-built rig");
            sample_studio_environment_with_rig_shadow(
                dir,
                lambda_nm,
                preset,
                exposure,
                rig,
                observer,
                head_shadow_cosines(head_shadow_deg),
            )
        }
        EnvironmentSource::HdrMap(map) => map.radiance_at(dir, lambda_nm),
    }
}

/// Fills every channel of `radiance` with the backdrop card's radiance if the scene
/// has one, and reports whether it did. For the camera ray only (bounce 0: unit Stokes
/// intensity, no NEE weight), so the assignment is that ray's whole contribution.
pub(in super::super) fn fill_backdrop<const N: usize>(
    environment: EnvironmentSource<'_>,
    lambdas: &[f32; N],
    radiance: &mut [f32; N],
) -> bool {
    match environment {
        EnvironmentSource::Studio {
            preset, backdrop, ..
        } if backdrop > 0.0 => {
            // The preset's Planck constants are wavelength-independent, so they are
            // resolved once for the whole channel loop; each value is unchanged.
            let spectrum = preset.illuminant_spectrum();
            for (out, &lambda_nm) in radiance.iter_mut().zip(lambdas) {
                *out = backdrop * spectrum.power(lambda_nm);
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
///
/// [`EnvironmentMap::sample`]: crate::renderer::env_map::EnvironmentMap::sample
/// [`EnvironmentMap::radiance_at`]: crate::renderer::env_map::EnvironmentMap::radiance_at
///
/// For the analytic sun ([`LightingModel::DaylightSun`]) `rgb` is unused and `sun` carries
/// the wavelength-independent radiance factor instead, because the sun's radiance is the
/// preset's own spectral power times a scalar, not an RGB texel: use [`Self::radiance`],
/// which picks the right source.
pub(in super::super) struct EnvNeeSample {
    pub(in super::super) dir: Vec3,
    pub(in super::super) pdf: f32,
    pub(in super::super) rgb: [f32; 3],
    /// `Some` for an analytic-sun draw, `None` for an HDR-map draw.
    pub(in super::super) sun: Option<SunNeeRadiance>,
}

/// The radiance of an analytic-sun NEE draw: `geometry * (spectral_power(lambda) *
/// exposure)`, the very expression [`DirectionLighting::Scaled`] evaluates for a direction
/// inside the disc, so the light-sampled and the BSDF-sampled technique see the same
/// radiance.
#[derive(Clone, Copy)]
pub(in super::super) struct SunNeeRadiance {
    geometry: f32,
    exposure: f32,
    preset: LightingPreset,
}

impl EnvNeeSample {
    /// This draw's spectral radiance at `lambda_nm`: the HDR texel lifted to a spectrum, or
    /// the sun's `geometry * (spectral_power * exposure)`.
    #[must_use]
    pub(in super::super) fn radiance(&self, lambda_nm: f32) -> f32 {
        self.sun.map_or_else(
            || crate::renderer::env_map::rgb_to_spectral_radiance(self.rgb, lambda_nm),
            |sun| sun.geometry * (sun.preset.spectral_power(lambda_nm) * sun.exposure),
        )
    }
}

/// Whether `environment` offers a light-sampling (NEE) technique at all: an HDR map
/// (importance-sampled texels) or the analytic sun of [`LightingPreset::DaylightSun`]
/// (uniform cone sampling). Every other analytic rig is smooth enough for BSDF sampling
/// alone. Replaces the old `matches!(environment, HdrMap(_))` switch at the entry points.
#[must_use]
pub(in super::super) fn environment_supports_nee(environment: EnvironmentSource<'_>) -> bool {
    match environment {
        EnvironmentSource::Studio { preset, .. } => preset.model() == LightingModel::DaylightSun,
        EnvironmentSource::HdrMap(_) => true,
    }
}

/// Draws one NEE direction from `environment`'s own importance distribution, from two
/// independent uniform `[0, 1)` randoms.
///
/// - [`EnvironmentSource::HdrMap`]: the map's importance distribution; `None` for a
///   degenerate draw (`pdf <= 0.0`, e.g. a direction whose row/column solid angle collapsed
///   to nothing at a pole).
/// - [`EnvironmentSource::Studio`] with [`LightingPreset::DaylightSun`]: a direction
///   uniform over the sun disc ([`sun_cone_direction`]), `pdf = 1 / SUN_SOLID_ANGLE`.
/// - every other analytic rig: `None` (no light-sampling technique, see
///   `scattering::NeeContext`'s doc comment).
///
/// Either way a `None` means the caller should simply skip this NEE contribution for the
/// current event, not fabricate one.
#[must_use]
pub(in super::super) fn sample_environment_for_nee(
    environment: EnvironmentSource<'_>,
    u0: f32,
    u1: f32,
) -> Option<EnvNeeSample> {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            light_yaw,
            light_pitch,
            ..
        } if preset.model() == LightingModel::DaylightSun => {
            let key_dir = key_dir_only(light_yaw, light_pitch);
            Some(EnvNeeSample {
                dir: sun_cone_direction(key_dir, u0, u1),
                pdf: 1.0 / SUN_SOLID_ANGLE,
                rgb: [0.0; 3],
                sun: Some(SunNeeRadiance {
                    geometry: sun_radiance_factor(key_dir),
                    exposure,
                    preset,
                }),
            })
        }
        EnvironmentSource::Studio { .. } => None,
        EnvironmentSource::HdrMap(map) => {
            let (dir, rgb, pdf) = map.sample(u0, u1);
            (pdf > 0.0).then_some(EnvNeeSample {
                dir,
                pdf,
                rgb,
                sun: None,
            })
        }
    }
}

/// The key light's direction for a light pose, with exactly the arithmetic of
/// `StudioRig::new`'s `key_dir` (bit-identical, pinned by a test) but without building the
/// whole rig (~40 sin/cos and 16 normalizes), which the per-event NEE entry points do not need.
fn key_dir_only(light_yaw: f32, light_pitch: f32) -> Vec3 {
    let cos_lp = light_pitch.cos();
    let sin_lp = light_pitch.sin();
    let cos_ly = light_yaw.cos();
    let sin_ly = light_yaw.sin();
    Vec3::new(cos_lp * sin_ly, sin_lp, cos_lp * cos_ly).normalize()
}

/// Test hook: [`key_dir_only`] for the bit-identity test against `StudioRig`.
#[cfg(test)]
pub(super) fn key_dir_only_for_test(light_yaw: f32, light_pitch: f32) -> Vec3 {
    key_dir_only(light_yaw, light_pitch)
}

/// The solid-angle-measure pdf [`sample_environment_for_nee`] would assign to `dir`,
/// computed independently of any particular sample -- the OTHER half of a balance-heuristic
/// MIS weight: evaluating the light-sampling technique's own density at a direction the
/// COMPETING (BSDF/phase) technique produced. `0.0` for every [`EnvironmentSource::Studio`]
/// preset except [`LightingPreset::DaylightSun`] (matching [`sample_environment_for_nee`]'s
/// `None`: no light-sampling technique exists to compete against there); for the sun it is
/// `1 / SUN_SOLID_ANGLE` inside the disc and `0.0` outside ([`sun_nee_pdf`]).
#[must_use]
pub(in super::super) fn environment_nee_pdf(environment: EnvironmentSource<'_>, dir: Vec3) -> f32 {
    match environment {
        EnvironmentSource::Studio {
            preset,
            light_yaw,
            light_pitch,
            ..
        } if preset.model() == LightingModel::DaylightSun => {
            sun_nee_pdf(key_dir_only(light_yaw, light_pitch), dir)
        }
        EnvironmentSource::Studio { .. } => 0.0,
        EnvironmentSource::HdrMap(map) => map.pdf(dir),
    }
}

// ---------------------------------------------------------------------------------
// The direct sun of `LightingModel::DaylightSun` (GPU id 5).
//
// Geometry. A disc of angular radius 0.27 degrees about the key direction (the real sun
// is 0.266 +- 0.008). `SUN_DISC_COS` is cos(0.27 deg) rounded to f32; the f32 value is
// `1 - 186 * 2^-24`, so the solid angle is derived from that very literal:
//     SUN_ONE_MINUS_COS = 186 * 2^-24                  = 1.1086464e-5
//     SUN_SOLID_ANGLE   = 2 pi * SUN_ONE_MINUS_COS     = 6.9658306e-5 sr
// (the exact small-angle value pi r^2 for 0.27 degrees is 6.9766e-5 sr; the 0.16 % difference
// is the f32 rounding of the cosine, and is irrelevant because every consumer reads the
// literals; the disc radius it encodes is 0.2698 degrees).
//
// Radiance. The sky it sits in is `DaylightDome`'s: `sky(y) = 0.18 - 0.08 y` (y = d.y)
// plus a `0.30 cos^8` aureole around the key. Horizontal irradiance of that sky on an
// upward-facing plane (unit radiance units of the model):
//     E_sky_dome = 2 pi * integral_0^1 (0.18 y - 0.08 y^2) dy = 2 pi (0.09 - 0.08 / 3)
//                = 0.39794
//     E_aureole  = 0.30 * sin(e) * 2 pi / 10          (lobe fully above the horizon for
//                                                      e >~ 30 deg; sin(e) = key.y)
//                = 0.17927 at the default key pitch of 72 degrees (sin = 0.95106)
//     E_sky      = 0.57721 at e = 72 deg.
// The direct sun delivers E_sun = L_sun * SUN_SOLID_ANGLE * sin(e). Wanting E_sun about
// 82 % of E_sky + E_sun (a clear day: 80-85 % direct, the rest skylight):
//     E_sun = 0.82 / 0.18 * 0.57721 = 2.6295  ->  L_sun = 2.6295 / (6.9658e-5 * 0.95106)
//           = 39 691  ->  SUN_RADIANCE = 40 000   (E_sun = 2.650, direct share 82.1 %).
// At other key elevations the share follows sin(e): 73.9 % at 30 degrees (E_sun 1.393,
// E_sky 0.492), 87.5 % at 90 degrees. The radiance ratio to the sky (40 000 / 0.14 ~ 3e5)
// is the right order for a real clear sky (sun ~1.6e9 cd/m2, sky ~5e3 cd/m2, ratio ~3e5).
//
// The sun is NOT dimmed by the observer's head shadow: the shadow models the observer's
// body blocking the large sky dome reflected in a face-up table, and the NEE draw has no
// access to the observer. It IS faded by the horizon blend evaluated once at the key
// direction (`sun_radiance_factor`), so a key below the horizon switches the sun off
// everywhere on the disc.
//
// Spectrum: D65 for now. Direct sun is ~5500-5800 K (warmer than the 6500 K whole-sky mix);
// a separate sun SPD would need its own `spec_power` input in the twins.
// ---------------------------------------------------------------------------------

/// Cosine of the sun disc's angular radius (0.27 degrees), as an f32 literal.
pub(super) const SUN_DISC_COS: f32 = 0.999_988_9;
/// `1 - SUN_DISC_COS`, exactly (`186 * 2^-24`), kept as its own literal so the cone sampler
/// never subtracts two nearly equal numbers.
pub(super) const SUN_ONE_MINUS_COS: f32 = 1.108_646_4e-5;
/// Solid angle of the sun disc in steradians: `2 pi * SUN_ONE_MINUS_COS`.
pub(super) const SUN_SOLID_ANGLE: f32 = 6.965_831e-5;
/// Radiance of the sun disc in the dome's radiance units (see the derivation above).
pub(super) const SUN_RADIANCE: f32 = 40_000.0;

/// The sun's wavelength-independent radiance factor: [`SUN_RADIANCE`] faded by the horizon
/// blend at the key direction. The disc is uniform, so this one number is the sun's radiance
/// for every direction inside it, for the BSDF-sampled lookup and the NEE draw alike.
pub(super) fn sun_radiance_factor(key_dir: Vec3) -> f32 {
    SUN_RADIANCE * horizon_blend(key_dir)
}

/// A direction drawn uniformly over the sun disc about `key_dir` from two uniform `[0, 1)`
/// randoms (equal-area cone sampling: `1 - cos(theta) = u0 * (1 - cos(theta_max))`).
/// `sin(theta)` comes from `(1 - cos)(1 + cos)` with `1 - cos` kept exact, so the radial
/// resolution is not quantised by the f32 spacing of the cosine near 1. The WGSL twin
/// `daylight_sun_cone_direction` is operation for operation the same.
pub(super) fn sun_cone_direction(key_dir: Vec3, u0: f32, u1: f32) -> Vec3 {
    let one_minus_cos = u0 * SUN_ONE_MINUS_COS;
    let cos_t = 1.0 - one_minus_cos;
    let sin_t = (one_minus_cos * (2.0 - one_minus_cos)).max(0.0).sqrt();
    let phi = 2.0 * std::f32::consts::PI * u1;
    let (sin_p, cos_p) = phi.sin_cos();
    let a = if key_dir.x.abs() > 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let t = (a - key_dir * key_dir.dot(a)).normalize_or_zero();
    let b = key_dir.cross(t);
    (t * (sin_t * cos_p) + b * (sin_t * sin_p) + key_dir * cos_t).normalize_or_zero()
}

/// The solid-angle pdf of [`sun_cone_direction`] at `dir`: `1 / SUN_SOLID_ANGLE` inside the
/// disc, `0.0` outside. `dir` must be a unit vector.
pub(super) fn sun_nee_pdf(key_dir: Vec3, dir: Vec3) -> f32 {
    if dir.dot(key_dir) >= SUN_DISC_COS {
        1.0 / SUN_SOLID_ANGLE
    } else {
        0.0
    }
}

/// Cosines of the cone half-angles the lit models are built from. Literal values,
/// never computed, so the CPU and the WGSL twins (`transport_physics.wgsl`,
/// `transport_bounce.wgsl`, `environment.wgsl`) use identical bits.
///
/// The observer head shadow is not a constant: it is a scene parameter
/// (`EnvironmentSource::Studio::head_shadow_deg`, default 16 degrees = fully dark within
/// 14 degrees of the eye direction, gone by 18), evaluated to the `[outer, inner]`
/// cosine pair once by [`head_shadow_cosines`](super::head_shadow_cosines) and passed in.
/// (The sun disc of `DaylightSun` is a hard-edged 0.27 degree cone, see `SUN_DISC_COS`
/// below; `DaylightDome` has no disc any more.)
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
/// head-shadow cone around `observer` (see
/// [`sample_studio_environment_observed`](super::sample_studio_environment_observed)).
fn observer_visibility(d: Vec3, observer: Vec3, shadow: [f32; 2]) -> f32 {
    1.0 - smoothstep(shadow[0], shadow[1], d.dot(observer))
}

/// `0.0` below the girdle plane, `1.0` above, blended over `-0.05..0.05` in `d.y` so
/// the horizon never aliases.
fn horizon_blend(d: Vec3) -> f32 {
    smoothstep(-0.05, 0.05, d.y)
}

/// One direction's lighting with every wavelength-independent factor already evaluated.
///
/// Only the illuminant's spectral power differs between the channels that look at the
/// same direction, so the geometry (normalisation, dot products, `powi`s, smoothsteps) is
/// built once per direction and [`Self::radiance`] finishes each channel with the same
/// multiplies and fused multiply-adds, in the same order, the one-shot evaluation used.
#[derive(Clone, Copy)]
enum DirectionLighting {
    /// The analytic studio rig: the backdrop term plus the key, fill and ring intensities
    /// that passed their thresholds, each already scaled by exposure.
    Rig {
        /// Backdrop intensity (multiplied by the spectral power, not fused).
        bg_val: f32,
        /// Key softbox intensity, `None` when `d` is not in the key's forward hemisphere.
        softbox: Option<f32>,
        /// Fill softbox intensity, `None` when `d` is not in the fill's forward hemisphere.
        fill: Option<f32>,
        /// Intensities of the ring emitters whose `dot > 0.96`, in ring order.
        ring: [f32; RING_LIGHT_COUNT],
        /// How many leading entries of `ring` are live.
        ring_len: usize,
    },
    /// A lit model: a geometric factor scaled by `spec_power * exposure`.
    Scaled(f32),
    /// The ASET contrast view: three zone amplitudes (they sum to the horizon blend)
    /// lit by one narrow spectral band each. For this variant the `spec_power` argument
    /// of [`Self::radiance`] is the wavelength in nm, not an illuminant power (see
    /// [`aset_spec_input`]).
    Banded { red: f32, green: f32, blue: f32 },
}

/// Centre wavelengths (nm) of the three ASET zone bands.
const ASET_RED_NM: f32 = 610.0;
const ASET_GREEN_NM: f32 = 540.0;
const ASET_BLUE_NM: f32 = 460.0;
/// Gaussian sigma of an ASET band: 20 nm FWHM, `20 / (2 * sqrt(2 ln 2))`.
const ASET_SIGMA_NM: f32 = 8.493_218;
/// Per-zone radiance gains. A 20 nm line carries a twentieth of the energy of the broad
/// D65 curve and the eye is far less sensitive at 460 nm than at 540 nm, so the gains
/// lift the bands to comparable perceived brightness (estimated from the V(lambda) values
/// at the band centres, to be tuned by eye).
const ASET_RED_GAIN: f32 = 8.0;
const ASET_GREEN_GAIN: f32 = 5.0;
const ASET_BLUE_GAIN: f32 = 16.0;
/// Elevation zone edges as `sin(elevation)` (= `d.y`): 45 and 75 degrees, each blended over
/// +-0.02 so a zone boundary never aliases.
const ASET_EDGE_45_LO: f32 = 0.687_106_8;
const ASET_EDGE_45_HI: f32 = 0.727_106_8;
const ASET_EDGE_75_LO: f32 = 0.945_925_8;
const ASET_EDGE_75_HI: f32 = 0.985_925_8;

/// The value the ASET model reads as its `spec_power`: the wavelength itself, so the zone
/// bands can be evaluated where the model runs. Every other preset gets its illuminant's
/// relative power.
const fn aset_spec_input(lambda_nm: f32) -> f32 {
    lambda_nm
}

/// Unit-peak Gaussian band at `centre_nm`.
fn aset_band(lambda_nm: f32, centre_nm: f32) -> f32 {
    let z = (lambda_nm - centre_nm) / ASET_SIGMA_NM;
    (-0.5 * z * z).exp()
}

impl DirectionLighting {
    /// The radiance of this direction under an illuminant of relative power `spec_power`.
    fn radiance(self, spec_power: f32, exposure: f32) -> f32 {
        match self {
            Self::Rig {
                bg_val,
                softbox,
                fill,
                ring,
                ring_len,
            } => {
                let mut radiance = bg_val * spec_power;
                if let Some(softbox) = softbox {
                    radiance = softbox.mul_add(spec_power, radiance);
                }
                if let Some(fill) = fill {
                    radiance = fill.mul_add(spec_power, radiance);
                }
                for &intensity in &ring[..ring_len] {
                    radiance = intensity.mul_add(spec_power, radiance);
                }
                radiance
            }
            Self::Scaled(factor) => factor * (spec_power * exposure),
            Self::Banded { red, green, blue } => {
                let lambda_nm = spec_power;
                let r = (red * aset_band(lambda_nm, ASET_RED_NM)) * ASET_RED_GAIN;
                let g = (green * aset_band(lambda_nm, ASET_GREEN_NM)) * ASET_GREEN_GAIN;
                let b = (blue * aset_band(lambda_nm, ASET_BLUE_NM)) * ASET_BLUE_GAIN;
                ((r + g) + b) * exposure
            }
        }
    }
}

fn iso_hemisphere_lighting(d: Vec3, observer: Vec3, shadow: [f32; 2]) -> DirectionLighting {
    DirectionLighting::Scaled(horizon_blend(d) * observer_visibility(d, observer, shadow))
}

fn light_tent_lighting(
    d: Vec3,
    spot_mult: f32,
    tent: super::TentParams,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    // Tent walls: 0.14 at the girdle plane rising to 0.22 at the zenith -- middle grey
    // after the ACES curve, so a facet that sees nothing but the tent is grey, not
    // white. The black cards cut that to a tenth.
    //
    // The per-preset parameters are exact identities at `TentParams::DEFAULT` (`x * 1.0`
    // is `x` in f32, and the ground literal is the same `0.02`), so `LightTent` stays bit
    // for bit what it was; the other tent presets only change the four floats.
    let mut walls = 0.08f32.mul_add(d.y.max(0.0), 0.14) * tent.walls;
    // `flat` blends the gradient towards its value at 30 degrees elevation; the branch (not
    // a `mix` by zero) keeps `flat == 0` bit-identical to the old arithmetic.
    if tent.flat > 0.0 {
        walls = (0.18 * tent.walls).mul_add(tent.flat, walls * (1.0 - tent.flat));
    }
    let mut card = 0.0f32;
    for slot in CARD_RING_SLOTS {
        card = card.max(smoothstep(
            CARD_OUTER_COS,
            CARD_INNER_COS,
            d.dot(rig.ring_dirs[slot]),
        ));
    }
    walls *= (card * tent.cards).mul_add(-0.9, 1.0);
    let key =
        smoothstep(TENT_KEY_OUTER_COS, TENT_KEY_INNER_COS, d.dot(rig.key_dir)) * (1.4 * spot_mult);
    let spark = smoothstep(SPARK_OUTER_COS, SPARK_INNER_COS, d.dot(rig.fill_dir))
        * (5.0 * spot_mult)
        * tent.spark;
    let above = ((walls + key) + spark) * (horizon * observer_visibility(d, observer, shadow));
    let ground = tent.ground * (1.0 - horizon);
    DirectionLighting::Scaled(above + ground)
}

fn daylight_dome_lighting(
    d: Vec3,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let sun_dot = d.dot(rig.key_dir);
    // A clear sky is brightest at the horizon and around the sun, darkest at the zenith.
    let sky = 0.08f32.mul_add(1.0 - d.y.max(0.0), 0.10);
    // `sun_dot^8`, written as three squarings so the GPU twin multiplies identically.
    let glow = sun_dot.max(0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    // Sky only: the old 2-4 degree "sun" (1 % of the irradiance, with no NEE to make it
    // anything but a glow) is gone; `DaylightSun` is the preset with a real sun. For every
    // direction the old smoothstep disc left at zero this is bit-identical to before.
    let above = (sky + aureole) * (horizon * observer_visibility(d, observer, shadow));
    let ground = 0.04 * (1.0 - horizon);
    DirectionLighting::Scaled(above + ground)
}

fn studio_rig_lighting(
    d: Vec3,
    spot_mult: f32,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    ambient_fill: bool,
) -> DirectionLighting {
    // 1. Ambient luxury studio backdrop (pure neutral dark charcoal velvet); absent for
    //    the UV lamps (dark room).
    let bg_val = if ambient_fill {
        0.012f32.mul_add(d.y.mul_add(0.5, 0.5), 0.015).max(0.005) * exposure
    } else {
        0.0
    };

    // 2. Main Key Softbox Light
    let key_dot = d.dot(rig.key_dir).max(0.0);
    let softbox = (key_dot > 0.0).then(|| key_dot.powi(28) * 12.0 * spot_mult * exposure);

    // 3. Fill Softbox Light (side reflector offset by 140 deg)
    let fill_dot = d.dot(rig.fill_dir).max(0.0);
    let fill = (fill_dot > 0.0).then(|| fill_dot.powi(18) * 4.5 * exposure);

    // 4. Circular Ring Scintillation Lights (16 sparkling pinpoint sources rotating with lighting rig)
    let mut ring = [0.0f32; RING_LIGHT_COUNT];
    let mut ring_len = 0;
    for ring_dir in rig.ring_dirs {
        let ring_dot = d.dot(ring_dir).max(0.0);
        if ring_dot > 0.96 {
            let spark = (ring_dot - 0.96) / 0.04;
            ring[ring_len] = spark.powi(6) * 22.0 * spot_mult * exposure;
            ring_len += 1;
        }
    }

    DirectionLighting::Rig {
        bg_val,
        softbox,
        fill,
        ring,
        ring_len,
    }
}

/// Evaluates `lighting_preset`'s wavelength-independent lighting of `dir` once.
fn direction_lighting(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
) -> DirectionLighting {
    let d = dir.normalize();
    let LightingRigParams {
        spot_mult, tent, ..
    } = lighting_preset.params();
    match lighting_preset.model() {
        LightingModel::Studio => studio_rig_lighting(
            d,
            spot_mult,
            exposure,
            rig,
            lighting_preset.has_ambient_fill(),
        ),
        LightingModel::IsoHemisphere => iso_hemisphere_lighting(d, observer, shadow),
        LightingModel::LightTent => light_tent_lighting(d, spot_mult, tent, rig, observer, shadow),
        LightingModel::DaylightDome => daylight_dome_lighting(d, rig, observer, shadow),
        LightingModel::Aset => aset_radiance(d, rig, observer, shadow),
        LightingModel::DaylightSun => daylight_sun_radiance(d, rig, observer, shadow),
    }
}

/// ASET-style contrast view (model id 4), after the Angular Spectrum Evaluation Tool.
///
/// Zones by elevation above the horizon: green 0-45 degrees (low-angle light), red 45-75
/// (the high, useful light) and blue 75-90, the region straight above the stone where the
/// viewer's own head and lens sit (in a real ASET that is the obstruction zone; here it is
/// a fixed elevation cap rather than a cone around the view ray, so the map does not move
/// with the camera). Black below the horizon. Each zone emits one narrow band, so the
/// stone's facets show which zone they return light from. The head shadow is not applied
/// (`observer`/`shadow` are unused): the blue cap is the head region.
fn aset_radiance(
    d: Vec3,
    _rig: &crate::optics::studio_rig::StudioRig,
    _observer: Vec3,
    _shadow: [f32; 2],
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let s45 = smoothstep(ASET_EDGE_45_LO, ASET_EDGE_45_HI, d.y);
    let s75 = smoothstep(ASET_EDGE_75_LO, ASET_EDGE_75_HI, d.y);
    DirectionLighting::Banded {
        red: (s45 * (1.0 - s75)) * horizon,
        green: (1.0 - s45) * horizon,
        blue: s75 * horizon,
    }
}

/// Daylight sky plus a physically bright direct sun (model id 5): `DaylightDome`'s sky,
/// aureole, ground and head shadow, plus the hard-edged 0.27 degree sun disc of
/// [`SUN_RADIANCE`] (see the derivation above [`SUN_DISC_COS`]). The sun is not scaled by the
/// head shadow; it carries its own horizon fade ([`sun_radiance_factor`]). The disc is
/// sampled analytically by next-event estimation ([`sample_environment_for_nee`]).
fn daylight_sun_radiance(
    d: Vec3,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let sun_dot = d.dot(rig.key_dir);
    let sky = 0.08f32.mul_add(1.0 - d.y.max(0.0), 0.10);
    let glow = sun_dot.max(0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let above = (sky + aureole) * (horizon * observer_visibility(d, observer, shadow));
    let ground = 0.04 * (1.0 - horizon);
    let sun = if sun_dot >= SUN_DISC_COS {
        sun_radiance_factor(rig.key_dir)
    } else {
        0.0
    };
    DirectionLighting::Scaled((above + ground) + sun)
}

/// Radiance along `dir` for a pre-built [`StudioRig`](crate::optics::studio_rig::StudioRig).
///
/// The rig-independent body of
/// [`sample_studio_environment_observed`](super::sample_studio_environment_observed):
/// identical arithmetic, in the identical order, just reading `key_dir`/`fill_dir`/
/// `ring_dirs`/`sin_light_pitch` off an already-built `rig` instead of constructing one
/// from `(light_yaw, light_pitch)` itself. Dispatches on the preset's [`LightingModel`];
/// the `Studio` arm is the original rig body, untouched.
///
/// The entry point for callers that look up many directions under one light pose: build
/// the [`StudioRig`](crate::optics::studio_rig::StudioRig) once and pass it to every call.
#[must_use]
pub fn sample_studio_environment_with_rig(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
) -> f32 {
    sample_studio_environment_with_rig_shadow(
        dir,
        lambda_nm,
        lighting_preset,
        exposure,
        rig,
        observer,
        DEFAULT_HEAD_SHADOW_COSINES,
    )
}

/// [`sample_studio_environment_with_rig`] with an explicit head-shadow cone.
///
/// `shadow` is the `[outer, inner]` cosine pair from [`head_shadow_cosines`](super::head_shadow_cosines)
/// (the default pair reproduces the unparameterised call bit for bit).
#[must_use]
pub fn sample_studio_environment_with_rig_shadow(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
) -> f32 {
    direction_lighting(dir, lighting_preset, exposure, rig, observer, shadow).radiance(
        if lighting_preset == LightingPreset::Aset {
            aset_spec_input(lambda_nm)
        } else {
            lighting_preset.spectral_power(lambda_nm)
        },
        exposure,
    )
}

/// [`sample_studio_environment_with_rig`] for every wavelength in `lambdas` looking along
/// the same `dir`: the direction's geometry and the preset's Planck constants are
/// evaluated once, then each channel only applies its own spectral power. Element `k` is
/// bit-identical to `sample_studio_environment_with_rig(dir, lambdas[k], ..)`, since the
/// hoisted terms never depend on the wavelength.
#[must_use]
fn sample_studio_environment_channels<const N: usize>(
    dir: Vec3,
    lambdas: &[f32; N],
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
) -> [f32; N] {
    let lighting = direction_lighting(dir, lighting_preset, exposure, rig, observer, shadow);
    let spectrum = lighting_preset.illuminant_spectrum();
    let aset = lighting_preset == LightingPreset::Aset;
    std::array::from_fn(|k| {
        let spec_power = if aset {
            aset_spec_input(lambdas[k])
        } else {
            spectrum.power(lambdas[k])
        };
        lighting.radiance(spec_power, exposure)
    })
}

/// [`sample_environment_channel`] for every wavelength in `lambdas` looking along the same
/// `dir` -- the escaped-ray lookup, where all channels share the exit direction. Element
/// `k` is bit-identical to `sample_environment_channel(environment, dir, lambdas[k], ..)`.
#[inline]
pub(in super::super) fn sample_environment_channels<const N: usize>(
    environment: EnvironmentSource<'_>,
    dir: Vec3,
    lambdas: &[f32; N],
    studio_rig: Option<&crate::optics::studio_rig::StudioRig>,
    observer: Vec3,
) -> [f32; N] {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            head_shadow_deg,
            ..
        } => {
            let rig = studio_rig
                .expect("sample_environment_channels: Studio environment needs a pre-built rig");
            sample_studio_environment_channels(
                dir,
                lambdas,
                preset,
                exposure,
                rig,
                observer,
                head_shadow_cosines(head_shadow_deg),
            )
        }
        EnvironmentSource::HdrMap(map) => std::array::from_fn(|k| map.radiance_at(dir, lambdas[k])),
    }
}
