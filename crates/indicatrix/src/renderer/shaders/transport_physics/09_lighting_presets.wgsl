// optics::raytracer::environment -- the lit lighting models (`LightingModel::
// IsoHemisphere` / `LightTent` / `DaylightDome`), transcribed operation for operation
// (`fma` for `mul_add`, explicit squarings where the CPU squares, the literal
// `smoothstep`) so Tier 2's `run_studio_env` holds at its ULP budget. The cone cosines
// are the same decimal literals as the Rust constants.
const HEAD_SHADOW_OUTER_COS: f32 = 0.9510565;
const HEAD_SHADOW_INNER_COS: f32 = 0.9702957;
const SUN_OUTER_COS: f32 = 0.9975641;
const SUN_INNER_COS: f32 = 0.9993908;
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
fn observer_visibility(d: vec3<f32>, observer: vec3<f32>) -> f32 {
    return 1.0 - smoothstep_f32(HEAD_SHADOW_OUTER_COS, HEAD_SHADOW_INNER_COS, dot(d, observer));
}

fn horizon_blend(d: vec3<f32>) -> f32 {
    return smoothstep_f32(-0.05, 0.05, d.y);
}

fn sample_iso_hemisphere(d: vec3<f32>, spec_power: f32, exposure: f32, observer: vec3<f32>) -> f32 {
    return (horizon_blend(d) * observer_visibility(d, observer)) * (spec_power * exposure);
}

fn sample_daylight_dome(
    d: vec3<f32>,
    spec_power: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    observer: vec3<f32>,
) -> f32 {
    let horizon = horizon_blend(d);
    let sun_dot = dot(d, key_dir);
    let sky = fma(0.08, 1.0 - max(d.y, 0.0), 0.10);
    let glow = max(sun_dot, 0.0);
    let glow2 = glow * glow;
    let glow4 = glow2 * glow2;
    let aureole = (glow4 * glow4) * 0.30;
    let sun = smoothstep_f32(SUN_OUTER_COS, SUN_INNER_COS, sun_dot) * 10.0;
    let above = ((sky + aureole) + sun) * (horizon * observer_visibility(d, observer));
    let ground = 0.04 * (1.0 - horizon);
    return (above + ground) * (spec_power * exposure);
}
