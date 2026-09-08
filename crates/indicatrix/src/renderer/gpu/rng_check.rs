//! RNG/integer bit-exactness self-test (Tier 1), plus a Tier 2 ULP-budget check for the
//! fields in the same dispatch that aren't integer-derived.
//!
//! Dispatches `shaders/rng_equivalence.wgsl` over ~10^6 `(pixel, sample, bounce)` tuples
//! and compares every value against the identical computation performed on the CPU with
//! this crate's real `hash_u32`/seed-formula/`low_discrepancy_base2`/
//! `cranley_patterson_rotate`/`wrapped_hero_wavelengths` code -- see [`cpu_record`],
//! which calls straight into `optics::raytracer`, not a reimplementation of it.
//!
//! # Two tiers, two tolerances, in one dispatch
//!
//! [`compare_record`] (Tier 1, [`RngCheckResult`]) covers `seed`, `rot_jx_hash`,
//! `rot_jy_hash`, `rot_hero_hash`, `sample_reversed`, and the three per-bounce stream
//! draws -- all pure `u32` arithmetic, exact in WGSL, so this tier's tolerance is zero:
//! any disagreement is a bug, not noise.
//!
//! [`check_float_ulp`] (Tier 2, [`FloatUlpResult`]) covers `jx`, `jy`, `hero_rand`, and
//! `lambdas` -- fields built from float arithmetic (division, the Cranley-Patterson
//! rotation's add/floor/subtract, or `fma`) rather than pure integer ops. These don't
//! belong in Tier 1: on this workspace's dev hardware (AMD RDNA2 iGPU, Vulkan), the
//! shader compiler doesn't always fuse WGSL's `fma()` into a true single-rounding
//! hardware FMA the way `f32::mul_add` is on the CPU -- a real, measured small-ULP-scale
//! discrepancy, not a porting bug. See [`FLOAT_ULP_BUDGET`] for the budget.

use crate::{
    optics::raytracer::{
        BIREFRINGENT_SPLIT_STREAM, FRESNEL_BRANCH_STREAM, FROSTED_DIR_U_STREAM,
        FROSTED_DIR_V_STREAM, HERO_WAVELENGTH_ROTATION_STREAM, MODE_COUPLING_STREAM,
        PIXEL_JITTER_X_ROTATION_STREAM, PIXEL_JITTER_Y_ROTATION_STREAM, RUSSIAN_ROULETTE_STREAM,
        cranley_patterson_rotate, hash_u32, low_discrepancy_base2, radical_inverse_base,
        wrapped_hero_wavelengths,
    },
    renderer::gpu::compute,
};

const SHADER_SRC: &str = include_str!("../shaders/rng_equivalence.wgsl");

/// Must match `shaders/rng_equivalence.wgsl`'s `RngRecord` field-for-field. Every field
/// is a plain scalar/array, so `#[repr(C)]` packing already agrees with WGSL here --
/// no vec3/vec4 alignment pitfall.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RngRecord {
    pub seed: u32,
    /// `hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM)`, Tier 1 (pure integer).
    pub rot_jx_hash: u32,
    /// As [`Self::rot_jx_hash`], for `PIXEL_JITTER_Y_ROTATION_STREAM`.
    pub rot_jy_hash: u32,
    /// As [`Self::rot_jx_hash`], for `HERO_WAVELENGTH_ROTATION_STREAM`.
    pub rot_hero_hash: u32,
    /// `sample.reverse_bits()`, the base-2 van der Corput term index, Tier 1.
    pub sample_reversed: u32,
    /// Stratified pixel-jitter-X draw fed to `Camera::generate_ray`. Tier 2 (float).
    pub jx: f32,
    /// As [`Self::jx`], for pixel-jitter-Y.
    pub jy: f32,
    /// Stratified hero-wavelength draw fed to `wrapped_hero_wavelengths`. Tier 2.
    pub hero_rand: f32,
    pub lambdas: [f32; 8],
    pub fresnel_draws: [u32; 4],
    pub rr_draws: [u32; 4],
    pub biref_draws: [u32; 4],
    /// Tier 1 integer draw for the o<->e (uniaxial) / mode-A<->mode-B (biaxial)
    /// re-coupling decision -- see `optics::raytracer::apply_internal_mode_coupling`.
    pub mode_coupling_draws: [u32; 4],
    /// Girdle finish: Tier 1 draws for `apply_frosted_bounce`'s cosine-weighted
    /// hemisphere sample -- see `optics::raytracer::FROSTED_DIR_U_STREAM`.
    pub frosted_dir_u_draws: [u32; 4],
    /// As [`Self::frosted_dir_u_draws`], for `optics::raytracer::FROSTED_DIR_V_STREAM`.
    pub frosted_dir_v_draws: [u32; 4],
}

const _: () = assert!(size_of::<RngRecord>() == 160);

/// Must match `shaders/rng_equivalence.wgsl`'s `Params` uniform.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    num_samples: u32,
    num_bounces: u32,
    _pad0: u32,
    _pad1: u32,
}

/// Number of bounces exercised per `(pixel, sample)` pair. Fixed at 4 to match
/// `RngRecord`'s fixed-capacity per-bounce arrays.
pub const NUM_BOUNCES: u32 = 4;

/// Computes the exact CPU-side equivalent of one GPU `RngRecord`.
///
/// Calls straight into `optics::raytracer`'s real hashing/wavelength functions rather
/// than a parallel reimplementation, since a bug shared by both would never be caught
/// by comparing them.
#[must_use]
pub fn cpu_record(pixel: u32, sample: u32) -> RngRecord {
    let seed = hash_u32(pixel.wrapping_mul(0x9e37_79b9) ^ sample.wrapping_mul(0x85eb_ca6b));

    // Stratified pixel jitter and hero wavelength each use a different prime base
    // (2, 3, 5): same base for all three measured worse variance on high-variance pixels.
    let rot_jx_hash = hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM);
    let rot_jy_hash = hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM);
    let rot_hero_hash = hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM);
    // Tier 1 precursor for `jx` (base 2) only -- `jy`/`hero_rand` (bases 3/5) have no
    // comparably cheap pure-integer precursor since `radical_inverse_base` interleaves
    // digit extraction with float accumulation instead of resolving to one integer.
    let sample_reversed = sample.reverse_bits();

    let rot_jx = low_discrepancy_base2(rot_jx_hash);
    let rot_jy = low_discrepancy_base2(rot_jy_hash);
    let rot_hero = low_discrepancy_base2(rot_hero_hash);
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample, 3), rot_jy) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample, 5), rot_hero);

    let lambdas: [f32; 8] = wrapped_hero_wavelengths(hero_rand);

    let mut fresnel_draws = [0u32; 4];
    let mut rr_draws = [0u32; 4];
    let mut biref_draws = [0u32; 4];
    let mut mode_coupling_draws = [0u32; 4];
    let mut frosted_dir_u_draws = [0u32; 4];
    let mut frosted_dir_v_draws = [0u32; 4];
    for bounce in 0..NUM_BOUNCES {
        let b = bounce;
        fresnel_draws[bounce as usize] = hash_u32(seed ^ hash_u32(b ^ FRESNEL_BRANCH_STREAM));
        rr_draws[bounce as usize] = hash_u32(seed ^ hash_u32(b ^ RUSSIAN_ROULETTE_STREAM));
        biref_draws[bounce as usize] = hash_u32(seed ^ hash_u32(b ^ BIREFRINGENT_SPLIT_STREAM));
        mode_coupling_draws[bounce as usize] = hash_u32(seed ^ hash_u32(b ^ MODE_COUPLING_STREAM));
        frosted_dir_u_draws[bounce as usize] = hash_u32(seed ^ hash_u32(b ^ FROSTED_DIR_U_STREAM));
        frosted_dir_v_draws[bounce as usize] = hash_u32(seed ^ hash_u32(b ^ FROSTED_DIR_V_STREAM));
    }

    RngRecord {
        seed,
        rot_jx_hash,
        rot_jy_hash,
        rot_hero_hash,
        sample_reversed,
        jx,
        jy,
        hero_rand,
        lambdas,
        fresnel_draws,
        rr_draws,
        biref_draws,
        mode_coupling_draws,
        frosted_dir_u_draws,
        frosted_dir_v_draws,
    }
}

/// One (pixel, sample) record's disagreement between GPU and CPU, with enough detail
/// to diagnose without re-running anything.
#[derive(Debug, Clone)]
pub struct RngMismatch {
    pub pixel: u32,
    pub sample: u32,
    pub field: &'static str,
    pub cpu: String,
    pub gpu: String,
}

/// Tier 1's zero-tolerance integer result, plus Tier 2's float-field ULP-budget result,
/// from the same GPU dispatch and CPU comparison loop in [`run`].
#[derive(Debug, Clone)]
pub struct RngCheckResult {
    pub total_records: usize,
    /// Tier 1: pure-integer fields, zero tolerance.
    pub mismatches: Vec<RngMismatch>,
    /// Tier 2: `jx`/`jy`/`hero_rand`/`lambdas` against [`FLOAT_ULP_BUDGET`].
    pub float_ulp: FloatUlpResult,
}

impl RngCheckResult {
    /// Tier 1 alone: true iff every pure-integer field matched exactly.
    #[must_use]
    pub const fn tier1_passed(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// Both tiers: Tier 1 exact AND Tier 2 within [`FLOAT_ULP_BUDGET`].
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.tier1_passed() && self.float_ulp.passed()
    }
}

/// Tier 1 ONLY: pure-integer fields, zero tolerance. `jx`/`jy`/`hero_rand`/`lambdas`
/// are excluded -- see [`FloatUlpAccumulator`] for why they belong in Tier 2.
fn compare_record(
    pixel: u32,
    sample: u32,
    cpu: &RngRecord,
    gpu: &RngRecord,
    out: &mut Vec<RngMismatch>,
) {
    macro_rules! check {
        ($field:ident, $name:literal) => {
            if cpu.$field != gpu.$field {
                out.push(RngMismatch {
                    pixel,
                    sample,
                    field: $name,
                    cpu: format!("{:?}", cpu.$field),
                    gpu: format!("{:?}", gpu.$field),
                });
            }
        };
    }
    check!(seed, "seed");
    check!(rot_jx_hash, "rot_jx_hash");
    check!(rot_jy_hash, "rot_jy_hash");
    check!(rot_hero_hash, "rot_hero_hash");
    check!(sample_reversed, "sample_reversed");
    check!(fresnel_draws, "fresnel_draws");
    check!(rr_draws, "rr_draws");
    check!(biref_draws, "biref_draws");
    check!(mode_coupling_draws, "mode_coupling_draws");
    check!(frosted_dir_u_draws, "frosted_dir_u_draws");
    check!(frosted_dir_v_draws, "frosted_dir_v_draws");
}

/// Tier 2 ULP budget for `jx`/`jy`/`hero_rand`/`lambdas` (fields built from float
/// arithmetic -- division, Cranley-Patterson add/floor/subtract, or `fma`).
///
/// Measured on this workspace's dev hardware (AMD RDNA2 iGPU, Vulkan) over a
/// 1,048,576-tuple run: observed max was **1 ULP**, on ~0.024% of records, consistent
/// with the shader compiler not always fusing `fma()` into a true hardware FMA. 4 ULP
/// gives comfortable margin for other GPU/drivers while staying far below what a real
/// algebra bug produces (typically thousands of ULP) -- see [`crate::renderer::gpu::ulp`]
/// for why ULP, not a relative epsilon. This divergence is also why a later Tier 3
/// (statistical image comparison) must be variance-scaled rather than exact.
pub const FLOAT_ULP_BUDGET: u32 = 4;

/// Absolute-difference floor for `jx`/`jy`/`hero_rand`/`lambdas` comparisons.
///
/// See [`crate::renderer::gpu::ulp::within_tolerance`] for the general rationale. `jx`/`jy`
/// range over `[-0.5, 0.5)` and legitimately land near zero (measured: a genuine,
/// non-buggy pair at `jy ~= -6e-6` registered as 262,144 ULP apart from sign-boundary
/// proximity alone). `lambdas` never approaches zero (`[380.0, 780.0]`), so the floor
/// is inert there.
pub const FLOAT_ABS_FLOOR: f32 = 1e-4;

/// One float field's ULP distance from the CPU reference, kept only when it's the
/// running argmax -- see [`FloatUlpResult`].
///
/// `field` names the `RngRecord` field
/// (`"jx"`, `"jy"`, `"hero_rand"`, `"lambdas"`); `channel` is the `lambdas` array index
/// and `0` otherwise.
#[derive(Debug, Clone, Copy)]
pub struct FloatUlpArgmax {
    pub pixel: u32,
    pub sample: u32,
    pub field: &'static str,
    pub channel: usize,
    pub cpu: f32,
    pub gpu: f32,
    pub ulp: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct FloatUlpResult {
    pub budget: u32,
    /// Max ULP among comparisons NOT exempted by [`FLOAT_ABS_FLOOR`] -- what
    /// [`Self::passed`] checks against `budget`.
    pub max_ulp: u32,
    /// Max ULP among ALL comparisons, including exempted ones -- purely informational.
    pub max_raw_ulp: u32,
    pub argmax: Option<FloatUlpArgmax>,
    pub over_budget_count: usize,
    pub exempted_count: usize,
    pub total_values_compared: usize,
}

impl FloatUlpResult {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.over_budget_count == 0
    }
}

/// Streaming accumulator for the Tier 2 float-field ULP check -- fed one record pair
/// at a time from the same loop [`compare_record`] runs in, so `run` never holds all
/// `total` record pairs in memory at once.
#[derive(Debug, Default)]
struct FloatUlpAccumulator {
    max_ulp: u32,
    max_raw_ulp: u32,
    argmax: Option<FloatUlpArgmax>,
    over_budget_count: usize,
    exempted_count: usize,
    total_values_compared: usize,
}

impl FloatUlpAccumulator {
    fn record_one(
        &mut self,
        pixel: u32,
        sample: u32,
        field: &'static str,
        channel: usize,
        c: f32,
        g: f32,
    ) {
        use crate::renderer::gpu::ulp::{ulp_distance, within_tolerance};

        self.total_values_compared += 1;
        let ulp = ulp_distance(c, g);
        self.max_raw_ulp = self.max_raw_ulp.max(ulp);

        if within_tolerance(c, g, FLOAT_ULP_BUDGET, FLOAT_ABS_FLOOR) {
            if ulp > FLOAT_ULP_BUDGET {
                // Within tolerance only via the abs-floor clause: a near-zero exemption.
                self.exempted_count += 1;
            }
            return;
        }

        self.over_budget_count += 1;
        if ulp > self.max_ulp {
            self.max_ulp = ulp;
            self.argmax = Some(FloatUlpArgmax {
                pixel,
                sample,
                field,
                channel,
                cpu: c,
                gpu: g,
                ulp,
            });
        }
    }

    fn record(&mut self, pixel: u32, sample: u32, cpu: &RngRecord, gpu: &RngRecord) {
        self.record_one(pixel, sample, "jx", 0, cpu.jx, gpu.jx);
        self.record_one(pixel, sample, "jy", 0, cpu.jy, gpu.jy);
        self.record_one(pixel, sample, "hero_rand", 0, cpu.hero_rand, gpu.hero_rand);
        for (channel, (&c, &g)) in cpu.lambdas.iter().zip(gpu.lambdas.iter()).enumerate() {
            self.record_one(pixel, sample, "lambdas", channel, c, g);
        }
    }

    const fn finish(self) -> FloatUlpResult {
        FloatUlpResult {
            budget: FLOAT_ULP_BUDGET,
            max_ulp: self.max_ulp,
            max_raw_ulp: self.max_raw_ulp,
            argmax: self.argmax,
            over_budget_count: self.over_budget_count,
            exempted_count: self.exempted_count,
            total_values_compared: self.total_values_compared,
        }
    }
}

/// Runs the RNG bit-exactness self-test against a live GPU.
///
/// Exercises `num_pixels * num_samples` `(pixel, sample)` pairs, each carrying
/// [`NUM_BOUNCES`] bounces.
///
/// # Panics
///
/// Panics on `wgpu` API misuse.
#[must_use]
pub fn run(
    ctx: &crate::renderer::gpu::GpuContext,
    num_pixels: u32,
    num_samples: u32,
) -> RngCheckResult {
    let total = (num_pixels as usize) * (num_samples as usize);

    let params = Params {
        num_samples,
        num_bounces: NUM_BOUNCES,
        _pad0: 0,
        _pad1: 0,
    };
    let params_buf = compute::upload(
        &ctx.device,
        "rng_equivalence params",
        std::slice::from_ref(&params),
        wgpu::BufferUsages::UNIFORM,
    );
    let out_buf = compute::zeroed_buffer::<RngRecord>(
        &ctx.device,
        "rng_equivalence output",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );

    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "rng_equivalence", SHADER_SRC, "main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "rng_equivalence bind group",
        &pipeline,
        &[(0, &params_buf), (1, &out_buf)],
    );

    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );

    let gpu_records: Vec<RngRecord> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    // Both tiers walk every record in one pass. Tier 1's diagnostic list caps at 64
    // entries (plenty to diagnose a break), but that cap must not cut the Tier 2 scan
    // short -- a real bug can corrupt both tiers at once, so Tier 2 still needs an
    // accurate reading.
    let mut mismatches = Vec::new();
    let mut float_acc = FloatUlpAccumulator::default();
    for (idx, gpu_record) in gpu_records.iter().enumerate() {
        let pixel = (idx as u32) / num_samples;
        let sample = (idx as u32) % num_samples;
        let cpu = cpu_record(pixel, sample);
        if mismatches.len() < 64 {
            compare_record(pixel, sample, &cpu, gpu_record, &mut mismatches);
        }
        float_acc.record(pixel, sample, &cpu, gpu_record);
    }

    RngCheckResult {
        total_records: total,
        mismatches,
        float_ulp: float_acc.finish(),
    }
}
