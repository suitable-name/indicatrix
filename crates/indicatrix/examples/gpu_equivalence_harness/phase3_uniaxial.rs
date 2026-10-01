//! Phase 3 (uniaxial birefringence): `theta_c` fixed-point iteration,
//! `extraordinary_poynting_dir` walk-off, per-mode index ULP budgets, and Tier 3
//! statistical image comparison on Zircon, Tourmaline, Quartz, Rutile and
//! Synthetic Moissanite.

use indicatrix::renderer::gpu::{GpuContext, estimator_check, transport_check};

use crate::common::{report_image_comparison_material, report_transport_ulp_check};

// ---------------------------------------------------------------------------------
// Uniaxial birefringence.
// ---------------------------------------------------------------------------------

/// Exit-event spectral splitting: `narrow_compat`'s own reporter -- its
/// `NarrowCompatResult` is an exact u32 equality check, not a ULP-budget one, so it
/// does not fit `report_transport_ulp_check`'s generic shape.
fn report_narrow_compat(result: &transport_check::NarrowCompatResult) -> bool {
    print!("[Tier 2] narrow_compat (P6 exit-event spectral splitting) ... ");
    if result.passed() {
        println!(
            "PASS ({} cases, 0 mismatches, exact u32 equality)",
            result.total_cases
        );
        return true;
    }
    println!(
        "FAIL ({} of {} channel comparisons mismatched)",
        result.mismatches, result.total_cases
    );
    println!("  first mismatch: {}", result.describe_first_mismatch());
    false
}

/// Uniaxial birefringence checks. Pulled out of `main` for the same function-length
/// reason as [`run_phase2_checks`] -- returns whether every check here passed.
pub fn run_phase3_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== Phase 3: uniaxial birefringence (biaxial is covered by Phase 4) ==");
    let theta_c_passed =
        report_transport_ulp_check("theta_c_for_bounce", &transport_check::run_theta_c(ctx));
    let walk_off_passed = report_transport_ulp_check(
        "extraordinary_poynting_dir",
        &transport_check::run_walk_off(ctx),
    );
    let per_mode_index_passed = report_transport_ulp_check(
        "per_channel_uniaxial_indices",
        &transport_check::run_per_mode_index(ctx),
    );
    // P1 assigned-mode absorption (uniaxial branch): replaces pleochroic_channel_alpha
    // on the interior transport path's anisotropic branch -- see
    // `transport_check::absorption_pleochroism::run_assigned_mode_alpha_uniaxial`'s own
    // doc comment.
    let assigned_mode_alpha_uniaxial_passed = report_transport_ulp_check(
        "assigned_mode_alpha (uniaxial, P1)",
        &transport_check::run_assigned_mode_alpha_uniaxial(ctx),
    );
    // Full uniaxial Fresnel (Lekner 1991): kernel-level equivalence for the exact
    // closed-form entry/internal solve itself, independent of the megakernel's own
    // dispatch wiring below -- see `transport_check::p2_uniaxial_fresnel`'s own doc
    // comment.
    let entry_solve_pair_passed = report_transport_ulp_check(
        "entry_solve_pair (P2 full uniaxial Fresnel)",
        &transport_check::run_entry_solve_pair(ctx),
    );
    let internal_solve_passed = report_transport_ulp_check(
        "internal_solve (P2 full uniaxial Fresnel)",
        &transport_check::run_internal_solve(ctx),
    );
    // Exit-event spectral splitting: kernel-level equivalence for the three pure
    // per-channel helpers the megakernel's exit-event mismatch sites call -- see
    // `transport_check::p6_exit_splitting`'s own doc comment.
    let channel_transmission_passed = report_transport_ulp_check(
        "compute_channel_transmission (P6 exit-event spectral splitting)",
        &transport_check::run_channel_transmission(ctx),
    );
    let uniaxial_exit_transmission_passed = report_transport_ulp_check(
        "compute_uniaxial_exit_transmission (P6 exit-event spectral splitting)",
        &transport_check::run_uniaxial_exit_transmission(ctx),
    );
    let narrow_compat_passed = report_narrow_compat(&transport_check::run_narrow_compat(ctx));
    let zircon_image_comparison_passed = report_image_comparison_material(
        "Zircon, delta=+0.0590",
        &estimator_check::run_image_comparison_zircon(ctx),
    );
    let tourmaline_image_comparison_passed = report_image_comparison_material(
        "Tourmaline, delta=-0.0210",
        &estimator_check::run_image_comparison_tourmaline(ctx),
    );
    // Quartz is the one built-in with a genuine independent extraordinary-ray curve
    // (Zircon/Tourmaline above both only exercise the constant-offset fallback) -- see
    // `estimator_check::quartz_material`'s doc comment.
    let quartz_image_comparison_passed = report_image_comparison_material(
        "Quartz, genuine e-ray dispersion curve",
        &estimator_check::run_image_comparison_quartz(ctx),
    );
    // Rutile is this crate's most extreme birefringence built-in -- see
    // `estimator_check::rutile_material`'s own doc comment for why this is the
    // strongest single Tier 3 check the closed-form solve's WGSL mirror has.
    let rutile_image_comparison_passed = report_image_comparison_material(
        "Rutile, delta=+0.2957",
        &estimator_check::run_image_comparison_rutile(ctx),
    );
    // Final Tier 3 check: a strongly dispersive uniaxial material's refractive
    // (non-TIR-only) paths, isolating the exit-event mismatch sites the WGSL port
    // touches -- see `estimator_check::run_image_comparison_synthetic_moissanite`'s own
    // doc comment.
    let synthetic_moissanite_image_comparison_passed = report_image_comparison_material(
        "Synthetic Moissanite, delta=+0.0415 (P6 exit-event spectral splitting)",
        &estimator_check::run_image_comparison_synthetic_moissanite(ctx),
    );
    // The pre-existing Diamond frosted-girdle Tier 3 scene (run and gated separately by
    // `run_task2_frosted_girdle_checks` below) must pass again now that the WGSL side
    // mirrors the CPU's final estimator -- see `shaders/spectral_transport.wgsl`'s own
    // header comment; not re-run here to avoid dispatching the same expensive Tier 3
    // scene twice.

    theta_c_passed
        && walk_off_passed
        && per_mode_index_passed
        && assigned_mode_alpha_uniaxial_passed
        && entry_solve_pair_passed
        && internal_solve_passed
        && channel_transmission_passed
        && uniaxial_exit_transmission_passed
        && narrow_compat_passed
        && zircon_image_comparison_passed
        && tourmaline_image_comparison_passed
        && quartz_image_comparison_passed
        && rutile_image_comparison_passed
        && synthetic_moissanite_image_comparison_passed
}
