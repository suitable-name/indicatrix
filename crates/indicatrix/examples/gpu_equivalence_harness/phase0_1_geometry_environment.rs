//! Phase 0 (GPU self-determinism, struct-layout echoes, RNG bit-exactness) and
//! Phase 1 (geometry/environment: camera ray generation, `intersect_polyhedron`,
//! studio environment sampling, CIE 1931 CMF integration, von Kries white balance,
//! and the furnace anchor) checks.

use indicatrix::renderer::gpu::{
    GpuContext, camera_check, determinism_check, environment_check, furnace_check, layout_check,
    polyhedron_check, rng_check,
};

use crate::{
    common::LayoutCheckFn,
    summary::{UlpComparison, UlpTier, record_ulp},
};

// ~10^6 (pixel, sample, bounce) tuples: 4096 pixels * 64 samples * 4 bounces = 1,048,576.
const NUM_PIXELS: u32 = 4096;
const NUM_SAMPLES: u32 = 64;

const DET_PIXELS: u32 = 65_536;
const DET_SAMPLES: u32 = 256;

/// Tier 1a: the mandatory struct-layout GPU echo test. Returns whether it passed.
fn report_layout_check(ctx: &GpuContext) -> bool {
    print!("[Tier 1] struct-layout echo test (GpuGemMaterial, 480 bytes) ... ");
    let result = layout_check::run(ctx);
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
            "  byte offset {:>3} ({}): expected 0x{:02x}, got 0x{:02x}",
            m.offset,
            layout_check::field_name_at_offset(m.offset),
            m.expected,
            m.actual
        );
    }
    false
}

/// Tier 1a: the struct-layout GPU echo tests for `GpuFacetPlane`, `GpuCameraParams`,
/// `GpuRay`, `GpuHitRecord`. Returns whether ALL FOUR passed.
fn report_phase1_layout_checks(ctx: &GpuContext) -> bool {
    let checks: [(&str, LayoutCheckFn); 5] = [
        ("GpuFacetPlane, 16 bytes", layout_check::run_facet_plane),
        ("GpuCameraParams, 64 bytes", layout_check::run_camera_params),
        ("GpuRay, 32 bytes", layout_check::run_ray),
        ("GpuHitRecord, 32 bytes", layout_check::run_hit_record),
        (
            "facet_finish array<u32>, girdle finish",
            layout_check::run_facet_finish,
        ),
    ];
    let mut all_passed = true;
    for (label, check_fn) in checks {
        print!("[Tier 1] Phase 1 struct-layout echo test ({label}) ... ");
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

/// Tier 1b (integer bit-exactness) and Tier 2 (`jx`/`jy`/`hero_rand`/`lambdas` ULP
/// budget), both from the same `rng_check::run` dispatch -- see [`rng_check`]'s module
/// doc comment for why those float fields are excluded from Tier 1. Returns whether
/// BOTH passed.
fn report_rng_check(ctx: &GpuContext) -> bool {
    let total_tuples =
        u64::from(NUM_PIXELS) * u64::from(NUM_SAMPLES) * u64::from(rng_check::NUM_BOUNCES);
    print!(
        "[Tier 1] RNG bit-exactness ({} pixel*sample pairs x {} bounces = {total_tuples} tuples) ... ",
        NUM_PIXELS * NUM_SAMPLES,
        rng_check::NUM_BOUNCES,
    );
    let result = rng_check::run(ctx, NUM_PIXELS, NUM_SAMPLES);
    let tier1_passed = result.tier1_passed();
    if tier1_passed {
        println!("PASS ({} records compared)", result.total_records);
    } else {
        println!(
            "FAIL ({} mismatch(es) out of {} records, showing up to 64)",
            result.mismatches.len(),
            result.total_records
        );
        for m in &result.mismatches {
            println!(
                "  pixel={} sample={} field={}: cpu={} gpu={}",
                m.pixel, m.sample, m.field, m.cpu, m.gpu
            );
        }
    }

    let float = result.float_ulp;
    print!(
        "[Tier 2] jx/jy/hero_rand/lambdas ULP budget ({} values compared, budget={}) ... ",
        float.total_values_compared, float.budget
    );
    let tier2_passed = float.passed();
    record_ulp(&UlpComparison {
        tier: UlpTier::Tier2,
        label: "jx/jy/hero_rand/lambdas",
        comparisons: float.total_values_compared,
        max_genuine_ulp: float.max_ulp,
        max_raw_ulp: float.max_raw_ulp,
        exempted: float.exempted_count,
        budget: Some(float.budget),
        passed: tier2_passed,
    });
    if tier2_passed {
        println!(
            "PASS (max genuine ULP = {}, max raw ULP = {}, {} exempted near-zero)",
            float.max_ulp, float.max_raw_ulp, float.exempted_count
        );
    } else {
        println!(
            "FAIL (max ULP distance = {} exceeds budget of {}; {} value(s) over budget, {} exempted)",
            float.max_ulp, float.budget, float.over_budget_count, float.exempted_count
        );
        if let Some(a) = float.argmax {
            println!(
                "  argmax: pixel={} sample={} field={} channel={}: cpu={:e} (0x{:08x}) gpu={:e} (0x{:08x}) ULP={}",
                a.pixel,
                a.sample,
                a.field,
                a.channel,
                a.cpu,
                a.cpu.to_bits(),
                a.gpu,
                a.gpu.to_bits(),
                a.ulp
            );
        }
    }

    tier1_passed && tier2_passed
}

/// Tier 0: GPU self-determinism (two runs of the same dispatch, byte-for-byte).
fn report_determinism_check(ctx: &GpuContext) -> bool {
    print!(
        "[Tier 0] GPU self-determinism ({DET_PIXELS} pixels x {DET_SAMPLES} samples, two runs) ... "
    );
    let result = determinism_check::run(ctx, DET_PIXELS, DET_SAMPLES);
    if result.passed() {
        println!("PASS (two runs byte-identical across {DET_PIXELS} pixels)");
        return true;
    }
    println!(
        "FAIL ({} pixel(s) differed between runs, showing up to 64)",
        result.mismatches.len()
    );
    for m in &result.mismatches {
        let ulp = (i64::from(m.run1.to_bits()) - i64::from(m.run2.to_bits())).unsigned_abs();
        println!(
            "  pixel={}: run1={:e} (0x{:08x}) run2={:e} (0x{:08x}) ULP-distance={}",
            m.pixel,
            m.run1,
            m.run1.to_bits(),
            m.run2,
            m.run2.to_bits(),
            ulp
        );
    }
    false
}

/// Tier 2: camera ray generation (`Camera::new` + `Camera::generate_ray`, including
/// RNG-derived jitter).
fn report_camera_check(ctx: &GpuContext) -> bool {
    print!("[Tier 2] Phase 1 camera ray generation (dense + adversarial case grid) ... ");
    let result = camera_check::run(ctx);
    record_ulp(&UlpComparison {
        tier: UlpTier::Tier2,
        label: "camera ray generation",
        comparisons: result.total_cases * 6,
        max_genuine_ulp: result.max_ulp,
        max_raw_ulp: result.max_raw_ulp,
        exempted: result.exempted_count,
        budget: Some(result.budget),
        passed: result.passed(),
    });
    if result.passed() {
        println!(
            "PASS ({} cases, {} components compared, max genuine ULP = {}, max raw ULP = {}, {} exempted near-zero)",
            result.total_cases,
            result.total_cases * 6,
            result.max_ulp,
            result.max_raw_ulp,
            result.exempted_count
        );
        return true;
    }
    println!(
        "FAIL (max ULP = {} exceeds budget of {}; {} component(s) over budget, {} exempted)",
        result.max_ulp, result.budget, result.over_budget_count, result.exempted_count
    );
    if let Some(a) = result.argmax {
        println!(
            "  argmax: case[{}]={:?} component={}: cpu={:e} (0x{:08x}) gpu={:e} (0x{:08x}) ULP={}",
            a.case_index,
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

/// `intersect_polyhedron` case-bank check (entry AND exit branches, plus adversarial
/// denom/tie cases). Not a bare ULP sweep -- see [`polyhedron_check`]'s module doc
/// comment.
fn report_polyhedron_check(ctx: &GpuContext) -> bool {
    print!("[Tier 2] Phase 1 intersect_polyhedron case bank (57-facet round brilliant) ... ");
    let result = polyhedron_check::run(ctx);
    if result.passed() {
        println!(
            "PASS ({} cases, {} whitelisted grazing ties)",
            result.total_cases, result.whitelisted_ties
        );
        return true;
    }
    println!(
        "FAIL ({} mismatch(es) out of {} cases, {} whitelisted ties, showing up to 64)",
        result.total_mismatches, result.total_cases, result.whitelisted_ties
    );
    for m in &result.mismatches {
        println!(
            "  case[{}] label={} outcome={:?}: {}",
            m.case_index, m.case.label, m.outcome, m.detail
        );
    }
    false
}

/// Generic ULP-budget check reporter, shared by the four [`environment_check`]
/// instances.
fn report_ulp_check<Case: Clone + std::fmt::Debug>(
    label: &str,
    result: &environment_check::UlpCheckResult<Case>,
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

/// The furnace anchor -- see [`furnace_check`]'s module doc comment.
fn report_furnace_check(ctx: &GpuContext) -> bool {
    print!(
        "[Furnace] Phase 1 furnace anchor ({} tuples) ... ",
        NUM_PIXELS as usize * (NUM_SAMPLES as usize)
    );
    let result = furnace_check::run(ctx);
    let passed = result.passed();
    record_ulp(&UlpComparison {
        tier: UlpTier::Furnace,
        label: "furnace anchor per-tuple CPU vs GPU",
        comparisons: result.total_tuples,
        max_genuine_ulp: result.per_tuple_max_ulp,
        max_raw_ulp: result.per_tuple_max_ulp,
        exempted: 0,
        budget: Some(result.per_tuple_ulp_budget),
        passed: result.per_tuple_over_budget_count == 0,
    });
    if passed {
        println!("PASS");
    } else {
        println!("FAIL");
    }
    println!(
        "  analytic target XYZ = {:?}",
        (
            result.analytic_target.x,
            result.analytic_target.y,
            result.analytic_target.z
        )
    );
    println!(
        "  per-tuple ULP: max={} (budget={}), {} over budget",
        result.per_tuple_max_ulp, result.per_tuple_ulp_budget, result.per_tuple_over_budget_count
    );
    if let Some(a) = result.per_tuple_argmax {
        println!(
            "    argmax: pixel={} sample={} component={}: cpu={:e} gpu={:e} ULP={}",
            a.pixel, a.sample, a.component, a.cpu, a.gpu, a.ulp
        );
    }
    println!(
        "  CPU mean XYZ = {:?} (relative error {:.6}, tolerance {})",
        (result.cpu_mean.x, result.cpu_mean.y, result.cpu_mean.z),
        result.cpu_relative_error,
        furnace_check::CONVERGENCE_RELATIVE_TOLERANCE
    );
    println!(
        "  GPU mean XYZ = {:?} (relative error {:.6}, tolerance {})",
        (result.gpu_mean.x, result.gpu_mean.y, result.gpu_mean.z),
        result.gpu_relative_error,
        furnace_check::CONVERGENCE_RELATIVE_TOLERANCE
    );
    println!(
        "  determinism: {}/{} pixel-sum values differed between two runs",
        result.determinism_mismatches, result.determinism_sample_count
    );
    passed
}

/// RNG / struct-layout / self-determinism and geometry/environment checks. Pulled out
/// of `main` for the same function-length reason as
/// [`run_phase2_checks`]/[`run_phase3_checks`] -- returns whether every one passed.
pub fn run_phase0_and_phase1_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== Phase 0: RNG / struct-layout / self-determinism ==");
    let layout_passed = report_layout_check(ctx);
    let rng_passed = report_rng_check(ctx);
    let determinism_passed = report_determinism_check(ctx);

    println!();
    println!("== Phase 1: geometry / environment ==");
    let phase1_layout_passed = report_phase1_layout_checks(ctx);
    let camera_passed = report_camera_check(ctx);
    let polyhedron_passed = report_polyhedron_check(ctx);
    let cmf_result = environment_check::run_cmf(ctx);
    let cmf_passed = report_ulp_check("cie_1931_cmf", &cmf_result);
    let blackbody_result = environment_check::run_blackbody(ctx);
    let blackbody_passed = report_ulp_check("blackbody_spectrum", &blackbody_result);
    let studio_env_result = environment_check::run_studio_env(ctx);
    let studio_env_passed = report_ulp_check("sample_studio_environment", &studio_env_result);
    let white_balance_result = environment_check::run_white_balance(ctx);
    let white_balance_passed =
        report_ulp_check("compute_illuminant_white_balance", &white_balance_result);
    let furnace_passed = report_furnace_check(ctx);

    layout_passed
        && rng_passed
        && determinism_passed
        && phase1_layout_passed
        && camera_passed
        && polyhedron_passed
        && cmf_passed
        && blackbody_passed
        && studio_env_passed
        && white_balance_passed
        && furnace_passed
}
