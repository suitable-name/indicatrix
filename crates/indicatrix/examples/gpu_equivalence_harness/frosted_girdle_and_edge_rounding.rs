//! GPU port checks for the frosted (bruted) girdle finish and facet edge rounding
//! (shading-normal perturbation).

use indicatrix::renderer::gpu::{
    GpuContext, estimator_check, shading_normal_check, transport_check,
};

use crate::common::{report_image_comparison_material, report_transport_ulp_check};

// ---------------------------------------------------------------------------------
// GPU port: frosted (bruted) girdle finish.
// ---------------------------------------------------------------------------------

/// Tier 2: the SAME energy-conservation furnace anchor as
/// [`report_furnace_anchor_v2`], but with the girdle band bruted -- see
/// [`estimator_check::run_furnace_frosted_girdle`]'s doc comment.
fn report_furnace_frosted_girdle(ctx: &GpuContext) -> bool {
    print!(
        "[Furnace, frosted girdle] furnace anchor (bruted girdle band, still \
         energy-conserving) ... "
    );
    let result = estimator_check::run_furnace_frosted_girdle(ctx);
    // A WIDER relative-error-vs-analytic-target tolerance than the polished furnace
    // anchor -- see `FurnaceResult::passed_frosted_girdle`'s and
    // `FROSTED_FURNACE_CONVERGENCE_TOLERANCE`'s doc comments for why: a frosted facet's
    // diffuse scattering interacts with Russian Roulette's rescaling to produce a
    // heavier-tailed estimator than the polished furnace's, converging to the analytic
    // target more slowly at a practical sample budget -- identically on CPU and GPU
    // (the z-score gate below stays tight and confirms that).
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

/// Every check exercising the frosted girdle finish port. Pulled out of `main`
/// for the same function-length reason as [`run_phase2_checks`]/[`run_phase3_checks`].
pub fn run_task2_frosted_girdle_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== GPU port: frosted (bruted) girdle finish ==");
    let cosine_hemisphere_passed = report_transport_ulp_check(
        "cosine_weighted_hemisphere",
        &transport_check::run_cosine_hemisphere(ctx),
    );
    let frosted_bounce_passed = report_transport_ulp_check(
        "apply_frosted_bounce",
        &transport_check::run_frosted_bounce(ctx),
    );
    let furnace_frosted_passed = report_furnace_frosted_girdle(ctx);
    let image_comparison_frosted_passed = report_image_comparison_material(
        "Diamond, frosted girdle",
        &estimator_check::run_image_comparison_frosted_girdle(ctx),
    );

    cosine_hemisphere_passed
        && frosted_bounce_passed
        && furnace_frosted_passed
        && image_comparison_frosted_passed
}

// ---------------------------------------------------------------------------------
// GPU port: facet edge rounding (shading-normal perturbation).
// ---------------------------------------------------------------------------------

/// Edge rounding, Tier 2: the SAME energy-conservation furnace anchor as
/// [`report_furnace_anchor_v2`], but with a nonzero
/// `edge_rounding_radius` -- see [`estimator_check::run_furnace_edge_rounding`]'s doc
/// comment.
fn report_furnace_edge_rounding(ctx: &GpuContext) -> bool {
    print!(
        "[Furnace, edge rounding] furnace anchor (rounded meet edges, still \
         energy-conserving) ... "
    );
    let result = estimator_check::run_furnace_edge_rounding(ctx);
    let passed = result.passed_edge_rounding();
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

/// Edge rounding, Tier 2: `shading_normal_near_edge`'s dedicated case-bank
/// self-test -- see [`shading_normal_check`]'s own module doc comment.
fn report_shading_normal_check(ctx: &GpuContext) -> bool {
    print!("[Tier 2] shading_normal_near_edge ... ");
    let result = shading_normal_check::run(ctx);
    let passed = result.passed();
    println!(
        "{} ({} cases x 3 components, max genuine ULP = {}, max raw ULP = {}, {} exempted \
         near-zero, {} over budget)",
        if passed { "PASS" } else { "FAIL" },
        result.total,
        result.max_genuine_ulp,
        result.max_raw_ulp,
        result.exempted_near_zero,
        result.over_budget_count
    );
    passed
}

/// Edge rounding: every check exercising the facet edge-rounding port. Pulled
/// out of `main` for the same function-length reason as
/// [`run_phase2_checks`]/[`run_phase3_checks`].
pub fn run_task2_edge_rounding_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== GPU port: facet edge rounding ==");
    let shading_normal_passed = report_shading_normal_check(ctx);
    let furnace_edge_rounding_passed = report_furnace_edge_rounding(ctx);
    let image_comparison_edge_rounding_passed = report_image_comparison_material(
        "Diamond, edge_rounding_radius=0.02",
        &estimator_check::run_image_comparison_edge_rounding(ctx),
    );

    shading_normal_passed && furnace_edge_rounding_passed && image_comparison_edge_rounding_passed
}
