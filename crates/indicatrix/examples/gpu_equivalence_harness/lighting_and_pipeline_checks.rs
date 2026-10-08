//! Lighting-model image comparisons (iso-hemisphere, light tent, daylight dome),
//! the production frame renderer's chunked-dispatch and wavefront-pipeline
//! equivalence checks, and material-class kernel specialisation.

use indicatrix::{
    optics::raytracer::environment::LightingPreset,
    renderer::gpu::{GpuContext, estimator_check},
};

use crate::common::report_image_comparison_material;

/// Tier 3 image comparisons for the lighting models. Pulled out of `main` for the same
/// function-length reason as [`run_phase2_checks`] -- returns whether every check here passed.
pub fn run_lighting_model_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== Lighting models: Tier 3 statistical image comparisons ==");
    let iso_passed = report_image_comparison_material(
        &format!("Diamond, {}", LightingPreset::IsoHemisphere.label()),
        &estimator_check::run_image_comparison_iso_hemisphere(ctx),
    );
    let light_tent_passed = report_image_comparison_material(
        &format!("Diamond, {}", LightingPreset::LightTent.label()),
        &estimator_check::run_image_comparison_light_tent(ctx),
    );
    let daylight_dome_passed = report_image_comparison_material(
        &format!("Diamond, {}", LightingPreset::DaylightDome.label()),
        &estimator_check::run_image_comparison_daylight_dome(ctx),
    );
    let daylight_sun_passed = report_image_comparison_material(
        &format!("Diamond, {}", LightingPreset::DaylightSun.label()),
        &estimator_check::run_image_comparison_daylight_sun(ctx),
    );
    let daylight_sun_frosted_passed = report_image_comparison_material(
        &format!(
            "Diamond, frosted girdle, {} (sun NEE + MIS)",
            LightingPreset::DaylightSun.label()
        ),
        &estimator_check::run_image_comparison_daylight_sun_frosted_girdle(ctx),
    );
    let aset_passed = report_image_comparison_material(
        &format!("Diamond, {}", LightingPreset::Aset.label()),
        &estimator_check::run_image_comparison_aset(ctx),
    );
    let glare_zero_passed = report_image_comparison_material(
        "Diamond, Daylight D65, surface glare 0.0",
        &estimator_check::run_image_comparison_surface_glare(ctx, 0.0),
    );
    let glare_half_passed = report_image_comparison_material(
        "Diamond, Daylight D65, surface glare 0.5",
        &estimator_check::run_image_comparison_surface_glare(ctx, 0.5),
    );
    iso_passed
        && light_tent_passed
        && daylight_dome_passed
        && daylight_sun_passed
        && daylight_sun_frosted_passed
        && aset_passed
        && glare_zero_passed
        && glare_half_passed
}

/// The production frame renderer's own check: a chunked dispatch (`pixel_offset != 0`).
///
/// Every other check in this harness dispatches a whole frame at once, so none of them
/// ever exercises the chunked path -- see `renderer::gpu::frame::run_chunk_equivalence`
/// for why that matters. Builds its own `GpuFrameRenderer` rather than reusing the
/// harness's `GpuContext`, because owning the device and pipeline across frames is part
/// of what this checks.
pub fn run_chunk_check() -> bool {
    println!();
    println!("== Production frame renderer: chunked dispatch ==");
    match indicatrix::renderer::gpu::GpuFrameRenderer::new() {
        Ok(mut renderer) => {
            let r = indicatrix::renderer::gpu::frame::run_chunk_equivalence(&mut renderer);
            println!(
                "[Chunking] one dispatch vs {} chunks, bit-exact ... {}",
                r.chunks_forced,
                if r.passed() { "PASS" } else { "FAIL" }
            );
            println!(
                "  {} / {} pixels differ, max |delta| = {:e}",
                r.differing_pixels, r.total_pixels, r.max_abs_diff
            );
            r.passed()
        }
        Err(e) => {
            println!("[Chunking] FAIL -- could not build a frame renderer: {e}");
            false
        }
    }
}

/// The wavefront transport pipeline's own checks -- bit-identical
/// `out_xyz` against the megakernel on the same fixture
/// (`renderer::gpu::frame::run_pipeline_equivalence`), plus the wavefront pipeline's own
/// two-runs-identical determinism check
/// (`renderer::gpu::frame::run_wavefront_determinism`), the wavefront-pipeline
/// counterpart of [`run_chunk_check`]/`estimator_check::run_determinism`. Builds its own
/// `GpuFrameRenderer` for the same reason `run_chunk_check` does.
pub fn run_wavefront_pipeline_checks() -> bool {
    println!();
    println!("== Wavefront transport pipeline ==");
    match indicatrix::renderer::gpu::GpuFrameRenderer::new() {
        Ok(mut renderer) => {
            let equiv = indicatrix::renderer::gpu::frame::run_pipeline_equivalence(&mut renderer);
            println!(
                "[Wavefront] megakernel vs wavefront, bit-exact ... {}",
                if equiv.passed() { "PASS" } else { "FAIL" }
            );
            println!(
                "  {} / {} pixels differ, max |delta| = {:e}",
                equiv.differing_pixels, equiv.total_pixels, equiv.max_abs_diff
            );

            let det = indicatrix::renderer::gpu::frame::run_wavefront_determinism(&mut renderer);
            println!(
                "[Wavefront] two runs, bit-exact ... {}",
                if det.passed() { "PASS" } else { "FAIL" }
            );
            println!(
                "  {} / {} pixels differ, max |delta| = {:e}",
                det.differing_pixels, det.total_pixels, det.max_abs_diff
            );

            equiv.passed() && det.passed()
        }
        Err(e) => {
            println!("[Wavefront] FAIL -- could not build a frame renderer: {e}");
            false
        }
    }
}

/// Kernel specialisation: GPU dispatch determinism for each specialised pipeline (the
/// SAME pipeline, dispatched twice against identical input, must be byte-identical),
/// plus a diagnostic GENERIC-vs-specialised diff count -- see
/// [`indicatrix::renderer::gpu::frame::run_specialisation_equivalence`]'s doc comment for
/// why GENERIC-vs-specialised is NOT required to be bit-exact (that rigorous
/// correctness gate is [`run_specialisation_image_comparison_check`] below).
fn run_specialisation_check() -> bool {
    println!();
    println!("== Production frame renderer: material-class kernel specialisation ==");
    match indicatrix::renderer::gpu::GpuFrameRenderer::new() {
        Ok(mut renderer) => {
            let r = indicatrix::renderer::gpu::frame::run_specialisation_equivalence(&mut renderer);
            for case in &r.cases {
                println!(
                    "[Specialisation] {} (class={}): same pipeline twice, self-deterministic ... {}",
                    case.material_name,
                    case.material_class,
                    if case.passed() { "PASS" } else { "FAIL" }
                );
                println!(
                    "  self-determinism: {} / {} pixels differ",
                    case.self_determinism_differing_pixels, case.total_pixels
                );
                println!(
                    "  diagnostic (not gated): GENERIC vs specialised {} / {} pixels differ, max |delta| = {:e}",
                    case.generic_vs_specialised_differing_pixels,
                    case.total_pixels,
                    case.generic_vs_specialised_max_abs_diff
                );
            }
            r.passed()
        }
        Err(e) => {
            println!("[Specialisation] FAIL -- could not build a frame renderer: {e}");
            false
        }
    }
}

/// Kernel specialisation: the rigorous GENERIC-vs-specialised correctness gate --
/// Tier 3 statistical image comparison, the SAME z-score/clustering
/// criteria [`report_image_comparison_material`] already uses for CPU-vs-GPU, on one
/// representative material per class. See
/// [`indicatrix::renderer::gpu::estimator_check::run_specialisation_image_comparison`]'s doc
/// comment for why this (not bit-exact equality) is the right instrument.
fn run_specialisation_image_comparison_check(ctx: &GpuContext) -> bool {
    println!();
    println!(
        "== Production frame renderer: material-class specialisation, Tier 3 statistical comparison =="
    );
    let materials = [
        ("Diamond", "isotropic"),
        ("Zircon", "uniaxial"),
        ("Alexandrite", "biaxial"),
    ];
    let mut all_passed = true;
    for (name, class_label) in materials {
        // Bare built-in on purpose (scale 1.0, no `material_for_stone`): this compares CPU against GPU on the same material, so only parity matters, not the render's colour scale.
        let material =
            indicatrix::optics::materials::GemMaterial::by_name(name).unwrap_or_else(|| {
                panic!("{name:?} is a built-in material in GemMaterial::all_materials()")
            });
        let result = estimator_check::run_specialisation_image_comparison(ctx, &material);
        all_passed &= report_image_comparison_material(
            &format!("{name}, {class_label}, GENERIC vs specialised"),
            &result,
        );
    }
    all_passed
}

/// Every kernel-specialisation check: GPU dispatch determinism per specialised
/// pipeline plus a diagnostic diff count ([`run_specialisation_check`]), and the
/// rigorous Tier 3 statistical GENERIC-vs-specialised comparison
/// ([`run_specialisation_image_comparison_check`]). Combined purely to keep `main`
/// under clippy's function-length lint.
pub fn run_all_specialisation_checks(ctx: &GpuContext) -> bool {
    let specialisation_passed = run_specialisation_check();
    let specialisation_image_comparison_passed = run_specialisation_image_comparison_check(ctx);
    specialisation_passed && specialisation_image_comparison_passed
}
