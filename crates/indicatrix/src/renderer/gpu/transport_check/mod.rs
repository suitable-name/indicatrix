//! Per-function ULP checks for the small pieces `shaders/spectral_transport.wgsl`'s
//! megakernel inlines -- driven by `shaders/transport_functions.wgsl`.
//!
//! Every case bank is compared against the REAL CPU function it was translated from
//! (never a hand-written parallel reimplementation): the four
//! `optics::polarization::MuellerMatrix` constructors plus `StokesVector::apply_matrix`,
//! `optics::raytracer::signed_frame_rotation_psi`, `optics::raytracer::tir_phase_delta`,
//! `optics::dispersion::DispersionModel::evaluate`, `optics::raytracer::spectral_absorption`,
//! `optics::birefringence::{BirefringenceParams::ordinary_eigen_polarization,
//! BirefringenceParams::extraordinary_eigen_polarization, pleochroic_channel_alpha}`.
//!
//! Deliberately NOT covered here: a standalone `r_s`/`r_p`/`t_s`/`t_p`-FROM-PHYSICS
//! check. Those scalar Fresnel-amplitude formulas are inlined at four separate call
//! sites in `optics::raytracer`, never factored into one shared function, so a
//! standalone check would have to re-derive the textbook formula independently rather
//! than call the real CPU code. They ARE exercised, via the real CPU function, by
//! `estimator_check`'s furnace anchor and Tier 3 image comparison -- a coarser
//! diagnostic, but not a gap in what gets verified, only in how precisely a failure
//! localizes.
//!
//! Split into one file per case-bank family; see each submodule's own doc comment for
//! what it owns. This file keeps only what every submodule shares: the generated shader
//! source, the `UlpCheckResult`/`UlpArgmax`/`UlpAccumulator` harness types, the
//! `STOKES_SAMPLES` fixture, and the `run_stokes_case_bank` dispatcher shared by the
//! four Mueller-matrix-and-`StokesVector::apply_matrix` case banks.

use crate::renderer::gpu::{
    compute,
    ulp::{ulp_distance, within_tolerance},
};

mod absorption_pleochroism;
mod bounce;
mod dispersion;
mod eigenmodes_biaxial;
mod eigenmodes_uniaxial;
mod frame_and_tir;
mod fresnel;
mod nee;
mod p2_uniaxial_fresnel;
mod p6_exit_splitting;
mod scattering;

pub use nee::{
    BalanceHeuristicCase, Dist1dFindBucketCase, Dist2dPdfCase, Dist2dSampleCase,
    NeeFrostedExteriorCase, NeeHgScatterCase, run_balance_heuristic, run_dist1d_find_bucket,
    run_dist2d_pdf, run_dist2d_sample, run_nee_frosted_exterior, run_nee_hg_scatter,
};

pub use absorption_pleochroism::{
    AbsorptionBandGpu, AbsorptionCase, AssignedModeAlphaUniaxialCase, PleochroicCase,
    run_absorption, run_assigned_mode_alpha_uniaxial, run_pleochroic,
};
pub use bounce::{
    CosineHemisphereCase, FrostedBounceCase, run_cosine_hemisphere, run_frosted_bounce,
};
pub use dispersion::{DispersionCase, run_dispersion};
pub use eigenmodes_biaxial::{
    AssignedModeAlphaBiaxialCase, BiaxialEigenPolarizationCase, BiaxialModePoyntingCase,
    BiaxialPleochroicCase, BiaxialResolveEntryModeCase, BiaxialWaveIndicesCase,
    run_assigned_mode_alpha_biaxial, run_biaxial_eigen_polarization, run_biaxial_mode_poynting,
    run_biaxial_pleochroic, run_biaxial_resolve_entry_mode, run_biaxial_wave_indices,
};
pub use eigenmodes_uniaxial::{
    EigenPolarizationCase, PerModeIndexCase, ThetaCCase, WalkOffCase, run_eigen_polarization,
    run_per_mode_index, run_theta_c, run_walk_off,
};
pub use frame_and_tir::{
    FrameRotationCase, SignedPsiCase, TirPhaseDeltaCase, TirRetardationCase, run_frame_rotation,
    run_signed_psi, run_tir_phase_delta, run_tir_retardation,
};
pub use fresnel::{
    FresnelReflectionCase, FresnelTransmissionCase, run_fresnel_reflection,
    run_fresnel_transmission,
};
pub use p2_uniaxial_fresnel::{
    EntrySolvePairCase, InternalSolveCase, run_entry_solve_pair, run_internal_solve,
};
pub use p6_exit_splitting::{
    ChannelTransmissionCase, NarrowCompatCase, NarrowCompatResult, UniaxialExitTransmissionCase,
    run_channel_transmission, run_narrow_compat, run_uniaxial_exit_transmission,
};
pub use scattering::{
    HgPhaseCase, HgSampleCase, ScatterOrExtinguishCase, run_hg_phase, run_hg_sample,
    run_scatter_or_extinguish,
};

// `transport_functions.wgsl` alone is not valid WGSL any more -- it assumes
// `shaders/transport_physics.wgsl`'s functions are already in scope. `build.rs`
// concatenates the two into `$OUT_DIR/transport_functions.generated.wgsl`, which is
// what gets compiled here; see `transport_physics.wgsl`'s header comment for why (and
// this module's own header comment for the fault-injection experiment this exists to
// pass).
const SHADER_SRC: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/transport_functions.generated.wgsl"
));

// ---------------------------------------------------------------------------------
// Shared accumulator -- a deliberate duplicate of `environment_check`'s private
// `UlpAccumulator`, not a refactored-out shared version: see `renderer::gpu::ulp`'s doc
// comment for why this crate prefers duplicating a small self-test helper over touching
// an already-shipped phase's file.
// ---------------------------------------------------------------------------------

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

/// A handful of representative Stokes states every Mueller-matrix case bank applies its
/// matrix to: fully unpolarized, fully +Q polarized, fully +U (45 degree) polarized,
/// fully +V (right circular) polarized, and one generic partially-polarized state with
/// all four components nonzero -- so a bug that only shows up when a specific component
/// is nonzero (e.g. a swapped row/column) cannot hide behind an all-unpolarized sweep.
const STOKES_SAMPLES: [[f32; 4]; 5] = [
    [1.0, 0.0, 0.0, 0.0],
    [1.0, 1.0, 0.0, 0.0],
    [1.0, 0.0, 1.0, 0.0],
    [1.0, 0.0, 0.0, 1.0],
    [1.0, 0.3, -0.4, 0.2],
];

/// Bundles a Mueller-matrix case bank's fixed dispatch/tolerance configuration, purely
/// to keep [`run_stokes_case_bank`]'s argument count within clippy's
/// `too_many_arguments` limit -- every field is a compile-time-known constant at each
/// of the four call sites, never data.
struct StokesCaseBankConfig {
    entry_point: &'static str,
    in_binding: u32,
    out_binding: u32,
    budget: u32,
    abs_floor: f32,
}

/// Shared dispatch for the four Mueller-matrix case banks above: upload `cases` to
/// `config.in_binding`, dispatch `config.entry_point`, read back 4 floats/case from
/// `config.out_binding`, and compare against `cpu_fn` under a hybrid ULP/abs-floor
/// accumulator.
fn run_stokes_case_bank<C: bytemuck::Pod + Clone + std::fmt::Debug>(
    ctx: &crate::renderer::gpu::GpuContext,
    config: &StokesCaseBankConfig,
    cases: &[C],
    cpu_fn: impl Fn(&C) -> [f32; 4],
) -> UlpCheckResult<C> {
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "transport case in",
        cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "transport case out",
        total * 4,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        config.entry_point,
        SHADER_SRC,
        config.entry_point,
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "transport case bind group",
        &pipeline,
        &[(config.in_binding, &in_buf), (config.out_binding, &out_buf)],
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
        entry_point_label(config.entry_point),
        config.budget,
        config.abs_floor,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_fn(case);
        for (c_idx, comp) in ["i", "q", "u", "v"].iter().enumerate() {
            acc.record(case, comp, cpu[c_idx], gpu_out[idx * 4 + c_idx]);
        }
    }
    acc.finish()
}

const fn entry_point_label(entry_point: &str) -> &'static str {
    match entry_point.as_bytes() {
        b"frame_rotation_main" => "frame_rotation",
        b"fresnel_reflection_main" => "fresnel_reflection",
        b"fresnel_transmission_main" => "fresnel_transmission",
        b"tir_retardation_main" => "tir_retardation",
        _ => "unknown",
    }
}
