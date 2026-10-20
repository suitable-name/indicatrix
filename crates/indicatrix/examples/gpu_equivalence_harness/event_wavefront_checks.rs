//! Event wavefront transport pipeline checks against the megakernel.

use indicatrix::renderer::gpu::{GpuContext, frame::run_event_wavefront_equivalence};

/// Runs event wavefront equivalence checks across material classes 1, 2, and 3.
///
/// Class 1 covers studio and HDR environments, a frosted girdle, scattering (studio and
/// HDR; only the HDR case sees NEE or MIS bugs) and a `max_bounces = 3` survivor case;
/// class 2 covers uniaxial stones (plain, frosted girdle, HDR); class 3 biaxial (plain,
/// HDR). NaN or Inf in either image is a failure, never a tolerance pass.
pub fn run_event_wavefront_pipeline_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== Event wavefront transport pipeline: class equivalence ==");

    let mut all_passed = true;

    for class in [1u32, 2, 3] {
        let label = match class {
            1 => "class 1 (isotropic)",
            2 => "class 2 (uniaxial)",
            3 => "class 3 (biaxial)",
            _ => "unknown",
        };
        let equiv = run_event_wavefront_equivalence(ctx, class);
        let passed = equiv.passed();
        println!(
            "[Event Wavefront] {label} equivalence ... {}",
            if passed { "PASS" } else { "FAIL" }
        );
        println!(
            "  {} / {} pixels differ, {} tol-failing, max |delta| = {:e}",
            equiv.differing_pixels,
            equiv.total_pixels,
            equiv.tol_failing_pixels,
            equiv.max_abs_diff
        );
        if !passed {
            all_passed = false;
        }
    }

    all_passed
}
