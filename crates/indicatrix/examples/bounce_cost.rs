//! Measures what raising the GUI's max-bounces cap actually costs and buys.
//!
//! The GUI offers a bounce-cap ladder (today: 4 / 8 / 12 / 16 / 24, default 12). This
//! harness answers, with numbers rather than intuition, whether a much higher cap (the
//! question on the table was 1024) is cheap or expensive, and where the image actually
//! stops changing. Three measurements, each done separately per material (a colourless
//! high-RI stone and a lower-RI absorbing one behave very differently here):
//!
//! 1. **Termination histogram.** Every traced path is instrumented (via
//!    [`trace_spectral_ray_with_finish_instrumented`]) with how many bounces it took and
//!    why it stopped -- see [`PathTermination`]. Traced at [`HIST_CAP`] (1024, higher
//!    than the 256 a truncation-free distribution nominally needs) so the histogram is
//!    the path population's genuine, untruncated shape. From that one histogram we read
//!    off what fraction of paths would hit any smaller cap (a path whose natural,
//!    untruncated termination bounce is `>= N` is exactly a path a cap of `N` would have
//!    truncated -- Russian roulette's decision at each bounce does not depend on
//!    `max_bounces`, so this inference is exact, not an approximation).
//! 2. **Wall-time curve.** The SAME scene, rendered with the SAME per-pixel/per-sample
//!    seeds, at every cap in [`CPU_CAP_LIST`], through the production (uninstrumented)
//!    [`trace_spectral_ray_with_finish`] entry point -- so these numbers are exactly
//!    what the GUI would measure, not an instrumented approximation of it.
//! 3. **Convergence / truncation bias.** Every capped render is compared against the
//!    [`HIST_CAP`]-bounce image: mean mean pixel luminance (energy-conservation view --
//!    truncating paths early always loses energy, never gains it, so this only climbs
//!    then plateaus) and mean/max encoded-sRGB difference (the perceptual view: what a
//!    viewer would actually see change).
//!
//! With the `gpu` feature enabled and a real adapter present, also runs the wall-time
//! curve through the production GPU path ([`indicatrix::renderer::gpu::GpuFrameRenderer`])
//! over a smaller cap list ([`GPU_CAP_LIST`]) -- see that list's own doc comment for why
//! it stops short of the CPU list's 512/1024.
//!
//! ```text
//! cargo run --release -p indicatrix --example bounce_cost
//! cargo run --release -p indicatrix --features gpu --example bounce_cost   # + GPU curve
//! ```
//!
//! Prints a plain-text report to stdout; redirect it to a file if you want to keep a
//! copy. No files are read or written.

use glam::Vec3;
use indicatrix::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{
            Camera, FacetFinish, HERO_WAVELENGTH_ROTATION_STREAM, LightingPreset,
            PIXEL_JITTER_X_ROTATION_STREAM, PIXEL_JITTER_Y_ROTATION_STREAM, PathTermination,
            cranley_patterson_rotate, hash_u32, low_discrepancy_base2, radical_inverse_base,
            trace_spectral_ray_with_finish, trace_spectral_ray_with_finish_instrumented,
            xyz_to_srgb_gamma,
        },
    },
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

/// Frame size -- deliberately tiny (this measures a *ratio*, not a picture) so the whole
/// sweep, including the 512/1024-bounce CPU points, finishes in a couple of minutes.
const WIDTH: u32 = 240;
const HEIGHT: u32 = 135;
const SPP: u32 = 64;

/// CPU wall-time-curve caps. Includes the GUI's current ladder (4/8/12/16/24 -- 4 folded
/// in as the low anchor) plus the 1024 the question on the table actually asked about,
/// so that number is measured, not extrapolated.
const CPU_CAP_LIST: [u32; 13] = [4, 8, 12, 16, 24, 32, 48, 64, 96, 128, 256, 512, 1024];

/// GPU wall-time-curve caps. Stops at 256, well short of the CPU list's 512/1024: a
/// workgroup's 64 lanes run in lockstep, so a handful of long paths in one dispatch
/// stall every lane in that group until the slowest one finishes (see this file's
/// closing report section) -- there is a real risk of a single dispatch at 512+ bounces
/// taking drastically longer than its CPU counterpart, and the CPU curve already
/// establishes the underlying per-bounce cost trend without needing to court that.
#[cfg(feature = "gpu")]
const GPU_CAP_LIST: [u32; 11] = [4, 8, 12, 16, 24, 32, 48, 64, 96, 128, 256];

/// The cap the termination histogram is traced at, and the reference image every
/// capped render's convergence bias is measured against. Higher than the 256 a
/// truncation-free histogram nominally needs, so "what fraction of paths hit cap N"
/// is read off a population that is genuinely untruncated even at N=256.
const HIST_CAP: u32 = 1024;

/// Cap values the termination histogram's survival fractions are reported at.
const HIST_THRESHOLDS: [u32; 7] = [8, 12, 16, 24, 32, 64, 128];

/// One fixed scene: standard round brilliant, a single camera pose, ring-light studio
/// environment. Held constant across every material/cap combination so only
/// `max_bounces` and the material itself ever change.
struct Scene {
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
}

impl Scene {
    fn standard() -> Self {
        Self {
            camera: Camera::new(0.6, 0.45, 2.4, 42.0),
            planes: StandardGemCuts::standard_round_brilliant(),
        }
    }
}

/// One trace job's fixed inputs, bundled so the row tracers below take one argument
/// each (clippy's argument-count limit).
struct TraceJob<'a> {
    camera: &'a Camera,
    planes: &'a [GpuFacetPlane],
    material: &'a GemMaterial,
    width: u32,
    height: u32,
    spp: u32,
    max_bounces: u32,
}

/// The one fixed lighting rig every render in this harness uses.
const fn studio_environment() -> indicatrix::optics::raytracer::EnvironmentSource<'static> {
    LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
}

impl TraceJob<'_> {
    fn ray_for(&self, x: u32, y: u32, jx: f32, jy: f32) -> indicatrix::optics::raytracer::Ray {
        self.camera.generate_ray(
            x as f32,
            y as f32,
            self.width as f32,
            self.height as f32,
            jx,
            jy,
        )
    }
}

/// Per-bounce-count and per-reason tallies. Indexed `0..=HIST_CAP`; bucket `i` holds the
/// count of paths whose termination bounce (see [`PathTermination`]'s doc comment for
/// exactly what that counts) was `i`. Bucket `HIST_CAP` therefore holds every path that
/// never terminated before the cap -- [`PathTermination::HitCap`] at this cap.
#[derive(Clone)]
struct TerminationStats {
    bounce_counts: Vec<u64>,
    reason_counts: [u64; 4],
}

impl TerminationStats {
    fn new(cap: u32) -> Self {
        Self {
            bounce_counts: vec![0u64; cap as usize + 1],
            reason_counts: [0; 4],
        }
    }

    fn record(&mut self, bounces: u32, reason: PathTermination) {
        let idx = (bounces as usize).min(self.bounce_counts.len() - 1);
        self.bounce_counts[idx] += 1;
        self.reason_counts[reason_index(reason)] += 1;
    }

    fn merge(&mut self, other: &Self) {
        for (a, b) in self.bounce_counts.iter_mut().zip(&other.bounce_counts) {
            *a += b;
        }
        for (a, b) in self.reason_counts.iter_mut().zip(&other.reason_counts) {
            *a += b;
        }
    }

    fn total(&self) -> u64 {
        self.reason_counts.iter().sum()
    }

    /// Fraction of paths whose untruncated termination bounce is `>= cap` -- exactly
    /// the population a real `max_bounces = cap` render would have cut off.
    fn fraction_hitting_cap(&self, cap: u32) -> f64 {
        let cap = (cap as usize).min(self.bounce_counts.len() - 1);
        let hit: u64 = self.bounce_counts[cap..].iter().sum();
        hit as f64 / self.total() as f64
    }

    /// (mean, median, p95, max) bounce count.
    fn summary(&self) -> (f64, u32, u32, u32) {
        let total = self.total();
        let mean = self
            .bounce_counts
            .iter()
            .enumerate()
            .map(|(i, &c)| i as f64 * c as f64)
            .sum::<f64>()
            / total as f64;
        let median = self.percentile(total, 0.5);
        let p95 = self.percentile(total, 0.95);
        let max = self.bounce_counts.iter().rposition(|&c| c > 0).unwrap_or(0) as u32;
        (mean, median, p95, max)
    }

    fn percentile(&self, total: u64, p: f64) -> u32 {
        let target = ((p * total as f64).ceil() as u64).max(1);
        let mut cumulative = 0u64;
        for (i, &c) in self.bounce_counts.iter().enumerate() {
            cumulative += c;
            if cumulative >= target {
                return i as u32;
            }
        }
        (self.bounce_counts.len() - 1) as u32
    }

    /// A view of these stats excluding paths that never even reached the gem: a camera
    /// ray whose very first `intersect_polyhedron_soa` call misses (background/sky
    /// pixels in a framed scene, or the margin around the stone). Bucket 0 can ONLY ever
    /// be [`PathTermination::Escaped`] -- at that point in `trace_spectral_ray_inner`'s
    /// loop no bounce dispatch has run yet, so nothing else can have happened -- which is
    /// what makes zeroing it out both in `bounce_counts` and in `reason_counts` exact,
    /// not an approximation. Background misses are cap-invariant (a `max_bounces` change
    /// can never affect whether the FIRST ray hits the polyhedron), so leaving them in
    /// would dilute every fraction/percentile below with samples the cap question has
    /// nothing to say about.
    fn interior_only(&self) -> Self {
        let mut out = self.clone();
        let misses = out.bounce_counts[0];
        out.bounce_counts[0] = 0;
        out.reason_counts[reason_index(PathTermination::Escaped)] -= misses;
        out
    }
}

const fn reason_index(reason: PathTermination) -> usize {
    match reason {
        PathTermination::Escaped => 0,
        PathTermination::ScatterAbsorbed => 1,
        PathTermination::RussianRoulette => 2,
        PathTermination::HitCap => 3,
    }
}

const REASON_LABELS: [&str; 4] = [
    "escaped to environment",
    "scatter-absorbed",
    "killed by Russian roulette",
    "hit the bounce cap",
];

/// Every seed/jitter draw a pixel needs, computed once per pixel and reused across its
/// `spp` samples -- the same stratified construction `pgo_train`'s `trace_row` and the
/// real viewer/exporter use.
#[expect(
    clippy::struct_field_names,
    reason = "the three fields share a _rotation suffix on purpose: jitter-x, jitter-y \
              and hero-wavelength are three independent Cranley-Patterson rotations of \
              their own low-discrepancy sequence, and unrelated names would read as \
              less clear, not more"
)]
struct PixelRng {
    jx_rotation: f32,
    jy_rotation: f32,
    hero_rotation: f32,
}

fn pixel_rng(pixel: u32) -> PixelRng {
    PixelRng {
        jx_rotation: low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM)),
        jy_rotation: low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM)),
        hero_rotation: low_discrepancy_base2(hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM)),
    }
}

/// One sample's seed, jitter and hero draw, stratified exactly like the real render
/// loop.
struct SampleDraw {
    seed: u32,
    jx: f32,
    jy: f32,
    hero: f32,
}

fn sample_draw(pixel: u32, sample: u32, rng: &PixelRng) -> SampleDraw {
    SampleDraw {
        seed: hash_u32(pixel.wrapping_mul(0x9e37_79b9) ^ sample.wrapping_mul(0x85eb_ca6b)),
        jx: cranley_patterson_rotate(low_discrepancy_base2(sample), rng.jx_rotation) - 0.5,
        jy: cranley_patterson_rotate(radical_inverse_base(sample, 3), rng.jy_rotation) - 0.5,
        hero: cranley_patterson_rotate(radical_inverse_base(sample, 5), rng.hero_rotation),
    }
}

/// Traces a full frame through the production (uninstrumented) entry point -- exactly
/// what the GUI's own render loop calls, so these are the real wall-time numbers.
fn trace_frame_plain(job: &TraceJob<'_>) -> Vec<Vec3> {
    let environment = studio_environment();
    let threads = std::thread::available_parallelism().map_or(8, std::num::NonZero::get);
    let next_row = AtomicUsize::new(0);
    let rows: Vec<(usize, Vec<Vec3>)> = std::thread::scope(|s| {
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
                        let mut row = vec![Vec3::ZERO; job.width as usize];
                        for x in 0..job.width {
                            let pixel = y as u32 * job.width + x;
                            let rng = pixel_rng(pixel);
                            for sample in 0..job.spp {
                                let draw = sample_draw(pixel, sample, &rng);
                                let ray = job.ray_for(x, y as u32, draw.jx, draw.jy);
                                row[x as usize] += trace_spectral_ray_with_finish(
                                    ray,
                                    job.planes,
                                    &[] as &[FacetFinish],
                                    job.material,
                                    job.max_bounces,
                                    environment,
                                    draw.seed,
                                    draw.hero,
                                    None,
                                );
                            }
                        }
                        out.push((y, row));
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
    assemble_rows(rows, job.width, job.height)
}

/// Traces a full frame through [`trace_spectral_ray_with_finish_instrumented`],
/// returning both the averaged image and the merged per-path termination statistics.
fn trace_frame_instrumented(job: &TraceJob<'_>) -> (Vec<Vec3>, TerminationStats) {
    let environment = studio_environment();
    let threads = std::thread::available_parallelism().map_or(8, std::num::NonZero::get);
    let next_row = AtomicUsize::new(0);
    let results: Vec<(usize, Vec<Vec3>, TerminationStats)> = std::thread::scope(|s| {
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
                        let mut row = vec![Vec3::ZERO; job.width as usize];
                        let mut stats = TerminationStats::new(job.max_bounces);
                        for x in 0..job.width {
                            let pixel = y as u32 * job.width + x;
                            let rng = pixel_rng(pixel);
                            for sample in 0..job.spp {
                                let draw = sample_draw(pixel, sample, &rng);
                                let ray = job.ray_for(x, y as u32, draw.jx, draw.jy);
                                let (radiance, bounces, reason) =
                                    trace_spectral_ray_with_finish_instrumented(
                                        ray,
                                        job.planes,
                                        &[] as &[FacetFinish],
                                        job.material,
                                        job.max_bounces,
                                        environment,
                                        draw.seed,
                                        draw.hero,
                                    );
                                row[x as usize] += radiance;
                                stats.record(bounces, reason);
                            }
                        }
                        out.push((y, row, stats));
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
    let mut stats = TerminationStats::new(job.max_bounces);
    let mut rows = Vec::with_capacity(results.len());
    for (y, row, row_stats) in results {
        stats.merge(&row_stats);
        rows.push((y, row));
    }
    (assemble_rows(rows, job.width, job.height), stats)
}

fn assemble_rows(rows: Vec<(usize, Vec<Vec3>)>, width: u32, height: u32) -> Vec<Vec3> {
    let mut frame = vec![Vec3::ZERO; (width * height) as usize];
    let w = width as usize;
    for (y, row) in rows {
        frame[y * w..(y + 1) * w].copy_from_slice(&row);
    }
    frame
}

fn average(accum: &[Vec3], spp: u32) -> Vec<Vec3> {
    accum.iter().map(|v| *v / spp as f32).collect()
}

/// Mean pixel luminance (CIE `Y`, linear -- no tonemap/gamma), the honest
/// energy-conservation view: truncating paths early can only lose energy, so this
/// climbs monotonically toward the high-cap reference as the cap rises, and where it
/// plateaus is the point beyond which more bounces buy nothing.
fn mean_luminance(image: &[Vec3]) -> f32 {
    image.iter().map(|v| v.y).sum::<f32>() / image.len() as f32
}

/// Mean absolute and max absolute per-channel difference between two images, in encoded
/// 8-bit sRGB (via the same [`xyz_to_srgb_gamma`] the real export path uses) -- the
/// perceptual view: what a viewer would actually see change between `image` and
/// `reference`.
fn srgb_diff(image: &[Vec3], reference: &[Vec3]) -> (f64, u8) {
    let mut sum_abs = 0u64;
    let mut count = 0u64;
    let mut max_abs = 0u8;
    for (&a, &b) in image.iter().zip(reference) {
        let pa = xyz_to_srgb_gamma(a);
        let pb = xyz_to_srgb_gamma(b);
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            sum_abs += u64::from(d);
            count += 1;
            max_abs = max_abs.max(d);
        }
    }
    (sum_abs as f64 / count as f64, max_abs)
}

fn fmt_duration(d: Duration) -> String {
    format!("{:>9.1} ms", d.as_secs_f64() * 1000.0)
}

/// Runs the full measurement suite (histogram, wall-time curve, convergence bias) for
/// one material.
fn run_material(name: &str, material: &GemMaterial, scene: &Scene) {
    println!();
    println!("=================================================================");
    println!(
        "  {name}  (crystal_system={:?}, birefringence_delta={:.4}, scattering_sigma_s={:.3})",
        material.crystal_system, material.birefringence_delta, material.scattering_sigma_s
    );
    println!("=================================================================");

    let hist_job = TraceJob {
        camera: &scene.camera,
        planes: &scene.planes,
        material,
        width: WIDTH,
        height: HEIGHT,
        spp: SPP,
        max_bounces: HIST_CAP,
    };
    println!(
        "\n[1] termination histogram -- {} paths traced at max_bounces={HIST_CAP} (untruncated \
         reference population)",
        u64::from(WIDTH) * u64::from(HEIGHT) * u64::from(SPP)
    );
    let hist_t0 = Instant::now();
    let (reference_image, stats) = trace_frame_instrumented(&hist_job);
    println!("    (traced in {})", fmt_duration(hist_t0.elapsed()));
    let reference_avg = average(&reference_image, SPP);

    let total = stats.total();
    let background = stats.bounce_counts[0];
    println!(
        "    {background} of {total} traced samples ({:.2}%) never hit the gem at all \
         (background/margin around the stone) -- excluded from everything below, since a \
         bounce cap has nothing to say about a ray that misses on its very first test.",
        100.0 * background as f64 / total as f64
    );
    let interior = stats.interior_only();
    let interior_total = interior.total();
    println!("    of the {interior_total} paths that DID enter the gem, termination reasons:");
    for (label, &count) in REASON_LABELS.iter().zip(&interior.reason_counts) {
        let pct = 100.0 * count as f64 / interior_total as f64;
        println!("      {label:<28} {count:>10} paths  ({pct:>6.2}%)");
    }
    let (mean_b, median_b, p95_b, max_b) = interior.summary();
    println!("    bounce count: mean={mean_b:.2}  median={median_b}  p95={p95_b}  max={max_b}");
    println!("    fraction of interior paths that would hit a cap of:");
    for cap in HIST_THRESHOLDS {
        println!(
            "      {cap:>4}: {:>6.2}%",
            100.0 * interior.fraction_hitting_cap(cap)
        );
    }
    println!(
        "      {HIST_CAP:>4}: {:>6.2}%  (== the 'hit the bounce cap' row above, by construction)",
        100.0 * interior.fraction_hitting_cap(HIST_CAP)
    );

    println!(
        "\n[2] CPU wall-time curve + [3] convergence bias vs. the max_bounces={HIST_CAP} reference"
    );
    println!(
        "    {:>6}  {:>12}  {:>9}  {:>12}  {:>10}  {:>9}",
        "cap", "wall time", "x vs 12", "mean lum.", "% of ref", "sRGB MAD/max"
    );
    let reference_luminance = f64::from(mean_luminance(&reference_avg));
    let mut baseline_12_ms: Option<f64> = None;
    for &cap in &CPU_CAP_LIST {
        let job = TraceJob {
            camera: &scene.camera,
            planes: &scene.planes,
            material,
            width: WIDTH,
            height: HEIGHT,
            spp: SPP,
            max_bounces: cap,
        };
        let t0 = Instant::now();
        let image = trace_frame_plain(&job);
        let elapsed = t0.elapsed();
        let ms = elapsed.as_secs_f64() * 1000.0;
        if cap == 12 {
            baseline_12_ms = Some(ms);
        }
        let avg = average(&image, SPP);
        let luminance = f64::from(mean_luminance(&avg));
        let (mad, max_diff) = srgb_diff(&avg, &reference_avg);
        let x_vs_12 = baseline_12_ms.map_or(f64::NAN, |b| ms / b);
        println!(
            "    {cap:>6}  {}  {:>8.3}x  {luminance:>12.5}  {:>9.2}%  {mad:>5.2} / {max_diff:>3}",
            fmt_duration(elapsed),
            x_vs_12,
            100.0 * luminance / reference_luminance,
        );
    }

    #[cfg(feature = "gpu")]
    run_gpu_curve(name, material, scene);
}

#[cfg(feature = "gpu")]
fn run_gpu_curve(name: &str, material: &GemMaterial, scene: &Scene) {
    use indicatrix::renderer::gpu::{GpuFrameError, GpuFrameRenderer, GpuFrameScene};

    println!("\n[GPU] production megakernel wall-time curve");
    let mut renderer = match GpuFrameRenderer::new() {
        Ok(r) => r,
        Err(e) => {
            println!("    could not acquire a GPU adapter/device ({e}) -- skipping for {name}.");
            return;
        }
    };
    println!("    adapter: {}", renderer.adapter_label());
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let mut baseline_12_ms: Option<f64> = None;
    for &cap in &GPU_CAP_LIST {
        let scene_desc = GpuFrameScene {
            camera: &scene.camera,
            width: WIDTH,
            height: HEIGHT,
            planes: &scene.planes,
            facet_finishes: &[],
            material,
            max_bounces: cap,
            environment,
        };
        let mut accum = vec![Vec3::ZERO; (WIDTH * HEIGHT) as usize];
        let t0 = Instant::now();
        let result: Result<(), GpuFrameError> =
            renderer.accumulate(&scene_desc, 0, SPP, &mut accum);
        let elapsed = t0.elapsed();
        match result {
            Ok(()) => {
                let ms = elapsed.as_secs_f64() * 1000.0;
                if cap == 12 {
                    baseline_12_ms = Some(ms);
                }
                let x_vs_12 = baseline_12_ms.map_or(f64::NAN, |b| ms / b);
                println!("    {cap:>6}  {}  {:>8.3}x", fmt_duration(elapsed), x_vs_12);
            }
            Err(e) => println!("    {cap:>6}  GPU dispatch error: {e}"),
        }
    }
}

fn main() {
    println!("bounce_cost: {WIDTH}x{HEIGHT} @ {SPP} spp, standard round brilliant");
    println!(
        "gpu feature: {}",
        if cfg!(feature = "gpu") {
            "on"
        } else {
            "off (CPU-only run)"
        }
    );
    let scene = Scene::standard();

    let diamond = GemMaterial::diamond();
    run_material(
        "Diamond (RI~2.417, colourless, cubic/isotropic)",
        &diamond,
        &scene,
    );

    let quartz = GemMaterial::by_name("Quartz").expect("Quartz must be a built-in material");
    run_material(
        "Quartz (RI~1.544, weakly absorbing, uniaxial)",
        &quartz,
        &scene,
    );

    // Zircon is this crate's most strongly birefringent built-in
    // (`birefringence_delta = +0.0590`, more than 4x Quartz's +0.0130) and has a much
    // higher RI (~1.93 vs Quartz's ~1.54), so it drives far more internal TIR/reflect
    // bounces through the uniaxial closed-form path per sample -- reported alongside
    // Diamond/Quartz to show the batched/cached uniaxial solve's cost at the
    // high-birefringence end, not just Quartz's comparatively mild case.
    let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
    run_material(
        "Zircon (RI~1.93, colourless, strongly birefringent uniaxial)",
        &zircon,
        &scene,
    );

    println!();
    println!("bounce_cost: done.");
}
