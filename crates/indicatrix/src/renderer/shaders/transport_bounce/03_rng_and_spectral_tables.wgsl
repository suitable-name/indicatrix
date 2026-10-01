
// hash_u32 -- optics::raytracer::hash_u32, bit-exact. Defined in
// `transport_physics.wgsl` -- look there, not here.
//
// optics::raytracer::{low_discrepancy_base2, radical_inverse_base,
// cranley_patterson_rotate, PIXEL_JITTER_X_ROTATION_STREAM,
// PIXEL_JITTER_Y_ROTATION_STREAM, HERO_WAVELENGTH_ROTATION_STREAM}, bit-exact (see
// shaders/rng_equivalence.wgsl for the dedicated GPU/CPU RNG self-test).
//
// jx/jy/hero_rand use three different prime bases (2, 3, 5), not the same base rotated
// three ways -- measured: same-base pairing made variance worse for the
// highest-variance pixels.

const PIXEL_JITTER_X_ROTATION_STREAM: u32 = 0xA511E9B3u;
const PIXEL_JITTER_Y_ROTATION_STREAM: u32 = 0x63D81B23u;
const HERO_WAVELENGTH_ROTATION_STREAM: u32 = 0x1B873593u;

fn low_discrepancy_base2(n: u32) -> f32 {
    return f32(reverseBits(n)) / 4294967296.0;
}

// optics::raytracer::radical_inverse_base -- general prime-base radical inverse (base 2
// uses the faster bit-reversal path above instead). This uses `fma()`
// (`val = fma(f32(digit), inv_base, val)` below), matching the CPU side's own
// `(digit as f32).mul_add(inv_base, val)` exactly.
fn radical_inverse_base(n_in: u32, base: u32) -> f32 {
    var n = n_in;
    var val: f32 = 0.0;
    var inv_base: f32 = 1.0 / f32(base);
    loop {
        if (n == 0u) {
            break;
        }
        let digit = n % base;
        val = fma(f32(digit), inv_base, val);
        inv_base = inv_base / f32(base);
        n = n / base;
    }
    return val;
}

fn cranley_patterson_rotate(x: f32, offset: f32) -> f32 {
    let sum = x + offset;
    return sum - floor(sum);
}

// The CIE 1931 colour matching table, `cie_1931_cmf`, and
// `integrate_channels_to_xyz_family` live in `transport_physics/05_nee_env_sampling.wgsl` so the
// standalone transport-function kernels share them with the megakernel.

// optics::renderer::env_map_spectrum::rgb_to_spectral_radiance -- used for the
// direction-independent "uniform furnace" environment (env_mode == 0u): a grey
// `EnvironmentMap::uniform(w, h, [l0, l0, l0])` that is still wavelength-dependent, so
// this is deliberately not flattened to a bare `l0` return.

// asymmetric_gaussian and rgb_to_spectral_radiance (neutral part on the wide bumps, chroma
// remainder on the narrow bumps) are defined in transport_physics.wgsl.

// optics::raytracer::sample_studio_environment (+ optics::studio_rig::StudioRig) --
// ported identically to shaders/environment.wgsl.

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

// optics::raytracer::environment::{CIE_D65_SPD_380_780_10NM, d65_relative_spectral_power}
// (CIE 15:2004) -- the "D65 Daylight" preset's real measured table, used instead of
// `blackbody_spectrum` above. A `const` array, not a uniform/storage buffer: 41 `f32`s
// is small enough to inline directly, needing no extra bind-group slot or upload.
const CIE_D65_SPD_380_780_10NM: array<f32, 41> = array<f32, 41>(
    49.9755, 54.6482, 82.7549, 91.4860, 93.4318, 86.6823, 104.865, 117.008, 117.812, 114.861,
    115.923, 108.811, 109.354, 107.802, 104.790, 107.689, 104.405, 104.046, 100.000, 96.3342,
    95.7880, 88.6856, 90.0062, 89.5991, 87.6987, 83.2886, 83.6992, 80.0268, 80.2146, 82.2778,
    78.2842, 69.7213, 71.6091, 74.3496, 61.6045, 69.8856, 75.0870, 63.5928, 46.4182, 66.8054,
    63.3828,
);

// Ported op-for-op from `optics::raytracer::environment::d65_relative_spectral_power`:
// same 560nm-normalization (dividing by the table's `100.000` entry), same
// clamp-to-table-edge behaviour outside 380-780nm.
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

