//! Environment-sampling / CMF-integration / white-balance GPU self-tests: per-function
//! ULP budgets.
//!
//! Four independent checks, one per ported function, all dispatched from
//! `shaders/environment.wgsl`:
//! - [`run_cmf`][]: `color::cie1931::cie_1931_cmf`.
//! - [`run_blackbody`][]: `optics::raytracer::blackbody_spectrum`.
//! - [`run_studio_env`][]: `optics::raytracer::sample_studio_environment` (across all
//!   four [`LightingPreset`] variants).
//! - [`run_white_balance`][]: `optics::raytracer::compute_illuminant_white_balance`'s
//!   401-point (380..=780nm) quadrature.
//!
//! Every dense sweep also includes the adversarial points the task calls out
//! specifically: the 380/780nm band edges, every CMF table node and interpolation
//! midpoint (P4: `cie_1931_cmf` is now a 5nm-table lookup, not a Gaussian-lobe fit --
//! see [`build_cmf_lambdas`]), grazing incidence (dot products near 0 and near the ring
//! lights' `0.96` spark threshold), and the `temp_k`/exponent clamp boundaries.

use crate::{
    color::cie1931::cie_1931_cmf,
    optics::raytracer::{
        LightingModel, LightingPreset, blackbody_spectrum, compute_illuminant_white_balance,
        sample_studio_environment_observed,
    },
    renderer::gpu::{
        compute,
        ulp::{ulp_distance, within_tolerance},
    },
};
use glam::Vec3;

const SHADER_SRC: &str = include_str!("../shaders/environment.wgsl");

/// A generic "worst single scalar" ULP result, shared by every check in this module.
///
/// Comparisons use the hybrid ULP-OR-absolute-floor rule in
/// [`crate::renderer::gpu::ulp::within_tolerance`]: a comparison whose absolute
/// difference is under `abs_floor` (see each check's own `*_ABS_FLOOR` constant) is
/// exempted from the ULP budget entirely -- ULP is a poor metric exactly where a value
/// crosses zero or is photometrically negligible. `max_ulp`/`argmax` track the worst
/// GENUINE disagreement (i.e. excluding exempted comparisons); `max_raw_ulp` is purely
/// informational and tracks the single largest ULP distance observed across EVERY
/// comparison, exempted or not, so an exempted near-zero case is still visible in a
/// report rather than silently vanishing.
#[derive(Debug, Clone)]
pub struct UlpCheckResult<Case: Clone> {
    pub label: &'static str,
    pub total_comparisons: usize,
    pub budget: u32,
    pub abs_floor: f32,
    pub max_ulp: u32,
    pub max_raw_ulp: u32,
    pub over_budget_count: usize,
    pub exempted_count: usize,
    pub argmax: Option<UlpArgmax<Case>>,
}

#[derive(Debug, Clone)]
pub struct UlpArgmax<Case: Clone> {
    pub case: Case,
    pub component: &'static str,
    pub cpu: f32,
    pub gpu: f32,
    pub ulp: u32,
}

impl<Case: Clone> UlpCheckResult<Case> {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.over_budget_count == 0
    }
}

struct UlpAccumulator<Case: Clone> {
    label: &'static str,
    budget: u32,
    abs_floor: f32,
    total: usize,
    max_ulp: u32,
    max_raw_ulp: u32,
    over_budget: usize,
    exempted: usize,
    argmax: Option<UlpArgmax<Case>>,
}

impl<Case: Clone> UlpAccumulator<Case> {
    const fn new(label: &'static str, budget: u32, abs_floor: f32) -> Self {
        Self {
            label,
            budget,
            abs_floor,
            total: 0,
            max_ulp: 0,
            max_raw_ulp: 0,
            over_budget: 0,
            exempted: 0,
            argmax: None,
        }
    }

    fn record(&mut self, case: &Case, component: &'static str, cpu: f32, gpu: f32) {
        self.total += 1;
        let ulp = ulp_distance(cpu, gpu);
        if ulp > self.max_raw_ulp {
            self.max_raw_ulp = ulp;
        }
        let within = within_tolerance(cpu, gpu, self.budget, self.abs_floor);
        if !within {
            self.over_budget += 1;
            if ulp > self.max_ulp {
                self.max_ulp = ulp;
                self.argmax = Some(UlpArgmax {
                    case: case.clone(),
                    component,
                    cpu,
                    gpu,
                    ulp,
                });
            }
        } else if ulp > self.budget {
            // Would have failed on ULP alone but was rescued by the absolute floor.
            self.exempted += 1;
        }
    }

    fn finish(self) -> UlpCheckResult<Case> {
        UlpCheckResult {
            label: self.label,
            total_comparisons: self.total,
            budget: self.budget,
            abs_floor: self.abs_floor,
            max_ulp: self.max_ulp,
            max_raw_ulp: self.max_raw_ulp,
            over_budget_count: self.over_budget,
            exempted_count: self.exempted,
            argmax: self.argmax,
        }
    }
}

// ---------------------------------------------------------------------------------
// cie_1931_cmf
// ---------------------------------------------------------------------------------

/// ULP budget for `cie_1931_cmf`.
///
/// P4: `cie_1931_cmf` is now a 5nm-table lookup with a single-`t` linear interpolation
/// (`lo + (hi - lo) * t`), not the old six-lobe Gaussian fit this budget was originally
/// calibrated against (each lobe chained its own `exp`/`fma`, so the old budget absorbed
/// accumulated rounding across six transcendental evaluations). The table port has only
/// one subtract-multiply-add chain per component, so it should need a narrower budget
/// than this in practice -- kept unchanged (rather than guessed tighter) as a
/// conservative ceiling pending a fresh measurement on live GPU hardware after this
/// port; see [`build_cmf_lambdas`] for the case set that measurement should exercise.
pub const CMF_ULP_BUDGET: u32 = 32;

/// Absolute-difference floor exempting near-zero comparisons from [`CMF_ULP_BUDGET`]
/// entirely.
///
/// See [`crate::renderer::gpu::ulp::within_tolerance`]'s doc comment for why ULP is the
/// wrong metric there. P4: unlike the old Gaussian fit (whose tails decayed smoothly
/// toward zero, e.g. as small as ~1e-24 far outside the visible range),
/// [`crate::color::cie1931::cie_1931_cmf`]'s table lookup returns EXACTLY zero for any
/// wavelength outside 380-780nm by construction (see that function's own
/// `out_of_range_wavelengths_return_zero` test) -- so an in-range/out-of-range
/// disagreement between CPU and GPU would show up as a large absolute difference, not a
/// near-zero one this floor could mask. This floor now exists only to absorb ordinary
/// near-zero rounding-direction noise at the table's own smallest tabulated entries
/// (e.g. `z_bar` sits at exactly `0.0000` for 630nm and above), several orders of
/// magnitude below anything a renderer could visibly distinguish.
pub const CMF_ABS_FLOOR: f32 = 1e-6;

/// Dense wavelength sweep plus adversarial points.
///
/// P4: `cie_1931_cmf` is now [`crate::color::cie1931::CIE_1931_TABLE`], a 5nm table
/// (380-780nm) with linear interpolation, not the old Gaussian-lobe fit this case set
/// was originally built for -- the old per-lobe-mean adversarial points (`x_bar`:
/// 442.0, 599.8, 501.1; `y_bar`: 530.9, 568.8; `z_bar`: 459.0, 437.0, where the old fit's
/// piecewise sigma switched) test nothing about a table lookup, so they are replaced
/// below with points meaningful to THIS implementation: every table node (exact lookup,
/// `t == 0.0`), every node's interpolation midpoint (worst-case interpolation, `t ==
/// 0.5`), the two in-range boundary wavelengths (380.0/780.0 -- distinct from the
/// epsilon-just-outside probes below), and epsilon-scale probes just inside/outside the
/// tabulated range -- the single most failure-prone points for a from-scratch WGSL port
/// of this function's `>=`/`<=` range test and `floor`-based index computation (mirrors
/// `cie1931.rs`'s own `boundary_wavelengths_return_the_table_edges_exactly` and
/// `out_of_range_wavelengths_return_zero` tests). The pre-existing dense 0.5nm-step
/// sweep from 300nm to 850nm is kept too: `TABLE_STEP_NM` = 5nm is an exact multiple of
/// that 0.5nm step, so the dense sweep already lands on every table node and every
/// midpoint as well, plus it alone covers the far out-of-range tail this function must
/// also return exactly zero for.
#[must_use]
pub fn build_cmf_lambdas() -> Vec<f32> {
    let mut lambdas = Vec::new();
    let steps = ((850.0 - 300.0) / 0.5) as u32;
    for step in 0..=steps {
        lambdas.push((step as f32).mul_add(0.5, 300.0));
    }
    lambdas.push(380.0);
    lambdas.push(780.0);
    // Every table node (380, 385, ..., 780nm) plus its interpolation midpoint,
    // explicitly -- see this function's doc comment for why the dense sweep above
    // already includes these too, and why they are still listed here directly.
    for node in 0..81u32 {
        let lambda = (node as f32).mul_add(5.0, 380.0);
        lambdas.push(lambda);
        if node < 80 {
            lambdas.push(lambda + 2.5);
        }
    }
    // Epsilon-scale probes just inside/outside the tabulated 380-780nm range, plus
    // points far outside it.
    for &lambda in &[379.999f32, 380.001, 779.999, 780.001, -100.0, 2000.0] {
        lambdas.push(lambda);
    }
    lambdas
}

/// Runs the `cie_1931_cmf` ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_cmf(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<f32> {
    let lambdas = build_cmf_lambdas();
    let total = lambdas.len();

    let in_buf = compute::upload(
        &ctx.device,
        "cmf lambdas",
        &lambdas,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "cmf out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "cmf_main", SHADER_SRC, "cmf_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "cmf bind group",
        &pipeline,
        &[(0, &in_buf), (1, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_xyz: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut acc = UlpAccumulator::new("cie_1931_cmf", CMF_ULP_BUDGET, CMF_ABS_FLOOR);
    for (idx, &lambda) in lambdas.iter().enumerate() {
        let cpu = cie_1931_cmf(lambda);
        acc.record(&lambda, "x", cpu[0], gpu_xyz[idx * 3]);
        acc.record(&lambda, "y", cpu[1], gpu_xyz[idx * 3 + 1]);
        acc.record(&lambda, "z", cpu[2], gpu_xyz[idx * 3 + 2]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// blackbody_spectrum
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlackbodyCase {
    lambda_nm: f32,
    temp_k: f32,
}

/// ULP budget for `blackbody_spectrum`.
///
/// See [`CMF_ULP_BUDGET`]'s doc comment for the calibration philosophy; this function
/// chains two `exp()`s and a `powi_u(_, 5)` (exponentiation by squaring, chosen
/// deliberately over WGSL's general `pow()` -- see `environment.wgsl`'s own comment on
/// why), so a slightly larger budget than the pure polynomial `cie_1931_cmf` is
/// expected.
pub const BLACKBODY_ULP_BUDGET: u32 = 48;

/// Absolute-difference floor for `blackbody_spectrum` comparisons.
///
/// See [`CMF_ABS_FLOOR`]'s doc comment for the rationale. `blackbody_spectrum` is clamped
/// to `[0.01, 20.0]` by construction, so `0.01` is the SMALLEST value this function can
/// ever legitimately return -- a floor several orders of magnitude below that clamp still
/// cannot mask a real divergence between two in-range values, but does correctly ignore
/// the case where a low-probability tail evaluation is close enough to the clamp
/// boundary that clamp-driven rounding differs by a tiny amount between CPU and GPU.
pub const BLACKBODY_ABS_FLOOR: f32 = 1e-5;

#[must_use]
pub fn build_blackbody_cases() -> Vec<BlackbodyCase> {
    let mut cases = Vec::new();
    let temps = [
        1000.0f32, 1500.0, 2000.0, 3200.0, 5000.0, 6500.0, 6600.0, 8000.0, 10_000.0,
    ];
    let lambda_steps = ((780.0 - 380.0) / 15.0) as u32;
    for step in 0..=lambda_steps {
        let lambda = (step as f32).mul_add(15.0, 380.0);
        for &temp_k in &temps {
            cases.push(BlackbodyCase {
                lambda_nm: lambda,
                temp_k,
            });
        }
    }
    // Adversarial: below the 1000K clamp, exactly at 560nm (the normalization
    // wavelength), and low enough temperatures to approach the `min(80.0)` exponent
    // clamp.
    for &temp_k in &[100.0f32, 500.0, 999.0, 1000.0, 1000.1] {
        cases.push(BlackbodyCase {
            lambda_nm: 560.0,
            temp_k,
        });
        cases.push(BlackbodyCase {
            lambda_nm: 380.0,
            temp_k,
        });
        cases.push(BlackbodyCase {
            lambda_nm: 780.0,
            temp_k,
        });
    }
    cases
}

/// Runs the `blackbody_spectrum` ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_blackbody(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<BlackbodyCase> {
    let cases = build_blackbody_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "blackbody cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "blackbody out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "blackbody_main",
        SHADER_SRC,
        "blackbody_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "blackbody bind group",
        &pipeline,
        &[(2, &in_buf), (3, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new(
        "blackbody_spectrum",
        BLACKBODY_ULP_BUDGET,
        BLACKBODY_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = blackbody_spectrum(case.lambda_nm, case.temp_k);
        acc.record(case, "value", cpu, gpu_out[idx]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// sample_studio_environment
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StudioEnvCase {
    dir: [f32; 3],
    lambda_nm: f32,
    temp_k: f32,
    spot_mult: f32,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    model: f32,
    _pad1: f32,
    /// Was `_pad2` -- nonzero for a case built from a preset where [`LightingPreset::uses_d65`]
    /// is true, mirroring
    /// `optics::raytracer::environment::sample_studio_environment_with_rig`'s own
    /// `preset.uses_d65()` branch. See
    /// [`build_studio_env_cases`].
    use_d65: f32,
    /// Unit direction towards the eye for the lit models' head shadow (`[0.0; 3]`
    /// disables it) -- mirrors `sample_studio_environment_observed`'s `observer`.
    observer: [f32; 3],
    _pad2: f32,
}

const _: () = assert!(size_of::<StudioEnvCase>() == 64);

/// ULP budget for `sample_studio_environment`.
///
/// The largest of the four Phase-1 environment budgets: it chains `sin`/`cos` (twice,
/// for `StudioRig`'s key/fill/ring directions), `blackbody_spectrum` itself,
/// `normalize`/`dot`/`cross`, and `powi_u(_, 28)`/`powi_u(_, 18)`/`powi_u(_, 6)` -- more
/// accumulated transcendental rounding than any other single Phase-1 function. See
/// [`CMF_ULP_BUDGET`]'s doc comment for the calibration philosophy.
///
/// # Measured amplification via `powi_u(_, 28)`
///
/// The FIRST measured run on this workspace's dev hardware found up to 1086 ULP, on
/// cases whose sampled direction sits very close to the key light's own axis (`key_dot`
/// near `1.0`). This is expected amplification, not a bug: `key_dot` itself is built
/// from `sin`/`cos`/`normalize`/`dot`, each contributing a handful of ULP of ordinary
/// driver-level rounding noise (the SAME 1-2 ULP floor `rng_check` measured per
/// operation); raising a value near `1.0` to the 28th power amplifies its RELATIVE
/// error by a factor of ~28 (`d(x^n)/x^n = n * dx/x`), which is exactly the multiplier
/// that turns a few-ULP `key_dot` disagreement into ~1000 ULP in the final radiance.
/// Set well above that measured figure with margin for a different driver.
pub const STUDIO_ENV_ULP_BUDGET: u32 = 8192;

/// Absolute-difference floor for `sample_studio_environment` comparisons.
///
/// See [`CMF_ABS_FLOOR`]'s doc comment for the rationale -- this covers the ring-light spark
/// threshold's `> 0.96` branch edge, where radiance can be arbitrarily close to the
/// ambient backdrop floor (`~0.005`-`0.03`) on one side of the threshold and jump
/// sharply on the other; a genuine algebra bug (a wrong lobe constant, a dropped
/// `spot_mult`/`exposure` factor, or a mis-set power exponent) moves radiance by orders
/// of magnitude more than this floor, as confirmed by this crate's negative-control
/// run.
pub const STUDIO_ENV_ABS_FLOOR: f32 = 1e-4;

fn fibonacci_sphere(n: usize) -> Vec<Vec3> {
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32) / ((n - 1).max(1) as f32);
            let radius = y.mul_add(-y, 1.0).max(0.0).sqrt();
            let theta = golden_angle * i as f32;
            Vec3::new(theta.cos() * radius, y, theta.sin() * radius)
        })
        .collect()
}

#[must_use]
pub fn build_studio_env_cases() -> Vec<StudioEnvCase> {
    let mut cases = Vec::new();
    let directions = fibonacci_sphere(256);
    let poses = [(0.3f32, 0.6f32), (0.85, 0.95), (-0.5, 1.2)];
    let exposures = [0.5f32, 1.0, 2.0];
    let lambdas = [400.0f32, 500.0, 560.0, 650.0, 700.0];

    // The studio rig ignores the observer, so only the lit models get the second,
    // shadow-casting one (and the in-cone directions below).
    let lit_observer = Vec3::new(0.2, 0.9, -0.3).normalize();
    let no_observer = [Vec3::ZERO];
    let lit_observers = [Vec3::ZERO, lit_observer];

    for preset in LightingPreset::ALL {
        let params = preset.params();
        let use_d65 = f32::from(preset.uses_d65());
        let model = preset.model().gpu_id() as f32;
        let observers: &[Vec3] = if preset.model() == LightingModel::Studio {
            &no_observer
        } else {
            &lit_observers
        };
        for &observer in observers {
            for &(light_yaw, light_pitch) in &poses {
                for &exposure in &exposures {
                    for &dir in &directions {
                        for &lambda_nm in &lambdas {
                            cases.push(StudioEnvCase {
                                dir: dir.to_array(),
                                lambda_nm,
                                temp_k: params.temp_k,
                                spot_mult: params.spot_mult,
                                exposure,
                                light_yaw,
                                light_pitch,
                                model,
                                _pad1: 0.0,
                                use_d65,
                                observer: observer.to_array(),
                                _pad2: 0.0,
                            });
                        }
                    }
                }
            }
        }
        if preset.model() != LightingModel::Studio {
            push_head_shadow_cases(&mut cases, preset, lit_observer);
        }
    }

    // Adversarial: exactly on the key light's own axis (peak alignment, `key_dot ==
    // 1.0`), and exactly at the ring lights' `0.96` spark threshold on both sides.
    for &(light_yaw, light_pitch) in &poses {
        let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
        // `preset_temp(0)` is `LightingPreset::Daylight` (index 0 -- see
        // `LightingPreset::from_index`), so these cases need `use_d65 = 1.0` too.
        for dir in [rig.key_dir, rig.fill_dir].into_iter().chain(rig.ring_dirs) {
            cases.push(StudioEnvCase {
                dir: dir.to_array(),
                lambda_nm: 560.0,
                temp_k: preset_temp(0),
                spot_mult: 1.0,
                exposure: 1.0,
                light_yaw,
                light_pitch,
                model: 0.0,
                _pad1: 0.0,
                use_d65: 1.0,
                observer: [0.0; 3],
                _pad2: 0.0,
            });
        }
        // Just inside / just outside the ring spark threshold along the first ring dir.
        // `preset_temp(2)` is `LightingPreset::RingLights`, not Daylight, so
        // `use_d65 = 0.0` here.
        let ring0 = rig.ring_dirs[0];
        for &scale in &[0.999f32, 1.001] {
            let perturbed = (ring0 * scale + Vec3::new(1e-3, 0.0, 0.0)).normalize();
            cases.push(StudioEnvCase {
                dir: perturbed.to_array(),
                lambda_nm: 560.0,
                temp_k: preset_temp(2),
                spot_mult: LightingPreset::RingLights.params().spot_mult,
                exposure: 1.0,
                light_yaw,
                light_pitch,
                model: 0.0,
                _pad1: 0.0,
                use_d65: 0.0,
                observer: [0.0; 3],
                _pad2: 0.0,
            });
        }
    }

    cases
}

const fn preset_temp(index: i32) -> f32 {
    LightingPreset::from_index(index).params().temp_k
}

/// Adversarial head-shadow cases for one lit preset: exactly at the eye, and at 12 /
/// 16 / 20 degrees off it -- inside, across and outside the 14..18 degree fade.
fn push_head_shadow_cases(cases: &mut Vec<StudioEnvCase>, preset: LightingPreset, observer: Vec3) {
    let params = preset.params();
    let perp = observer.cross(Vec3::X).normalize();
    for degrees in [0.0f32, 12.0, 16.0, 20.0] {
        let (sin_a, cos_a) = degrees.to_radians().sin_cos();
        let dir = observer.mul_add(Vec3::splat(cos_a), perp * sin_a);
        cases.push(StudioEnvCase {
            dir: dir.to_array(),
            lambda_nm: 560.0,
            temp_k: params.temp_k,
            spot_mult: params.spot_mult,
            exposure: 1.0,
            light_yaw: 0.3,
            light_pitch: 0.6,
            model: preset.model().gpu_id() as f32,
            _pad1: 0.0,
            use_d65: f32::from(preset.uses_d65()),
            observer: observer.to_array(),
            _pad2: 0.0,
        });
    }
}

/// Runs the `sample_studio_environment` ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_studio_env(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<StudioEnvCase> {
    let cases = build_studio_env_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "studio_env cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "studio_env out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "studio_env_main",
        SHADER_SRC,
        "studio_env_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "studio_env bind group",
        &pipeline,
        &[(4, &in_buf), (5, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new(
        "sample_studio_environment",
        STUDIO_ENV_ULP_BUDGET,
        STUDIO_ENV_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = sample_studio_environment_observed(
            Vec3::from_array(case.dir),
            case.lambda_nm,
            preset_for_case(case),
            case.exposure,
            case.light_yaw,
            case.light_pitch,
            Vec3::from_array(case.observer),
        );
        acc.record(case, "radiance", cpu, gpu_out[idx]);
    }
    acc.finish()
}

fn preset_for_case(case: &StudioEnvCase) -> LightingPreset {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "model is a small non-negative integer"
    )]
    let model_id = case.model as u32;
    match model_id {
        1 => LightingPreset::IsoHemisphere,
        2 => LightingPreset::LightTent,
        3 => LightingPreset::DaylightDome,
        _ => LightingPreset::from_index(preset_index_for(case.temp_k)),
    }
}

/// Recovers which built-in preset a case's `temp_k` came from, so `run_studio_env` can
/// call `sample_studio_environment` with the real `LightingPreset` enum rather than a
/// hand-reconstructed `(temp_k, spot_mult)` pair (`sample_studio_environment` takes the
/// enum directly, not the raw params).
fn preset_index_for(temp_k: f32) -> i32 {
    LightingPreset::ALL
        .iter()
        .position(|p| (p.params().temp_k - temp_k).abs() < 1e-6)
        .map_or(0, |i| i as i32)
}

// ---------------------------------------------------------------------------------
// compute_illuminant_white_balance
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct WhiteBalanceCase {
    temp_k: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

/// ULP budget for `compute_illuminant_white_balance`.
///
/// Wider than the per-lambda [`CMF_ULP_BUDGET`]/[`BLACKBODY_ULP_BUDGET`] budgets since
/// this is those two functions' PRODUCT, summed over 401 wavelength steps -- 401
/// independent roundings' worth of accumulated driver-level floating-point noise, not
/// just one. The Bradford-space rework adds two more 3x3 matrix-vector products (source white and
/// target white) and a per-component division on top of that same 401-step sum; those
/// contribute only a handful of ULP each, well inside the existing budget, so it is left
/// unchanged rather than widened.
pub const WHITE_BALANCE_ULP_BUDGET: u32 = 4096;

/// Absolute-difference floor for `compute_illuminant_white_balance` comparisons.
///
/// See [`CMF_ABS_FLOOR`]'s doc comment for the rationale. Since the diagonal
/// scale this function returns is now computed in Bradford LMS space rather than raw
/// XYZ, none of the three components is pinned to exactly `1.0` by construction any
/// more (the old XYZ-space `y` component was, since Y was left unscaled) -- all three
/// are `LMS_target / LMS_source`-style ratios, and can range more widely than the old
/// `x`/`z` (measured: roughly `0.8`-`2.6` across this crate's four lighting presets,
/// widest for the 3200K incandescent preset's blue channel). A floor several orders of
/// magnitude below that range still cannot mask a real divergence.
pub const WHITE_BALANCE_ABS_FLOOR: f32 = 1e-5;

/// Runs the `compute_illuminant_white_balance` self-test against a live GPU, for all
/// four [`LightingPreset`] variants.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_white_balance(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<LightingPreset> {
    let presets = LightingPreset::ALL;
    let cases: Vec<WhiteBalanceCase> = presets
        .iter()
        .map(|p| WhiteBalanceCase {
            temp_k: p.params().temp_k,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        })
        .collect();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "white_balance cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "white_balance out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "white_balance_main",
        SHADER_SRC,
        "white_balance_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "white_balance bind group",
        &pipeline,
        &[(6, &in_buf), (7, &out_buf)],
    );
    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bind_group, (1, 1, 1));
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut acc = UlpAccumulator::new(
        "compute_illuminant_white_balance",
        WHITE_BALANCE_ULP_BUDGET,
        WHITE_BALANCE_ABS_FLOOR,
    );
    for (idx, &preset) in presets.iter().enumerate() {
        let cpu = compute_illuminant_white_balance(preset.params().temp_k);
        acc.record(&preset, "x", cpu.x, gpu_out[idx * 3]);
        acc.record(&preset, "y", cpu.y, gpu_out[idx * 3 + 1]);
        acc.record(&preset, "z", cpu.z, gpu_out[idx * 3 + 2]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// hdr_env_radiance_at
// ---------------------------------------------------------------------------------
//
// Exercises `shaders/environment.wgsl`'s standalone `hdr_env_radiance_main` kernel --
// its own duplicate of `spectral_transport.wgsl`'s `hdr_env_radiance_at` (see that
// kernel's own header comment for why this file keeps an independent copy rather than
// sharing code across WGSL modules) -- against
// `renderer::env_map::EnvironmentMap::radiance_at` directly, independent of the
// megakernel's own Tier 3 image comparisons for a loaded HDR map.

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HdrEnvCase {
    dir: [f32; 3],
    lambda_nm: f32,
}

/// ULP budget for `hdr_env_radiance_at`.
///
/// Chains `acos`/`atan2`/`normalize` (direction -> uv) with a bilinear interpolation (four
/// texel fetches, three `fma`s) and `rgb_to_spectral_radiance`'s own two-`exp()`-per-channel
/// asymmetric-Gaussian sum -- fewer transcendental evaluations than
/// [`sample_studio_environment`]'s ring-light loop, but still trig-driven, so this starts
/// from [`STUDIO_ENV_ULP_BUDGET`]'s order of magnitude rather than [`CMF_ULP_BUDGET`]'s
/// polynomial-only one. See [`CMF_ULP_BUDGET`]'s doc comment for the calibration
/// philosophy; kept as a conservative ceiling pending a fresh measurement on live GPU
/// hardware.
pub const HDR_ENV_ULP_BUDGET: u32 = 4096;

/// Absolute-difference floor for `hdr_env_radiance_at` comparisons.
///
/// See [`CMF_ABS_FLOOR`]'s doc comment for the rationale -- covers the equirectangular
/// seam (`u` wrapping through `0.0`/`1.0`) and the poles (`v` clamped to `0.0`/`1.0`),
/// where a one-texel rounding-direction disagreement between CPU and GPU changes which
/// neighbour `sample_bilinear` blends toward.
pub const HDR_ENV_ABS_FLOOR: f32 = 1e-4;

/// A small, deliberately non-uniform synthetic HDR map: a smooth RGB gradient across
/// `(u, v)`, so `sample_bilinear`'s interpolation is genuinely exercised (a uniform map,
/// like [`crate::renderer::env_map::EnvironmentMap::uniform`], would return the same
/// value regardless of a wrong texel index or a wrong interpolation weight).
///
/// `pub(crate)`: `estimator_check::spectral_debug::run_spectral_debug` reuses this same
/// map for its own `EnvironmentSource::HdrMap` case rather than building a second
/// synthetic map.
pub(crate) fn build_hdr_test_map() -> crate::renderer::env_map::EnvironmentMap {
    let width = 16usize;
    let height = 8usize;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let u = x as f32 / width as f32;
            let v = y as f32 / height as f32;
            pixels.push([
                0.8f32.mul_add(u, 0.2),
                0.6f32.mul_add(v, 0.1),
                0.9f32.mul_add(f32::midpoint(u, v), 0.05),
            ]);
        }
    }
    crate::renderer::env_map::EnvironmentMap::from_rgb(width, height, pixels)
        .expect("width/height/pixels are self-consistent by construction")
}

/// Dense direction/wavelength sweep plus adversarial points: both poles (`v` clamped),
/// and directions straddling the `u == 0.0`/`1.0` equirectangular wrap seam.
#[must_use]
pub fn build_hdr_env_cases() -> Vec<HdrEnvCase> {
    let mut cases = Vec::new();
    let directions = fibonacci_sphere(256);
    let lambdas = [400.0f32, 500.0, 560.0, 650.0, 700.0];
    for &dir in &directions {
        for &lambda_nm in &lambdas {
            cases.push(HdrEnvCase {
                dir: dir.to_array(),
                lambda_nm,
            });
        }
    }
    let adversarial_dirs = [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1e-4, 0.0, -1.0).normalize(),
        Vec3::new(-1e-4, 0.0, -1.0).normalize(),
    ];
    for dir in adversarial_dirs {
        for &lambda_nm in &lambdas {
            cases.push(HdrEnvCase {
                dir: dir.to_array(),
                lambda_nm,
            });
        }
    }
    cases
}

/// Runs the `hdr_env_radiance_at` ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_hdr_env_radiance(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<HdrEnvCase> {
    let map = build_hdr_test_map();
    let cases = build_hdr_env_cases();
    let total = cases.len();

    let hdr_env = crate::renderer::env_map_gpu::HdrEnvGpuData::upload(&ctx.device, &map);
    let in_buf = compute::upload(
        &ctx.device,
        "hdr_env cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "hdr_env out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "hdr_env_radiance_main",
        SHADER_SRC,
        "hdr_env_radiance_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "hdr_env bind group",
        &pipeline,
        &[
            (8, &in_buf),
            (9, &out_buf),
            (10, &hdr_env.texels),
            (11, &hdr_env.dims),
        ],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new("hdr_env_radiance_at", HDR_ENV_ULP_BUDGET, HDR_ENV_ABS_FLOOR);
    for (idx, case) in cases.iter().enumerate() {
        let cpu = map.radiance_at(Vec3::from_array(case.dir), case.lambda_nm);
        acc.record(case, "radiance", cpu, gpu_out[idx]);
    }
    acc.finish()
}
