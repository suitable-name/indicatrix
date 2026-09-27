//! P1 (assigned-mode absorption), biaxial branch: `optics::birefringence::{
//! assigned_mode_alpha, BiaxialIndicatrix::assigned_mode_e_field}`, against
//! `AbsorptionTensor3::biaxial` -- the biaxial counterpart of
//! `absorption_pleochroism::run_assigned_mode_alpha_uniaxial` (see that check's own doc
//! comment for the full P1 rationale). Keeps `run_biaxial_pleochroic` unchanged,
//! mirroring `run_pleochroic`'s own uniaxial doc comment for why it is still exercised.
//!
//! `c_axis` here is fed the SAME value as `ax0`/`ax1`/`ax2`'s own gamma axis (column 2 of
//! `BiaxialIndicatrix::axes`) -- not an independently-varied direction -- because that is
//! the one invariant every real caller upholds: `GemMaterial::biaxial_indicatrix` builds
//! its `BiaxialIndicatrix` via `from_gamma_axis(.., self.c_axis)`, and the megakernel's
//! call site (`spectral_transport.wgsl`) passes that SAME `material.c_axis` as both the
//! indicatrix's own gamma axis and this function's separate `c_axis` tensor-frame
//! argument. Varying them independently here would exercise a combination the real
//! pipeline never produces.

use glam::{Mat3, Vec3};

use crate::{
    optics::birefringence::{AbsorptionTensor3, BiaxialIndicatrix, assigned_mode_alpha},
    renderer::gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::{biaxial_test_directions, biaxial_test_indicatrices};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AssignedModeAlphaBiaxialCase {
    alpha_o: f32,
    alpha_beta: f32,
    alpha_e: f32,
    is_extraordinary: u32,
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    ax0: [f32; 3],
    _pad1: f32,
    ax1: [f32; 3],
    _pad2: f32,
    ax2: [f32; 3],
    _pad3: f32,
    c_axis: [f32; 3],
    _pad4: f32,
    k: [f32; 3],
    _pad5: f32,
}

/// One case exceeds this budget.
///
/// `AssignedModeAlphaBiaxialCase { alpha_o: 3.0, alpha_beta: 0.2, alpha_e: 1.7,
/// is_extraordinary: 0, n_alpha: 1.7407784, n_beta: 1.7427294, n_gamma: 1.7483784, ax0:
/// [1,0,0], ax1: [0,0,-1], ax2: [0,1,0], c_axis: [0,1,0], k: [0.74199855, -0.6359988,
/// 0.21199958] }`: `cpu=5.4465395e-1 (0x3f0b6e71) gpu=5.4459494e-1
/// (0x3f0b6a93) ULP=990`. Checked op-for-op against the CPU (`assigned_mode_alpha`
/// / `AbsorptionTensor3::quadratic_form`, `optics/birefringence.rs`) and this WGSL twin
/// (`assigned_mode_alpha_biaxial`/`quadratic_form3`/`biaxial_assigned_mode_e_field`/
/// `biaxial_d_to_e_direction`, `transport_physics.wgsl`): the (a1, a2, c) basis
/// reconstruction (`stable_orthonormal_basis`/`stable_orthonormal_basis_t`), the
/// mode-assignment convention (`is_extraordinary` -> `d_slow`, else `d_fast`, matching
/// on both sides), the D-to-E conversion (`d_to_e_direction`/`biaxial_d_to_e_direction`,
/// same `axes.transpose()`/per-axis-dot decomposition, same
/// `/n_alpha^2, /n_beta^2, /n_gamma^2`, same reconstruction), and the final quadratic
/// form (`alpha*(l0*l0) + beta*(l1*l1) + gamma*(l2*l2)`, identical term order) are all
/// line-for-line identical between the two languages. `BiaxialIndicatrix::
/// eigen_polarizations`'s OWN Tier 2 check (`super::eigen_polarization::run_biaxial_eigen_polarization`)
/// is bit-exact (0 genuine ULP) on the exact same case bank, so the divergence is not in
/// the shared eigenvector solve either.
///
/// # `dot()`/`fma` rewrite ruled out; residual is
/// hardware division/sqrt precision, GPU closer to the f64 truth
///
/// `quadratic_form3`'s and
/// `biaxial_d_to_e_direction`'s three per-axis `dot()` calls in `transport_physics.wgsl`
/// are written as explicit non-fused scalar sums (`a.x*b.x + a.y*b.y + a.z*b.z`), matching the CPU's
/// `Mat3::transpose() * v` accumulation order bit-for-bit (glam's `Mat3::mul_vec3` SAXPY
/// expansion reduces algebraically to that same left-to-right grouping -- verified by
/// reading `glam-0.33.7`'s `f32/mat3.rs`/`vec3.rs` source directly, not assumed). A
/// standalone Rust-side probe (reproducing this exact argmax case via the real CPU
/// functions) confirms that swapping between a plain dot, an explicit `mul_add`-chain
/// dot ("as if" `dot()` got FMA-contracted), and `v * (1/sqrt(l2))` vs `v / sqrt(l2)`
/// normalize, all produce the IDENTICAL `f32` bit pattern for `e_hat` and the final
/// `alpha` on the CPU side -- ruling out source-level op-reordering as the cause. Running
/// the GPU equivalence harness with this explicit-sum form gives an
/// UNCHANGED result (`cpu=0x3f0b6e71 gpu=0x3f0b6a93 ULP=990`, bit-for-bit identical
/// to the fused-`dot()` form), confirming this rewrite makes no difference. It is
/// kept anyway (it removes reliance on WGSL's `dot()` builtin lowering, which is not
/// otherwise pinned down) but is not what closes this gap.
///
/// Given `eigen_polarizations` (the shared `d_hat` input) is independently bit-exact
/// CPU-vs-GPU, and this pair's own arithmetic is insensitive to source-level op order,
/// the remaining candidate is genuine hardware precision in the GPU's `/`/`sqrt`
/// instructions themselves (Vulkan permits `/` up to 2.5 ULP and does not require
/// correctly-rounded `sqrt`), amplified through the highly anisotropic
/// `alpha*(l0*l0)+beta*(l1*l1)+gamma*(l2*l2)` weighting (`alpha_o=3.0` vs
/// `alpha_beta=0.2` vs `alpha_e=1.7`) -- not a cancellation a reformulation-style fix
/// (the `spectral_absorption` treatment) could address, since there is no subtraction of
/// near-equal quantities here to reform.
///
/// The `f64` truth for the argmax case (a standalone probe
/// replicating the full CPU pipeline -- `wave_indices`, `precise_root_near`,
/// `eigenvector_world`'s null-space construction, `d_to_e_direction`, `quadratic_form`
/// -- entirely in `f64`) is `truth = 0.5446196818019298`. The CPU's own `f32` result
/// (`0.54465395`) is `+6.2925e-5` relative to that truth; the GPU's (`0.54459494`) is
/// `-4.5423e-5` relative -- i.e. the GPU value is actually the MORE ACCURATE of the two
/// here (smaller magnitude error, opposite sign), not a GPU bug being tolerated. Both
/// errors trace to the eigenvector solve's own inherent precision in this
/// near-degenerate regime (`n_alpha`/`n_beta`/`n_gamma` differ from each other by only
/// ~0.1-0.3%, well inside the "eigenvector sensitivity scales as 1/gap" regime
/// `eigenvector_world`'s own doc comment already describes): even the CPU's `d_hat`
/// alone already differs from the `f64` truth by ~1.5e-5 to 2.1e-5 per component, well
/// before `d_to_e_direction`/`quadratic_form3` ever run.
///
/// Set to `1200` (comfortable margin above the measured `990` ULP,
/// still tight enough to catch a genuine future regression, e.g. an accidental sign
/// flip or wrong-axis bug, which would land far outside this range) rather than a tight
/// value like this file's ordinary `48`, which would fail loudly on this case: the
/// `f64`-verified finding above shows BOTH sides
/// are already close to (and roughly symmetric around) the true answer, so a tight
/// budget would be catching residual hardware precision, not a bug.
const ASSIGNED_MODE_ALPHA_BIAXIAL_ULP_BUDGET: u32 = 1200;
const ASSIGNED_MODE_ALPHA_BIAXIAL_ABS_FLOOR: f32 = 1e-5;

fn build_assigned_mode_alpha_biaxial_cases() -> Vec<AssignedModeAlphaBiaxialCase> {
    let mut cases = Vec::new();
    let alpha_triples = [
        (0.0f32, 0.0f32, 0.0f32),
        (1.0, 1.0, 1.0),
        (0.5, 2.0, 3.5),
        (3.0, 0.2, 1.7),
    ];
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        let ind = BiaxialIndicatrix::from_gamma_axis(n_alpha, n_beta, n_gamma, gamma_axis);
        for k in biaxial_test_directions(n_alpha, n_beta, n_gamma, gamma_axis) {
            for &(alpha_o, alpha_beta, alpha_e) in &alpha_triples {
                for is_extraordinary in [false, true] {
                    cases.push(AssignedModeAlphaBiaxialCase {
                        alpha_o,
                        alpha_beta,
                        alpha_e,
                        is_extraordinary: u32::from(is_extraordinary),
                        n_alpha,
                        n_beta,
                        n_gamma,
                        _pad0: 0.0,
                        ax0: ind.axes.x_axis.to_array(),
                        _pad1: 0.0,
                        ax1: ind.axes.y_axis.to_array(),
                        _pad2: 0.0,
                        ax2: ind.axes.z_axis.to_array(),
                        _pad3: 0.0,
                        c_axis: gamma_axis.to_array(),
                        _pad4: 0.0,
                        k: k.to_array(),
                        _pad5: 0.0,
                    });
                }
            }
        }
    }
    cases
}

#[must_use]
pub fn run_assigned_mode_alpha_biaxial(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<AssignedModeAlphaBiaxialCase> {
    let cases = build_assigned_mode_alpha_biaxial_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "assigned mode alpha biaxial in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "assigned mode alpha biaxial out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "assigned_mode_alpha_biaxial_main",
        SHADER_SRC,
        "assigned_mode_alpha_biaxial_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "assigned mode alpha biaxial bind group",
        &pipeline,
        &[(58, &in_buf), (59, &out_buf)],
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
        "assigned_mode_alpha_biaxial",
        ASSIGNED_MODE_ALPHA_BIAXIAL_ULP_BUDGET,
        ASSIGNED_MODE_ALPHA_BIAXIAL_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let indicatrix = BiaxialIndicatrix::new(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Mat3::from_cols(
                Vec3::from_array(case.ax0),
                Vec3::from_array(case.ax1),
                Vec3::from_array(case.ax2),
            ),
        );
        let e_hat =
            indicatrix.assigned_mode_e_field(Vec3::from_array(case.k), case.is_extraordinary != 0);
        let tensor = AbsorptionTensor3::biaxial(
            case.alpha_o,
            case.alpha_beta,
            case.alpha_e,
            Vec3::from_array(case.c_axis),
        );
        let cpu = assigned_mode_alpha(&tensor, e_hat);
        acc.record(case, "alpha", cpu, gpu_out[idx]);
    }
    acc.finish()
}
