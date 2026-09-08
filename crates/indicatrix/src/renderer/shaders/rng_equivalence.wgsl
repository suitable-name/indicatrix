// RNG/integer bit-exactness self-test kernel, driven by `renderer::gpu::rng_check` --
// not a physics kernel. Ports five pieces of `indicatrix`'s deterministic-sampling
// machinery to WGSL, bit-for-bit against their Rust source (WGSL `u32` arithmetic wraps
// modulo 2^32 the same as Rust's `wrapping_*`, so "bit-exact" is the actual bar):
//   1. `optics::raytracer::hash_u32` -- the integer hash everything else is built from.
//   2. `render_core.rs::trace_samples`'s per-sample seed formula.
//   3. Stratified pixel-jitter/hero-wavelength draw (`optics::raytracer::
//      {low_discrepancy_base2, cranley_patterson_rotate}` and the
//      `PIXEL_JITTER_X/Y_ROTATION_STREAM`/`HERO_WAVELENGTH_ROTATION_STREAM` salts).
//   4. `optics::raytracer::trace_spectral_ray`'s four salted per-bounce draws
//      (`FRESNEL_BRANCH_STREAM`/`RUSSIAN_ROULETTE_STREAM`/`BIREFRINGENT_SPLIT_STREAM`/
//      `MODE_COUPLING_STREAM`).
//   5. `optics::raytracer::wrapped_hero_wavelengths`'s hero-wavelength comb.
//
// `%` vs `rem_euclid`: WGSL's `%` is a truncating remainder, not `rem_euclid` (they
// differ for a negative dividend). `wrapped_hero_wavelengths` uses `rem_euclid` because
// its CPU call site can't prove `offset` stays non-negative. Here `offset` is provably
// >= 0.0 by construction (`lambda_hero >= 380.0`, `k * channel_width >= 0.0`), and for a
// non-negative dividend with a positive divisor `%` and `rem_euclid` agree -- so plain
// `%` is safe here ONLY because of that invariant.
//
// `fma`, not `a * b + c`: the CPU formula uses `f32::mul_add` (single-rounding fused
// multiply-add); WGSL's `fma()` carries the same guarantee, so both call sites here use
// it instead of `*`/`+`. Measured on this workspace's dev hardware (AMD RDNA2 iGPU,
// Vulkan): integer fields come back byte-exact against the CPU; the `fma`-built floats
// (`lambdas`, `jx`, `jy`, `hero_rand`) differ by up to 1 ULP in ~0.02% of records,
// consistent with the driver lowering `fma()` to a non-fused multiply-add rather than a
// real algebra bug. `renderer::gpu::rng_check` tiers tolerance accordingly: integer
// fields (Tier 1) at zero tolerance, `fma`-built floats (Tier 2) at 1 ULP.

struct RngRecord {
    seed: u32,
    rot_jx_hash: u32,
    rot_jy_hash: u32,
    rot_hero_hash: u32,
    sample_reversed: u32,
    jx: f32,
    jy: f32,
    hero_rand: f32,
    lambdas: array<f32, 8>,
    fresnel_draws: array<u32, 4>,
    rr_draws: array<u32, 4>,
    biref_draws: array<u32, 4>,
    mode_coupling_draws: array<u32, 4>,
    // Tier 1 integer draws for apply_frosted_bounce's 2D cosine-weighted-hemisphere
    // direction sample -- see optics::raytracer::{FROSTED_DIR_U_STREAM, FROSTED_DIR_V_STREAM}.
    frosted_dir_u_draws: array<u32, 4>,
    frosted_dir_v_draws: array<u32, 4>,
}

struct Params {
    num_samples: u32,
    // Must be <= 4 -- RngRecord's per-bounce arrays are fixed at capacity 4. Enforced by
    // the host (`renderer::gpu::rng_check`) before dispatch, not re-checked here.
    num_bounces: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> out_records: array<RngRecord>;

const SPECTRUM_MIN: f32 = 380.0;
const SPECTRUM_SPAN: f32 = 400.0; // 780.0 - 380.0
const NUM_CHANNELS: u32 = 8u;

// Same four stream salts as `optics::raytracer::{FRESNEL_BRANCH_STREAM,
// RUSSIAN_ROULETTE_STREAM, BIREFRINGENT_SPLIT_STREAM, MODE_COUPLING_STREAM}`.
const FRESNEL_BRANCH_STREAM: u32 = 0x9E3779B1u;
const RUSSIAN_ROULETTE_STREAM: u32 = 0x517CC1B7u;
const BIREFRINGENT_SPLIT_STREAM: u32 = 0x2545F491u;
const MODE_COUPLING_STREAM: u32 = 0xCC9E2D51u;
// optics::raytracer::{FROSTED_DIR_U_STREAM, FROSTED_DIR_V_STREAM}.
const FROSTED_DIR_U_STREAM: u32 = 0x27D4EB2Fu;
const FROSTED_DIR_V_STREAM: u32 = 0x165667B1u;

// Same three stream salts as `optics::raytracer::{PIXEL_JITTER_X_ROTATION_STREAM,
// PIXEL_JITTER_Y_ROTATION_STREAM, HERO_WAVELENGTH_ROTATION_STREAM}`.
const PIXEL_JITTER_X_ROTATION_STREAM: u32 = 0xA511E9B3u;
const PIXEL_JITTER_Y_ROTATION_STREAM: u32 = 0x63D81B23u;
const HERO_WAVELENGTH_ROTATION_STREAM: u32 = 0x1B873593u;

fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x = x * 0x85ebca6bu;
    x = x ^ (x >> 13u);
    x = x * 0xc2b2ae35u;
    x = x ^ (x >> 16u);
    return x;
}

// optics::raytracer::{low_discrepancy_base2, radical_inverse_base,
// cranley_patterson_rotate}. jx/jy/hero_rand use bases 2/3/5 -- using the same base for
// all three measured WORSE variance for the highest-variance pixels.
fn low_discrepancy_base2(n: u32) -> f32 {
    return f32(reverseBits(n)) / 4294967296.0;
}

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

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&out_records)) {
        return;
    }
    let pixel = idx / params.num_samples;
    let sample = idx % params.num_samples;

    // render_core.rs::trace_samples's seed formula -- seeds every per-bounce draw below.
    let seed = hash_u32((pixel * 0x9e3779b9u) ^ (sample * 0x85ebca6bu));

    // Stratified pixel jitter and hero wavelength.
    let rot_jx_hash = hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM);
    let rot_jy_hash = hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM);
    let rot_hero_hash = hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM);
    let sample_reversed = reverseBits(sample);

    var rec: RngRecord;
    rec.seed = seed;
    rec.rot_jx_hash = rot_jx_hash;
    rec.rot_jy_hash = rot_jy_hash;
    rec.rot_hero_hash = rot_hero_hash;
    rec.sample_reversed = sample_reversed;

    let rot_jx = low_discrepancy_base2(rot_jx_hash);
    let rot_jy = low_discrepancy_base2(rot_jy_hash);
    let rot_hero = low_discrepancy_base2(rot_hero_hash);
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample, 3u), rot_jy) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample, 5u), rot_hero);
    rec.jx = jx;
    rec.jy = jy;
    rec.hero_rand = hero_rand;

    let channel_width = SPECTRUM_SPAN / f32(NUM_CHANNELS);
    let lambda_hero = fma(hero_rand, SPECTRUM_SPAN, SPECTRUM_MIN);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let offset = fma(f32(k), channel_width, lambda_hero - SPECTRUM_MIN);
        // Invariant documented above: `offset` is always >= 0.0 here, so plain `%`
        // agrees with the CPU's `rem_euclid`.
        let wrapped = offset % SPECTRUM_SPAN;
        rec.lambdas[k] = SPECTRUM_MIN + wrapped;
    }

    for (var b: u32 = 0u; b < params.num_bounces; b = b + 1u) {
        rec.fresnel_draws[b] = hash_u32(seed ^ hash_u32(b ^ FRESNEL_BRANCH_STREAM));
        rec.rr_draws[b] = hash_u32(seed ^ hash_u32(b ^ RUSSIAN_ROULETTE_STREAM));
        rec.biref_draws[b] = hash_u32(seed ^ hash_u32(b ^ BIREFRINGENT_SPLIT_STREAM));
        rec.mode_coupling_draws[b] = hash_u32(seed ^ hash_u32(b ^ MODE_COUPLING_STREAM));
        rec.frosted_dir_u_draws[b] = hash_u32(seed ^ hash_u32(b ^ FROSTED_DIR_U_STREAM));
        rec.frosted_dir_v_draws[b] = hash_u32(seed ^ hash_u32(b ^ FROSTED_DIR_V_STREAM));
    }

    out_records[idx] = rec;
}
