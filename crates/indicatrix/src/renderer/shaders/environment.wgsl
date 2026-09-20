// Phase 1: environment-sampling / CMF-integration / white-balance kernels -- driven by
// `renderer::gpu::environment_check`.
//
// Fresh translations of, in order: `color::cie1931::cie_1931_cmf` (the single source of
// truth for the CIE 1931 CMF, tabulated at 5nm and linearly interpolated -- see that
// module's own doc comment for why a table replaced the old Gaussian-lobe fit),
// `optics::raytracer::blackbody_spectrum`, `optics::raytracer::sample_studio_environment`
// (plus the `optics::studio_rig::StudioRig` key/fill/ring directions it depends on),
// and `optics::raytracer::compute_illuminant_white_balance` (the 401-point 380..=780nm
// von Kries integration).
//
// Every output is a flat `array<f32>` (never `array<vec3<f32>>`/`array<vec2<f32>>`)
// specifically to sidestep WGSL's storage-array element-stride rounding for vec2/vec3
// (`roundUp(align, size)`, which does NOT match a tightly-packed Rust `[f32; N]`) --
// see `renderer::buffers`' module doc comment for the bug class that kind of mismatch
// causes. Multi-component results are written at `idx * N + component`.

const PI: f32 = 3.14159265358979323846;
const RING_LIGHT_COUNT: u32 = 16u;

// Rust's `f32::powi(n)` lowers to exponentiation-by-squaring (LLVM's `llvm.powi`
// intrinsic), not the general `exp(log(x) * n)` path WGSL's `pow()` builtin normally
// takes for a non-integer exponent. Every `powi` call site in the ported CPU code
// (`blackbody_spectrum`'s `.powi(5)`, `sample_studio_environment`'s `.powi(28)`
// /`.powi(18)`/`.powi(6)`) uses this instead of `pow()`, to keep the GPU port on the
// same, tighter-rounding algorithm rather than introducing an avoidable extra source of
// ULP divergence.
fn powi_u(base: f32, exp: u32) -> f32 {
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

// ---------------------------------------------------------------------------------
// cie_1931_cmf
// ---------------------------------------------------------------------------------
//
// P4: CIE 1931 2-degree observer, tabulated at 5nm (CIE 15:2004 Table T.4, 380-780nm)
// and linearly interpolated -- ported identically to shaders/furnace.wgsl and the CMF
// region of shaders/spectral_transport.wgsl (all three copies transcribed by hand from
// `color::cie1931::CIE_1931_TABLE`, so an edit to one must be mirrored into the other
// two by hand too). Replaces the retired Wyman/Sloan/Shirley Gaussian-lobe fit, which
// carried 1-3% XYZ error against the real tabulated observer -- see `color::cie1931`'s
// module doc comment. Deliberately plain f32 arithmetic in the same fixed order as that
// Rust function (floor, fraction, `lo + (hi - lo) * t`), no mul_add/fma, no f64
// intermediate anywhere, so this reproduces it bit-for-bit modulo ordinary driver-level
// f32 rounding.

const CIE_15_2004_CMF_START_NM: f32 = 380.0;
const CIE_15_2004_CMF_STEP_NM: f32 = 5.0;
const CIE_15_2004_CMF_LAST_INDEX: u32 = 80u;
// CIE_15_2004_CMF_START_NM + 80.0 * CIE_15_2004_CMF_STEP_NM, precomputed since WGSL
// `const` initializers can't call CIE_15_2004_CMF_TABLE.length() the way Rust's
// `CIE_1931_TABLE.len()` can.
const CIE_15_2004_CMF_END_NM: f32 = 780.0;

const CIE_15_2004_CMF_TABLE: array<vec3<f32>, 81> = array<vec3<f32>, 81>(
    vec3<f32>(0.0014, 0.0000, 0.0065), // 380nm
    vec3<f32>(0.0022, 0.0001, 0.0105), // 385nm
    vec3<f32>(0.0042, 0.0001, 0.0201), // 390nm
    vec3<f32>(0.0076, 0.0002, 0.0362), // 395nm
    vec3<f32>(0.0143, 0.0004, 0.0679), // 400nm
    vec3<f32>(0.0232, 0.0006, 0.1102), // 405nm
    vec3<f32>(0.0435, 0.0012, 0.2074), // 410nm
    vec3<f32>(0.0776, 0.0022, 0.3713), // 415nm
    vec3<f32>(0.1344, 0.0040, 0.6456), // 420nm
    vec3<f32>(0.2148, 0.0073, 1.0391), // 425nm
    vec3<f32>(0.2839, 0.0116, 1.3856), // 430nm
    vec3<f32>(0.3285, 0.0168, 1.6230), // 435nm
    vec3<f32>(0.3483, 0.0230, 1.7471), // 440nm
    vec3<f32>(0.3481, 0.0298, 1.7826), // 445nm
    vec3<f32>(0.3362, 0.0380, 1.7721), // 450nm
    vec3<f32>(0.3187, 0.0480, 1.7441), // 455nm
    vec3<f32>(0.2908, 0.0600, 1.6692), // 460nm
    vec3<f32>(0.2511, 0.0739, 1.5281), // 465nm
    vec3<f32>(0.1954, 0.0910, 1.2876), // 470nm
    vec3<f32>(0.1421, 0.1126, 1.0419), // 475nm
    vec3<f32>(0.0956, 0.1390, 0.8130), // 480nm
    vec3<f32>(0.0580, 0.1693, 0.6162), // 485nm
    vec3<f32>(0.0320, 0.2080, 0.4652), // 490nm
    vec3<f32>(0.0147, 0.2586, 0.3533), // 495nm
    vec3<f32>(0.0049, 0.3230, 0.2720), // 500nm
    vec3<f32>(0.0024, 0.4073, 0.2123), // 505nm
    vec3<f32>(0.0093, 0.5030, 0.1582), // 510nm
    vec3<f32>(0.0291, 0.6082, 0.1117), // 515nm
    vec3<f32>(0.0633, 0.7100, 0.0782), // 520nm
    vec3<f32>(0.1096, 0.7932, 0.0573), // 525nm
    vec3<f32>(0.1655, 0.8620, 0.0422), // 530nm
    vec3<f32>(0.2257, 0.9149, 0.0298), // 535nm
    vec3<f32>(0.2904, 0.9540, 0.0203), // 540nm
    vec3<f32>(0.3597, 0.9803, 0.0134), // 545nm
    vec3<f32>(0.4334, 0.9950, 0.0087), // 550nm
    vec3<f32>(0.5121, 1.0000, 0.0057), // 555nm
    vec3<f32>(0.5945, 0.9950, 0.0039), // 560nm
    vec3<f32>(0.6784, 0.9786, 0.0027), // 565nm
    vec3<f32>(0.7621, 0.9520, 0.0021), // 570nm
    vec3<f32>(0.8425, 0.9154, 0.0018), // 575nm
    vec3<f32>(0.9163, 0.8700, 0.0017), // 580nm
    vec3<f32>(0.9786, 0.8163, 0.0014), // 585nm
    vec3<f32>(1.0263, 0.7570, 0.0011), // 590nm
    vec3<f32>(1.0567, 0.6949, 0.0010), // 595nm
    vec3<f32>(1.0622, 0.6310, 0.0008), // 600nm
    vec3<f32>(1.0456, 0.5668, 0.0006), // 605nm
    vec3<f32>(1.0026, 0.5030, 0.0003), // 610nm
    vec3<f32>(0.9384, 0.4412, 0.0002), // 615nm
    vec3<f32>(0.8544, 0.3810, 0.0002), // 620nm
    vec3<f32>(0.7514, 0.3210, 0.0001), // 625nm
    vec3<f32>(0.6424, 0.2650, 0.0000), // 630nm
    vec3<f32>(0.5419, 0.2170, 0.0000), // 635nm
    vec3<f32>(0.4479, 0.1750, 0.0000), // 640nm
    vec3<f32>(0.3608, 0.1382, 0.0000), // 645nm
    vec3<f32>(0.2835, 0.1070, 0.0000), // 650nm
    vec3<f32>(0.2187, 0.0816, 0.0000), // 655nm
    vec3<f32>(0.1649, 0.0610, 0.0000), // 660nm
    vec3<f32>(0.1212, 0.0446, 0.0000), // 665nm
    vec3<f32>(0.0874, 0.0320, 0.0000), // 670nm
    vec3<f32>(0.0636, 0.0232, 0.0000), // 675nm
    vec3<f32>(0.0468, 0.0170, 0.0000), // 680nm
    vec3<f32>(0.0329, 0.0119, 0.0000), // 685nm
    vec3<f32>(0.0227, 0.0082, 0.0000), // 690nm
    vec3<f32>(0.0158, 0.0057, 0.0000), // 695nm
    vec3<f32>(0.0114, 0.0041, 0.0000), // 700nm
    vec3<f32>(0.0081, 0.0029, 0.0000), // 705nm
    vec3<f32>(0.0058, 0.0021, 0.0000), // 710nm
    vec3<f32>(0.0041, 0.0015, 0.0000), // 715nm
    vec3<f32>(0.0029, 0.0010, 0.0000), // 720nm
    vec3<f32>(0.0020, 0.0007, 0.0000), // 725nm
    vec3<f32>(0.0014, 0.0005, 0.0000), // 730nm
    vec3<f32>(0.0010, 0.0004, 0.0000), // 735nm
    vec3<f32>(0.0007, 0.0002, 0.0000), // 740nm
    vec3<f32>(0.0005, 0.0002, 0.0000), // 745nm
    vec3<f32>(0.0003, 0.0001, 0.0000), // 750nm
    vec3<f32>(0.0002, 0.0001, 0.0000), // 755nm
    vec3<f32>(0.0002, 0.0001, 0.0000), // 760nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 765nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 770nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 775nm
    vec3<f32>(0.0000, 0.0000, 0.0000), // 780nm
);

fn cie_1931_cmf(l: f32) -> vec3<f32> {
    if (!(CIE_15_2004_CMF_START_NM <= l && l <= CIE_15_2004_CMF_END_NM)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let position = (l - CIE_15_2004_CMF_START_NM) / CIE_15_2004_CMF_STEP_NM;
    let index0 = min(u32(floor(position)), CIE_15_2004_CMF_LAST_INDEX - 1u);
    let index1 = index0 + 1u;
    let t = position - f32(index0);
    let lo = CIE_15_2004_CMF_TABLE[index0];
    let hi = CIE_15_2004_CMF_TABLE[index1];
    return lo + (hi - lo) * t;
}

@group(0) @binding(0) var<storage, read> cmf_lambdas: array<f32>;
@group(0) @binding(1) var<storage, read_write> cmf_out: array<f32>;

@compute @workgroup_size(64)
fn cmf_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&cmf_lambdas)) {
        return;
    }
    let xyz = cie_1931_cmf(cmf_lambdas[idx]);
    cmf_out[idx * 3u + 0u] = xyz.x;
    cmf_out[idx * 3u + 1u] = xyz.y;
    cmf_out[idx * 3u + 2u] = xyz.z;
}

// ---------------------------------------------------------------------------------
// blackbody_spectrum
// ---------------------------------------------------------------------------------

fn blackbody_spectrum(lambda_nm: f32, temp_k: f32) -> f32 {
    let t_k = max(temp_k, 1000.0);
    let h_c_k: f32 = 14388000.0;
    let exp_val = exp(min(h_c_k / (lambda_nm * t_k), 80.0));
    let exp_560 = exp(min(h_c_k / (560.0 * t_k), 80.0));
    let denom = max(exp_val - 1.0, 1e-6);
    let denom_560 = max(exp_560 - 1.0, 1e-6);
    let ratio = denom_560 / denom;
    return clamp(powi_u(560.0 / lambda_nm, 5u) * ratio, 0.01, 20.0);
}

// P4 (tabulated D65 GPU port): optics::raytracer::environment::{CIE_D65_SPD_380_780_10NM,
// d65_relative_spectral_power} -- ported identically to `shaders/spectral_transport.wgsl`
// (see that file's own copy of these same two items for the full citation/rationale;
// duplicated here rather than shared because this Phase 1 self-test shader and the
// production megakernel are two independent WGSL modules with no shared-include
// mechanism -- see this file's own header comment).
const CIE_D65_SPD_380_780_10NM: array<f32, 41> = array<f32, 41>(
    49.9755, 54.6482, 82.7549, 91.4860, 93.4318, 86.6823, 104.865, 117.008, 117.812, 114.861,
    115.923, 108.811, 109.354, 107.802, 104.790, 107.689, 104.405, 104.046, 100.000, 96.3342,
    95.7880, 88.6856, 90.0062, 89.5991, 87.6987, 83.2886, 83.6992, 80.0268, 80.2146, 82.2778,
    78.2842, 69.7213, 71.6091, 74.3496, 61.6045, 69.8856, 75.0870, 63.5928, 46.4182, 66.8054,
    63.3828,
);

fn d65_relative_spectral_power(lambda_nm: f32) -> f32 {
    let start_nm: f32 = 380.0;
    let step_nm: f32 = 10.0;
    let last_index: u32 = 40u; // CIE_D65_SPD_380_780_10NM.len() - 1

    let clamped = clamp(lambda_nm, start_nm, fma(step_nm, f32(last_index), start_nm));
    let position = (clamped - start_nm) / step_nm;
    let index0 = min(u32(floor(position)), last_index - 1u);
    let index1 = index0 + 1u;
    let frac = position - f32(index0);

    let v0 = CIE_D65_SPD_380_780_10NM[index0];
    let v1 = CIE_D65_SPD_380_780_10NM[index1];
    return fma(frac, v1 - v0, v0) / 100.0;
}

struct BlackbodyCase {
    lambda_nm: f32,
    temp_k: f32,
}

@group(0) @binding(2) var<storage, read> blackbody_cases: array<BlackbodyCase>;
@group(0) @binding(3) var<storage, read_write> blackbody_out: array<f32>;

@compute @workgroup_size(64)
fn blackbody_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&blackbody_cases)) {
        return;
    }
    let c = blackbody_cases[idx];
    blackbody_out[idx] = blackbody_spectrum(c.lambda_nm, c.temp_k);
}

// ---------------------------------------------------------------------------------
// sample_studio_environment (+ StudioRig)
// ---------------------------------------------------------------------------------

struct StudioEnvCase {
    dir_x: f32,
    dir_y: f32,
    dir_z: f32,
    lambda_nm: f32,
    temp_k: f32,
    spot_mult: f32,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    model: f32,
    _pad1: f32,
    // P4 (tabulated D65 GPU port): was `_pad2` -- mirrors
    // `optics::raytracer::LightingPreset::Daylight`'s own D65-vs-Planckian selection;
    // see `renderer::gpu::environment_check::build_studio_env_cases`.
    use_d65: f32,
    // Unit direction towards the eye for the lit models' head shadow; zero disables it.
    observer_x: f32,
    observer_y: f32,
    observer_z: f32,
    _pad2: f32,
}

fn studio_rig_key_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let cos_lp = cos(light_pitch);
    let sin_lp = sin(light_pitch);
    let cos_ly = cos(light_yaw);
    let sin_ly = sin(light_yaw);
    return normalize(vec3<f32>(cos_lp * sin_ly, sin_lp, cos_lp * cos_ly));
}

fn studio_rig_fill_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let fill_yaw = fma(PI, 0.78, light_yaw);
    let fill_pitch = clamp(light_pitch * 0.65, 0.15, 1.2);
    return normalize(vec3<f32>(cos(fill_pitch) * sin(fill_yaw), sin(fill_pitch), cos(fill_pitch) * cos(fill_yaw)));
}

fn studio_rig_ring_dir(i: u32, light_yaw: f32, sin_lp: f32) -> vec3<f32> {
    let angle = fma(f32(i), PI * 2.0 / f32(RING_LIGHT_COUNT), light_yaw);
    return normalize(vec3<f32>(cos(angle) * 0.75, sin_lp * 0.8, sin(angle) * 0.75));
}

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

// optics::raytracer::environment::sample_light_tent -- needs `studio_rig_ring_dir` for
// the three black cards on ring slots 4/8/12, so it lives here rather than in the shared
// prelude with the other lit models.
fn sample_light_tent(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
) -> f32 {
    let horizon = horizon_blend(d);
    var walls = fma(0.08, max(d.y, 0.0), 0.14);
    var card: f32 = 0.0;
    for (var slot: u32 = 4u; slot < RING_LIGHT_COUNT; slot = slot + 4u) {
        let card_dir = studio_rig_ring_dir(slot, light_yaw, sin_lp);
        card = max(card, smoothstep_f32(CARD_OUTER_COS, CARD_INNER_COS, dot(d, card_dir)));
    }
    walls = walls * fma(card, -0.9, 1.0);
    let key = smoothstep_f32(TENT_KEY_OUTER_COS, TENT_KEY_INNER_COS, dot(d, key_dir)) * (1.4 * spot_mult);
    let spark = smoothstep_f32(SPARK_OUTER_COS, SPARK_INNER_COS, dot(d, fill_dir)) * (5.0 * spot_mult);
    let above = ((walls + key) + spark) * (horizon * observer_visibility(d, observer));
    let ground = 0.02 * (1.0 - horizon);
    return (above + ground) * (spec_power * exposure);
}

fn sample_studio_rig(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
) -> f32 {
    let bg_val = max(fma(0.012, fma(d.y, 0.5, 0.5), 0.015), 0.005) * exposure;
    var radiance = bg_val * spec_power;

    let key_dot = max(dot(d, key_dir), 0.0);
    if (key_dot > 0.0) {
        let softbox = powi_u(key_dot, 28u) * 12.0 * spot_mult * exposure;
        radiance = fma(softbox, spec_power, radiance);
    }

    let fill_dot = max(dot(d, fill_dir), 0.0);
    if (fill_dot > 0.0) {
        let fill = powi_u(fill_dot, 18u) * 4.5 * exposure;
        radiance = fma(fill, spec_power, radiance);
    }

    for (var i: u32 = 0u; i < RING_LIGHT_COUNT; i = i + 1u) {
        let ring_dir = studio_rig_ring_dir(i, light_yaw, sin_lp);
        let ring_dot = max(dot(d, ring_dir), 0.0);
        if (ring_dot > 0.96) {
            let spark = (ring_dot - 0.96) / 0.04;
            let intensity = powi_u(spark, 6u) * 22.0 * spot_mult * exposure;
            radiance = fma(intensity, spec_power, radiance);
        }
    }

    return radiance;
}

fn studio_dispatch(
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
) -> f32 {
    switch (model) {
        case 1u: {
            return sample_iso_hemisphere(d, spec_power, exposure, observer);
        }
        case 2u: {
            return sample_light_tent(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw, observer);
        }
        case 3u: {
            return sample_daylight_dome(d, spec_power, exposure, key_dir, observer);
        }
        default: {
            return sample_studio_rig(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw);
        }
    }
}

fn sample_studio_environment(
    dir_in: vec3<f32>,
    lambda_nm: f32,
    temp_k: f32,
    spot_mult: f32,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    use_d65: f32,
    model: f32,
    observer: vec3<f32>,
) -> f32 {
    let d = normalize(dir_in);
    var spec_power: f32;
    if (use_d65 != 0.0) {
        spec_power = d65_relative_spectral_power(lambda_nm);
    } else {
        spec_power = blackbody_spectrum(lambda_nm, temp_k);
    }

    let key_dir = studio_rig_key_dir(light_yaw, light_pitch);
    let fill_dir = studio_rig_fill_dir(light_yaw, light_pitch);
    let sin_lp = sin(light_pitch);

    return studio_dispatch(
        u32(model),
        d,
        spec_power,
        spot_mult,
        exposure,
        key_dir,
        fill_dir,
        sin_lp,
        light_yaw,
        observer,
    );
}

@group(0) @binding(4) var<storage, read> studio_cases: array<StudioEnvCase>;
@group(0) @binding(5) var<storage, read_write> studio_out: array<f32>;

@compute @workgroup_size(64)
fn studio_env_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&studio_cases)) {
        return;
    }
    let c = studio_cases[idx];
    studio_out[idx] = sample_studio_environment(
        vec3<f32>(c.dir_x, c.dir_y, c.dir_z),
        c.lambda_nm,
        c.temp_k,
        c.spot_mult,
        c.exposure,
        c.light_yaw,
        c.light_pitch,
        c.use_d65,
        c.model,
        vec3<f32>(c.observer_x, c.observer_y, c.observer_z),
    );
}

// ---------------------------------------------------------------------------------
// compute_illuminant_white_balance: 401-point (380..=780nm, 1nm step) quadrature.
//
// Fix 3: diagonalised in Bradford LMS space, not raw XYZ -- see
// `optics::raytracer::compute_illuminant_white_balance`'s doc comment for the full
// rationale. `BRADFORD_XYZ_TO_LMS`/`BRADFORD_LMS_TO_XYZ` and `D65_WHITE_X`/
// `D65_WHITE_Y` below are the exact same constants as that Rust function's, so this
// kernel's output stays within `environment_check::WHITE_BALANCE_ULP_BUDGET` of it.
// ---------------------------------------------------------------------------------

const D65_WHITE_X: f32 = 0.3127;
const D65_WHITE_Y: f32 = 0.3290;

const BRADFORD_XYZ_TO_LMS = mat3x3<f32>(
    vec3<f32>(0.8951, -0.7502, 0.0389),
    vec3<f32>(0.2664, 1.7135, -0.0685),
    vec3<f32>(-0.1614, 0.0367, 1.0296),
);

const BRADFORD_LMS_TO_XYZ = mat3x3<f32>(
    vec3<f32>(0.986993, 0.432305, -0.008529),
    vec3<f32>(-0.147054, 0.518360, 0.040043),
    vec3<f32>(0.159963, 0.049291, 0.968487),
);

struct WhiteBalanceCase {
    temp_k: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(6) var<storage, read> wb_cases: array<WhiteBalanceCase>;
@group(0) @binding(7) var<storage, read_write> wb_out: array<f32>;

@compute @workgroup_size(64)
fn white_balance_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&wb_cases)) {
        return;
    }
    let temp_k = wb_cases[idx].temp_k;

    var xyz_w = vec3<f32>(0.0, 0.0, 0.0);
    for (var step: i32 = 0; step <= (780 - 380); step = step + 1) {
        let lambda = 380.0 + f32(step);
        xyz_w = xyz_w + cie_1931_cmf(lambda) * blackbody_spectrum(lambda, temp_k);
    }

    let target_y = max(xyz_w.y, 1e-6);
    let xyz_target = vec3<f32>(
        (D65_WHITE_X / D65_WHITE_Y) * target_y,
        target_y,
        ((1.0 - D65_WHITE_X - D65_WHITE_Y) / D65_WHITE_Y) * target_y,
    );

    let lms_source = BRADFORD_XYZ_TO_LMS * xyz_w;
    let lms_target = BRADFORD_XYZ_TO_LMS * xyz_target;

    var scale: vec3<f32>;
    if (lms_source.x > 1e-6) {
        scale.x = lms_target.x / lms_source.x;
    } else {
        scale.x = 1.0;
    }
    if (lms_source.y > 1e-6) {
        scale.y = lms_target.y / lms_source.y;
    } else {
        scale.y = 1.0;
    }
    if (lms_source.z > 1e-6) {
        scale.z = lms_target.z / lms_source.z;
    } else {
        scale.z = 1.0;
    }

    wb_out[idx * 3u + 0u] = scale.x;
    wb_out[idx * 3u + 1u] = scale.y;
    wb_out[idx * 3u + 2u] = scale.z;
}

// ---------------------------------------------------------------------------------
// Finding G6: renderer::env_map::EnvironmentMap::{direction_to_uv, sample_bilinear,
// radiance_at}, plus renderer::env_map_spectrum::rgb_to_spectral_radiance.
//
// A SEPARATE port from `shaders/spectral_transport.wgsl`'s own `hdr_direction_to_uv`/
// `hdr_env_sample_bilinear`/`hdr_env_radiance_at`/`rgb_to_spectral_radiance` -- this file
// and the production megakernel are two independent WGSL modules with no shared-include
// mechanism (see this file's own header comment), so the two copies are kept in sync by
// hand like every other duplicated function here (`blackbody_spectrum`, the CMF table,
// `d65_relative_spectral_power`). `renderer::gpu::environment_check::run_hdr_env_radiance`
// exercises THIS copy directly against the CPU, independent of the megakernel's own Tier
// 3 image comparisons.
// ---------------------------------------------------------------------------------

fn hdr_asymmetric_gaussian(x: f32, mu: f32, sigma_lo: f32, sigma_hi: f32) -> f32 {
    var sigma: f32;
    if (x < mu) {
        sigma = sigma_lo;
    } else {
        sigma = sigma_hi;
    }
    let t = (x - mu) / sigma;
    return exp(-0.5 * t * t);
}

fn hdr_rgb_to_spectral_radiance(r: f32, g: f32, b: f32, lambda_nm: f32) -> f32 {
    let rc = max(r, 0.0);
    let gc = max(g, 0.0);
    let bc = max(b, 0.0);
    return fma(
        rc, hdr_asymmetric_gaussian(lambda_nm, 615.0, 45.0, 65.0),
        fma(
            gc, hdr_asymmetric_gaussian(lambda_nm, 545.0, 45.0, 45.0),
            bc * hdr_asymmetric_gaussian(lambda_nm, 465.0, 40.0, 45.0),
        ),
    );
}

struct HdrEnvDims {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

struct HdrEnvCase {
    dir_x: f32,
    dir_y: f32,
    dir_z: f32,
    lambda_nm: f32,
}

@group(0) @binding(8) var<storage, read> hdr_env_cases: array<HdrEnvCase>;
@group(0) @binding(9) var<storage, read_write> hdr_env_out: array<f32>;
@group(0) @binding(10) var<storage, read> hdr_texels: array<vec4<f32>>;
@group(0) @binding(11) var<uniform> hdr_env_dims: HdrEnvDims;

fn hdr_wrap_x(x: i32, width: i32) -> u32 {
    return u32(((x % width) + width) % width);
}

fn hdr_clamp_y(y: i32, height: i32) -> u32 {
    return u32(clamp(y, 0, height - 1));
}

fn hdr_texel(x: u32, y: u32, width: u32) -> vec3<f32> {
    return hdr_texels[y * width + x].xyz;
}

fn hdr_direction_to_uv(dir: vec3<f32>) -> vec2<f32> {
    let d = normalize(dir);
    let theta = acos(clamp(d.y, -1.0, 1.0));
    let phi = atan2(d.x, d.z);
    let v = theta / PI;
    let u = fract(phi / (2.0 * PI));
    return vec2<f32>(u, v);
}

fn hdr_env_sample_bilinear(u_in: f32, v_in: f32) -> vec3<f32> {
    let width = hdr_env_dims.width;
    let height = hdr_env_dims.height;
    let width_i = i32(width);
    let height_i = i32(height);

    let u_wrapped = fract(u_in);
    let v_clamped = clamp(v_in, 0.0, 1.0);
    let fx = fma(u_wrapped, f32(width), -0.5);
    let fy = fma(v_clamped, f32(height), -0.5);

    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;

    let x0i = hdr_wrap_x(i32(x0), width_i);
    let x1i = hdr_wrap_x(i32(x0) + 1, width_i);
    let y0i = hdr_clamp_y(i32(y0), height_i);
    let y1i = hdr_clamp_y(i32(y0) + 1, height_i);

    let p00 = hdr_texel(x0i, y0i, width);
    let p10 = hdr_texel(x1i, y0i, width);
    let p01 = hdr_texel(x0i, y1i, width);
    let p11 = hdr_texel(x1i, y1i, width);

    let top = fma(p10, vec3<f32>(tx), p00 * (1.0 - tx));
    let bottom = fma(p11, vec3<f32>(tx), p01 * (1.0 - tx));
    return fma(bottom, vec3<f32>(ty), top * (1.0 - ty));
}

fn hdr_env_radiance_at(dir: vec3<f32>, lambda_nm: f32) -> f32 {
    let uv = hdr_direction_to_uv(dir);
    let rgb = hdr_env_sample_bilinear(uv.x, uv.y);
    return hdr_rgb_to_spectral_radiance(rgb.x, rgb.y, rgb.z, lambda_nm);
}

@compute @workgroup_size(64)
fn hdr_env_radiance_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&hdr_env_cases)) {
        return;
    }
    let c = hdr_env_cases[idx];
    hdr_env_out[idx] = hdr_env_radiance_at(vec3<f32>(c.dir_x, c.dir_y, c.dir_z), c.lambda_nm);
}
