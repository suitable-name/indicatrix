//! Phase 4 (biaxial birefringence) checks: `BiaxialIndicatrix::{wave_indices,
//! eigen_polarizations, mode_poynting_dir, resolve_entry_mode}` ULP budgets, the
//! biaxial `pleochroic_channel_alpha` ULP budget, and Tier 3 statistical image
//! comparison on Alexandrite, Topaz and Tanzanite.

use indicatrix::renderer::gpu::{GpuContext, estimator_check, transport_check};

use crate::common::{report_image_comparison_material, report_transport_ulp_check};

// ---------------------------------------------------------------------------------
// Biaxial birefringence.
// ---------------------------------------------------------------------------------

/// Genuinely biaxial birefringence checks. Pulled out of `main` for the same
/// function-length reason as [`run_phase2_checks`]/[`run_phase3_checks`] -- returns
/// whether every check here passed.
///
/// Whether the port checked here is actually trusted for a real render stays a
/// SEPARATE decision from whether it passes these checks -- see
/// [`indicatrix::optics::materials::GemMaterial::gpu_supported`]'s own doc comment, which
/// is where that answer lives. Today it is `true` unconditionally, biaxial built-ins
/// included, so this verifies a path that real renders genuinely take.
pub fn run_phase4_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== Phase 4: biaxial birefringence (live path -- gpu_supported() is true) ==");
    let wave_indices_passed = report_transport_ulp_check(
        "BiaxialIndicatrix::wave_indices",
        &transport_check::run_biaxial_wave_indices(ctx),
    );
    let eigen_polarization_passed = report_transport_ulp_check(
        "BiaxialIndicatrix::eigen_polarizations",
        &transport_check::run_biaxial_eigen_polarization(ctx),
    );
    let mode_poynting_passed = report_transport_ulp_check(
        "BiaxialIndicatrix::mode_poynting_dir",
        &transport_check::run_biaxial_mode_poynting(ctx),
    );
    let resolve_entry_mode_passed = report_transport_ulp_check(
        "BiaxialIndicatrix::resolve_entry_mode",
        &transport_check::run_biaxial_resolve_entry_mode(ctx),
    );
    let biaxial_pleochroic_passed = report_transport_ulp_check(
        "pleochroic_channel_alpha (biaxial)",
        &transport_check::run_biaxial_pleochroic(ctx),
    );
    // P1 assigned-mode absorption (biaxial branch): replaces
    // pleochroic_channel_alpha_biaxial on the interior transport path's anisotropic
    // branch -- see `transport_check::eigenmodes_biaxial::run_assigned_mode_alpha_biaxial`'s
    // own doc comment.
    let assigned_mode_alpha_biaxial_passed = report_transport_ulp_check(
        "assigned_mode_alpha (biaxial, P1)",
        &transport_check::run_assigned_mode_alpha_biaxial(ctx),
    );
    let alexandrite_image_comparison_passed = report_image_comparison_material(
        "Alexandrite, biaxial trichroic",
        &estimator_check::run_image_comparison_alexandrite(ctx),
    );
    let topaz_image_comparison_passed = report_image_comparison_material(
        "Topaz, biaxial two-band absorption",
        &estimator_check::run_image_comparison_topaz(ctx),
    );
    let tanzanite_image_comparison_passed = report_image_comparison_material(
        "Tanzanite, biaxial trichroic",
        &estimator_check::run_image_comparison_tanzanite(ctx),
    );

    wave_indices_passed
        && eigen_polarization_passed
        && mode_poynting_passed
        && resolve_entry_mode_passed
        && biaxial_pleochroic_passed
        && assigned_mode_alpha_biaxial_passed
        && alexandrite_image_comparison_passed
        && topaz_image_comparison_passed
        && tanzanite_image_comparison_passed
}
