//! Profile-guided-optimization training workload.
//!
//! Run by `scripts/pgo-build.ps1` / `scripts/pgo-bolt-build.sh` inside an instrumented
//! build. Collects a representative execution profile of everything the CPU spends its
//! time on, across every material optical character and every code path a production
//! binary (`indicatrix-worker`, `indicatrix-cut`, `indicatrix-cut`) actually exercises:
//!
//! | Stage             | Code exercised |
//! |---|---|
//! | `train_tracer`    | Spectral path tracer (`optics::raytracer::transport`): isotropic (Diamond, Cubic Zirconia), uniaxial +/- (Sapphire/Zircon), biaxial (Alexandrite, Topaz); dispersive and near-non-dispersive built-ins; plain and frosted-girdle finishes; inclusion scattering; edge rounding; the hero-wavelength comb; plane-SoA intersection (`simd::slab_scan`/`PlanesSoA32`); the A-Trous denoiser; both the linear-HDR float buffer and the tonemapped 8-bit sRGB output. |
//! | `train_brep`      | `geometry::cuts::StandardGemCuts::from_asc_schedule` -> `geometry::GemPolyhedron::from_planes` (dual-hull B-Rep reconstruction) and `untouched_planes`/`volume`/`facet_areas`; `indicatrix_formats::asc::to_asc_string` (the `.asc` *writer*, round-tripped back through `parse_asc`). |
//! | `train_solver`    | Meet-point solver, all three phases plus the verified repair search (`geometry::meet_solver::{solve_meet_points, solve_meet_points_verified}`), which in turn dispatch the SIMD candidate-vertex kernels (`simd::{classify_feasibility, solve_triple_batch, PlanesSoA64}`); external solid measurement (`geometry::stone_metrics::measure_solid`). |
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
//! Deterministic and self-contained; runs in roughly 10-60 s at the default scale on a
//! 16-thread desktop (longer with the `gpu` feature and a real adapter). Honours:
//!
//! - `INDICATRIX_SIMD=scalar|avx2` so a portable (scalar) build can be trained on an AVX
//!   machine -- see `simd::simd_level`.
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

use glam::Vec3;
use indicatrix::{
    geometry::{
        GpuFacetPlane, cuts::StandardGemCuts, girdle_facet_finishes, meet_solver, stone_metrics,
    },
    optics::{
        materials::GemMaterial,
        raytracer::{
            Camera, FacetFinish, HERO_WAVELENGTH_ROTATION_STREAM, LightingPreset,
            PIXEL_JITTER_X_ROTATION_STREAM, PIXEL_JITTER_Y_ROTATION_STREAM, build_plane_soa,
            cranley_patterson_rotate, hash_u32, low_discrepancy_base2, radical_inverse_base,
            trace_spectral_ray_with_finish_soa,
        },
    },
    renderer::{
        denoise::{AtrousDenoiser, AtrousParams, GBuffers},
        tonemap::tonemap_to_rgba,
    },
    simd::PlanesSoA32,
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

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

/// Scales `base` by `scale`, rounding to the nearest integer and never going below
/// `min` -- every stage's frame sizes/iteration counts stay at least `min` so a tiny
/// `PGO_SCALE` still exercises the real code path, not a degenerate zero-sized one.
fn scaled(base: u32, scale: f32, min: u32) -> u32 {
    ((base as f32 * scale).round() as u32).max(min)
}

/// One frame's inputs, bundled so the row tracer takes one argument.
///
/// `plane_soa` is built ONCE per cut (see `train_tracer`'s outer loop) and shared by
/// every material/preset/frame traced against that same `planes` -- not rebuilt on every
/// sample the way `trace_spectral_ray_with_finish` alone would (that per-sample rebuild
/// is exactly the cost this training workload is meant to profile realistically, so this
/// harness has to avoid paying it artificially many times over).
struct TraceJob<'a> {
    camera: &'a Camera,
    planes: &'a [GpuFacetPlane],
    plane_soa: &'a PlanesSoA32,
    finishes: &'a [FacetFinish],
    material: &'a GemMaterial,
    preset: LightingPreset,
    width: u32,
    height: u32,
    spp: u32,
    max_bounces: u32,
}

/// One traced row: radiance sums plus the primary-hit guide values.
struct Row {
    y: usize,
    accum: Vec<Vec3>,
    depth: Vec<f32>,
    normal: Vec<Vec3>,
    facet: Vec<i32>,
}

struct Frame {
    accum: Vec<Vec3>,
    depth: Vec<f32>,
    normal: Vec<Vec3>,
    facet: Vec<i32>,
}

/// Traces one row with the viewer's own seed/jitter construction.
fn trace_row(job: &TraceJob<'_>, y: usize) -> Row {
    let w = job.width as usize;
    let lighting = job.preset.studio(1.0, 0.85, 0.95);
    let mut row = Row {
        y,
        accum: vec![Vec3::ZERO; w],
        depth: vec![1.0e6f32; w],
        normal: vec![Vec3::ZERO; w],
        facet: vec![-1i32; w],
    };
    for x in 0..job.width {
        let pixel = y as u32 * job.width + x;
        let rot_jx = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM));
        let rot_jy = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM));
        let rot_hero = low_discrepancy_base2(hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM));
        let mut primary = None;
        for sample in 0..job.spp {
            let seed = hash_u32(pixel.wrapping_mul(0x9e37_79b9) ^ sample.wrapping_mul(0x85eb_ca6b));
            let jx = cranley_patterson_rotate(low_discrepancy_base2(sample), rot_jx) - 0.5;
            let jy = cranley_patterson_rotate(radical_inverse_base(sample, 3), rot_jy) - 0.5;
            let hero = cranley_patterson_rotate(radical_inverse_base(sample, 5), rot_hero);
            let ray = job.camera.generate_ray(
                x as f32,
                y as f32,
                job.width as f32,
                job.height as f32,
                jx,
                jy,
            );
            row.accum[x as usize] += trace_spectral_ray_with_finish_soa(
                ray,
                job.planes,
                job.plane_soa,
                job.finishes,
                job.material,
                job.max_bounces,
                lighting,
                seed,
                hero,
                Some(&mut primary),
            );
        }
        if let Some(h) = primary {
            row.depth[x as usize] = h.t;
            row.normal[x as usize] = h.normal;
            row.facet[x as usize] = h.facet_idx as i32;
        }
    }
    row
}

/// Traces every row of the frame, rows handed out through a shared counter.
fn trace_frame(job: &TraceJob<'_>) -> Frame {
    let threads = std::thread::available_parallelism().map_or(8, std::num::NonZero::get);
    let next_row = AtomicUsize::new(0);
    let rows: Vec<Row> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let next_row = &next_row;
                s.spawn(move || {
                    let mut out = Vec::new();
                    loop {
                        let y = next_row.fetch_add(1, Ordering::Relaxed);
                        if y >= job.height as usize {
                            break;
                        }
                        out.push(trace_row(job, y));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("trace thread panicked"))
            .collect()
    });
    let pixels = (job.width * job.height) as usize;
    let w = job.width as usize;
    let mut frame = Frame {
        accum: vec![Vec3::ZERO; pixels],
        depth: vec![1.0e6; pixels],
        normal: vec![Vec3::ZERO; pixels],
        facet: vec![-1; pixels],
    };
    for row in rows {
        let span = row.y * w..(row.y + 1) * w;
        frame.accum[span.clone()].copy_from_slice(&row.accum);
        frame.depth[span.clone()].copy_from_slice(&row.depth);
        frame.normal[span.clone()].copy_from_slice(&row.normal);
        frame.facet[span].copy_from_slice(&row.facet);
    }
    frame
}

/// Every built-in material the tracer is trained against, spanning all three optical
/// characters (isotropic, uniaxial, biaxial), dispersive and near-non-dispersive, and
/// both clean and inclusion-scattering variants -- `bool` marks "trace with a frosted
/// girdle finish".
fn training_materials() -> Vec<(&'static str, GemMaterial, bool)> {
    let by_name = |name: &str| GemMaterial::by_name(name).expect("built-in material");
    vec![
        // Isotropic (cubic): Diamond (highly dispersive), Cubic Zirconia (much less
        // so) -- both take the isotropic Fresnel path in `transport.rs`, with no
        // birefringent mode-coupling code ever entered.
        ("Diamond", GemMaterial::diamond(), false),
        ("Cubic Zirconia", by_name("Cubic Zirconia"), false),
        // Uniaxial +/-: Zircon (positive) and Sapphire/Ruby/Tourmaline (negative) all
        // take `transport.rs`'s anisotropic o<->e re-coupling path.
        ("Zircon", by_name("Zircon"), true),
        ("Tourmaline", by_name("Tourmaline"), false),
        ("Ruby", by_name("Ruby").with_edge_rounding(0.01), false),
        (
            "Sapphire+inclusions",
            by_name("Sapphire").with_scattering_amount(0.4),
            true,
        ),
        // Biaxial +: Alexandrite and Topaz both exercise `BiaxialIndicatrix`'s
        // eigen-polarization/mode-Poynting construction and the mode-A<->mode-B
        // re-coupling variant of the same internal-reflection dispatch uniaxial
        // materials use.
        ("Alexandrite", by_name("Alexandrite"), false),
        ("Topaz", by_name("Topaz"), false),
    ]
}

fn train_tracer(scale: f32) -> f32 {
    let width = scaled(256, scale, 16);
    let height = scaled(192, scale, 12);
    let spp = std::env::var("INDICATRIX_SAMPLES")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or_else(|| scaled(16, scale, 1));
    let max_bounces = scaled(24, scale.max(0.5), 4);

    let cuts = [
        (
            "round brilliant",
            StandardGemCuts::standard_round_brilliant(),
        ),
        ("emerald", StandardGemCuts::emerald_cut()),
    ];
    let materials = training_materials();
    let presets = [LightingPreset::RingLights, LightingPreset::Incandescent];
    let mut denoiser = AtrousDenoiser::new();
    let mut filtered = Vec::new();
    let mut checksum = 0.0f32;
    for (cut_name, planes) in &cuts {
        // Built once per cut (`planes` is fixed for every material/preset/frame traced
        // below), not once per sample -- see `TraceJob::plane_soa`'s own doc comment.
        let plane_soa = build_plane_soa(planes);
        let girdle = girdle_facet_finishes(planes);
        for (name, material, frosted) in &materials {
            let finishes: &[FacetFinish] = if *frosted { &girdle } else { &[] };
            for (i, preset) in presets.iter().enumerate() {
                let pose = i as f32;
                let camera =
                    Camera::new(pose.mul_add(0.3, 0.6), pose.mul_add(-0.2, 0.45), 2.4, 42.0);
                let job = TraceJob {
                    camera: &camera,
                    planes,
                    plane_soa: &plane_soa,
                    finishes,
                    material,
                    preset: *preset,
                    width,
                    height,
                    spp,
                    max_bounces,
                };
                let t0 = Instant::now();
                let frame = trace_frame(&job);
                let avg: Vec<Vec3> = frame.accum.iter().map(|v| *v / spp as f32).collect();
                let g = GBuffers {
                    color: &avg,
                    depth: &frame.depth,
                    normal: &frame.normal,
                    facet_id: &frame.facet,
                    width: width as usize,
                    height: height as usize,
                    spp,
                };
                denoiser.denoise_into(&g, &AtrousParams::default(), &mut filtered);
                // The linear-HDR float buffer -- what a remote worker would send over
                // the wire and what a float/EXR export would encode -- summed BEFORE
                // the 8-bit tonemap below touches it.
                let hdr_sum: f32 = filtered.iter().map(|v| v.x + v.y + v.z).sum();
                let rgba = tonemap_to_rgba(&filtered, 1.0);
                let srgb_sum: f32 = rgba.iter().map(|&b| f32::from(b)).sum();
                checksum = srgb_sum.mul_add(1e-6, hdr_sum.mul_add(1e-6, checksum));
                println!(
                    "  tracer: {cut_name} / {name} / {preset:?}: {:.2?}",
                    t0.elapsed()
                );
            }
        }
    }
    checksum
}

const SCHEDULES: [&str; 3] = [
    "GemCad 5.0\ng 96 0.0\ny 6 y\nI 1.72\nH Train A\n\
     a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
     a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
     a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
     a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
     a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
     a 0.000000 0.44000000 n T\n",
    "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\nH Train B\n\
     a -43.000000 0.70000000 96 n P1 12 24 36 48 60 72 84\n\
     a -41.000000 0.68000000 6 n P2 18 30 42 54 66 78 90\n\
     a -90.000000 1.00000000 96 n G 12 24 36 48 60 72 84\n\
     a -90.000000 1.00000000 6 n G2 18 30 42 54 66 78 90\n\
     a 42.000000 0.72000000 96 n C1 12 24 36 48 60 72 84\n\
     a 27.000000 0.62000000 6 n C2 18 30 42 54 66 78 90\n\
     a 0.000000 0.40000000 n T\n",
    "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.62\nH Train C\n\
     a -45.000000 0.75000000 96 n 1 24 48 72\n\
     a -40.000000 0.70000000 12 n 2 36 60 84\n\
     a -90.000000 1.05000000 96 n G 24 48 72\n\
     a -90.000000 1.05000000 12 n G2 36 60 84\n\
     a 35.000000 0.70000000 96 n 3 24 48 72\n\
     a 20.000000 0.58000000 12 n 4 36 60 84\n\
     a 0.000000 0.42000000 n T\n",
];

/// Printed-proportion targets measured from the schedule's own recorded masts,
/// so the verified repair search has genuine figures to score against.
fn targets_from_recorded_masts(
    schedule: &indicatrix_formats::asc::AscSchedule,
    tiers: &[meet_solver::MeetTierInput],
) -> Option<stone_metrics::ExternalProportions> {
    let normals = meet_solver::tier_instance_normals(schedule.gear_teeth_abs(), tiers);
    let planes: Vec<(glam::DVec3, f64)> = normals
        .iter()
        .zip(&schedule.tiers)
        .flat_map(|(ns, t)| ns.iter().map(move |&n| (n, t.mast)))
        .collect();
    let m = stone_metrics::measure_solid(&planes)?;
    let w = m.width_axis;
    Some(stone_metrics::ExternalProportions {
        vol_w3: Some(m.volume / (w * w * w)),
        lw: Some(m.length_axis / w),
        cw: m.crown_height.map(|c| c / w),
        pw: m.pavilion_depth.map(|p| p / w),
        hw: Some(m.total_height / w),
    })
}

fn train_solver(scale: f32) -> f64 {
    let main_iters = scaled(60, scale, 4);
    let verified_iters = scaled(3, scale, 1);
    let mut checksum = 0.0f64;
    for (i, text) in SCHEDULES.iter().enumerate() {
        let schedule = indicatrix_formats::asc::parse_asc(text).expect("training schedule parses");
        let mut tiers = meet_solver::meet_tier_inputs_from_asc(&schedule);
        // Anchor the first three tiers on their recorded masts, as the validation
        // probe's baseline report does.
        for j in [0usize, 1, 2] {
            tiers[j].constraint =
                meet_solver::MeetConstraint::ScaleReference(schedule.tiers[j].mast);
        }
        let t0 = Instant::now();
        for _ in 0..main_iters {
            let solved = meet_solver::solve_meet_points(schedule.gear_teeth_abs(), &tiers);
            checksum += solved.iter().map(|s| s.mast).sum::<f64>();
        }
        if let Some(targets) = targets_from_recorded_masts(&schedule, &tiers) {
            for _ in 0..verified_iters {
                let (solved, report) = meet_solver::solve_meet_points_verified(
                    schedule.gear_teeth_abs(),
                    &tiers,
                    &targets,
                    &[],
                );
                checksum += solved.iter().map(|s| s.mast).sum::<f64>() + report.final_score;
            }
        }
        println!("  solver: schedule {i}: {:.2?}", t0.elapsed());
    }
    checksum
}

/// `.asc` reader/writer round trip, plus B-Rep solid reconstruction from real
/// (non-fabricated) cutting schedules -- the two `indicatrix-formats`/`geometry::cuts` paths a
/// production tracer never exercises on its own, since it always renders from
/// already-resolved [`GpuFacetPlane`]s, never from a raw schedule.
///
/// # Panics
///
/// If a training schedule fails to reconstruct into a valid solid or fails to
/// round-trip through the `.asc` writer -- both would mean this file's own fixture
/// data (or the reconstruction/writer code it exercises) regressed, either of which
/// this training run should surface loudly rather than silently skip.
fn train_brep() -> f64 {
    let mut checksum = 0.0f64;
    for (i, text) in SCHEDULES.iter().enumerate() {
        let t0 = Instant::now();
        let schedule = indicatrix_formats::asc::parse_asc(text).expect("training schedule parses");

        // Writer round trip: `to_asc_string` -> `parse_asc` must reproduce the same
        // tier count (see `indicatrix_formats::asc::to_asc_string`'s own doc comment for the
        // exact round-trip guarantee this checks).
        let written = indicatrix_formats::asc::to_asc_string(&schedule);
        let reparsed = indicatrix_formats::asc::parse_asc(&written).expect("written .asc reparses");
        assert_eq!(
            reparsed.tiers.len(),
            schedule.tiers.len(),
            "asc writer round trip must preserve tier count"
        );

        // Real (non-fabricated) B-Rep reconstruction: the schedule's own recorded
        // masts, through `from_asc_schedule` -> `GemPolyhedron::from_planes`.
        let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule)
            .expect("training schedule reconstructs into a valid solid");
        let area_sum: f32 = hull.facet_areas().iter().sum();
        checksum += f64::from(hull.volume().mul_add(1e-3, area_sum * 1e-4));

        println!("  brep: schedule {i}: {:.2?}", t0.elapsed());
    }
    checksum
}

/// Tilt-performance sweep: `indicatrix-worker`'s `TILT_CURVES` request and
/// `indicatrix-cut`'s tilt-profile panel both bottom out in
/// [`indicatrix::color::metrics::evaluate_full_axis_profile_at_azimuth`], called once per
/// [`indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG`] entry -- reproduced exactly here.
fn train_tilt(scale: f32) -> f32 {
    use indicatrix::color::metrics::{PROFILE_AZIMUTHS_DEG, evaluate_full_axis_profile_at_azimuth};

    // At very small scale, still cover at least one full axis rather than none.
    let axis_count = scaled(PROFILE_AZIMUTHS_DEG.len() as u32, scale, 1)
        .min(PROFILE_AZIMUTHS_DEG.len() as u32) as usize;

    let planes = StandardGemCuts::standard_round_brilliant();
    let materials = [
        ("Diamond", GemMaterial::diamond()),
        (
            "Alexandrite",
            GemMaterial::by_name("Alexandrite").expect("built-in material"),
        ),
    ];
    let mut checksum = 0.0f32;
    for (name, material) in &materials {
        let t0 = Instant::now();
        for &azimuth_deg in &PROFILE_AZIMUTHS_DEG[..axis_count] {
            let (brilliance, extinction, windowing) =
                evaluate_full_axis_profile_at_azimuth(&planes, material, azimuth_deg, 0.85, 0.95);
            checksum = windowing.iter().sum::<f32>().mul_add(
                1e-4,
                extinction
                    .iter()
                    .sum::<f32>()
                    .mul_add(1e-4, brilliance.iter().sum::<f32>().mul_add(1e-4, checksum)),
            );
        }
        println!(
            "  tilt: {name} ({axis_count} axis/axes): {:.2?}",
            t0.elapsed()
        );
    }
    checksum
}

/// CPU-side GPU chunk orchestration: `renderer::gpu::hybrid::{HybridSplit::calibrated,
/// render_hybrid}`, the same dispatch/readback loop `apps/indicatrix-cut`'s and
/// `apps/indicatrix-worker`'s `gpu` features route progressive/export rendering through.
/// Only compiled and only runs when this crate is built with `--features gpu`; skips
/// itself gracefully (prints and returns `0.0`) when no adapter is present, and can be
/// disabled outright via `PGO_TRAIN_SKIP_GPU=1` (e.g. to keep GPU dispatch code out of
/// a deliberately CPU-only profile even in a `gpu`-feature build). Never trains the
/// WGSL compute shader itself -- LLVM PGO has no visibility into it.
#[cfg(feature = "gpu")]
fn train_gpu(scale: f32) -> f32 {
    use indicatrix::renderer::gpu::{
        GpuFrameRenderer, GpuFrameScene,
        hybrid::{HybridSplit, render_hybrid},
    };

    if std::env::var("PGO_TRAIN_SKIP_GPU").as_deref() == Ok("1") {
        println!("  gpu: skipped (PGO_TRAIN_SKIP_GPU=1)");
        return 0.0;
    }

    let mut renderer = match GpuFrameRenderer::new() {
        Ok(r) => r,
        Err(e) => {
            println!("  gpu: skipped, no adapter available ({e})");
            return 0.0;
        }
    };
    println!("  gpu: adapter = {}", renderer.adapter_label());

    let width = scaled(160, scale, 16);
    let height = scaled(120, scale, 12);
    let total_spp = scaled(32, scale, 4);
    let warmup_spp = scaled(8, scale, 2);
    let max_bounces = scaled(16, scale.max(0.5), 4);

    let camera = Camera::new(0.6, 0.45, 2.4, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces,
        environment,
    };

    let t0 = Instant::now();
    let split = HybridSplit::calibrated(&mut renderer, &scene, total_spp, warmup_spp)
        .expect("gpu-supported material calibrates cleanly");
    let mut accum = vec![Vec3::ZERO; (width * height) as usize];
    let stats = render_hybrid(&mut renderer, &scene, split, &mut accum)
        .expect("hybrid render succeeds against a real adapter");
    println!(
        "  gpu: {width}x{height} spp={total_spp} (gpu={}, cpu={}): {:.2?}",
        stats.gpu_samples,
        stats.cpu_samples,
        t0.elapsed()
    );
    accum.iter().map(|v| v.x + v.y + v.z).sum::<f32>() * 1e-6
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
        "pgo_train done in {:.1?} (checksums tracer={tracer_checksum:.3} brep={brep_checksum:.6} \
         solver={solver_checksum:.6} tilt={tilt_checksum:.3} gpu={gpu_checksum:.3})",
        start.elapsed()
    );
}
