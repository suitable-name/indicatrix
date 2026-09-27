//! Per-pixel z-score statistics shared by every statistical image comparison in this
//! crate: `estimator_check`'s Tier 3 CPU-vs-GPU checks and the cross-backend merge test
//! (`renderer::gpu::merge_tests`).
//!
//! One definition of the pass criteria, not one per caller: a pixel "fails" when its
//! two-sample z-score exceeds [`SIGMA_THRESHOLD`], and an image passes when the failing
//! count stays within a generous multiple of its binomial expectation AND no
//! 4-connected cluster of failing pixels is larger than [`MAX_CLUSTER_PIXELS`] (a
//! structured bias clusters; noise salts-and-peppers).

/// Per-pixel `|z|` above which a pixel counts as failing.
pub const SIGMA_THRESHOLD: f64 = 3.0;

/// Two-sided probability of `|z| > 3` under the null hypothesis (standard normal): ~0.27%.
pub const NULL_OVER_3_SIGMA_RATE: f64 = 0.0027;

/// Largest 4-connected cluster of failing pixels an image may contain and still pass.
pub const MAX_CLUSTER_PIXELS: usize = 6;

/// Welford online mean/M2 in f64 for accumulator stability (an analysis tool applied
/// after the f32 estimator produced its samples).
#[derive(Debug, Clone, Copy, Default)]
pub struct Welford {
    /// Samples seen.
    pub n: u64,
    /// Running mean.
    pub mean: f64,
    /// Running sum of squared deviations from the mean.
    pub m2: f64,
}

impl Welford {
    /// Adds one sample.
    pub fn update(&mut self, x: f64) {
        self.n += 1;
        let delta = x - self.mean;
        self.mean += delta / self.n as f64;
        let delta2 = x - self.mean;
        self.m2 = delta.mul_add(delta2, self.m2);
    }

    /// Builds the statistics of `n` samples from their raw moments instead of one
    /// sample at a time: `sum` is `Σx`, `sum_sq` is `Σx²`, and `mean` is the mean the
    /// caller reports (normally `sum / n`, but a caller may supply the mean its own
    /// production buffer yields). `m2 = Σ(x - mean)²`, expanded, clamped at zero
    /// against rounding.
    #[cfg(test)]
    pub fn from_moments(n: u64, mean: f64, sum: f64, sum_sq: f64) -> Self {
        let n_f = n as f64;
        let m2 = (n_f * mean).mul_add(mean, 2.0f64.mul_add(-mean * sum, sum_sq));
        Self {
            n,
            mean,
            m2: m2.max(0.0),
        }
    }

    /// Unbiased sample variance; `0.0` below two samples.
    #[must_use]
    pub fn variance(&self) -> f64 {
        if self.n < 2 {
            0.0
        } else {
            self.m2 / (self.n as f64 - 1.0)
        }
    }

    /// Squared standard error of the mean; `0.0` with no samples.
    #[must_use]
    pub fn standard_error_sq(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.variance() / self.n as f64
        }
    }
}

/// Two-sample z-score of `b`'s mean against `a`'s, assuming the two sample sets are
/// independent (disjoint sample ranges). `0.0` when both standard errors vanish.
pub fn z_score(a: &Welford, b: &Welford) -> f64 {
    let se2 = a.standard_error_sq() + b.standard_error_sq();
    if se2 <= 0.0 {
        return 0.0;
    }
    (b.mean - a.mean) / se2.sqrt()
}

/// The shared pass criteria: `over_sigma_count <= max(5 * expected_rate * total_pixels,
/// 10)` AND the largest cluster (`cluster_sizes` is sorted largest first, as
/// [`connected_components`] returns it) is at most [`MAX_CLUSTER_PIXELS`].
pub fn passes_z_criteria(
    over_sigma_count: usize,
    total_pixels: usize,
    expected_rate: f64,
    cluster_sizes: &[usize],
) -> bool {
    let expected_count = expected_rate * total_pixels as f64;
    let observed_ok = (over_sigma_count as f64) <= (expected_count * 5.0).max(10.0);
    let largest = cluster_sizes.first().copied().unwrap_or(0);
    observed_ok && largest <= MAX_CLUSTER_PIXELS
}

/// 4-connectivity connected-component sizes of `|z| > threshold` pixels in a `width x
/// height` grid, largest first. Standard flood fill -- this matters more than a bare
/// failing-pixel count: a connected region (one facet, a grazing-angle band, one branch
/// of the physics) is what a real porting bug leaves behind, while independent
/// per-sample noise crossing the 3-sigma threshold lands as scattered singletons.
pub fn connected_components(z_grid: &[f64], width: u32, height: u32, threshold: f64) -> Vec<usize> {
    let w = width as usize;
    let h = height as usize;
    let mut visited = vec![false; z_grid.len()];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..z_grid.len() {
        if visited[start] || z_grid[start].abs() <= threshold {
            continue;
        }
        stack.push(start);
        visited[start] = true;
        let mut size = 0usize;
        while let Some(idx) = stack.pop() {
            size += 1;
            let x = idx % w;
            let y = idx / w;
            let neighbors = [
                (x.checked_sub(1), Some(y)),
                (Some(x + 1).filter(|&nx| nx < w), Some(y)),
                (Some(x), y.checked_sub(1)),
                (Some(x), Some(y + 1).filter(|&ny| ny < h)),
            ];
            for (nx, ny) in neighbors {
                if let (Some(nx), Some(ny)) = (nx, ny) {
                    let nidx = ny * w + nx;
                    if !visited[nidx] && z_grid[nidx].abs() > threshold {
                        visited[nidx] = true;
                        stack.push(nidx);
                    }
                }
            }
        }
        sizes.push(size);
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes
}
