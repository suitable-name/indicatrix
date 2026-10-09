//! GPU port checks for inclusion/subsurface scattering (homogeneous
//! Henyey-Greenstein phase/sampling) and the `absorption_path_scale` field.

use indicatrix::renderer::gpu::{GpuContext, estimator_check, transport_check};

use crate::common::{report_image_comparison_material, report_transport_ulp_check};

// ---------------------------------------------------------------------------------
// GPU port: inclusion/subsurface scattering (homogeneous
// Henyey-Greenstein).
// ---------------------------------------------------------------------------------

/// Tier 2: the SAME energy-conservation furnace anchor as
/// [`report_furnace_anchor_v2`], but with a LOSSLESS scattering medium active -- see
/// [`estimator_check::run_furnace_scattering`]'s doc comment. This is the decisive
/// correctness check: scattering must redistribute energy, never create or
/// destroy it, on either engine.
fn report_furnace_scattering(ctx: &GpuContext) -> bool {
    print!(
        "[Furnace, scattering] furnace anchor (lossless Henyey-Greenstein medium, \
         still energy-conserving) ... "
    );
    let result = estimator_check::run_furnace_scattering(ctx);
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

/// Every check exercising the Henyey-Greenstein inclusion/subsurface scattering
/// port. Pulled out of `main` for the same function-length reason as
/// [`run_phase2_checks`]/[`run_phase3_checks`].
pub fn run_task1_scattering_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== GPU port: inclusion/subsurface scattering ==");
    let hg_phase_passed = report_transport_ulp_check(
        "henyey_greenstein_phase",
        &transport_check::run_hg_phase(ctx),
    );
    let hg_sample_passed = report_transport_ulp_check(
        "sample_henyey_greenstein_direction",
        &transport_check::run_hg_sample(ctx),
    );
    let scatter_passed = report_transport_ulp_check(
        "maybe_scatter_or_extinguish",
        &transport_check::run_scatter_or_extinguish(ctx),
    );
    let furnace_scattering_passed = report_furnace_scattering(ctx);
    let image_comparison_scattering_passed = report_image_comparison_material(
        "Ruby, sigma_s=1.5 g=0.3",
        &estimator_check::run_image_comparison_scattering(ctx),
    );
    // Pins the GPU/CPU parity fix for the scattering block's `is_biaxial` branching --
    // see `estimator_check::run_image_comparison_biaxial_scattering`'s doc comment.
    let image_comparison_biaxial_scattering_passed = report_image_comparison_material(
        "Alexandrite, biaxial, sigma_s=1.5 g=0.3",
        &estimator_check::run_image_comparison_biaxial_scattering(ctx),
    );

    hg_phase_passed
        && hg_sample_passed
        && scatter_passed
        && furnace_scattering_passed
        && image_comparison_scattering_passed
        && image_comparison_biaxial_scattering_passed
}

/// Every check exercising `GemMaterial::absorption_path_scale`.
/// `maybe_scatter_or_extinguish`'s own Tier 2 ULP
/// budget (already reported by [`run_task1_scattering_checks`] above) now exercises a
/// mix of `path_scale` values via its own case bank (see
/// `renderer::gpu::transport_check::scattering::build_scatter_cases`'s doc comment), and
/// the Tier 1 struct-layout echo (`renderer::gpu::layout_check::run`, reported by
/// [`run_phase0_and_phase1_checks`]) now covers the new `absorption_path_scale` field via
/// `layout_check::sample_material`'s own non-1.0 value -- neither needs a separate call
/// here. This function adds the Tier 3 statistical image comparison the physics change
/// itself specifically calls for: a colored (chromatically absorbing) stone at a
/// genuinely non-1.0 scale.
pub fn run_p1_absorption_path_scale_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== P1 GPU port: absorption path scale ==");
    report_image_comparison_material(
        "Ruby, absorption_path_scale=3.0",
        &estimator_check::run_image_comparison_absorption_path_scale(ctx),
    )
}

/// Zoned-absorption GPU checks (`zoning` feature): the zone-table layout echo, then one
/// Tier 3 CPU-vs-GPU image comparison per `ZonedImageCase`.
///
/// OWNER: the first run with `--features zoning` records these as the zoned goldens. Each
/// comparison passes or fails on `ImageComparisonResult::passed` (the same z-score and
/// cluster criteria as every other Tier 3 check); the soft cases are the ones to read before
/// lowering `renderer::buffers::GPU_SOFT_SUBDIV`.
#[cfg(feature = "zoning")]
pub fn run_zoning_checks(ctx: &GpuContext) -> bool {
    use indicatrix::renderer::gpu::{layout_check, zoned_cases::ZonedImageCase};

    println!();
    println!("== Zoned absorption (zoning feature) ==");
    print!("[Tier 1] zone-table struct-layout echo test (GpuZoneTable) ... ");
    let layout = layout_check::run_zone_table(ctx);
    let mut all_passed = layout.passed();
    if layout.passed() {
        println!("PASS");
    } else {
        println!(
            "FAIL ({} byte(s) mismatched, showing up to 32)",
            layout.mismatches.len()
        );
        for m in &layout.mismatches {
            println!(
                "  byte offset {:>4}: expected 0x{:02x}, got 0x{:02x}",
                m.offset, m.expected, m.actual
            );
        }
    }
    for case in ZonedImageCase::ALL {
        all_passed &= report_image_comparison_material(
            case.label(),
            &estimator_check::run_image_comparison_zoned(ctx, case),
        );
    }
    all_passed
}
