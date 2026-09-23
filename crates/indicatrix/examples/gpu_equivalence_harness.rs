//! GPU/CPU equivalence harness covering: struct-layout echoes, RNG/integer
//! bit-exactness, geometry/environment (camera ray generation,
//! `intersect_polyhedron`, studio environment sampling, CIE 1931 CMF integration, von
//! Kries white balance, and the furnace anchor tying all of those together against
//! analytically computable truth), the full isotropic spectral estimator (Fresnel/TIR
//! with PDF-division throughput, Stokes-Mueller polarized transport, pleochroic
//! Beer-Lambert absorption, Russian roulette, spectral MIS -- cubic materials), and
//! birefringence, both uniaxial (the `theta_c` fixed-point iteration, the 50/50
//! ordinary/extraordinary eigenmode split, `extraordinary_poynting_dir` walk-off) and
//! biaxial (see [`indicatrix::optics::materials::GemMaterial::gpu_supported`], which is
//! unconditionally `true`; see [`indicatrix::renderer::gpu::estimator_check`] and
//! [`indicatrix::renderer::gpu::transport_check`]'s own doc comments).
//!
//! Not a `cargo test` target: it needs a real GPU adapter, which isn't guaranteed on
//! every machine that builds this workspace, so this is a `gpu`-feature-gated example
//! instead (see this crate's `Cargo.toml`, `required-features = ["gpu"]`), run
//! explicitly:
//!
//! ```text
//! cargo run --profile probe -p indicatrix --features gpu --example gpu_equivalence_harness
//! ```
//!
//! Exits nonzero (via [`std::process::exit`]) if any check fails, after printing
//! diagnostic detail for every failing check -- never a bare assert. Prints
//! [`indicatrix::BUILD_ID`] at the top so a failure report is traceable to the exact
//! source snapshot (Rust *and* WGSL) that produced it.
//!
//! If no GPU adapter is available at all, this reports that plainly and exits nonzero
//! -- it does not panic, and it does not report untested checks as passing.
//!
//! # Tiers
//!
//! - **Tier 0** (GPU self-determinism): [`indicatrix::renderer::gpu::determinism_check`],
//!   plus the furnace-anchor determinism check (two `furnace_accumulate_main`
//!   dispatches, byte-for-byte).
//! - **Tier 1** (integer bit-exactness against the CPU, zero tolerance, plus the
//!   struct-layout echo tests): [`indicatrix::renderer::gpu::rng_check`]'s
//!   `RngCheckResult::tier1_passed`, [`indicatrix::renderer::gpu::layout_check`]
//!   (`GpuGemMaterial`, `GpuFacetPlane`, `GpuCameraParams`, `GpuRay`, `GpuHitRecord`).
//! - **Tier 2** (per-function ULP budgets against `optics::*`): `lambdas`, camera ray
//!   generation, `cie_1931_cmf`, `blackbody_spectrum`, `sample_studio_environment`, and
//!   `compute_illuminant_white_balance` -- and `intersect_polyhedron`'s case-bank check
//!   (a discrete facet-index comparison, not a bare ULP sweep). Also the Mueller-matrix
//!   constructors + application, `signed_frame_rotation_psi`, `tir_phase_delta`,
//!   `DispersionModel::evaluate`, `spectral_absorption`,
//!   `ordinary`/`extraordinary_eigen_polarization`, and `pleochroic_channel_alpha` --
//!   see [`indicatrix::renderer::gpu::transport_check`]'s own doc comment for what is
//!   deliberately NOT covered as a standalone check (a bare Fresnel
//!   `r_s`/`r_p`/`t_s`/`t_p`-from-physics sweep) and why.
//! - **Furnace anchor**: [`indicatrix::renderer::gpu::furnace_check`] (uniform
//!   environment, zero geometry) plus
//!   [`indicatrix::renderer::gpu::estimator_check::run_furnace`] (a real colourless
//!   non-dispersive gem inside a uniform environment) -- both glue every ported
//!   function together against a uniform environment whose expected XYZ is
//!   analytically computable, checking both CPU and GPU against that
//!   independently-derived truth rather than merely against each other.
//! - **Tier 3** (statistical image comparison):
//!   [`indicatrix::renderer::gpu::estimator_check::run_image_comparison`] -- Welford
//!   per-pixel mean/M2 on CPU and GPU DISJOINT sample ranges (as production renders
//!   would split work), z-score, and connected-component clustering of failing pixels.
//!   The measured, unavoidable ULP-scale float divergence this harness already found
//!   (see `rng_check`'s module doc comment) is exactly why this comparison is
//!   variance-scaled statistical rather than exact. Also two instances on real
//!   uniaxial-birefringent built-ins: Zircon (the largest birefringence in the material
//!   set, `birefringence_delta = +0.0590`) and Tourmaline (strongly negative,
//!   `-0.0210`).
//!
//! The isotropic estimator dispatch handles cubic materials only. Uniaxial
//! birefringence uses the SAME megakernel and dispatch path (not a separate kernel) --
//! see `shaders/spectral_transport.wgsl`'s own header comment for how the isotropic
//! case is provably a special case of the general uniaxial computation, not a parallel
//! code path. The SAME megakernel further generalizes to genuinely biaxial materials
//! (Alexandrite, Topaz, Tanzanite): the `BiaxialIndicatrix` machinery (`wave_indices`,
//! `eigen_polarizations`, `mode_poynting_dir`, `resolve_entry_mode`) plus the
//! three-coefficient pleochroic absorption path. Whether this verified port is
//! actually routed to for a real render is governed entirely by
//! [`indicatrix::optics::materials::GemMaterial::gpu_supported`] -- read that
//! function's own doc comment for the current state, don't infer it from this harness
//! passing alone.

use indicatrix::renderer::gpu::{
    GpuContext, camera_check, determinism_check, environment_check, estimator_check, furnace_check,
    layout_check, polyhedron_check, rng_check, shading_normal_check, transport_check,
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
type LayoutCheckFn = fn(&GpuContext) -> layout_check::LayoutCheckResult;

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

// ---------------------------------------------------------------------------------
// Transport physics.
// ---------------------------------------------------------------------------------

/// Tier 1: the struct-layout GPU echo test for `GpuTransportParams`.
fn report_transport_params_layout_check(ctx: &GpuContext) -> bool {
    print!("[Tier 1] Phase 2 struct-layout echo test (GpuTransportParams, 80 bytes) ... ");
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

/// Generic Tier-2 ULP-budget check reporter, shared by every [`transport_check`]
/// instance (its `UlpCheckResult` is a deliberate duplicate of `environment_check`'s,
/// not the same type -- see `transport_check`'s own doc comment).
fn report_transport_ulp_check<Case: Clone + std::fmt::Debug>(
    label: &str,
    result: &transport_check::UlpCheckResult<Case>,
) -> bool {
    print!("[Tier 2] {label} ... ");
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

/// The energy-conservation furnace anchor (real gem geometry, colourless
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
    print!(
        "[Tier 3] statistical image comparison (Spinel, studio rig, {}x{} pixels) ... ",
        48, 48
    );
    let result = estimator_check::run_image_comparison(ctx);
    let passed = result.passed();
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

// ---------------------------------------------------------------------------------
// Uniaxial birefringence.
// ---------------------------------------------------------------------------------

/// Tier 3: statistical image equivalence for a real uniaxial-birefringent material --
/// see [`estimator_check::run_image_comparison`]'s doc comment for the method
/// (identical here, just a different material); reused by both the Zircon and
/// Tourmaline instances below.
fn report_image_comparison_material(
    label: &str,
    result: &estimator_check::ImageComparisonResult,
) -> bool {
    print!(
        "[Tier 3] statistical image comparison ({label}, studio rig, {}x{} pixels) ... ",
        result.width, result.height
    );
    let passed = result.passed();
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

/// Isotropic spectral estimator checks (cubic materials only). Pulled out of `main`
/// purely to keep that function under clippy's function-length lint -- returns whether
/// every check here passed.
fn run_phase2_checks(ctx: &GpuContext) -> bool {
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

/// Uniaxial birefringence checks. Pulled out of `main` for the same function-length
/// reason as [`run_phase2_checks`] -- returns whether every check here passed.
fn run_phase3_checks(ctx: &GpuContext) -> bool {
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
        "Rutile, delta=+0.287",
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
fn run_phase4_checks(ctx: &GpuContext) -> bool {
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
fn run_task1_scattering_checks(ctx: &GpuContext) -> bool {
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
/// itself specifically calls for: a coloured (chromatically absorbing) stone at a
/// genuinely non-1.0 scale.
fn run_p1_absorption_path_scale_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== P1 GPU port: absorption path scale ==");
    report_image_comparison_material(
        "Ruby, absorption_path_scale=3.0",
        &estimator_check::run_image_comparison_absorption_path_scale(ctx),
    )
}

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
fn run_task2_frosted_girdle_checks(ctx: &GpuContext) -> bool {
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
fn run_task2_edge_rounding_checks(ctx: &GpuContext) -> bool {
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

/// Tier 3 image comparisons for the lighting models. Pulled out of `main` for the same
/// function-length reason as [`run_phase2_checks`] -- returns whether every check here passed.
fn run_lighting_model_checks(ctx: &GpuContext) -> bool {
    println!();
    println!("== Lighting models: Tier 3 statistical image comparisons ==");
    let iso_passed = report_image_comparison_material(
        "Diamond, ISO hemisphere",
        &estimator_check::run_image_comparison_iso_hemisphere(ctx),
    );
    let light_tent_passed = report_image_comparison_material(
        "Diamond, Light tent + black cards",
        &estimator_check::run_image_comparison_light_tent(ctx),
    );
    let daylight_dome_passed = report_image_comparison_material(
        "Diamond, Daylight sky + sun",
        &estimator_check::run_image_comparison_daylight_dome(ctx),
    );
    iso_passed && light_tent_passed && daylight_dome_passed
}

/// The production frame renderer's own check: a chunked dispatch (`pixel_offset != 0`).
///
/// Every other check in this harness dispatches a whole frame at once, so none of them
/// ever exercises the chunked path -- see `renderer::gpu::frame::run_chunk_equivalence`
/// for why that matters. Builds its own `GpuFrameRenderer` rather than reusing the
/// harness's `GpuContext`, because owning the device and pipeline across frames is part
/// of what this checks.
fn run_chunk_check() -> bool {
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
fn run_wavefront_pipeline_checks() -> bool {
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
fn run_all_specialisation_checks(ctx: &GpuContext) -> bool {
    let specialisation_passed = run_specialisation_check();
    let specialisation_image_comparison_passed = run_specialisation_image_comparison_check(ctx);
    specialisation_passed && specialisation_image_comparison_passed
}

/// RNG / struct-layout / self-determinism and geometry/environment checks. Pulled out
/// of `main` for the same function-length reason as
/// [`run_phase2_checks`]/[`run_phase3_checks`] -- returns whether every one passed.
fn run_phase0_and_phase1_checks(ctx: &GpuContext) -> bool {
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
fn run_finding_g7_nee_checks(ctx: &GpuContext) -> bool {
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

fn main() {
    // Without a subscriber, `GpuContext::acquire_async`'s `on_uncaptured_error` handler's
    // `tracing::error!` call goes nowhere -- the actual wgpu validation/device error text
    // that produces a `GpuFrameError::DeviceLost` report below would otherwise be
    // unrecoverable from this binary's output. `warn` level (not `info`/`debug`) keeps
    // this quiet on a clean run; `fmt().with_writer(stderr)` keeps it off the
    // check-by-check `stdout` log this harness is meant to be grepped from.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .init();

    println!("indicatrix::BUILD_ID = {}", indicatrix::BUILD_ID);

    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("gpu_equivalence_harness: cannot acquire a GPU adapter: {e}");
            eprintln!(
                "gpu_equivalence_harness: this is a clean skip, not a crash -- no GPU-feature \
                 check below could run. Nothing was tested; do not treat this as a pass."
            );
            std::process::exit(2);
        }
    };
    let adapter_info = ctx.adapter.get_info();
    println!(
        "adapter: {} ({:?}, backend={:?})",
        adapter_info.name, adapter_info.device_type, adapter_info.backend
    );

    let phase0_and_phase1_passed = run_phase0_and_phase1_checks(&ctx);

    let phase2_passed = run_phase2_checks(&ctx);
    let phase3_passed = run_phase3_checks(&ctx);
    let phase4_passed = run_phase4_checks(&ctx);
    let task1_scattering_passed = run_task1_scattering_checks(&ctx);
    let task2_frosted_girdle_passed = run_task2_frosted_girdle_checks(&ctx);
    let task2_edge_rounding_passed = run_task2_edge_rounding_checks(&ctx);
    let p1_absorption_path_scale_passed = run_p1_absorption_path_scale_checks(&ctx);
    let lighting_models_passed = run_lighting_model_checks(&ctx);

    let chunk_passed = run_chunk_check();
    let wavefront_passed = run_wavefront_pipeline_checks();
    let specialisation_passed = run_all_specialisation_checks(&ctx);
    let g7_nee_passed = run_finding_g7_nee_checks(&ctx);

    let all_passed = phase0_and_phase1_passed
        && phase2_passed
        && phase3_passed
        && phase4_passed
        && task1_scattering_passed
        && task2_frosted_girdle_passed
        && task2_edge_rounding_passed
        && p1_absorption_path_scale_passed
        && lighting_models_passed
        && chunk_passed
        && wavefront_passed
        && specialisation_passed
        && g7_nee_passed;

    println!();
    if all_passed {
        println!(
            "gpu_equivalence_harness: ALL CHECKS PASSED (Phase 0 Tier 0-1 complete; Phase 1 \
             geometry/environment complete; Phase 2 isotropic spectral estimator complete: \
             struct-layout echo, Tier 2 per-function ULP budgets (Mueller/Fresnel/TIR/frame- \
             rotation/dispersion/absorption/pleochroic), determinism, energy-conservation \
             furnace anchor, Tier 3 statistical image comparison with clustering, spectral- \
             space debug self-consistency. Phase 3 uniaxial birefringence complete: theta_c \
             fixed-point iteration, extraordinary_poynting_dir walk-off, per-mode index Tier 2 \
             ULP budgets, Tier 3 statistical image comparison on Zircon (delta=+0.0590) and \
             Tourmaline (delta=-0.0210). Phase 4 biaxial birefringence verification complete: \
             BiaxialIndicatrix::{{wave_indices, eigen_polarizations, mode_poynting_dir, \
             resolve_entry_mode}} Tier 2 ULP budgets, biaxial pleochroic_channel_alpha Tier 2 ULP \
             budget, Tier 3 statistical image comparison on Alexandrite, Topaz and Tanzanite -- \
             see optics::materials::GemMaterial::gpu_supported's own doc comment for whether this \
             verified port is actually enabled for a real render. \
             Inclusion/subsurface scattering: HG phase/sampling \
             Tier 2 ULP budgets, maybe_scatter_or_extinguish Tier 2 ULP budget, lossless-scattering \
             energy-conservation furnace anchor, Tier 3 statistical image comparison on Ruby with \
             scattering enabled. Frosted girdle finish: GPU port verified. Facet edge \
             rounding: shading_normal_near_edge Tier 2 case-bank \
             self-test, energy-conservation furnace anchor with rounded edges, Tier 3 statistical \
             image comparison on Diamond with edge rounding enabled. Absorption path scale: \
             maybe_scatter_or_extinguish Tier 2 ULP budget covers non-1.0 \
             path_scale cases, Tier 1 struct-layout echo covers the GpuGemMaterial field, \
             Tier 3 statistical image comparison on Ruby at absorption_path_scale=3.0. \
             Production frame renderer wired up: chunked \
             dispatch (GpuTransportParams::pixel_offset) is bit-identical to a \
             single whole-frame dispatch. Kernel specialisation: \
             each material-class-specialised pipeline GpuFrameRenderer::accumulate \
             dispatches through is self-deterministic (byte-identical across two runs), \
             and a Tier 3 statistical image comparison (the same z-score/clustering \
             criteria as CPU-vs-GPU) confirms it against the GENERIC pipeline for \
             representative isotropic/uniaxial/biaxial materials -- see \
             renderer::gpu::frame's \"Material-class kernel specialisation\" doc section \
             for why GENERIC-vs-specialised is a statistical check, not bit-exact.)"
        );
    } else {
        println!("gpu_equivalence_harness: FAILED -- see diagnostics above");
        std::process::exit(1);
    }
}
