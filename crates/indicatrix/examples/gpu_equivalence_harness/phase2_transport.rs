//! Phase 2 (isotropic spectral estimator transport physics): struct-layout echoes,
//! the furnace anchor tying every ported function together against analytically
//! computable truth, statistical image comparison, and spectral-space debug
//! self-consistency.

use indicatrix::renderer::{
    buffers::GpuTransportParams,
    gpu::{GpuContext, estimator_check, layout_check, transport_check},
};

use crate::common::{LayoutCheckFn, report_image_comparison_material, report_transport_ulp_check};

// ---------------------------------------------------------------------------------
// Transport physics.
// ---------------------------------------------------------------------------------

/// Tier 1: the struct-layout GPU echo test for `GpuTransportParams`.
fn report_transport_params_layout_check(ctx: &GpuContext) -> bool {
    print!(
        "[Tier 1] Phase 2 struct-layout echo test (GpuTransportParams, {} bytes) ... ",
        std::mem::size_of::<GpuTransportParams>()
    );
    let result = layout_check::run_transport_params(ctx);
    if result.passed() {
        println!("PASS");
        return true;
    }
    println!(
        "FAIL ({} byte(s) mismatched, showing up to 32)",
        result.mismatches.len()
    );
    for m in &result.mismatches {
        println!(
            "  byte offset {:>3}: expected 0x{:02x}, got 0x{:02x}",
            m.offset, m.expected, m.actual
        );
    }
    false
}

/// The struct-layout GPU echo tests for `GpuReduceParams`, `GpuWavefrontParams`,
/// `GpuHdrEnvDims`, `GpuDistDims`: `layout_check::run` (Tier 1a, `GpuGemMaterial`) and
/// [`report_transport_params_layout_check`] (`GpuTransportParams`) cover their own
/// structs at the byte-exact level, but these four 16-byte scalar structs otherwise
/// have no such check beyond "verified matching by eye"; see
/// [`layout_check::run_reduce_params`]'s own doc comment. Same
/// `[(&str, LayoutCheckFn); N]` shape as [`report_phase1_layout_checks`]. Returns
/// whether ALL FOUR passed.
fn report_phase2_layout_checks(ctx: &GpuContext) -> bool {
    let checks: [(&str, LayoutCheckFn); 4] = [
        ("GpuReduceParams, 16 bytes", layout_check::run_reduce_params),
        (
            "GpuWavefrontParams, 16 bytes",
            layout_check::run_wavefront_params,
        ),
        ("GpuHdrEnvDims, 16 bytes", layout_check::run_hdr_env_dims),
        ("GpuDistDims, 16 bytes", layout_check::run_dist_dims),
    ];
    let mut all_passed = true;
    for (label, check_fn) in checks {
        print!("[Tier 1] Phase 2 struct-layout echo test ({label}) ... ");
        let result = check_fn(ctx);
        if result.passed() {
            println!("PASS");
        } else {
            all_passed = false;
            println!(
                "FAIL ({} byte(s) mismatched, showing up to 32)",
                result.mismatches.len()
            );
            for m in &result.mismatches {
                println!(
                    "  byte offset {:>3}: expected 0x{:02x}, got 0x{:02x}",
                    m.offset, m.expected, m.actual
                );
            }
        }
    }
    all_passed
}

/// The energy-conservation furnace anchor (real gem geometry, colorless
/// non-dispersive material, uniform environment) -- see [`estimator_check::run_furnace`]'s
/// doc comment.
fn report_furnace_anchor_v2(ctx: &GpuContext) -> bool {
    print!("[Furnace v2] Phase 2 furnace anchor (real gem geometry, Fresnel/TIR/RR/MIS) ... ");
    let result = estimator_check::run_furnace(ctx);
    let passed = result.passed();
    println!("{}", if passed { "PASS" } else { "FAIL" });
    println!(
        "  analytic target XYZ = {:?}",
        (
            result.analytic_target.x,
            result.analytic_target.y,
            result.analytic_target.z
        )
    );
    println!(
        "  CPU mean XYZ = {:?} ({} samples, relative error {:.6})",
        (result.cpu_mean.x, result.cpu_mean.y, result.cpu_mean.z),
        result.total_cpu_samples,
        result.cpu_relative_error
    );
    println!(
        "  GPU mean XYZ = {:?} ({} samples, relative error {:.6})",
        (result.gpu_mean.x, result.gpu_mean.y, result.gpu_mean.z),
        result.total_gpu_samples,
        result.gpu_relative_error
    );
    println!(
        "  CPU-vs-GPU pooled z-score (X,Y,Z) = ({:.3}, {:.3}, {:.3})",
        result.cpu_gpu_z[0], result.cpu_gpu_z[1], result.cpu_gpu_z[2]
    );
    passed
}

/// Determinism: two `transport_main` dispatches against identical input.
fn report_transport_determinism(ctx: &GpuContext) -> bool {
    print!("[Determinism] Phase 2 transport_main, two runs ... ");
    let result = estimator_check::run_determinism(ctx);
    if result.passed() {
        println!(
            "PASS (two runs byte-identical across {} XYZ float values)",
            result.total_values
        );
        return true;
    }
    println!(
        "FAIL ({} of {} float values differed between runs)",
        result.mismatches, result.total_values
    );
    false
}

/// Tier 3: statistical image equivalence (Welford per-pixel mean/M2, z-score,
/// connected-component clustering) -- see [`estimator_check::run_image_comparison`]'s
/// doc comment.
fn report_image_comparison(ctx: &GpuContext) -> bool {
    report_image_comparison_material("Spinel", &estimator_check::run_image_comparison(ctx))
}

/// Spectral-space debug self-consistency (GPU per-channel radiance/lambdas/
/// `path_pdf` re-integrated through the REAL CPU `integrate_channels_to_xyz` and
/// compared to the GPU's own final XYZ) -- see
/// [`estimator_check::run_spectral_debug`]'s doc comment for the honest scope limit on
/// what this does and does not prove.
fn report_spectral_debug(ctx: &GpuContext) -> bool {
    print!("[Spectral debug] GPU per-channel radiance re-integration self-consistency ... ");
    let result = estimator_check::run_spectral_debug(ctx);
    if result.passed() {
        println!("PASS ({} cases)", result.total_cases);
        return true;
    }
    println!(
        "FAIL (max ULP = {}, {} of {} cases over budget)",
        result.max_self_consistency_ulp, result.over_budget_count, result.total_cases
    );
    false
}

/// Isotropic spectral estimator checks (cubic materials only). Pulled out of `main`
/// purely to keep that function under clippy's function-length lint -- returns whether
/// every check here passed.
pub fn run_phase2_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== Phase 2: isotropic spectral estimator (cubic materials only) ==");
    let transport_layout_passed = report_transport_params_layout_check(ctx);
    let phase2_layout_passed = report_phase2_layout_checks(ctx);
    let frame_rotation_passed = report_transport_ulp_check(
        "frame_rotation + apply_matrix",
        &transport_check::run_frame_rotation(ctx),
    );
    let fresnel_reflection_passed = report_transport_ulp_check(
        "fresnel_reflection + apply_matrix",
        &transport_check::run_fresnel_reflection(ctx),
    );
    let fresnel_transmission_passed = report_transport_ulp_check(
        "fresnel_transmission + apply_matrix",
        &transport_check::run_fresnel_transmission(ctx),
    );
    let tir_retardation_passed = report_transport_ulp_check(
        "tir_retardation + apply_matrix",
        &transport_check::run_tir_retardation(ctx),
    );
    let signed_psi_passed = report_transport_ulp_check(
        "signed_frame_rotation_psi",
        &transport_check::run_signed_psi(ctx),
    );
    let tir_phase_delta_passed = report_transport_ulp_check(
        "tir_phase_delta",
        &transport_check::run_tir_phase_delta(ctx),
    );
    let dispersion_passed = report_transport_ulp_check(
        "DispersionModel::evaluate",
        &transport_check::run_dispersion(ctx),
    );
    // Covers both `BandShape` variants (P6: wavelength- and energy-domain Gaussian
    // absorption bands) in one case bank against the one production struct/kernel --
    // see `transport_check::absorption_pleochroism::build_absorption_cases`'s own doc
    // comment.
    let absorption_passed =
        report_transport_ulp_check("spectral_absorption", &transport_check::run_absorption(ctx));
    let eigen_polarization_passed = report_transport_ulp_check(
        "ordinary/extraordinary_eigen_polarization",
        &transport_check::run_eigen_polarization(ctx),
    );
    let pleochroic_passed = report_transport_ulp_check(
        "pleochroic_channel_alpha",
        &transport_check::run_pleochroic(ctx),
    );

    let transport_determinism_passed = report_transport_determinism(ctx);
    let furnace_v2_passed = report_furnace_anchor_v2(ctx);
    let image_comparison_passed = report_image_comparison(ctx);
    let spectral_debug_passed = report_spectral_debug(ctx);

    transport_layout_passed
        && phase2_layout_passed
        && frame_rotation_passed
        && fresnel_reflection_passed
        && fresnel_transmission_passed
        && tir_retardation_passed
        && signed_psi_passed
        && tir_phase_delta_passed
        && dispersion_passed
        && absorption_passed
        && eigen_polarization_passed
        && pleochroic_passed
        && transport_determinism_passed
        && furnace_v2_passed
        && image_comparison_passed
        && spectral_debug_passed
}
