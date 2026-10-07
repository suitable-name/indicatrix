// optics::raytracer::environment -- the lit lighting models (`LightingModel::
// IsoHemisphere` / `LightTent` / `DaylightDome`), transcribed operation for operation
// (`fma` for `mul_add`, explicit squarings where the CPU squares, the literal
// `smoothstep`) so Tier 2's `run_studio_env` holds at its ULP budget. The cone cosines
// are the same decimal literals as the Rust constants. The head-shadow cone is a scene
// parameter (`EnvironmentSource::Studio::head_shadow_deg`), evaluated to two cosines on the
// CPU (`head_shadow_cosines`) and threaded in as `shadow` = (outer, inner).
const TENT_KEY_OUTER_COS: f32 = 0.7660444;
const TENT_KEY_INNER_COS: f32 = 0.9396926;
const SPARK_OUTER_COS: f32 = 0.9961947;
const SPARK_INNER_COS: f32 = 0.9993908;
const CARD_OUTER_COS: f32 = 0.898794;
const CARD_INNER_COS: f32 = 0.9612617;

fn smoothstep_f32(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * fma(-2.0, t, 3.0);
}

// 1.0 where `d` sees past the observer, 0.0 inside the head-shadow cone around
// `observer` (the unit direction towards the eye; a zero vector disables it).
fn observer_visibility(d: vec3<f32>, observer: vec3<f32>, shadow: vec2<f32>) -> f32 {
    return 1.0 - smoothstep_f32(shadow.x, shadow.y, dot(d, observer));
}

fn horizon_blend(d: vec3<f32>) -> f32 {
    return smoothstep_f32(-0.05, 0.05, d.y);
}

fn sample_iso_hemisphere(d: vec3<f32>, spec_power: f32, exposure: f32, observer: vec3<f32>, shadow: vec2<f32>) -> f32 {
    return (horizon_blend(d) * observer_visibility(d, observer, shadow)) * (spec_power * exposure);
}

fn sample_daylight_dome(
    d: vec3<f32>,
    spec_power: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    observer: vec3<f32>,
    shadow: vec2<f32>,
) -> f32 {
    let horizon = horizon_blend(d);
    let sun_dot = dot(d, key_dir);
    let sky = fma(0.08, 1.0 - max(d.y, 0.0), 0.10);
    let glow = max(sun_dot, 0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let above = (sky + aureole) * (horizon * observer_visibility(d, observer, shadow));
    let ground = 0.04 * (1.0 - horizon);
    return (above + ground) * (spec_power * exposure);
}

// ASET-style contrast view (model id 4): `optics::raytracer::environment::rig::aset_radiance`.
// `spec_power` is the wavelength in nm here (the callers pass it for model 4), not an
// illuminant power. Zones by `d.y` = sin(elevation): green 0-45, red 45-75, blue 75-90.
const ASET_RED_NM: f32 = 610.0;
const ASET_GREEN_NM: f32 = 540.0;
const ASET_BLUE_NM: f32 = 460.0;
const ASET_SIGMA_NM: f32 = 8.493218;
const ASET_RED_GAIN: f32 = 8.0;
const ASET_GREEN_GAIN: f32 = 5.0;
const ASET_BLUE_GAIN: f32 = 16.0;
const ASET_EDGE_45_LO: f32 = 0.6871068;
const ASET_EDGE_45_HI: f32 = 0.7271068;
const ASET_EDGE_75_LO: f32 = 0.9459258;
const ASET_EDGE_75_HI: f32 = 0.9859258;

fn aset_band(lambda_nm: f32, centre_nm: f32) -> f32 {
    let z = (lambda_nm - centre_nm) / ASET_SIGMA_NM;
    return exp(-0.5 * z * z);
}

fn aset_radiance(
    d: vec3<f32>,
    spec_power: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    observer: vec3<f32>,
    shadow: vec2<f32>,
) -> f32 {
    let horizon = horizon_blend(d);
    let s45 = smoothstep_f32(ASET_EDGE_45_LO, ASET_EDGE_45_HI, d.y);
    let s75 = smoothstep_f32(ASET_EDGE_75_LO, ASET_EDGE_75_HI, d.y);
    let red = (s45 * (1.0 - s75)) * horizon;
    let green = (1.0 - s45) * horizon;
    let blue = s75 * horizon;
    let r = (red * aset_band(spec_power, ASET_RED_NM)) * ASET_RED_GAIN;
    let g = (green * aset_band(spec_power, ASET_GREEN_NM)) * ASET_GREEN_GAIN;
    let b = (blue * aset_band(spec_power, ASET_BLUE_NM)) * ASET_BLUE_GAIN;
    return ((r + g) + b) * exposure;
}

// Daylight sky plus a physically bright direct sun (model id 5):
// `optics::raytracer::environment::rig::daylight_sun_radiance`, see the derivation of the
// disc constants above `SUN_DISC_COS` there. The sky is `sample_daylight_dome`'s, the sun a
// hard-edged 0.27 degree disc of radiance 40000 (fading with the horizon at the key
// direction, not with the head shadow). The constants are the same literals as the Rust
// ones: `SUN_DISC_COS` is cos(0.27 deg) as an f32 (= 1 - 186 * 2^-24).
const SUN_DISC_COS: f32 = 0.9999889;
const SUN_ONE_MINUS_COS: f32 = 0.0000110864639;
const SUN_SOLID_ANGLE: f32 = 0.000069658306;
const SUN_RADIANCE: f32 = 40000.0;

// `rig::sun_radiance_factor`.
fn daylight_sun_factor(key_dir: vec3<f32>) -> f32 {
    return SUN_RADIANCE * horizon_blend(key_dir);
}

fn daylight_sun_radiance(
    d: vec3<f32>,
    spec_power: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    observer: vec3<f32>,
    shadow: vec2<f32>,
) -> f32 {
    let horizon = horizon_blend(d);
    let sun_dot = dot(d, key_dir);
    let sky = fma(0.08, 1.0 - max(d.y, 0.0), 0.10);
    let glow = max(sun_dot, 0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let above = (sky + aureole) * (horizon * observer_visibility(d, observer, shadow));
    let ground = 0.04 * (1.0 - horizon);
    var sun: f32 = 0.0;
    if (sun_dot >= SUN_DISC_COS) {
        sun = daylight_sun_factor(key_dir);
    }
    return ((above + ground) + sun) * (spec_power * exposure);
}

// `rig::sun_cone_direction`: a direction uniform over the sun disc about `key_dir`
// (equal-area cone sampling) from two uniform [0, 1) randoms. Self-contained (no other
// piece's basis helper) so the Tier-2 copy in `environment.wgsl` is textually identical.
fn daylight_sun_cone_direction(key_dir: vec3<f32>, u0: f32, u1: f32) -> vec3<f32> {
    let one_minus_cos = u0 * SUN_ONE_MINUS_COS;
    let cos_t = 1.0 - one_minus_cos;
    let sin_t = sqrt(max(one_minus_cos * (2.0 - one_minus_cos), 0.0));
    let phi = 2.0 * 3.14159265358979323846 * u1;
    let sin_p = sin(phi);
    let cos_p = cos(phi);
    var a = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(key_dir.x) > 0.9) {
        a = vec3<f32>(0.0, 1.0, 0.0);
    }
    let perp = a - key_dir * dot(key_dir, a);
    let perp_len = length(perp);
    var t = vec3<f32>(0.0, 0.0, 0.0);
    if (perp_len > 0.0) {
        t = perp / perp_len;
    }
    let b = cross(key_dir, t);
    let dir = t * (sin_t * cos_p) + b * (sin_t * sin_p) + key_dir * cos_t;
    let dir_len = length(dir);
    if (dir_len > 0.0) {
        return dir / dir_len;
    }
    return vec3<f32>(0.0, 0.0, 0.0);
}

// `rig::sun_nee_pdf`: the solid-angle pdf of `daylight_sun_cone_direction` at the unit
// direction `dir` -- 1 / solid angle inside the disc, 0 outside.
fn daylight_sun_nee_pdf(dir: vec3<f32>, key_dir: vec3<f32>) -> f32 {
    if (dot(dir, key_dir) >= SUN_DISC_COS) {
        return 1.0 / SUN_SOLID_ANGLE;
    }
    return 0.0;
}
