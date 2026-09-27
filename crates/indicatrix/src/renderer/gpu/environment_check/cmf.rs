//! `cie_1931_cmf` ULP-budget self-test: [`build_cmf_lambdas`]'s dense wavelength sweep
//! plus adversarial points, dispatched by [`run_cmf`].

use crate::{color::cie1931::cie_1931_cmf, renderer::gpu::compute};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

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
