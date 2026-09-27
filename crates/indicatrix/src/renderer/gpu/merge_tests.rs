//! Cross-backend merge tests (implementation guide A7): an image whose samples come
//! from two backends over DISJOINT absolute sample ranges, merged the production way --
//! per-pixel sum buffers added together, divided by the summed sample count -- must be
//! indistinguishable from a single-backend render.
//!
//! A sample's identity is `(global pixel index, absolute sample number)` and nothing
//! else, on the CPU ([`cpu_accumulate`]) and the GPU ([`GpuFrameRenderer::accumulate`])
//! alike, both driven here through their real `sample_offset` entry points.
//!
//! # Why the GPU test compares against an INDEPENDENT reference range
//!
//! The merged image is CPU over `[0, N)` plus GPU over `[N, 2N)`. The obvious reference,
//! a CPU render over `[0, 2N)`, is NOT a valid two-sample z-test partner:
//!
//! - its `[0, N)` half is bit-identical to the merged image's CPU half (same backend,
//!   same draws), so that half cancels exactly in `merged - reference`;
//! - its `[N, 2N)` half uses the SAME draws as the GPU half, and the two backends agree
//!   per sample except where a 1-ULP difference flips a stochastic branch, so the
//!   remaining difference is mostly exact zeros plus a few large outliers.
//!
//! The two means are therefore strongly positively correlated, the z-score's
//! independent-samples denominator `sqrt(se_a² + se_b²)` grossly overstates the spread of
//! their difference, and the paired alternative is heavy-tailed rather than normal. The
//! test would be neither calibrated nor powerful. Instead the reference is a CPU render
//! over the disjoint range `[2N, 4N)`: same scene, same sample count, independent draws.
//! Under the null hypothesis (both backends are unbiased estimators of the same pixel
//! value and the merge is exact) the two per-pixel means share an expectation, their
//! difference is a difference of independent means, and `z_stats::z_score` applies as-is.
//! The low-discrepancy jitter and hero-wavelength sequences make the per-pixel estimate
//! LESS variable than the i.i.d. formula assumes, so the test errs conservative, exactly
//! like `estimator_check`'s Tier 3 comparisons that share these criteria.
//!
//! # The test-only second moment
//!
//! Production buffers hold Σ XYZ per pixel and one count per image -- no variance. The
//! z-test needs Σx and Σx² per pixel, so [`MomentAccumulator`] (test-only, this module is
//! `#[cfg(test)]`) drives each backend ONE sample per call and adds each one-sample
//! delta into (a) a production-format f32 Σ XYZ buffer, exactly as production adds a
//! backend's delta, and (b) f64 Σ Y and Σ Y² side buffers. Production accumulation is not
//! touched. The GPU half is additionally rendered in ONE production-shaped dispatch
//! (`spp = N`, `sample_offset = N`) and must match the per-sample Σ, proving the
//! one-sample calls draw exactly the samples a real dispatch would.

use std::{ops::Range, time::Instant};

use glam::Vec3;

use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{BACKDROP_GREY, Camera, LightingPreset},
    },
};

use super::{
    frame::{GpuFrameRenderer, GpuFrameScene},
    hybrid::cpu_accumulate,
    z_stats::{
        NULL_OVER_3_SIGMA_RATE, SIGMA_THRESHOLD, Welford, connected_components, passes_z_criteria,
        z_score,
    },
};

/// Relative tolerance for comparisons that differ only in f32 summation order.
const FOLD_ORDER_TOLERANCE: f32 = 1e-5;

/// The representative scene: a Diamond Standard Round Brilliant under the Light tent
/// preset with the grey backdrop -- the same stone, camera and lighting as
/// `estimator_check::run_image_comparison_light_tent`.
struct DiamondFixture {
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
}

impl DiamondFixture {
    fn new() -> Self {
        Self {
            camera: Camera::new(0.35, 0.28, 5.0, 18.0),
            planes: StandardGemCuts::standard_round_brilliant(),
            material: GemMaterial::by_name("Diamond").expect("Diamond is a built-in material"),
        }
    }

    fn scene(&self, width: u32, height: u32) -> GpuFrameScene<'_> {
        GpuFrameScene {
            camera: &self.camera,
            width,
            height,
            planes: &self.planes,
            facet_finishes: &[],
            material: &self.material,
            max_bounces: 10,
            environment: LightingPreset::LightTent
                .studio(1.0, 0.4, 0.35)
                .with_backdrop(BACKDROP_GREY),
        }
    }
}

/// Test-only per-pixel moment accumulator: the production Σ XYZ buffer plus the f64
/// Σ Y / Σ Y² a z-test needs. See the module doc comment.
#[derive(Clone)]
struct MomentAccumulator {
    /// Production-format running sum, f32, added to exactly as production adds a delta.
    sum: Vec<Vec3>,
    /// Σ Y per pixel in f64 -- only for the variance, never for the reported mean.
    sum_y: Vec<f64>,
    /// Σ Y² per pixel in f64.
    sum_y2: Vec<f64>,
    /// Samples per pixel in `sum` (one count per image, as in production).
    count: u32,
}

impl MomentAccumulator {
    fn new(num_pixels: usize) -> Self {
        Self {
            sum: vec![Vec3::ZERO; num_pixels],
            sum_y: vec![0.0; num_pixels],
            sum_y2: vec![0.0; num_pixels],
            count: 0,
        }
    }

    /// Traces `range` through `backend` (a `(sample_offset, spp, accum)` entry point that
    /// ADDS into `accum`), one absolute sample index per call, so every delta holds
    /// exactly one sample per pixel.
    fn trace(
        num_pixels: usize,
        range: Range<u32>,
        mut backend: impl FnMut(u32, u32, &mut [Vec3]),
    ) -> Self {
        let mut acc = Self::new(num_pixels);
        let mut delta = vec![Vec3::ZERO; num_pixels];
        for sample in range {
            delta.fill(Vec3::ZERO);
            backend(sample, 1, &mut delta);
            acc.add_one_sample_delta(&delta);
        }
        acc
    }

    fn add_one_sample_delta(&mut self, delta: &[Vec3]) {
        let moments = self
            .sum
            .iter_mut()
            .zip(&mut self.sum_y)
            .zip(&mut self.sum_y2);
        for (((sum, sum_y), sum_y2), d) in moments.zip(delta) {
            *sum += *d;
            let y = f64::from(d.y);
            *sum_y += y;
            *sum_y2 = y.mul_add(y, *sum_y2);
        }
        self.count += 1;
    }

    /// The production merge rule: Σ buffers, Σ counts. No weighting, no averaging.
    fn merge(&mut self, other: &Self) {
        for (dst, src) in self.sum.iter_mut().zip(&other.sum) {
            *dst += *src;
        }
        for (dst, src) in self.sum_y.iter_mut().zip(&other.sum_y) {
            *dst += *src;
        }
        for (dst, src) in self.sum_y2.iter_mut().zip(&other.sum_y2) {
            *dst += *src;
        }
        self.count += other.count;
    }

    /// Luminance statistics of one pixel. The mean is what the display shows -- the f32
    /// production Σ divided by the image's count; the spread comes from the f64 moments.
    fn luminance_stats(&self, pixel: usize) -> Welford {
        let n = u64::from(self.count);
        let mean = f64::from(self.sum[pixel].y) / f64::from(self.count);
        Welford::from_moments(n, mean, self.sum_y[pixel], self.sum_y2[pixel])
    }
}

/// Per-pixel z-scores of a candidate image against a reference, plus the summary the
/// shared pass criteria read.
struct MergeComparison {
    over_sigma_count: usize,
    total_pixels: usize,
    cluster_sizes: Vec<usize>,
    max_abs_z: f64,
    max_abs_z_pixel: (u32, u32),
    mean_z: f64,
}

impl MergeComparison {
    fn between(
        reference: &MomentAccumulator,
        candidate: &MomentAccumulator,
        width: u32,
        height: u32,
    ) -> Self {
        let total_pixels = (width * height) as usize;
        let z_grid: Vec<f64> = (0..total_pixels)
            .map(|p| z_score(&reference.luminance_stats(p), &candidate.luminance_stats(p)))
            .collect();
        let (max_idx, max_abs_z) =
            z_grid
                .iter()
                .map(|z| z.abs())
                .enumerate()
                .fold(
                    (0, 0.0f64),
                    |best, (i, z)| if z > best.1 { (i, z) } else { best },
                );
        Self {
            over_sigma_count: z_grid.iter().filter(|z| z.abs() > SIGMA_THRESHOLD).count(),
            total_pixels,
            cluster_sizes: connected_components(&z_grid, width, height, SIGMA_THRESHOLD),
            max_abs_z,
            max_abs_z_pixel: ((max_idx as u32) % width, (max_idx as u32) / width),
            mean_z: z_grid.iter().sum::<f64>() / total_pixels as f64,
        }
    }

    fn passed(&self) -> bool {
        passes_z_criteria(
            self.over_sigma_count,
            self.total_pixels,
            NULL_OVER_3_SIGMA_RATE,
            &self.cluster_sizes,
        )
    }

    fn summary(&self) -> String {
        format!(
            "max |z| {:.3} at {:?}, |z|>3 in {}/{} pixels, largest cluster {}, mean z {:+.4}",
            self.max_abs_z,
            self.max_abs_z_pixel,
            self.over_sigma_count,
            self.total_pixels,
            self.cluster_sizes.first().copied().unwrap_or(0),
            self.mean_z,
        )
    }
}

/// Largest per-pixel difference between two sum buffers, relative to that pixel's
/// largest component magnitude (so a pixel's small channel is judged against its
/// brightest one, not blown up by its own tiny size).
fn max_relative_difference(a: &[Vec3], b: &[Vec3]) -> f32 {
    assert_eq!(a.len(), b.len(), "buffers must cover the same pixels");
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            let scale = a.abs().max_element().max(b.abs().max_element());
            if scale > 0.0 {
                (*a - *b).abs().max_element() / scale
            } else {
                0.0
            }
        })
        .fold(0.0, f32::max)
}

/// CPU-only: two CPU "backends" over `[0, N)` and `[N, 2N)`, merged by adding their
/// buffers, equal one CPU run over `[0, 2N)` up to f32 summation order. Checks
/// [`cpu_accumulate`]'s `sample_offset` is an absolute sample index at the crate level
/// (the app-level live-merge test covers the orchestration around it).
#[test]
fn cpu_accumulate_over_disjoint_ranges_sums_to_the_single_run() {
    let fixture = DiamondFixture::new();
    let (width, height, half) = (32u32, 32u32, 16u32);
    let scene = fixture.scene(width, height);
    let num_pixels = (width * height) as usize;

    let mut single = vec![Vec3::ZERO; num_pixels];
    cpu_accumulate(&scene, 0, 2 * half, &mut single);

    let mut local = vec![Vec3::ZERO; num_pixels];
    cpu_accumulate(&scene, 0, half, &mut local);
    let mut remote = vec![Vec3::ZERO; num_pixels];
    cpu_accumulate(&scene, half, half, &mut remote);
    let merged: Vec<Vec3> = local.iter().zip(&remote).map(|(a, b)| *a + *b).collect();

    assert!(
        single.iter().any(|v| v.y > 0.0),
        "the lit scene must leave some radiance"
    );
    assert!(
        local != remote,
        "disjoint ranges must draw different samples"
    );
    let diff = max_relative_difference(&single, &merged);
    assert!(
        diff <= FOLD_ORDER_TOLERANCE,
        "split CPU render differs from the single run by {diff:e} (relative), above \
         {FOLD_ORDER_TOLERANCE:e}"
    );
}

/// Every test here needs a real `wgpu` adapter; each skips cleanly without one. Run with
/// `cargo test -p indicatrix --features gpu -- --test-threads=1 gpu_hardware_tests`.
mod gpu_hardware_tests {
    use super::{
        DiamondFixture, FOLD_ORDER_TOLERANCE, GpuFrameRenderer, Instant, MergeComparison,
        MomentAccumulator, Vec3, cpu_accumulate, max_relative_difference,
    };

    /// 64x64 pixels, N = 256 samples per backend half.
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;
    const HALF_SPP: u32 = 256;

    /// CPU over `[0, N)` merged with GPU over `[N, 2N)` (Σ buffers ÷ Σ counts) against
    /// an independent CPU reference over `[2N, 4N)` -- see the parent module's doc
    /// comment for why the reference must not overlap the merged range. Also a negative
    /// control: dividing the merged sum by only the CPU's count (a forgotten backend
    /// count) must fail the same criteria.
    #[test]
    fn cpu_gpu_merged_render_matches_an_independent_cpu_reference_statistically() {
        let Ok(mut renderer) = GpuFrameRenderer::new() else {
            println!(
                "skipping cpu_gpu_merged_render_matches_an_independent_cpu_reference_\
                 statistically: no GPU adapter"
            );
            return;
        };
        let started = Instant::now();
        let fixture = DiamondFixture::new();
        let scene = fixture.scene(WIDTH, HEIGHT);
        let num_pixels = (WIDTH * HEIGHT) as usize;
        let n = HALF_SPP;

        let cpu_half = MomentAccumulator::trace(num_pixels, 0..n, |offset, spp, buf| {
            cpu_accumulate(&scene, offset, spp, buf);
        });
        let gpu_half = MomentAccumulator::trace(num_pixels, n..2 * n, |offset, spp, buf| {
            renderer
                .accumulate(&scene, offset, spp, buf)
                .unwrap_or_else(|e| panic!("GPU sample {offset} failed: {e}"));
        });

        // One production-shaped dispatch over the same range must reproduce the
        // one-sample-per-call sum: same draws, only the f32 fold order may differ.
        let mut one_dispatch = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate(&scene, n, n, &mut one_dispatch)
            .expect("one-dispatch GPU render of [N, 2N)");
        let shape_diff = max_relative_difference(&one_dispatch, &gpu_half.sum);
        assert!(
            shape_diff <= FOLD_ORDER_TOLERANCE,
            "GPU [N, 2N) in one dispatch differs from per-sample calls by {shape_diff:e}"
        );

        let mut merged = cpu_half.clone();
        merged.merge(&gpu_half);
        assert_eq!(
            merged.count,
            2 * n,
            "the merged count is the sum of both counts"
        );

        let reference = MomentAccumulator::trace(num_pixels, 2 * n..4 * n, |offset, spp, buf| {
            cpu_accumulate(&scene, offset, spp, buf);
        });

        let result = MergeComparison::between(&reference, &merged, WIDTH, HEIGHT);
        println!(
            "merged CPU[0,{n}) + GPU[{n},{}) vs CPU[{},{}): {} ({:.1} s, one-dispatch \
             shape diff {shape_diff:e})",
            2 * n,
            2 * n,
            4 * n,
            result.summary(),
            started.elapsed().as_secs_f64(),
        );
        assert!(
            result.passed(),
            "merged image is not statistically equivalent to the CPU reference: {}",
            result.summary()
        );

        let mut forgot_gpu_count = merged;
        forgot_gpu_count.count = cpu_half.count;
        let control = MergeComparison::between(&reference, &forgot_gpu_count, WIDTH, HEIGHT);
        println!(
            "negative control (divisor = CPU count only): {}",
            control.summary()
        );
        assert!(
            !control.passed(),
            "the criteria must reject a merge divided by one backend's count: {}",
            control.summary()
        );
    }
}
