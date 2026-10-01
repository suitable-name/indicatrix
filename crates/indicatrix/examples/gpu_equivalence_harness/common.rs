//! Report-printing helpers shared across more than one GPU-equivalence check
//! module: the generic per-function ULP-budget reporter used by every
//! `transport_check` instance, the Tier 3 statistical image-comparison reporter
//! reused across phases and materials, and the `LayoutCheckFn` shape shared by the
//! Phase 1 and Phase 2 struct-layout echo tests.

use indicatrix::renderer::gpu::{GpuContext, estimator_check, layout_check, transport_check};

use crate::summary::{UlpComparison, UlpTier, record_image, record_ulp};

/// Common shape of a single struct-layout GPU echo check, used to build the
/// `[(&str, LayoutCheckFn); N]` tables in [`crate::phase0_1_geometry_environment`]'s
/// and [`crate::phase2_transport`]'s layout-check reporters.
pub type LayoutCheckFn = fn(&GpuContext) -> layout_check::LayoutCheckResult;

/// Generic Tier-2 ULP-budget check reporter, shared by every [`transport_check`]
/// instance (its `UlpCheckResult` is a deliberate duplicate of `environment_check`'s,
/// not the same type -- see `transport_check`'s own doc comment).
pub fn report_transport_ulp_check<Case: Clone + std::fmt::Debug>(
    label: &str,
    result: &transport_check::UlpCheckResult<Case>,
) -> bool {
    print!("[Tier 2] {label} ... ");
    record_ulp(&UlpComparison {
        tier: UlpTier::Tier2,
        label,
        comparisons: result.total_comparisons,
        max_genuine_ulp: result.max_ulp,
        max_raw_ulp: result.max_raw_ulp,
        exempted: result.exempted_count,
        budget: Some(result.budget),
        passed: result.passed(),
    });
    if result.passed() {
        println!(
            "PASS ({} comparisons, max genuine ULP = {}, max raw ULP = {}, {} exempted near-zero)",
            result.total_comparisons, result.max_ulp, result.max_raw_ulp, result.exempted_count
        );
        return true;
    }
    println!(
        "FAIL (max ULP = {} exceeds budget of {}; {} comparison(s) over budget, {} exempted)",
        result.max_ulp, result.budget, result.over_budget_count, result.exempted_count
    );
    if let Some(a) = &result.argmax {
        println!(
            "  argmax: case={:?} component={}: cpu={:e} (0x{:08x}) gpu={:e} (0x{:08x}) ULP={}",
            a.case,
            a.component,
            a.cpu,
            a.cpu.to_bits(),
            a.gpu,
            a.gpu.to_bits(),
            a.ulp
        );
    }
    false
}

/// Tier 3: statistical image equivalence for a real uniaxial-birefringent material --
/// see [`estimator_check::run_image_comparison`]'s doc comment for the method
/// (identical here, just a different material); reused by both the Zircon and
/// Tourmaline instances below.
pub fn report_image_comparison_material(
    label: &str,
    result: &estimator_check::ImageComparisonResult,
) -> bool {
    print!(
        "[Tier 3] statistical image comparison ({label}, studio rig, {}x{} pixels) ... ",
        result.width, result.height
    );
    let passed = result.passed();
    record_image(label, result, passed);
    println!("{}", if passed { "PASS" } else { "FAIL" });
    println!(
        "  {} pixels, {} CPU samples/pixel, {} GPU samples/pixel (disjoint ranges)",
        result.total_pixels, result.cpu_samples_per_pixel, result.gpu_samples_per_pixel
    );
    println!("  image-aggregate mean z = {:.4}", result.mean_z);
    println!(
        "  |z|>3 pixels: {} observed / {} total ({:.4}% vs {:.4}% binomial expectation)",
        result.over_3_sigma_count,
        result.total_pixels,
        100.0 * result.over_3_sigma_count as f64 / result.total_pixels as f64,
        100.0 * result.over_3_sigma_expected
    );
    println!(
        "  max |z| = {:.3} at pixel ({}, {})",
        result.max_abs_z, result.max_abs_z_pixel.0, result.max_abs_z_pixel.1
    );
    println!(
        "  connected components of |z|>3 pixels (largest first, up to 10 shown): {:?}",
        &result.cluster_sizes[..result.cluster_sizes.len().min(10)]
    );
    passed
}
