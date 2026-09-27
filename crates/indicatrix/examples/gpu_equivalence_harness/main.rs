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

use indicatrix::renderer::gpu::GpuContext;

mod common;
mod frosted_girdle_and_edge_rounding;
mod lighting_and_pipeline_checks;
mod nee_and_g7_checks;
mod phase0_1_geometry_environment;
mod phase2_transport;
mod phase3_uniaxial;
mod phase4_biaxial;
mod scattering_and_absorption;

use frosted_girdle_and_edge_rounding::{
    run_task2_edge_rounding_checks, run_task2_frosted_girdle_checks,
};
use lighting_and_pipeline_checks::{
    run_all_specialisation_checks, run_chunk_check, run_lighting_model_checks,
    run_wavefront_pipeline_checks,
};
use nee_and_g7_checks::run_finding_g7_nee_checks;
use phase0_1_geometry_environment::run_phase0_and_phase1_checks;
use phase2_transport::run_phase2_checks;
use phase3_uniaxial::run_phase3_checks;
use phase4_biaxial::run_phase4_checks;
use scattering_and_absorption::{run_p1_absorption_path_scale_checks, run_task1_scattering_checks};

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
