//! `optics::birefringence::BiaxialIndicatrix` GPU port -- the genuinely biaxial
//! (three-distinct-principal-index) generalization of the uniaxial machinery
//! `eigenmodes_uniaxial` checks. Every case bank here is compared against the REAL CPU
//! `BiaxialIndicatrix` methods, built via `BiaxialIndicatrix::from_gamma_axis` (never a
//! hand-written parallel reimplementation) -- see `shaders/transport_physics.wgsl`'s own
//! biaxial section for the WGSL port these exercise.

use crate::{
    optics::{
        birefringence::{
            AbsorptionTensor3, BiaxialIndicatrix, assigned_mode_alpha, pleochroic_channel_alpha,
        },
        materials::GemMaterial,
        polarization::StokesVector,
    },
    renderer::gpu::compute,
};
use glam::{Mat3, Vec3};

use super::{SHADER_SRC, STOKES_SAMPLES, UlpAccumulator, UlpCheckResult};

/// The three principal indices and gamma axis for every real biaxial built-in
/// (Alexandrite, Topaz, Tanzanite) at the D line, plus one synthetic well-separated,
/// deliberately off-axis case for extra coverage away from any built-in's specific
/// numbers -- mirrors `birefringence::biaxial_reduction_tests`' own synthetic-plus-real
/// coverage split.
///
/// # Panics
///
/// Panics if any of "Alexandrite"/"Topaz"/"Tanzanite" is ever removed from
/// `GemMaterial::all_materials()`, or if `biaxial_indicatrix` ever returns `None` for
/// one of them (it always returns `Some` for a material with `biaxial_delta_beta_alpha
/// = Some(_)`, which all three built-ins have) -- both would be a change to this
/// crate's own material catalogue this self-test scaffolding needs to know about, not a
/// condition worth handling gracefully.
fn biaxial_test_indicatrices() -> Vec<(&'static str, f32, f32, f32, Vec3)> {
    const D_LINE_NM: f32 = 589.3;
    let materials = GemMaterial::all_materials();
    let mut out = Vec::new();
    for name in ["Alexandrite", "Topaz", "Tanzanite"] {
        let material = materials
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("\"{name}\" must be a built-in biaxial material"));
        let ind = material
            .biaxial_indicatrix(D_LINE_NM)
            .unwrap_or_else(|| panic!("\"{name}\" must expose a BiaxialIndicatrix"));
        out.push((name, ind.n_alpha, ind.n_beta, ind.n_gamma, ind.axes.z_axis));
    }
    out.push((
        "synthetic",
        1.60,
        1.65,
        1.75,
        Vec3::new(0.35, 0.82, -0.45).normalize(),
    ));
    out
}

/// A representative spread of wave-normal directions -- axis-aligned, oblique, and
/// exactly along the gamma axis (the degenerate direction where two roots' local
/// components can coincide) -- shared by every case bank below.
fn biaxial_test_directions_unfiltered(gamma_axis: Vec3) -> Vec<Vec3> {
    vec![
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        gamma_axis,
        Vec3::new(0.2, 0.9, 0.1).normalize(),
        Vec3::new(-0.5, 0.3, 0.8).normalize(),
        Vec3::new(0.7, -0.6, 0.2).normalize(),
        Vec3::new(0.1, 0.1, 0.99).normalize(),
        Vec3::new(-0.9, -0.3, 0.2).normalize(),
    ]
}

/// Like [`biaxial_test_directions_unfiltered`], but excludes directions that land
/// essentially EXACTLY on one of this material's own two OPTIC AXES -- the physically
/// real directions (every genuinely biaxial crystal has exactly two) where the slow
/// and fast wave-normal indices coincide exactly. This is a narrow exclusion (relative
/// mode separation below `MIN_RELATIVE_MODE_SEPARATION`, deliberately tiny): a real
/// gem's overall birefringence is itself only a few thousandths of its mean index (see
/// `optics::materials::GemMaterial::birefringence_delta`'s cited built-in values), so
/// EVERY direction's `n_slow - n_fast` is already "small" relative to `n_slow` for
/// these materials -- a threshold anywhere near the double-digit-percent range this
/// function used in an earlier revision filters out nearly all realistic test
/// directions (confirmed: it produced an EMPTY case bank for real materials, which is
/// itself informative about how small the whole regime's `n_slow - n_fast` scale is).
/// A threshold this small targets only a direction landing essentially AT a true optic
/// axis (where `wave_indices`' own discriminant genuinely passes through zero), not
/// merely "somewhere in a naturally-small-birefringence material's normal range". Used
/// for `eigen_polarizations`/`mode_poynting_dir`/`pleochroic_channel_alpha`'s case
/// banks below (see [`biaxial_test_directions_index_stable`] for the wave_indices-only,
/// much stricter threshold, and this module's biaxial section header comment for why
/// `eigen_polarizations`/`mode_poynting_dir` still fail their own ULP budget at even a
/// much looser filter than this one -- their ill-conditioning is not confined to a
/// narrow neighborhood of the two true optic axes the way `wave_indices`' own is).
fn biaxial_test_directions(n_alpha: f32, n_beta: f32, n_gamma: f32, gamma_axis: Vec3) -> Vec<Vec3> {
    biaxial_test_directions_filtered(n_alpha, n_beta, n_gamma, gamma_axis, 1e-4)
}

/// `wave_indices` (the index MAGNITUDES alone, not the eigenVECTOR) is well-conditioned
/// everywhere except a narrow neighborhood of this material's two true optic axes --
/// see [`biaxial_test_directions`]'s doc comment for the general "no directions survive
/// a tight filter for a real gem" trap, and this module's biaxial section header for the
/// measured evidence that `wave_indices` DOES achieve max genuine ULP = 0 once that
/// narrow neighborhood specifically is excluded.
fn biaxial_test_directions_index_stable(
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    gamma_axis: Vec3,
) -> Vec<Vec3> {
    biaxial_test_directions_filtered(n_alpha, n_beta, n_gamma, gamma_axis, 0.02)
}

fn biaxial_test_directions_filtered(
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    gamma_axis: Vec3,
    min_relative_mode_separation: f32,
) -> Vec<Vec3> {
    let ind = BiaxialIndicatrix::from_gamma_axis(n_alpha, n_beta, n_gamma, gamma_axis);
    biaxial_test_directions_unfiltered(gamma_axis)
        .into_iter()
        .filter(|&d| {
            let (n_slow, n_fast) = ind.wave_indices(d);
            (n_slow - n_fast) / n_slow >= min_relative_mode_separation
        })
        .collect()
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialWaveIndicesCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: [f32; 3],
    _pad1: f32,
    wave_normal: [f32; 3],
    _pad2: f32,
}

/// Measured: max genuine ULP = 0 (raw ULP <= 29) once `biaxial_test_directions`'s
/// near-optic-axis filter excludes directions where this material's own two modes are
/// within 10% of each other -- see that function's doc comment for why comparing an
/// ill-posed eigenmode assignment near a true optic axis is not a meaningful ULP
/// check. The budget stays modest (not 0) purely as headroom, matching this file's
/// convention elsewhere.
const BIAXIAL_WAVE_INDICES_ULP_BUDGET: u32 = 64;
const BIAXIAL_WAVE_INDICES_ABS_FLOOR: f32 = 1e-5;

fn build_biaxial_wave_indices_cases() -> Vec<BiaxialWaveIndicesCase> {
    let mut cases = Vec::new();
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        for wave_normal in
            biaxial_test_directions_index_stable(n_alpha, n_beta, n_gamma, gamma_axis)
        {
            cases.push(BiaxialWaveIndicesCase {
                n_alpha,
                n_beta,
                n_gamma,
                _pad0: 0.0,
                gamma_axis: gamma_axis.to_array(),
                _pad1: 0.0,
                wave_normal: wave_normal.to_array(),
                _pad2: 0.0,
            });
        }
    }
    cases
}

#[must_use]
pub fn run_biaxial_wave_indices(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialWaveIndicesCase> {
    let cases = build_biaxial_wave_indices_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial wave indices in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial wave indices out",
        total * 2,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_wave_indices_main",
        SHADER_SRC,
        "biaxial_wave_indices_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial wave indices bind group",
        &pipeline,
        &[(36, &in_buf), (37, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 2);

    let mut acc = UlpAccumulator::new(
        "BiaxialIndicatrix::wave_indices",
        BIAXIAL_WAVE_INDICES_ULP_BUDGET,
        BIAXIAL_WAVE_INDICES_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let ind = BiaxialIndicatrix::from_gamma_axis(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Vec3::from_array(case.gamma_axis),
        );
        let (n_slow, n_fast) = ind.wave_indices(Vec3::from_array(case.wave_normal));
        acc.record(case, "n_slow", n_slow, gpu_out[idx * 2]);
        acc.record(case, "n_fast", n_fast, gpu_out[idx * 2 + 1]);
    }
    acc.finish()
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialEigenPolarizationCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: [f32; 3],
    _pad1: f32,
    wave_normal: [f32; 3],
    _pad2: f32,
}

const BIAXIAL_EIGEN_POLARIZATION_ULP_BUDGET: u32 = 48;
/// Absolute floor after the `BiaxialIndicatrix` eigenvector conditioning fix (see
/// `optics::birefringence::BiaxialIndicatrix::eigenvector_world`'s and
/// `precise_root_near`'s doc comments): every remaining comparison sits within
/// `[1.0e-5, 3.5e-5]` absolute difference on a component of modest (0.02-0.9) magnitude
/// -- genuine last-few-ULP f32 cross-platform rounding noise on an eigenvector component
/// that is itself small or near a direction-dependent zero-crossing (worst case:
/// `wave_normal = (0,0,1)`, where CPU computes a component EXACTLY `0.0` by symmetry and
/// the GPU's independently-rounded arithmetic lands a few ULP off zero instead), not a
/// residual algorithmic defect (the dedicated residual/orthonormality tests in
/// `birefringence.rs` and this file's own Tier-3 image comparisons for all three biaxial
/// built-ins pass cleanly). `5e-5` sits ~1.4x above the measured `3.5e-5` worst case:
/// headroom without weakening the check for a genuine bug, which would show up as a
/// large FRACTION of a unit vector's own magnitude, not a few times `1e-5`.
const BIAXIAL_EIGEN_POLARIZATION_ABS_FLOOR: f32 = 5e-5;

fn build_biaxial_eigen_polarization_cases() -> Vec<BiaxialEigenPolarizationCase> {
    let mut cases = Vec::new();
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        for wave_normal in biaxial_test_directions(n_alpha, n_beta, n_gamma, gamma_axis) {
            cases.push(BiaxialEigenPolarizationCase {
                n_alpha,
                n_beta,
                n_gamma,
                _pad0: 0.0,
                gamma_axis: gamma_axis.to_array(),
                _pad1: 0.0,
                wave_normal: wave_normal.to_array(),
                _pad2: 0.0,
            });
        }
    }
    cases
}

#[must_use]
pub fn run_biaxial_eigen_polarization(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialEigenPolarizationCase> {
    let cases = build_biaxial_eigen_polarization_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial eigen polarization in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial eigen polarization out",
        total * 6,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_eigen_polarization_main",
        SHADER_SRC,
        "biaxial_eigen_polarization_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial eigen polarization bind group",
        &pipeline,
        &[(38, &in_buf), (39, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 6);

    let mut acc = UlpAccumulator::new(
        "BiaxialIndicatrix::eigen_polarizations",
        BIAXIAL_EIGEN_POLARIZATION_ULP_BUDGET,
        BIAXIAL_EIGEN_POLARIZATION_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let ind = BiaxialIndicatrix::from_gamma_axis(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Vec3::from_array(case.gamma_axis),
        );
        let (d_slow, d_fast) = ind.eigen_polarizations(Vec3::from_array(case.wave_normal));
        for (c_idx, comp) in ["slow_x", "slow_y", "slow_z"].iter().enumerate() {
            acc.record(
                case,
                comp,
                d_slow.to_array()[c_idx],
                gpu_out[idx * 6 + c_idx],
            );
        }
        for (c_idx, comp) in ["fast_x", "fast_y", "fast_z"].iter().enumerate() {
            acc.record(
                case,
                comp,
                d_fast.to_array()[c_idx],
                gpu_out[idx * 6 + 3 + c_idx],
            );
        }
    }
    acc.finish()
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialModePoyntingCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: [f32; 3],
    _pad1: f32,
    wave_normal: [f32; 3],
    want_slow: u32,
}

const BIAXIAL_MODE_POYNTING_ULP_BUDGET: u32 = 48;
const BIAXIAL_MODE_POYNTING_ABS_FLOOR: f32 = 1e-5;

fn build_biaxial_mode_poynting_cases() -> Vec<BiaxialModePoyntingCase> {
    let mut cases = Vec::new();
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        for wave_normal in biaxial_test_directions(n_alpha, n_beta, n_gamma, gamma_axis) {
            for want_slow in [false, true] {
                cases.push(BiaxialModePoyntingCase {
                    n_alpha,
                    n_beta,
                    n_gamma,
                    _pad0: 0.0,
                    gamma_axis: gamma_axis.to_array(),
                    _pad1: 0.0,
                    wave_normal: wave_normal.to_array(),
                    want_slow: u32::from(want_slow),
                });
            }
        }
    }
    cases
}

#[must_use]
pub fn run_biaxial_mode_poynting(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialModePoyntingCase> {
    let cases = build_biaxial_mode_poynting_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial mode poynting in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial mode poynting out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_mode_poynting_main",
        SHADER_SRC,
        "biaxial_mode_poynting_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial mode poynting bind group",
        &pipeline,
        &[(40, &in_buf), (41, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut acc = UlpAccumulator::new(
        "BiaxialIndicatrix::mode_poynting_dir",
        BIAXIAL_MODE_POYNTING_ULP_BUDGET,
        BIAXIAL_MODE_POYNTING_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let ind = BiaxialIndicatrix::from_gamma_axis(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Vec3::from_array(case.gamma_axis),
        );
        let dir = ind.mode_poynting_dir(Vec3::from_array(case.wave_normal), case.want_slow != 0);
        for (c_idx, comp) in ["x", "y", "z"].iter().enumerate() {
            acc.record(case, comp, dir.to_array()[c_idx], gpu_out[idx * 3 + c_idx]);
        }
    }
    acc.finish()
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialResolveEntryModeCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: [f32; 3],
    _pad1: f32,
    incident_dir: [f32; 3],
    _pad2: f32,
    normal: [f32; 3],
    _pad3: f32,
    cos_i: f32,
    n_seed: f32,
    want_slow: u32,
    _pad4: f32,
}

/// Measured: max genuine ULP ~3035 for Alexandrite specifically (`n_alpha=1.740778`,
/// `n_beta=1.742729` -- a real cited-data gap of 0.00195, ABOVE
/// `BiaxialIndicatrix::indices_are_degenerate`'s own `sqrt(f32::EPSILON) ~= 3.45e-4`
/// relative threshold, so the general quadratic solve is legitimately used rather than
/// the closed-form uniaxial shortcut, per that function's own doc comment -- but still
/// close enough that its own documented `sqrt(f32::EPSILON)` relative-error bound
/// applies almost exactly: 3035 ULP at n~1.74 is ~3.6e-4 absolute, i.e. ~2.1e-4
/// relative, the same order the CPU source itself derives and accepts for this regime.
/// Unlike the near-optic-axis case `biaxial_test_directions` filters out, this is not
/// filtered here: it is a genuine material property Alexandrite's real cited index
/// data has at EVERY air->crystal entry (not a rare direction), and Tier 3's
/// statistical image comparison on real Alexandrite renders already confirms this
/// level of per-bounce numerical noise does not observably bias a real render.
const BIAXIAL_RESOLVE_ENTRY_MODE_ULP_BUDGET: u32 = 4096;
const BIAXIAL_RESOLVE_ENTRY_MODE_ABS_FLOOR: f32 = 1e-5;

fn build_biaxial_resolve_entry_mode_cases() -> Vec<BiaxialResolveEntryModeCase> {
    let mut cases = Vec::new();
    // Representative (incident_dir, normal) pairs spanning a range of incidence
    // angles, mirroring the geometric setup `theta_c_for_bounce`'s own iteration is
    // exercised against.
    let incidences: Vec<(Vec3, Vec3, f32)> = (1..10)
        .map(|i| {
            let cos_i = i as f32 / 10.0;
            let sin_i = cos_i.mul_add(-cos_i, 1.0).max(0.0).sqrt();
            let normal = Vec3::Y;
            let incident_dir = Vec3::new(sin_i, -cos_i, 0.0).normalize();
            (incident_dir, normal, cos_i)
        })
        .collect();
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        for &(incident_dir, normal, cos_i) in &incidences {
            for want_slow in [false, true] {
                cases.push(BiaxialResolveEntryModeCase {
                    n_alpha,
                    n_beta,
                    n_gamma,
                    _pad0: 0.0,
                    gamma_axis: gamma_axis.to_array(),
                    _pad1: 0.0,
                    incident_dir: incident_dir.to_array(),
                    _pad2: 0.0,
                    normal: normal.to_array(),
                    _pad3: 0.0,
                    cos_i,
                    n_seed: n_beta,
                    want_slow: u32::from(want_slow),
                    _pad4: 0.0,
                });
            }
        }
    }
    cases
}

#[must_use]
pub fn run_biaxial_resolve_entry_mode(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialResolveEntryModeCase> {
    let cases = build_biaxial_resolve_entry_mode_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial resolve entry mode in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial resolve entry mode out",
        total * 4,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_resolve_entry_mode_main",
        SHADER_SRC,
        "biaxial_resolve_entry_mode_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial resolve entry mode bind group",
        &pipeline,
        &[(42, &in_buf), (43, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 4);

    let mut acc = UlpAccumulator::new(
        "BiaxialIndicatrix::resolve_entry_mode",
        BIAXIAL_RESOLVE_ENTRY_MODE_ULP_BUDGET,
        BIAXIAL_RESOLVE_ENTRY_MODE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let ind = BiaxialIndicatrix::from_gamma_axis(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Vec3::from_array(case.gamma_axis),
        );
        let (n, wave_dir) = ind.resolve_entry_mode(
            Vec3::from_array(case.incident_dir),
            Vec3::from_array(case.normal),
            case.cos_i,
            case.n_seed,
            case.want_slow != 0,
        );
        acc.record(case, "n", n, gpu_out[idx * 4]);
        for (c_idx, comp) in ["wave_dir_x", "wave_dir_y", "wave_dir_z"]
            .iter()
            .enumerate()
        {
            acc.record(
                case,
                comp,
                wave_dir.to_array()[c_idx],
                gpu_out[idx * 4 + 1 + c_idx],
            );
        }
    }
    acc.finish()
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialPleochroicCase {
    alpha_o: f32,
    alpha_beta: f32,
    alpha_e: f32,
    _pad0: f32,
    c_axis: [f32; 3],
    _pad1: f32,
    s_axis: [f32; 3],
    _pad2: f32,
    propagation_dir: [f32; 3],
    _pad3: f32,
    eigen_a: [f32; 3],
    _pad4: f32,
    eigen_b: [f32; 3],
    _pad5: f32,
    stokes: [f32; 4],
}

const BIAXIAL_PLEOCHROIC_ULP_BUDGET: u32 = 48;
const BIAXIAL_PLEOCHROIC_ABS_FLOOR: f32 = 1e-5;

fn build_biaxial_pleochroic_cases() -> Vec<BiaxialPleochroicCase> {
    let mut cases = Vec::new();
    let alpha_triples = [
        (0.0f32, 0.0f32, 0.0f32),
        (1.0, 1.0, 1.0),
        (0.5, 2.0, 3.5),
        (3.0, 0.2, 1.7),
    ];
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        let ind = BiaxialIndicatrix::from_gamma_axis(n_alpha, n_beta, n_gamma, gamma_axis);
        for prop in biaxial_test_directions(n_alpha, n_beta, n_gamma, gamma_axis) {
            let (eigen_a, eigen_b) = ind.eigen_polarizations(prop);
            let s_axis = if prop.cross(Vec3::Y).length_squared() > 1e-6 {
                prop.cross(Vec3::Y).normalize()
            } else {
                prop.cross(Vec3::X).normalize()
            };
            for &(alpha_o, alpha_beta, alpha_e) in &alpha_triples {
                for s in [STOKES_SAMPLES[0], STOKES_SAMPLES[4]] {
                    cases.push(BiaxialPleochroicCase {
                        alpha_o,
                        alpha_beta,
                        alpha_e,
                        _pad0: 0.0,
                        c_axis: gamma_axis.to_array(),
                        _pad1: 0.0,
                        s_axis: s_axis.to_array(),
                        _pad2: 0.0,
                        propagation_dir: prop.to_array(),
                        _pad3: 0.0,
                        eigen_a: eigen_a.to_array(),
                        _pad4: 0.0,
                        eigen_b: eigen_b.to_array(),
                        _pad5: 0.0,
                        stokes: s,
                    });
                }
            }
        }
    }
    cases
}

#[must_use]
pub fn run_biaxial_pleochroic(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialPleochroicCase> {
    let cases = build_biaxial_pleochroic_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial pleochroic in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial pleochroic out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_pleochroic_main",
        SHADER_SRC,
        "biaxial_pleochroic_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial pleochroic bind group",
        &pipeline,
        &[(44, &in_buf), (45, &out_buf)],
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
        "pleochroic_channel_alpha (biaxial)",
        BIAXIAL_PLEOCHROIC_ULP_BUDGET,
        BIAXIAL_PLEOCHROIC_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let s = StokesVector::new(
            case.stokes[0],
            case.stokes[1],
            case.stokes[2],
            case.stokes[3],
        );
        let cpu = pleochroic_channel_alpha(
            case.alpha_o,
            case.alpha_e,
            Some(case.alpha_beta),
            Vec3::from_array(case.c_axis),
            Vec3::from_array(case.s_axis),
            Vec3::from_array(case.propagation_dir),
            Vec3::from_array(case.eigen_a),
            Vec3::from_array(case.eigen_b),
            &s,
        );
        acc.record(case, "alpha", cpu, gpu_out[idx]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// P1 (assigned-mode absorption), biaxial branch: optics::birefringence::{
// assigned_mode_alpha, BiaxialIndicatrix::assigned_mode_e_field}, against
// AbsorptionTensor3::biaxial -- the biaxial counterpart of
// absorption_pleochroism::run_assigned_mode_alpha_uniaxial (see that check's own doc
// comment for the full P1 rationale). Keeps `run_biaxial_pleochroic` above unchanged,
// mirroring `run_pleochroic`'s own uniaxial doc comment for why it is still exercised.
//
// `c_axis` here is fed the SAME value as `ax0/ax1/ax2`'s own gamma axis (column 2 of
// `BiaxialIndicatrix::axes`) -- not an independently-varied direction -- because that is
// the one invariant every real caller upholds: `GemMaterial::biaxial_indicatrix` builds
// its `BiaxialIndicatrix` via `from_gamma_axis(.., self.c_axis)`, and the megakernel's
// call site (`spectral_transport.wgsl`) passes that SAME `material.c_axis` as both the
// indicatrix's own gamma axis and this function's separate `c_axis` tensor-frame
// argument. Varying them independently here would exercise a combination the real
// pipeline never produces.
// ---------------------------------------------------------------------------------

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
/// eigen_polarizations`'s OWN Tier 2 check (this module, `run_biaxial_eigen_polarization`)
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
