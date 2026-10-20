//! Comparison engine for GPU golden capture files.
//!
//! Applies the three-tiered golden rule (§7.2, G-D10):
//! 1. **EXACT:** bitwise equal.
//! 2. **TOL:** otherwise, per pixel and channel, `|a-b| <= 1e-5 * max(|a|, |b|) + 1e-6 * spp`,
//!    allowing at most `max(2, (0.001 * total_pixels) as usize)` pixels to fail.
//! 3. **STAT:** otherwise, run the Tier-3 z-test helpers from `indicatrix::renderer::gpu::z_stats`
//!    (`SIGMA_THRESHOLD = 3.0`, `MAX_CLUSTER_PIXELS = 6`).

use std::{fmt::Write as _, path::Path};

use glam::Vec3;
use indicatrix::renderer::gpu::z_stats::{
    NULL_OVER_3_SIGMA_RATE, SIGMA_THRESHOLD, connected_components, passes_z_criteria,
};

/// Comparison verdict for a scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Exact bitwise identity across all pixels and channels.
    Exact,
    /// Within tolerance formula allowing at most max(2, 0.1% pixels) branch flips.
    Tol,
    /// Passed statistical z-test (Tier 3), no clusters larger than `MAX_CLUSTER_PIXELS`.
    StatPass,
    /// Failed statistical test (clustering or excessive outliers).
    Fail,
}

impl Verdict {
    /// User-facing label in Title Case / uppercase style specified by §7.2.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "EXACT",
            Self::Tol => "TOL",
            Self::StatPass => "STAT-PASS",
            Self::Fail => "FAIL",
        }
    }

    /// Whether this verdict counts as a clean pass without human decision (EXACT or TOL).
    #[must_use]
    pub const fn is_pass(self) -> bool {
        matches!(self, Self::Exact | Self::Tol)
    }
}

/// Detailed outcome of comparing two image buffers for one scene.
#[derive(Debug, Clone)]
pub struct ComparisonResult {
    /// Scene identifier.
    pub scene_name: String,
    /// Verdict according to the three-tiered golden rule.
    pub verdict: Verdict,
    /// Total pixels compared.
    pub total_pixels: usize,
    /// Number of pixels that differed bitwise.
    pub differing_pixels: usize,
    /// Number of pixels that failed the TOL threshold.
    pub tol_failing_pixels: usize,
    /// Maximum absolute component difference across all channels.
    pub max_abs_diff: f32,
    /// Maximum relative component difference across all channels.
    pub max_rel_diff: f32,
    /// Number of pixels with |z| > `SIGMA_THRESHOLD` (for STAT).
    pub stat_over_3_sigma: usize,
    /// Connected component cluster sizes of |z| > `SIGMA_THRESHOLD` pixels (largest first).
    pub stat_cluster_sizes: Vec<usize>,
}

/// Reads a little-endian `w*h*3` float buffer from a `.f32` file on disk.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if the file cannot be read or its size does not match
/// `expected_pixels * 12` bytes.
pub fn read_f32_buffer(path: &Path, expected_pixels: usize) -> std::io::Result<Vec<Vec3>> {
    let bytes = std::fs::read(path)?;
    let expected_bytes = expected_pixels * 3 * std::mem::size_of::<f32>();
    if bytes.len() != expected_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "file {}: expected {} bytes ({} pixels), found {} bytes",
                path.display(),
                expected_bytes,
                expected_pixels,
                bytes.len()
            ),
        ));
    }
    let mut buffer = Vec::with_capacity(expected_pixels);
    for chunk in bytes.as_chunks::<12>().0 {
        let x = f32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let y = f32::from_le_bytes(chunk[4..8].try_into().unwrap());
        let z = f32::from_le_bytes(chunk[8..12].try_into().unwrap());
        buffer.push(Vec3::new(x, y, z));
    }
    Ok(buffer)
}

/// Writes an accumulated `Vec3` buffer as little-endian `w*h*3` floats into `<path>`.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if writing fails.
pub fn write_f32_buffer(path: &Path, buffer: &[Vec3]) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(buffer.len() * 3 * std::mem::size_of::<f32>());
    for v in buffer {
        bytes.extend_from_slice(&v.x.to_le_bytes());
        bytes.extend_from_slice(&v.y.to_le_bytes());
        bytes.extend_from_slice(&v.z.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

/// The bitwise and tolerance counts of two finite buffers: what tiers 1 and 2 of the rule
/// decide on.
struct PixelTally {
    bitwise_exact: bool,
    differing_pixels: usize,
    tol_failing_pixels: usize,
    max_abs_diff: f32,
    max_rel_diff: f32,
}

/// The §7.2 TOL tolerance of one channel: `1e-5 * max(|a|, |b|) + 1e-6 * spp`.
#[expect(
    clippy::suboptimal_flops,
    reason = "the §7.2 rule is specified, and was applied to every recorded golden, as the plain \
              products; a fused multiply-add would move the threshold by an ulp"
)]
fn tolerance(largest_magnitude: f32, spp: u32) -> f32 {
    1e-5 * largest_magnitude + 1e-6 * (spp as f32)
}

/// Counts, over every pixel and channel, the bitwise differences and the ones beyond the
/// TOL tolerance. Both buffers must be finite (checked by the caller), so every comparison
/// here is NaN-safe.
fn tally_pixels(a: &[Vec3], b: &[Vec3], spp: u32) -> PixelTally {
    let mut tally = PixelTally {
        bitwise_exact: true,
        differing_pixels: 0,
        tol_failing_pixels: 0,
        max_abs_diff: 0.0,
        max_rel_diff: 0.0,
    };
    for (va, vb) in a.iter().zip(b) {
        let mut pixel_differs = false;
        let mut pixel_fails_tol = false;
        for (x, y) in va.to_array().into_iter().zip(vb.to_array()) {
            if x.to_bits() == y.to_bits() {
                continue;
            }
            pixel_differs = true;
            let abs_diff = (x - y).abs();
            tally.max_abs_diff = tally.max_abs_diff.max(abs_diff);
            let denom = x.abs().max(y.abs());
            if denom > 0.0 {
                tally.max_rel_diff = tally.max_rel_diff.max(abs_diff / denom);
            }
            if abs_diff > tolerance(denom, spp) {
                pixel_fails_tol = true;
            }
        }
        if pixel_differs {
            tally.bitwise_exact = false;
            tally.differing_pixels += 1;
        }
        if pixel_fails_tol {
            tally.tol_failing_pixels += 1;
        }
    }
    tally
}

/// How many pixels the TOL tier lets fail: `max(2, 0.1 % of the pixels)`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a non-negative fraction of a pixel count, far below `usize::MAX`"
)]
fn allowed_tol_failures(total_pixels: usize) -> usize {
    2.max((0.001 * total_pixels as f64) as usize)
}

/// Sample variance of the luminance (Y per sample) in the 3x3 window around `(x, y)` of
/// `image`, clipped at the image border.
fn window_variance(
    image: &[Vec3],
    (width, height): (usize, usize),
    (x, y): (usize, usize),
    spp: f32,
) -> f64 {
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    let mut count = 0.0_f64;
    for dy in [-1_isize, 0, 1] {
        for dx in [-1_isize, 0, 1] {
            let (Some(nx), Some(ny)) = (x.checked_add_signed(dx), y.checked_add_signed(dy)) else {
                continue;
            };
            if nx < width && ny < height {
                let val = f64::from(image[ny * width + nx].y / spp);
                sum += val;
                sum_sq = val.mul_add(val, sum_sq);
                count += 1.0;
            }
        }
    }
    let mean = sum / count;
    if count > 1.0 {
        ((-(count * mean)).mul_add(mean, sum_sq) / (count - 1.0)).max(0.0)
    } else {
        0.0
    }
}

/// The per-pixel z-scores of tier 3: the luminance difference over the standard error
/// estimated from `a`'s own 3x3 neighbourhood (floored by the TOL tolerance).
fn z_scores(a: &[Vec3], b: &[Vec3], width: u32, height: u32, spp: u32) -> Vec<f64> {
    let dims = (width as usize, height as usize);
    let spp_f32 = spp as f32;
    let mut z_grid = vec![0.0f64; a.len()];
    for y in 0..dims.1 {
        for x in 0..dims.0 {
            let idx = y * dims.0 + x;
            let ya = f64::from(a[idx].y / spp_f32);
            let yb = f64::from(b[idx].y / spp_f32);
            let delta = yb - ya;
            if delta == 0.0 {
                continue;
            }
            let variance = window_variance(a, dims, (x, y), spp_f32);
            let se_sample = (2.0 * variance / f64::from(spp)).sqrt();
            let tol_floor = 1e-5_f64.mul_add(ya.abs().max(yb.abs()), 1e-6);
            z_grid[idx] = delta / se_sample.max(tol_floor);
        }
    }
    z_grid
}

/// A result with no tier-3 statistics (tiers 1 and 2, and the non-finite failure).
fn result_without_stat(
    scene_name: &str,
    verdict: Verdict,
    total_pixels: usize,
    tally: &PixelTally,
) -> ComparisonResult {
    ComparisonResult {
        scene_name: scene_name.to_string(),
        verdict,
        total_pixels,
        differing_pixels: tally.differing_pixels,
        tol_failing_pixels: tally.tol_failing_pixels,
        max_abs_diff: tally.max_abs_diff,
        max_rel_diff: tally.max_rel_diff,
        stat_over_3_sigma: 0,
        stat_cluster_sizes: Vec::new(),
    }
}

/// Compares two sum buffers `a` and `b` according to the §7.2 golden rule.
#[must_use]
pub fn compare_buffers(
    scene_name: &str,
    a: &[Vec3],
    b: &[Vec3],
    width: u32,
    height: u32,
    spp: u32,
) -> ComparisonResult {
    let total_pixels = (width * height) as usize;
    assert_eq!(
        a.len(),
        total_pixels,
        "buffer a length must match width * height"
    );
    assert_eq!(
        b.len(),
        total_pixels,
        "buffer b length must match width * height"
    );

    // NaN/Inf anywhere is a hard FAIL, in either buffer, before any tier is considered:
    // `NaN > tol` is false, so without this a NaN pixel would count as "within TOL".
    let non_finite_pixels = a
        .iter()
        .zip(b)
        .filter(|(va, vb)| !va.is_finite() || !vb.is_finite())
        .count();
    if non_finite_pixels > 0 {
        let tally = PixelTally {
            bitwise_exact: false,
            differing_pixels: non_finite_pixels,
            tol_failing_pixels: non_finite_pixels,
            max_abs_diff: f32::INFINITY,
            max_rel_diff: f32::INFINITY,
        };
        return result_without_stat(scene_name, Verdict::Fail, total_pixels, &tally);
    }

    let tally = tally_pixels(a, b, spp);

    // Tier 1: EXACT
    if tally.bitwise_exact {
        let exact = PixelTally {
            bitwise_exact: true,
            differing_pixels: 0,
            tol_failing_pixels: 0,
            max_abs_diff: 0.0,
            max_rel_diff: 0.0,
        };
        return result_without_stat(scene_name, Verdict::Exact, total_pixels, &exact);
    }

    // Tier 2: TOL
    if tally.tol_failing_pixels <= allowed_tol_failures(total_pixels) {
        return result_without_stat(scene_name, Verdict::Tol, total_pixels, &tally);
    }

    // Tier 3: STAT (Tier-3 z-test helpers from z_stats)
    let z_grid = z_scores(a, b, width, height, spp);
    let over_3_sigma_count = z_grid
        .iter()
        .filter(|&&z| z.abs() > SIGMA_THRESHOLD)
        .count();
    let cluster_sizes = connected_components(&z_grid, width, height, SIGMA_THRESHOLD);
    let passed = passes_z_criteria(
        over_3_sigma_count,
        total_pixels,
        NULL_OVER_3_SIGMA_RATE,
        &cluster_sizes,
    );

    ComparisonResult {
        verdict: if passed {
            Verdict::StatPass
        } else {
            Verdict::Fail
        },
        stat_over_3_sigma: over_3_sigma_count,
        stat_cluster_sizes: cluster_sizes,
        ..result_without_stat(scene_name, Verdict::Fail, total_pixels, &tally)
    }
}

/// Formats a list of comparison results into a user-facing Title Case table.
#[must_use]
pub fn format_comparison_table(results: &[ComparisonResult]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<32} {:<11} {:<15} {:<12} {:<16} {:<16}",
        "Scene", "Verdict", "Diff Pixels", "Tol Fail", "Max Abs Diff", "Max Rel Diff"
    );
    let _ = writeln!(out, "{:-<94}", "");
    for r in results {
        let diff_str = format!("{} / {}", r.differing_pixels, r.total_pixels);
        let _ = writeln!(
            out,
            "{:<32} {:<11} {:<15} {:<12} {:<16.6e} {:<16.6e}",
            r.scene_name,
            r.verdict.as_str(),
            diff_str,
            r.tol_failing_pixels,
            r.max_abs_diff,
            r.max_rel_diff,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(n: usize, v: f32) -> Vec<Vec3> {
        vec![Vec3::splat(v); n]
    }

    #[test]
    fn identical_buffers_are_exact() {
        let a = flat(16, 1.5);
        let r = compare_buffers("t", &a, &a.clone(), 4, 4, 2);
        assert_eq!(r.verdict, Verdict::Exact);
        assert!(r.verdict.is_pass());
    }

    #[test]
    fn one_ulp_difference_is_tol() {
        let a = flat(16, 1.5);
        let mut b = a.clone();
        b[3].x = f32::from_bits(b[3].x.to_bits() + 1);
        let r = compare_buffers("t", &a, &b, 4, 4, 2);
        assert_eq!(r.verdict, Verdict::Tol);
        assert_eq!(r.differing_pixels, 1);
        assert!(r.verdict.is_pass());
    }

    #[test]
    fn large_localized_difference_is_not_a_clean_pass() {
        let a = flat(64, 1.0);
        let mut b = a.clone();
        // More than max(2, 0.1 %) pixels beyond tolerance: leaves the TOL tier.
        for px in b.iter_mut().take(8) {
            *px = Vec3::splat(5.0);
        }
        let r = compare_buffers("t", &a, &b, 8, 8, 2);
        assert!(!r.verdict.is_pass(), "got {:?}", r.verdict);
    }

    #[test]
    fn nan_in_either_buffer_is_fail() {
        let a = flat(16, 1.0);
        let mut b = a.clone();
        b[5].y = f32::NAN;
        assert_eq!(compare_buffers("t", &a, &b, 4, 4, 2).verdict, Verdict::Fail);
        assert_eq!(compare_buffers("t", &b, &a, 4, 4, 2).verdict, Verdict::Fail);
    }

    #[test]
    fn nan_in_both_buffers_at_same_place_is_still_fail() {
        let mut a = flat(16, 1.0);
        a[0].x = f32::NAN;
        let b = a.clone();
        assert_eq!(compare_buffers("t", &a, &b, 4, 4, 2).verdict, Verdict::Fail);
    }

    #[test]
    fn infinity_is_fail() {
        let a = flat(16, 1.0);
        let mut b = a.clone();
        b[15].z = f32::INFINITY;
        let r = compare_buffers("t", &a, &b, 4, 4, 2);
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(!r.verdict.is_pass());
    }
}
