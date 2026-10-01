//! Profile-guided-optimization training workload.
//!
//! Run by `scripts/pgo-build.ps1` / `scripts/pgo-bolt-build.sh` inside an instrumented
//! build. Collects a representative execution profile of everything the CPU spends its
//! time on, across every material optical character and every code path a production
//! binary (`indicatrix-worker`, `indicatrix-cut`) actually exercises:
//!
//! | Stage             | Code exercised |
//! |---|---|
//! | `train_tracer`    | Spectral path tracer (`optics::raytracer::transport`): isotropic (Diamond, Cubic Zirconia), uniaxial +/- (Sapphire/Zircon), biaxial (Alexandrite, Topaz); dispersive and near-non-dispersive built-ins; plain and frosted-girdle finishes; inclusion scattering; edge rounding; the hero-wavelength comb; plane-SoA intersection (`simd::slab_scan`/`PlanesSoA32`); the A-Trous denoiser; both the linear-HDR float buffer and the tonemapped 8-bit sRGB output. |
//! | `train_lighting`  | Every [`indicatrix::optics::raytracer::LightingPreset::ALL`] entry (all four `LightingModel`s: the analytic studio rig plus the `IsoHemisphere`/`LightTent`/`DaylightDome` lit models) through the SAME `TraceJob`/`trace_frame` path `train_tracer` uses, plus one synthetic HDR equirectangular map (`renderer::env_map::EnvironmentMap::from_rgb`) through `optics::raytracer::EnvironmentSource::HdrMap` -- the two branches `trace_spectral_ray_with_finish_soa`'s ray-miss lookup dispatches between. `train_tracer` above only visits two of the seven presets (chosen for material coverage); this is the stage that visits every lighting model and the HDR-map code path at all. |
//! | `train_brep`      | `geometry::cuts::StandardGemCuts::from_asc_schedule` -> `geometry::GemPolyhedron::from_planes` (plane-arrangement B-Rep reconstruction via `simd::solve_triple_batch`) and `untouched_planes`/`volume`/`facet_areas`; `indicatrix_formats::asc::to_asc_string` (the `.asc` *writer*, round-tripped back through `parse_asc`). |
//! | `train_solver`    | Meet-point solver, all three phases plus the verified repair search (`geometry::meet_solver::{solve_meet_points, solve_meet_points_verified}`), which in turn dispatch the SIMD candidate-vertex kernels (`simd::{classify_feasibility, solve_triple_batch, PlanesSoA64}`); external solid measurement and CAD-preview mesh extraction (`geometry::stone_metrics::{measure_solid, build_solid_mesh}`) on the solved geometry. Runs against three synthetic schedules AND five real, catalogue-sourced `.asc` designs (the crate's own CrackOtto-Step cost-probe fixture, 205 facet-plane instances, plus the four real designs `tests/optics_geometry_tests/asc_designs.rs` also exercises, via the same `include_str!`'d fixture files). |
//! | `train_tilt`      | The tilt-performance sweep (`color::metrics::evaluate_full_axis_profile_at_azimuth`, all `PROFILE_AZIMUTHS_DEG` axes) that backs `indicatrix-worker`'s `TILT_CURVES` request and `indicatrix-cut`'s tilt-profile panel. |
//! | `train_gpu`       | (only when built with `--features gpu`, and an adapter is present) the CPU-side chunked dispatch/readback orchestration in `renderer::gpu::hybrid` (`HybridSplit::calibrated` + `render_hybrid`) that `apps/indicatrix-cut`'s and `apps/indicatrix-worker`'s `gpu` features route progressive/export rendering through. Never trains the compute shader itself (WGSL is not covered by LLVM PGO) -- only the Rust-side dispatch/chunk-sizing/readback loop around it. |
//!
//! What is deliberately NOT trained here: the database (`indicatrix-vault` is never
//! opened), any file I/O, and the GPU compute shader's own WGSL source. `indicatrix-net`'s
//! wire protocol (`SceneState`/`StreamEvent` postcard encode/decode) cannot be trained
//! from this crate without giving `indicatrix` a dev-dependency on `indicatrix-net` (which
//! itself depends on `indicatrix`) -- see `scripts/pgo-bolt-build.sh`'s header comment for
//! how that gap is covered instead, by running `indicatrix-net`'s own `scene_roundtrip`
//! integration test under instrumentation as a second training binary.
//!
//! Deterministic and self-contained; measured around 25 s at the default scale
//! (`PGO_SCALE=1`, `--profile probe`, `--features gpu` with a real adapter present) on
//! a 16-thread desktop, well under the ~5 minute budget this file targets -- the
//! `[tracer]` and `[solver]` stages dominate, the latter mostly from real
//! CrackOtto-Step's 205 facet-plane instances (well over a second per plain solve,
//! even at this build's release-like optimisation level). Honours:
//!
//! - `INDICATRIX_SIMD=scalar|avx2|avx512` so a portable (scalar) build can be trained on
//!   an AVX machine -- see `simd::simd_level`.
//! - `INDICATRIX_SAMPLES=<n>`: overrides the tracer's samples-per-pixel directly (what
//!   `scripts/pgo-bolt-build.sh --samples` sets); takes priority over `PGO_SCALE` for
//!   that one number.
//! - `PGO_SCALE=<factor>` (default `1.0`): scales every stage's frame size, sample
//!   count, and iteration count by this factor, for a bounded total runtime. `0.05` is
//!   small enough to prove every stage executes in a few seconds; values above `1.0`
//!   are honoured for a deliberately heavier training run.
//! - `PGO_TRAIN_SKIP_GPU=1`: skips `train_gpu` even in a `gpu`-feature build (the
//!   stage already skips itself gracefully when no adapter is present; this is for
//!   deliberately excluding GPU dispatch code from the profile instead).
//!
//! ```text
//! cargo run --release -p indicatrix --all-features --example pgo_train
//! PGO_SCALE=0.05 cargo run --release -p indicatrix --features hdr,serde --example pgo_train
//! ```

mod stages;

#[cfg(feature = "gpu")]
use stages::train_gpu;
use stages::{train_brep, train_lighting, train_solver, train_tilt, train_tracer};
use std::time::Instant;

/// `PGO_SCALE` environment variable (default `1.0`, clamped to a sane positive range),
/// read once and shared by every stage below.
fn pgo_scale() -> f32 {
    std::env::var("PGO_SCALE")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0)
        .clamp(0.01, 20.0)
}

fn main() {
    let scale = pgo_scale();
    println!(
        "pgo_train: simd level {:?}, PGO_SCALE={scale}",
        indicatrix::simd::simd_level()
    );
    let start = Instant::now();

    let t0 = Instant::now();
    let tracer_checksum = train_tracer(scale);
    println!("pgo_train: [tracer] stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let lighting_checksum = train_lighting(scale);
    println!("pgo_train: [lighting] stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let brep_checksum = train_brep();
    println!("pgo_train: [brep]   stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let solver_checksum = train_solver(scale);
    println!("pgo_train: [solver] stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let tilt_checksum = train_tilt(scale);
    println!("pgo_train: [tilt]   stage done in {:.2?}", t0.elapsed());

    #[cfg(feature = "gpu")]
    let gpu_checksum = {
        let t0 = Instant::now();
        let c = train_gpu(scale);
        println!("pgo_train: [gpu]    stage done in {:.2?}", t0.elapsed());
        c
    };
    #[cfg(not(feature = "gpu"))]
    let gpu_checksum = 0.0f32;

    println!(
        "pgo_train done in {:.1?} (checksums tracer={tracer_checksum:.3} lighting={lighting_checksum:.3} \
         brep={brep_checksum:.6} solver={solver_checksum:.6} tilt={tilt_checksum:.3} gpu={gpu_checksum:.3})",
        start.elapsed()
    );
}
