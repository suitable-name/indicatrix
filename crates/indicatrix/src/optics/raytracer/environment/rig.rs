//! Numerically evaluates radiance for a sampled direction.
//!
//! The analytic studio rig's per-model falloff shapes
//! ([`sample_studio_environment_with_rig`] and its
//! `Studio`/`IsoHemisphere`/`LightTent`/`DaylightDome` arms), the observer head-shadow
//! and horizon blends they share, the lit backdrop tray every escape sees below the
//! horizon (`tray > 0.0`; see `tray.rs`). Next-event-estimation lives in `nee.rs`.

use super::sun::{SUN_DISC_COS, sun_radiance_factor};
use super::{
    DEFAULT_HEAD_SHADOW_COSINES, EnvironmentSource, LightingModel, LightingPreset,
    LightingRigParams, d65_relative_spectral_power, head_shadow_cosines,
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
            backdrop,
            ..
        } => {
            let rig = studio_rig
                .expect("sample_environment_channel: Studio environment needs a pre-built rig");
            sample_studio_environment_with_rig_shadow_tray(
                dir,
                lambda_nm,
                preset,
                exposure,
                rig,
                observer,
                head_shadow_cosines(head_shadow_deg),
                backdrop,
            )
        }
        EnvironmentSource::HdrMap(map) => map.radiance_at(dir, lambda_nm),
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
pub(super) fn horizon_blend(d: Vec3) -> f32 {
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
        /// The tray, `Some((horizon, ground))` only with `tray > 0.0`: `horizon` is the
        /// horizon blend and `ground` is `((1 - horizon) * tray) * exposure`, which the
        /// radiance finishes as `fma(horizon, rig, ground * spec_power)`.
        ground: Option<(f32, f32)>,
    },
    /// A lit model: a geometric factor scaled by `spec_power * exposure`.
    Scaled(f32),
    /// The ASET contrast view: three zone amplitudes (they sum to the horizon blend)
    /// lit by one narrow spectral band each. For this variant the `spec_power` argument
    /// of [`Self::radiance`] is the wavelength in nm, not an illuminant power (see
    /// [`aset_spec_input`]).
    /// `tray_ground` is `(1 - horizon) * tray`, `0.0` for As Lit.
    Banded {
        red: f32,
        green: f32,
        blue: f32,
        tray_ground: f32,
    },
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
                ground,
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
                ground.map_or(radiance, |(horizon, ground)| {
                    horizon.mul_add(radiance, ground * spec_power)
                })
            }
            Self::Scaled(factor) => factor * (spec_power * exposure),
            Self::Banded {
                red,
                green,
                blue,
                tray_ground,
            } => {
                let lambda_nm = spec_power;
                let r = (red * aset_band(lambda_nm, ASET_RED_NM)) * ASET_RED_GAIN;
                let g = (green * aset_band(lambda_nm, ASET_GREEN_NM)) * ASET_GREEN_GAIN;
                let b = (blue * aset_band(lambda_nm, ASET_BLUE_NM)) * ASET_BLUE_GAIN;
                let zones = ((r + g) + b) * exposure;
                // A neutral D65 tray (`E / pi := 1`); the branch keeps As Lit's bits.
                if tray_ground > 0.0 {
                    (tray_ground * d65_relative_spectral_power(lambda_nm)).mul_add(exposure, zones)
                } else {
                    zones
                }
            }
        }
    }
}

/// The ground term of the lit models. With a tray (`tray > 0.0`) the tray replaces the
/// model's own ground and gets the model's observer treatment: `((1 - horizon) * vis) * tray`;
/// As Lit keeps `own_ground * (1 - horizon)` exactly (a branch, never a multiply by zero).
fn lit_ground(horizon: f32, vis: f32, tray: f32, own_ground: f32) -> f32 {
    if tray > 0.0 {
        ((1.0 - horizon) * vis) * tray
    } else {
        own_ground * (1.0 - horizon)
    }
}

fn iso_hemisphere_lighting(
    d: Vec3,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let vis = observer_visibility(d, observer, shadow);
    if tray > 0.0 {
        // fma((1 - horizon) * vis, tray, horizon * vis)
        DirectionLighting::Scaled(((1.0 - horizon) * vis).mul_add(tray, horizon * vis))
    } else {
        DirectionLighting::Scaled(horizon * vis)
    }
}

fn light_tent_lighting(
    d: Vec3,
    spot_mult: f32,
    tent: super::TentParams,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
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
    let vis = observer_visibility(d, observer, shadow);
    let above = ((walls + key) + spark) * (horizon * vis);
    let ground = lit_ground(horizon, vis, tray, tent.ground);
    DirectionLighting::Scaled(above + ground)
}

fn daylight_dome_lighting(
    d: Vec3,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
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
    let vis = observer_visibility(d, observer, shadow);
    let above = (sky + aureole) * (horizon * vis);
    let ground = lit_ground(horizon, vis, tray, 0.04);
    DirectionLighting::Scaled(above + ground)
}

fn studio_rig_lighting(
    d: Vec3,
    spot_mult: f32,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    ambient_fill: bool,
    tray: f32,
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
    let mut sparks = [0.0f32; RING_LIGHT_COUNT];
    let mut ring_len = 0;
    for ring_dir in rig.ring_dirs {
        let ring_dot = d.dot(ring_dir).max(0.0);
        if ring_dot > 0.96 {
            let spark = (ring_dot - 0.96) / 0.04;
            sparks[ring_len] = spark.powi(6) * 22.0 * spot_mult * exposure;
            ring_len += 1;
        }
    }

    // The tray replaces everything below the horizon: `fma(h, rig, ((g * tray) * exposure) * S)`.
    // The rig ignores the observer, so the tray does too.
    let ground = (tray > 0.0).then(|| {
        let horizon = horizon_blend(d);
        (horizon, ((1.0 - horizon) * tray) * exposure)
    });

    DirectionLighting::Rig {
        bg_val,
        softbox,
        fill,
        ring: sparks,
        ring_len,
        ground,
    }
}

/// Evaluates `lighting_preset`'s wavelength-independent lighting of `dir` once.
///
/// `tray` is the lit backdrop's radiance at exposure 1 (`0.0` = As Lit, the preset's own
/// ground, which is today's arithmetic untouched; see `tray.rs`).
fn direction_lighting(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
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
            tray,
        ),
        LightingModel::IsoHemisphere => iso_hemisphere_lighting(d, observer, shadow, tray),
        LightingModel::LightTent => {
            light_tent_lighting(d, spot_mult, tent, rig, observer, shadow, tray)
        }
        LightingModel::DaylightDome => daylight_dome_lighting(d, rig, observer, shadow, tray),
        LightingModel::Aset => aset_radiance(d, rig, observer, shadow, tray),
        LightingModel::DaylightSun => daylight_sun_radiance(d, rig, observer, shadow, tray),
    }
}

/// The wavelength-independent geometry of `lighting_preset` along `dir` at exposure 1 with no
/// observer and no tray, `radiance(1.0, 1.0)`: the `G(d)` the tray's irradiance integrates.
pub(super) fn unit_radiance(
    dir: Vec3,
    lighting_preset: LightingPreset,
    rig: &crate::optics::studio_rig::StudioRig,
) -> f32 {
    direction_lighting(
        dir,
        lighting_preset,
        1.0,
        rig,
        Vec3::ZERO,
        DEFAULT_HEAD_SHADOW_COSINES,
        0.0,
    )
    .radiance(1.0, 1.0)
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
    tray: f32,
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let s45 = smoothstep(ASET_EDGE_45_LO, ASET_EDGE_45_HI, d.y);
    let s75 = smoothstep(ASET_EDGE_75_LO, ASET_EDGE_75_HI, d.y);
    DirectionLighting::Banded {
        red: (s45 * (1.0 - s75)) * horizon,
        green: (1.0 - s45) * horizon,
        blue: s75 * horizon,
        tray_ground: if tray > 0.0 {
            (1.0 - horizon) * tray
        } else {
            0.0
        },
    }
}

/// Daylight sky plus a physically bright direct sun (model id 5): `DaylightDome`'s sky,
/// aureole, ground and head shadow, plus the hard-edged 0.27 degree sun disc of
/// [`SUN_RADIANCE`](super::sun::SUN_RADIANCE) (see the derivation above [`SUN_DISC_COS`]). The sun is not scaled by the
/// head shadow; it carries its own horizon fade ([`sun_radiance_factor`]). The disc is
/// sampled analytically by next-event estimation ([`sample_environment_for_nee`]).
fn daylight_sun_radiance(
    d: Vec3,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> DirectionLighting {
    let horizon = horizon_blend(d);
    let sun_dot = d.dot(rig.key_dir);
    let sky = 0.08f32.mul_add(1.0 - d.y.max(0.0), 0.10);
    let glow = sun_dot.max(0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let vis = observer_visibility(d, observer, shadow);
    let above = (sky + aureole) * (horizon * vis);
    let ground = lit_ground(horizon, vis, tray, 0.04);
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
    sample_studio_environment_with_rig_shadow_tray(
        dir,
        lambda_nm,
        lighting_preset,
        exposure,
        rig,
        observer,
        shadow,
        0.0,
    )
}

/// [`sample_studio_environment_with_rig_shadow`] over a lit backdrop tray.
///
/// `tray` is [`EnvironmentSource::Studio`]'s `backdrop`: the tray's radiance at exposure 1
/// (`rho * E_rig / pi`, see `tray.rs`), seen below the horizon in place of the preset's own
/// ground. `0.0` is As Lit and is bit-identical to the unsuffixed entry point.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "the unsuffixed entry point plus the tray, kept flat to mirror its WGSL twin"
)]
pub fn sample_studio_environment_with_rig_shadow_tray(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> f32 {
    direction_lighting(dir, lighting_preset, exposure, rig, observer, shadow, tray)
        .radiance(studio_spec_power(lighting_preset, lambda_nm), exposure)
}

/// The `spec_power` input [`sample_studio_environment_with_rig_shadow`] feeds the radiance
/// at wavelength `lambda_nm`: the wavelength itself for the ASET view, the preset's
/// spectral power otherwise.
///
/// A pure function of `(lighting_preset, lambda_nm)` -- for a Planckian preset two `exp`
/// and a `powi` -- so a caller probing many directions at one wavelength computes it once
/// and passes it to [`sample_studio_environment_with_rig_spec_power`].
#[must_use]
pub fn studio_spec_power(lighting_preset: LightingPreset, lambda_nm: f32) -> f32 {
    if lighting_preset == LightingPreset::Aset {
        aset_spec_input(lambda_nm)
    } else {
        lighting_preset.spectral_power(lambda_nm)
    }
}

/// [`sample_studio_environment_with_rig`] with the wavelength's `spec_power` already evaluated.
///
/// `spec_power` is [`studio_spec_power`]'s value; the result is bit-identical to the wrapper when
/// `spec_power == studio_spec_power(lighting_preset, lambda_nm)`, since the radiance only
/// ever reads the wavelength through that one value.
#[must_use]
pub fn sample_studio_environment_with_rig_spec_power(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    spec_power: f32,
) -> f32 {
    sample_studio_environment_with_rig_spec_power_tray(
        dir,
        lighting_preset,
        exposure,
        rig,
        observer,
        spec_power,
        0.0,
    )
}

/// [`sample_studio_environment_with_rig_spec_power`] with an explicit head-shadow cone.
///
/// `shadow` is the `[outer, inner]` cosine pair from [`head_shadow_cosines`](super::head_shadow_cosines);
/// the default pair gives the same bits as the unsuffixed spec-power entry. One arithmetic
/// path serves every shadow setting (As Lit, no tray).
#[must_use]
pub fn sample_studio_environment_with_rig_spec_power_shadow(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    spec_power: f32,
    shadow: [f32; 2],
) -> f32 {
    sample_studio_environment_with_rig_spec_power_shadow_tray(
        dir,
        lighting_preset,
        exposure,
        rig,
        observer,
        spec_power,
        shadow,
        0.0,
    )
}

/// [`sample_studio_environment_with_rig_spec_power_shadow`] over a lit backdrop tray.
///
/// `tray` is [`EnvironmentSource::Studio`]'s `backdrop` (radiance at exposure 1, see
/// `tray.rs`); `0.0` is As Lit and bit-identical to the unsuffixed entry point. The
/// Ray-Traced leaf calls this one.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "the unsuffixed entry point plus the tray, kept flat to mirror its WGSL twin"
)]
pub fn sample_studio_environment_with_rig_spec_power_shadow_tray(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    spec_power: f32,
    shadow: [f32; 2],
    tray: f32,
) -> f32 {
    direction_lighting(dir, lighting_preset, exposure, rig, observer, shadow, tray)
        .radiance(spec_power, exposure)
}

/// [`sample_studio_environment_with_rig_spec_power`] over a lit backdrop tray (default head
/// shadow).
///
/// `tray = 0.0` is bit-identical to the unsuffixed entry point. See
/// [`sample_studio_environment_with_rig_shadow_tray`].
#[must_use]
pub fn sample_studio_environment_with_rig_spec_power_tray(
    dir: Vec3,
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    spec_power: f32,
    tray: f32,
) -> f32 {
    direction_lighting(
        dir,
        lighting_preset,
        exposure,
        rig,
        observer,
        DEFAULT_HEAD_SHADOW_COSINES,
        tray,
    )
    .radiance(spec_power, exposure)
}

/// [`sample_studio_environment_with_rig`] for every wavelength in `lambdas` looking along
/// the same `dir`: the direction's geometry and the preset's Planck constants are
/// evaluated once, then each channel only applies its own spectral power. Element `k` is
/// bit-identical to `sample_studio_environment_with_rig(dir, lambdas[k], ..)`, since the
/// hoisted terms never depend on the wavelength.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "the single-wavelength entry's parameters plus the lambda array and the tray"
)]
fn sample_studio_environment_channels<const N: usize>(
    dir: Vec3,
    lambdas: &[f32; N],
    lighting_preset: LightingPreset,
    exposure: f32,
    rig: &crate::optics::studio_rig::StudioRig,
    observer: Vec3,
    shadow: [f32; 2],
    tray: f32,
) -> [f32; N] {
    let lighting = direction_lighting(dir, lighting_preset, exposure, rig, observer, shadow, tray);
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
            backdrop,
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
                backdrop,
            )
        }
        EnvironmentSource::HdrMap(map) => {
            // The direction -> (u, v) -> bilinear RGB part is channel-invariant: evaluate it once
            // and convert per channel. Exactly `radiance_at`'s two steps, so each element stays
            // bit-identical to `map.radiance_at(dir, lambdas[k])`.
            let rgb = map.radiance_rgb(dir);
            std::array::from_fn(|k| {
                crate::renderer::env_map::rgb_to_spectral_radiance(rgb, lambdas[k])
            })
        }
    }
}
