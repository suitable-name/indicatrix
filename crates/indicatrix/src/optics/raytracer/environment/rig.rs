//! Numerically evaluates radiance for a sampled direction.
//!
//! The analytic studio rig's per-model falloff shapes
//! ([`sample_studio_environment_with_rig`] and its
//! `Studio`/`IsoHemisphere`/`LightTent`/`DaylightDome` arms), the observer head-shadow
//! and horizon blends they share, the backdrop-card fill for a camera ray that misses
//! the stone, and next-event-estimation sampling/pdf lookups against an `HdrMap`.

use super::{EnvironmentSource, LightingModel, LightingPreset, LightingRigParams};
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
pub(in super::super) struct EnvNeeSample {
    pub(in super::super) dir: Vec3,
    pub(in super::super) pdf: f32,
    pub(in super::super) rgb: [f32; 3],
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
pub(in super::super) fn sample_environment_for_nee(
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
pub(in super::super) fn environment_nee_pdf(environment: EnvironmentSource<'_>, dir: Vec3) -> f32 {
    match environment {
        EnvironmentSource::Studio { .. } => 0.0,
        EnvironmentSource::HdrMap(map) => map.pdf(dir),
    }
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
/// head-shadow cone around `observer` (see
/// [`sample_studio_environment_observed`](super::sample_studio_environment_observed)).
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
        }
    }
}

fn iso_hemisphere_lighting(d: Vec3, observer: Vec3) -> DirectionLighting {
    DirectionLighting::Scaled(horizon_blend(d) * observer_visibility(d, observer))
}

fn light_tent_lighting(
    d: Vec3,
    spot_mult: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
) -> DirectionLighting {
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
    DirectionLighting::Scaled(above + ground)
}

fn daylight_dome_lighting(
    d: Vec3,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
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
    let sun = smoothstep(SUN_OUTER_COS, SUN_INNER_COS, sun_dot) * 10.0;
    let above = ((sky + aureole) + sun) * (horizon * observer_visibility(d, observer));
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
) -> DirectionLighting {
    let d = dir.normalize();
    let LightingRigParams { spot_mult, .. } = lighting_preset.params();
    match lighting_preset.model() {
        LightingModel::Studio => studio_rig_lighting(
            d,
            spot_mult,
            exposure,
            rig,
            lighting_preset.has_ambient_fill(),
        ),
        LightingModel::IsoHemisphere => iso_hemisphere_lighting(d, observer),
        LightingModel::LightTent => light_tent_lighting(d, spot_mult, rig, observer),
        LightingModel::DaylightDome => daylight_dome_lighting(d, rig, observer),
    }
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
    direction_lighting(dir, lighting_preset, exposure, rig, observer)
        .radiance(lighting_preset.spectral_power(lambda_nm), exposure)
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
) -> [f32; N] {
    let lighting = direction_lighting(dir, lighting_preset, exposure, rig, observer);
    let spectrum = lighting_preset.illuminant_spectrum();
    std::array::from_fn(|k| lighting.radiance(spectrum.power(lambdas[k]), exposure))
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
            preset, exposure, ..
        } => {
            let rig = studio_rig
                .expect("sample_environment_channels: Studio environment needs a pre-built rig");
            sample_studio_environment_channels(dir, lambdas, preset, exposure, rig, observer)
        }
        EnvironmentSource::HdrMap(map) => std::array::from_fn(|k| map.radiance_at(dir, lambdas[k])),
    }
}
