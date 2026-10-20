// Lighting twins copied from the spectral rig; independent bindings.
fn wt_powi_u(base: f32, exp: u32) -> f32 {
    var result: f32 = 1.0;
    var b: f32 = base;
    var e: u32 = exp;
    loop {
        if (e == 0u) {
            break;
        }
        if ((e & 1u) == 1u) {
            result = result * b;
        }
        b = b * b;
        e = e >> 1u;
    }
    return result;
}

// optics::raytracer::environment -- the lit lighting models (`LightingModel::
// IsoHemisphere` / `LightTent` / `DaylightDome`), transcribed operation for operation
// (`fma` for `mul_add`, explicit squarings where the CPU squares, the literal
// `smoothstep`) so Tier 2's `run_studio_env` holds at its ULP budget. The cone cosines
// are the same decimal literals as the Rust constants. The head-shadow cone is a scene
// parameter (`EnvironmentSource::Studio::head_shadow_deg`), evaluated to two cosines on the
// CPU (`head_shadow_cosines`) and threaded in as `shadow` = (outer, inner).
const wt_TENT_KEY_OUTER_COS: f32 = 0.7660444;
const wt_TENT_KEY_INNER_COS: f32 = 0.9396926;
const wt_SPARK_OUTER_COS: f32 = 0.9961947;
const wt_SPARK_INNER_COS: f32 = 0.9993908;
const wt_CARD_OUTER_COS: f32 = 0.898794;
const wt_CARD_INNER_COS: f32 = 0.9612617;

fn wt_smoothstep_f32(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * fma(-2.0, t, 3.0);
}

// 1.0 where `d` sees past the observer, 0.0 inside the head-shadow cone around
// `observer` (the unit direction towards the eye; a zero vector disables it).
fn wt_observer_visibility(d: vec3<f32>, observer: vec3<f32>, shadow: vec2<f32>) -> f32 {
    return 1.0 - wt_smoothstep_f32(shadow.x, shadow.y, dot(d, observer));
}

fn wt_horizon_blend(d: vec3<f32>) -> f32 {
    return wt_smoothstep_f32(-0.05, 0.05, d.y);
}

// `tray` is the backdrop tray radiance at exposure 1 (`EnvironmentSource::Studio::backdrop`,
// uploaded in `values.z`; 0 = As Lit, the preset's own ground). The `tray > 0.0` branches
// mirror the CPU `_tray` expressions (and the transport units' twins) operation for
// operation; the As Lit expressions are untouched.
fn wt_sample_iso_hemisphere(d: vec3<f32>, spec_power: f32, exposure: f32, observer: vec3<f32>, shadow: vec2<f32>, tray: f32) -> f32 {
    let horizon = wt_horizon_blend(d);
    let vis = wt_observer_visibility(d, observer, shadow);
    if (tray > 0.0) {
        // Lit tray below the horizon: `fma((1 - h) * vis, tray, h * vis)`.
        let factor = fma((1.0 - horizon) * vis, tray, horizon * vis);
        return factor * (spec_power * exposure);
    }
    return (horizon * vis) * (spec_power * exposure);
}

fn wt_sample_daylight_dome(
    d: vec3<f32>,
    spec_power: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    observer: vec3<f32>,
    shadow: vec2<f32>,
    tray: f32,
) -> f32 {
    let horizon = wt_horizon_blend(d);
    let sun_dot = dot(d, key_dir);
    let sky = fma(0.08, 1.0 - max(d.y, 0.0), 0.10);
    let glow = max(sun_dot, 0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let vis = wt_observer_visibility(d, observer, shadow);
    let above = (sky + aureole) * (horizon * vis);
    var ground: f32 = 0.04 * (1.0 - horizon);
    if (tray > 0.0) {
        // The lit tray replaces the ground term (same observer treatment as the sky).
        ground = ((1.0 - horizon) * vis) * tray;
    }
    return (above + ground) * (spec_power * exposure);
}

// ASET-style contrast view (model id 4): `optics::raytracer::environment::rig::wt_aset_radiance`.
// `spec_power` is the wavelength in nm here (the callers pass it for model 4), not an
// illuminant power. Zones by `d.y` = sin(elevation): green 0-45, red 45-75, blue 75-90.
const wt_ASET_RED_NM: f32 = 610.0;
const wt_ASET_GREEN_NM: f32 = 540.0;
const wt_ASET_BLUE_NM: f32 = 460.0;
const wt_ASET_SIGMA_NM: f32 = 8.493218;
const wt_ASET_RED_GAIN: f32 = 8.0;
const wt_ASET_GREEN_GAIN: f32 = 5.0;
const wt_ASET_BLUE_GAIN: f32 = 16.0;
const wt_ASET_EDGE_45_LO: f32 = 0.6871068;
const wt_ASET_EDGE_45_HI: f32 = 0.7271068;
const wt_ASET_EDGE_75_LO: f32 = 0.9459258;
const wt_ASET_EDGE_75_HI: f32 = 0.9859258;

fn wt_aset_band(lambda_nm: f32, centre_nm: f32) -> f32 {
    let z = (lambda_nm - centre_nm) / wt_ASET_SIGMA_NM;
    return exp(-0.5 * z * z);
}

fn wt_aset_radiance(
    d: vec3<f32>,
    spec_power: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    observer: vec3<f32>,
    shadow: vec2<f32>,
) -> f32 {
    let horizon = wt_horizon_blend(d);
    let s45 = wt_smoothstep_f32(wt_ASET_EDGE_45_LO, wt_ASET_EDGE_45_HI, d.y);
    let s75 = wt_smoothstep_f32(wt_ASET_EDGE_75_LO, wt_ASET_EDGE_75_HI, d.y);
    let red = (s45 * (1.0 - s75)) * horizon;
    let green = (1.0 - s45) * horizon;
    let blue = s75 * horizon;
    let r = (red * wt_aset_band(spec_power, wt_ASET_RED_NM)) * wt_ASET_RED_GAIN;
    let g = (green * wt_aset_band(spec_power, wt_ASET_GREEN_NM)) * wt_ASET_GREEN_GAIN;
    let b = (blue * wt_aset_band(spec_power, wt_ASET_BLUE_NM)) * wt_ASET_BLUE_GAIN;
    return ((r + g) + b) * exposure;
}

// Daylight sky plus a physically bright direct sun (model id 5):
// `optics::raytracer::environment::rig::wt_daylight_sun_radiance`, see the derivation of the
// disc constants above `wt_SUN_DISC_COS` there. The sky is `wt_sample_daylight_dome`'s, the sun a
// hard-edged 0.27 degree disc of radiance 40000 (fading with the horizon at the key
// direction, not with the head shadow). The constants are the same literals as the Rust
// ones: `wt_SUN_DISC_COS` is cos(0.27 deg) as an f32 (= 1 - 186 * 2^-24).
const wt_SUN_DISC_COS: f32 = 0.9999889;
const wt_SUN_ONE_MINUS_COS: f32 = 0.0000110864639;
const wt_SUN_SOLID_ANGLE: f32 = 0.000069658306;
const wt_SUN_RADIANCE: f32 = 40000.0;

// `rig::sun_radiance_factor`.
fn wt_daylight_sun_factor(key_dir: vec3<f32>) -> f32 {
    return wt_SUN_RADIANCE * wt_horizon_blend(key_dir);
}

fn wt_daylight_sun_radiance(
    d: vec3<f32>,
    spec_power: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    observer: vec3<f32>,
    shadow: vec2<f32>,
    tray: f32,
) -> f32 {
    let horizon = wt_horizon_blend(d);
    let sun_dot = dot(d, key_dir);
    let sky = fma(0.08, 1.0 - max(d.y, 0.0), 0.10);
    let glow = max(sun_dot, 0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let vis = wt_observer_visibility(d, observer, shadow);
    let above = (sky + aureole) * (horizon * vis);
    var ground: f32 = 0.04 * (1.0 - horizon);
    if (tray > 0.0) {
        // The lit tray replaces the ground term; the sun disc below is unchanged.
        ground = ((1.0 - horizon) * vis) * tray;
    }
    var sun: f32 = 0.0;
    if (sun_dot >= wt_SUN_DISC_COS) {
        sun = wt_daylight_sun_factor(key_dir);
    }
    return ((above + ground) + sun) * (spec_power * exposure);
}

const wt_RING_LIGHT_COUNT: u32 = 16u;

fn wt_studio_rig_key_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let cos_lp = cos(light_pitch);
    let sin_lp = sin(light_pitch);
    let cos_ly = cos(light_yaw);
    let sin_ly = sin(light_yaw);
    return normalize(vec3<f32>(cos_lp * sin_ly, sin_lp, cos_lp * cos_ly));
}

fn wt_studio_rig_fill_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let fill_yaw = fma(wt_PI, 0.78, light_yaw);
    let fill_pitch = clamp(light_pitch * 0.65, 0.15, 1.2);
    return normalize(vec3<f32>(cos(fill_pitch) * sin(fill_yaw), sin(fill_pitch), cos(fill_pitch) * cos(fill_yaw)));
}

fn wt_studio_rig_ring_dir(i: u32, light_yaw: f32, sin_lp: f32) -> vec3<f32> {
    // CPU-prepared directions eliminate cross-device trig variation in the rig.
    return wt.ring_dirs[i].xyz;
}

// optics::raytracer::environment::wt_sample_light_tent -- needs `wt_studio_rig_ring_dir` for
// the three black cards on ring slots 4/8/12, so it lives here rather than in the shared
// prelude with the other lit models.
fn wt_sample_light_tent(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
    shadow: vec2<f32>,
    tent: vec4<f32>,
    tent_flat: f32,
    tray: f32,
) -> f32 {
    // `tent` = (walls, cards, spark, ground) = `TentParams`: exact identities at the light
    // tent's own values (x * 1.0, and the ground literal 0.02), see the CPU twin.
    // `tent_flat` blends the wall gradient towards its 30-degree value (0 = skipped).
    let horizon = wt_horizon_blend(d);
    var walls = fma(0.08, max(d.y, 0.0), 0.14) * tent.x;
    if (tent_flat > 0.0) {
        walls = fma(0.18 * tent.x, tent_flat, walls * (1.0 - tent_flat));
    }
    var card: f32 = 0.0;
    for (var slot: u32 = 4u; slot < wt_RING_LIGHT_COUNT; slot = slot + 4u) {
        let card_dir = wt_studio_rig_ring_dir(slot, light_yaw, sin_lp);
        card = max(card, wt_smoothstep_f32(wt_CARD_OUTER_COS, wt_CARD_INNER_COS, dot(d, card_dir)));
    }
    walls = walls * fma(card * tent.y, -0.9, 1.0);
    let key = wt_smoothstep_f32(wt_TENT_KEY_OUTER_COS, wt_TENT_KEY_INNER_COS, dot(d, key_dir)) * (1.4 * spot_mult);
    let spark = wt_smoothstep_f32(wt_SPARK_OUTER_COS, wt_SPARK_INNER_COS, dot(d, fill_dir)) * (5.0 * spot_mult) * tent.z;
    let vis = wt_observer_visibility(d, observer, shadow);
    let above = ((walls + key) + spark) * (horizon * vis);
    var ground: f32 = tent.w * (1.0 - horizon);
    if (tray > 0.0) {
        // The lit tray replaces the tent's own ground term.
        ground = ((1.0 - horizon) * vis) * tray;
    }
    return (above + ground) * (spec_power * exposure);
}

fn wt_sample_studio_rig(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    tray: f32,
) -> f32 {
    let bg_val = max(fma(0.012, fma(d.y, 0.5, 0.5), 0.015), 0.005) * exposure;
    var radiance = bg_val * spec_power;

    let key_dot = max(dot(d, key_dir), 0.0);
    if (key_dot > 0.0) {
        let softbox = wt_powi_u(key_dot, 28u) * 12.0 * spot_mult * exposure;
        radiance = fma(softbox, spec_power, radiance);
    }

    let fill_dot = max(dot(d, fill_dir), 0.0);
    if (fill_dot > 0.0) {
        let fill = wt_powi_u(fill_dot, 18u) * 4.5 * exposure;
        radiance = fma(fill, spec_power, radiance);
    }

    for (var i: u32 = 0u; i < wt_RING_LIGHT_COUNT; i = i + 1u) {
        let ring_dir = wt_studio_rig_ring_dir(i, light_yaw, sin_lp);
        let ring_dot = max(dot(d, ring_dir), 0.0);
        if (ring_dot > 0.96) {
            let spark = (ring_dot - 0.96) / 0.04;
            let intensity = wt_powi_u(spark, 6u) * 22.0 * spot_mult * exposure;
            radiance = fma(intensity, spec_power, radiance);
        }
    }

    if (tray > 0.0) {
        // The lit tray replaces everything below the horizon:
        // `fma(h, rig, ((g * tray) * exposure) * S)`.
        let horizon = wt_horizon_blend(d);
        radiance = fma(horizon, radiance, (((1.0 - horizon) * tray) * exposure) * spec_power);
    }

    return radiance;
}

fn wt_studio_dispatch(
    model: u32,
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
    shadow: vec2<f32>,
    tent: vec4<f32>,
    tent_flat: f32,
    tray: f32,
) -> f32 {
    switch (model) {
        case 1u: {
            return wt_sample_iso_hemisphere(d, spec_power, exposure, observer, shadow, tray);
        }
        case 2u: {
            return wt_sample_light_tent(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw, observer, shadow, tent, tent_flat, tray);
        }
        case 3u: {
            return wt_sample_daylight_dome(d, spec_power, exposure, key_dir, observer, shadow, tray);
        }
        case 4u: {
            // ASET never reaches this arm in the Ray-Traced unit: its environment is the
            // CPU-built 1024-row table (`wt.modes.y == 1`), which the CPU builds from the
            // tray-aware lookup, so the tray needs no shader arithmetic here.
            return wt_aset_radiance(d, spec_power, exposure, key_dir, observer, shadow);
        }
        case 5u: {
            return wt_daylight_sun_radiance(d, spec_power, exposure, key_dir, observer, shadow, tray);
        }
        default: {
            return wt_sample_studio_rig(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw, tray);
        }
    }
}

