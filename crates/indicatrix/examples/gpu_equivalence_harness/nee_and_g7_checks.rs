//! Finding G7 (next-event estimation) checks: the balance heuristic, `Dist1D`
//! bucket lookup, `Dist2D` sampling/pdf, NEE through a frosted exterior facet, NEE
//! HG-scatter sampling, and the scattering/frosted furnace-anchor NEE-equality
//! checks.

use indicatrix::renderer::gpu::{GpuContext, estimator_check, transport_check};

use crate::common::{report_image_comparison_material, report_transport_ulp_check};

fn report_furnace_nee_equality_scattering(ctx: &GpuContext) -> bool {
    print!("[Furnace, NEE scattering] furnace anchor (HdrMap NEE + MIS on scattering medium) ... ");
    let result = estimator_check::run_furnace_nee_equality_scattering(ctx);
    let passed = result.passed_scattering();
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

fn report_furnace_nee_equality_frosted(ctx: &GpuContext) -> bool {
    print!("[Furnace, NEE frosted] furnace anchor (HdrMap NEE + MIS on frosted girdle) ... ");
    let result = estimator_check::run_furnace_nee_equality_frosted(ctx);
    let passed = result.passed_frosted_girdle();
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

/// Next-event estimation (NEE) with multiple importance sampling (MIS, balance heuristic)
/// and 1D/2D environment map distribution importance sampling.
pub fn run_finding_g7_nee_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== Next-event estimation (NEE) & MIS for HDR environment maps ==");
    let balance_heuristic_passed = report_transport_ulp_check(
        "balance_heuristic",
        &transport_check::run_balance_heuristic(ctx),
    );
    let dist1d_find_bucket_passed = report_transport_ulp_check(
        "dist1d_find_bucket",
        &transport_check::run_dist1d_find_bucket(ctx),
    );
    let dist2d_sample_passed =
        report_transport_ulp_check("dist2d_sample", &transport_check::run_dist2d_sample(ctx));
    let dist2d_pdf_passed =
        report_transport_ulp_check("dist2d_pdf", &transport_check::run_dist2d_pdf(ctx));
    let nee_frosted_passed = report_transport_ulp_check(
        "nee_frosted_exterior",
        &transport_check::run_nee_frosted_exterior(ctx),
    );
    let nee_hg_passed =
        report_transport_ulp_check("nee_hg_scatter", &transport_check::run_nee_hg_scatter(ctx));
    let furnace_nee_scattering_passed = report_furnace_nee_equality_scattering(ctx);
    let furnace_nee_frosted_passed = report_furnace_nee_equality_frosted(ctx);
    let image_comparison_hdr_nee_passed = report_image_comparison_material(
        "Ruby, scattering, HDR NEE",
        &estimator_check::run_image_comparison_hdr_nee(ctx),
    );

    balance_heuristic_passed
        && dist1d_find_bucket_passed
        && dist2d_sample_passed
        && dist2d_pdf_passed
        && nee_frosted_passed
        && nee_hg_passed
        && furnace_nee_scattering_passed
        && furnace_nee_frosted_passed
        && image_comparison_hdr_nee_passed
}
