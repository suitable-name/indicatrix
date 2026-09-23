//! Spectral-space debug comparison: "before XYZ integration... so a CMF bug cannot
//! masquerade as a transport bug". See [`run_spectral_debug`]'s own doc comment for the
//! honest scope limit on what this can and cannot verify given the CPU
//! visibility-only constraint.

use crate::{
    optics::raytracer::{
        LightingPreset, apply_von_kries_white_balance, compute_illuminant_white_balance,
        illuminant_temperature_k, integrate_channels_to_xyz_families,
    },
    renderer::{
        buffers::{GpuGemMaterial, GpuTransportParams, transport_env_mode},
        gpu::{environment_check::build_hdr_test_map, frame},
    },
};
use glam::Vec3;

use super::{
    MaterialForDispatch, SceneBuffersForDispatch, TransportDispatch, all_polished_finishes,
    camera_params_for, dispatch_transport, dispatch_transport_for_class, round_brilliant_planes,
    test_camera, tier3_material,
};

#[derive(Debug, Clone)]
pub struct SpectralDebugResult {
    pub total_cases: usize,
    /// Max ULP distance between the GPU's own `out_xyz` and re-integrating the GPU's
    /// own per-channel `(radiance, lambdas, path_pdf)` debug output through the REAL
    /// CPU `optics::raytracer::integrate_channels_to_xyz` -- see this function's own
    /// doc comment for exactly what this does and does not prove.
    pub max_self_consistency_ulp: u32,
    pub over_budget_count: usize,
}

const SPECTRAL_DEBUG_ULP_BUDGET: u32 = 64;
const SPECTRAL_DEBUG_ABS_FLOOR: f32 = 1e-5;

/// Isolates the CMF-fit-and-MIS-weight integration step from everything upstream, using
/// the GPU's own per-channel debug output (no reimplementation of the bounce loop).
///
/// # What this checks, and the honest gap
///
/// `trace_spectral_ray` has no CPU debug hook for its internal per-channel
/// `radiance`/`lambdas`/`path_pdf` -- only the final `Vec3` -- and adding one would mean
/// duplicating the bounce loop, so there is no independent CPU per-channel reference to
/// compare the GPU's per-channel output against.
///
/// What this DOES check: the GPU already writes its pre-integration
/// `(radiance, lambdas, path_pdf)` to debug buffers. Feeding those into the real CPU
/// `integrate_channels_to_xyz` and comparing against the GPU's own `out_xyz` isolates
/// the integration step: agreement means any CPU/GPU divergence found elsewhere lives
/// in the transport physics upstream, not the integration; disagreement means the
/// integration step itself has a porting bug. It does NOT independently verify the
/// per-channel radiance values against a CPU transport reference -- that remains
/// covered only indirectly, via the furnace anchor and Tier 3's final-XYZ comparison.
///
/// Runs this isolation twice: once against `EnvironmentSource::Studio` (white balance
/// applied, `env_mode == STUDIO_RIG`), once against `EnvironmentSource::HdrMap` (finding
/// 70: white balance skipped on both sides, `env_mode == HDR_MAP`) -- reusing
/// `environment_check::build_hdr_test_map`'s synthetic map rather than a second one, and
/// `dispatch_transport_for_class` (not the `Studio`-only [`dispatch_transport`] wrapper)
/// to pass its uploaded [`crate::renderer::env_map_gpu::HdrEnvGpuData`] through. Results
/// from both are pooled into one [`SpectralDebugResult`]: this function isolates the
/// SAME integration step either way, so a porting bug in either environment mode shows
/// up as the same kind of disagreement.
#[must_use]
pub fn run_spectral_debug(ctx: &crate::renderer::gpu::GpuContext) -> SpectralDebugResult {
    let mut max_ulp = 0u32;
    let mut over_budget = 0usize;
    let mut total_cases = 0usize;

    let studio_total = run_spectral_debug_studio_case(ctx, &mut max_ulp, &mut over_budget);
    total_cases += studio_total;
    let hdr_total = run_spectral_debug_hdr_case(ctx, &mut max_ulp, &mut over_budget);
    total_cases += hdr_total;

    SpectralDebugResult {
        total_cases,
        max_self_consistency_ulp: max_ulp,
        over_budget_count: over_budget,
    }
}

/// The `EnvironmentSource::Studio` half of [`run_spectral_debug`] -- white balance
/// applied on both the GPU kernel and the CPU recombination, `env_mode ==
/// STUDIO_RIG`. Returns the number of (pixel, sample) tuples it dispatched, folding its
/// own comparisons into `max_ulp`/`over_budget`.
fn run_spectral_debug_studio_case(
    ctx: &crate::renderer::gpu::GpuContext,
    max_ulp: &mut u32,
    over_budget: &mut usize,
) -> usize {
    let camera = test_camera();
    let (width, height, samples) = (16u32, 16u32, 8u32);
    let planes = round_brilliant_planes();
    let material = tier3_material();
    let gpu_material = GpuGemMaterial::encode(&material);
    let temp_k = illuminant_temperature_k(LightingPreset::Daylight);
    let wb = compute_illuminant_white_balance(temp_k);
    let camera_params = camera_params_for(&camera, width, height, samples);
    let params = GpuTransportParams::new(
        width * height,
        10,
        0,
        transport_env_mode::STUDIO_RIG,
        0.0,
        temp_k,
        1.0,
        1.0,
        0.0,
        0.0,
        wb.to_array(),
    );
    let total = (width * height * samples) as usize;
    let finishes = all_polished_finishes(planes.len());
    let dispatch = dispatch_transport(
        ctx,
        &camera_params,
        &params,
        &gpu_material,
        &planes,
        &finishes,
        total,
    );
    accumulate_spectral_debug_case(&dispatch, total, Some(wb), max_ulp, over_budget);
    total
}

/// The `EnvironmentSource::HdrMap` half of [`run_spectral_debug`] -- NO
/// white balance on either side (`env_mode == HDR_MAP` skips it in the kernel the same
/// way `trace_spectral_ray_inner`'s `EnvironmentSource::HdrMap(_) => xyz` no-op does on
/// the CPU), so the recombination below applies none either. Otherwise identical in
/// shape to [`run_spectral_debug_studio_case`]; see that function's doc comment.
fn run_spectral_debug_hdr_case(
    ctx: &crate::renderer::gpu::GpuContext,
    max_ulp: &mut u32,
    over_budget: &mut usize,
) -> usize {
    let camera = test_camera();
    let (width, height, samples) = (16u32, 16u32, 8u32);
    let planes = round_brilliant_planes();
    let material = tier3_material();
    let gpu_material = GpuGemMaterial::encode(&material);
    let camera_params = camera_params_for(&camera, width, height, samples);
    // `studio_temp_k`/`studio_spot_mult`/`studio_exposure`/`studio_light_*`/
    // `white_balance` are all `Studio`-only fields the kernel never reads once
    // `env_mode == HDR_MAP` -- passed as harmless placeholders, mirroring how every
    // OTHER `HdrMap` GPU dispatch in this module (`environment_check::run_hdr_env_radiance`,
    // `image_comparison::run_image_comparison_hdr_nee`) leaves them at their defaults.
    let params = GpuTransportParams::new(
        width * height,
        10,
        0,
        transport_env_mode::HDR_MAP,
        0.0,
        0.0,
        1.0,
        1.0,
        0.0,
        0.0,
        Vec3::ONE.to_array(),
    );
    let total = (width * height * samples) as usize;
    let finishes = all_polished_finishes(planes.len());
    let map = build_hdr_test_map();
    let hdr_env = crate::renderer::env_map_gpu::HdrEnvGpuData::upload(&ctx.device, &map);
    let dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: frame::material_class::GENERIC,
        },
        SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &finishes,
            hdr_env: Some(&hdr_env),
        },
        total,
    );
    accumulate_spectral_debug_case(&dispatch, total, None, max_ulp, over_budget);
    total
}

/// Shared per-(pixel, sample) comparison loop for both [`run_spectral_debug_studio_case`]
/// and [`run_spectral_debug_hdr_case`] -- recombines the GPU's own pre-integration debug
/// output through the real CPU integration (plus white balance, when `wb` is `Some`) and
/// records each channel's ULP distance against the GPU's own `out_xyz`.
fn accumulate_spectral_debug_case(
    dispatch: &TransportDispatch,
    total: usize,
    wb: Option<Vec3>,
    max_ulp: &mut u32,
    over_budget: &mut usize,
) {
    for idx in 0..total {
        let mut radiance = [0.0f32; 8];
        let mut lambdas = [0.0f32; 8];
        let mut path_pdf = [0.0f32; 8];
        let mut compat = [0u8; 8];
        radiance.copy_from_slice(&dispatch.radiance[idx * 8..idx * 8 + 8]);
        lambdas.copy_from_slice(&dispatch.lambdas[idx * 8..idx * 8 + 8]);
        path_pdf.copy_from_slice(&dispatch.path_pdf[idx * 8..idx * 8 + 8]);
        for (c, &raw) in compat
            .iter_mut()
            .zip(&dispatch.compat[idx * 8..idx * 8 + 8])
        {
            *c = raw as u8;
        }

        // `dispatch.xyz` already has the kernel's own `params.white_balance` applied
        // via `apply_von_kries_white_balance` when `wb` is `Some` (Bradford LMS, not
        // raw XYZ multiply) -- apply the same transform to the recombined value for a
        // like-for-like compare; the `HdrMap` case passes `None` since the
        // kernel itself skips the transform for `env_mode == HDR_MAP`.
        //
        // The kernel's own final integration is `integrate_channels_to_xyz_families`
        // (per-channel MIS family weighting, not the single shared weight
        // `integrate_channels_to_xyz` uses) -- recombining with the old function would
        // disagree with `out_xyz` whenever this trace narrowed a family, which isn't a
        // porting bug, just the wrong CPU function. `dispatch.compat` is the GPU
        // kernel's own `compat[]` array from the same `out_compat` buffer.
        //
        // `.max(Vec3::ZERO)`: the kernel clamps its (optionally white-balanced) result
        // to non-negative like `trace_spectral_ray_inner` does, so the recombination
        // must too, or a narrow sample whose Bradford round trip dips below zero
        // compares `-4.9e-4` against `0`.
        let integrated =
            integrate_channels_to_xyz_families(&radiance, &lambdas, &path_pdf, 0, compat);
        let recombined = wb
            .map_or(integrated, |wb| {
                apply_von_kries_white_balance(integrated, wb)
            })
            .max(Vec3::ZERO);
        let gpu_xyz = Vec3::new(
            dispatch.xyz[idx * 3],
            dispatch.xyz[idx * 3 + 1],
            dispatch.xyz[idx * 3 + 2],
        );

        for (cpu, gpu) in [
            (recombined.x, gpu_xyz.x),
            (recombined.y, gpu_xyz.y),
            (recombined.z, gpu_xyz.z),
        ] {
            let ulp = crate::renderer::gpu::ulp::ulp_distance(cpu, gpu);
            if !crate::renderer::gpu::ulp::within_tolerance(
                cpu,
                gpu,
                SPECTRAL_DEBUG_ULP_BUDGET,
                SPECTRAL_DEBUG_ABS_FLOOR,
            ) {
                *over_budget += 1;
                *max_ulp = (*max_ulp).max(ulp);
            }
        }
    }
}

impl SpectralDebugResult {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.over_budget_count == 0
    }
}
