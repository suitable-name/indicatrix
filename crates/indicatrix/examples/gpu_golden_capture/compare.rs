//! Comparison engine for GPU golden capture files.
//!
//! Applies the three-tiered golden rule (§7.2, G-D10):
//! 1. **EXACT:** bitwise equal.
//! 2. **TOL:** otherwise, per pixel and channel, `|a-b| <= 1e-5 * max(|a|, |b|) + 1e-6 * spp`,
//!    allowing at most `max(2, (0.001 * total_pixels) as usize)` pixels to fail.
//! 3. **STAT:** otherwise, run the Tier-3 z-test helpers from `indicatrix::renderer::gpu::z_stats`
//!    (`SIGMA_THRESHOLD = 3.0`, `MAX_CLUSTER_PIXELS = 6`).

use std::path::Path;

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
    /// Passed statistical z-test (Tier 3), no clusters larger than MAX_CLUSTER_PIXELS.
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
    /// Number of pixels with |z| > SIGMA_THRESHOLD (for STAT).
    pub stat_over_3_sigma: usize,
    /// Connected component cluster sizes of |z| > SIGMA_THRESHOLD pixels (largest first).
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
    for chunk in bytes.chunks_exact(12) {
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

    let mut bitwise_exact = true;
    let mut differing_pixels = 0usize;
    let mut tol_failing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    let mut max_rel_diff = 0.0f32;

    for i in 0..total_pixels {
        let va = a[i];
        let vb = b[i];
        let mut pixel_differs = false;
        let mut pixel_fails_tol = false;

        let ca = [va.x, va.y, va.z];
        let cb = [vb.x, vb.y, vb.z];

        for c in 0..3 {
            let x = ca[c];
            let y = cb[c];
            if x.to_bits() != y.to_bits() {
                pixel_differs = true;
                let abs_diff = (x - y).abs();
                max_abs_diff = max_abs_diff.max(abs_diff);
                let denom = x.abs().max(y.abs());
                if denom > 0.0 {
                    max_rel_diff = max_rel_diff.max(abs_diff / denom);
                }
                // §7.2 TOL rule: |a - b| <= 1e-5 * max(|a|, |b|) + 1e-6 * spp
                let tol = 1e-5 * denom + 1e-6 * (spp as f32);
                if abs_diff > tol {
                    pixel_fails_tol = true;
                }
            }
        }

        if pixel_differs {
            bitwise_exact = false;
            differing_pixels += 1;
        }
        if pixel_fails_tol {
            tol_failing_pixels += 1;
        }
    }

    // Tier 1: EXACT
    if bitwise_exact {
        return ComparisonResult {
            scene_name: scene_name.to_string(),
            verdict: Verdict::Exact,
            total_pixels,
            differing_pixels: 0,
            tol_failing_pixels: 0,
            max_abs_diff: 0.0,
            max_rel_diff: 0.0,
            stat_over_3_sigma: 0,
            stat_cluster_sizes: Vec::new(),
        };
    }

    // Tier 2: TOL
    let allowed_tol_fail = 2.max((0.001 * total_pixels as f64) as usize);
    if tol_failing_pixels <= allowed_tol_fail {
        return ComparisonResult {
            scene_name: scene_name.to_string(),
            verdict: Verdict::Tol,
            total_pixels,
            differing_pixels,
            tol_failing_pixels,
            max_abs_diff,
            max_rel_diff,
            stat_over_3_sigma: 0,
            stat_cluster_sizes: Vec::new(),
        };
    }

    // Tier 3: STAT (Tier-3 z-test helpers from z_stats)
    let w = width as usize;
    let h = height as usize;
    let mut z_grid = vec![0.0f64; total_pixels];

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let ya = (a[idx].y / spp as f32) as f64;
            let yb = (b[idx].y / spp as f32) as f64;
            let delta = yb - ya;

            if delta == 0.0 {
                z_grid[idx] = 0.0;
                continue;
            }

            // Estimate local variance in 3x3 window around (x, y)
            let mut sum = 0.0;
            let mut sum_sq = 0.0;
            let mut count = 0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx >= 0 && nx < width as isize && ny >= 0 && ny < height as isize {
                        let val = (a[ny as usize * w + nx as usize].y / spp as f32) as f64;
                        sum += val;
                        sum_sq += val * val;
                        count += 1;
                    }
                }
            }
            let mean = sum / count as f64;
            let variance = if count > 1 {
                ((sum_sq - count as f64 * mean * mean) / (count - 1) as f64).max(0.0)
            } else {
                0.0
            };

            let se_sample = (2.0 * variance / spp as f64).sqrt();
            let tol_floor = 1e-5 * ya.abs().max(yb.abs()) + 1e-6;
            let se = se_sample.max(tol_floor);
            z_grid[idx] = delta / se;
        }
    }

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

    let verdict = if passed {
        Verdict::StatPass
    } else {
        Verdict::Fail
    };

    ComparisonResult {
        scene_name: scene_name.to_string(),
        verdict,
        total_pixels,
        differing_pixels,
        tol_failing_pixels,
        max_abs_diff,
        max_rel_diff,
        stat_over_3_sigma: over_3_sigma_count,
        stat_cluster_sizes: cluster_sizes,
    }
}

/// Formats a list of comparison results into a user-facing Title Case table.
#[must_use]
pub fn format_comparison_table(results: &[ComparisonResult]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<32} {:<11} {:<15} {:<16} {:<16}\n",
        "Scene", "Verdict", "Diff Pixels", "Max Abs Diff", "Max Rel Diff"
    ));
    out.push_str(&format!("{:-<94}\n", ""));
    for r in results {
        let diff_str = format!("{} / {}", r.differing_pixels, r.total_pixels);
        out.push_str(&format!(
            "{:<32} {:<11} {:<15} {:<16.6e} {:<16.6e}\n",
            r.scene_name,
            r.verdict.as_str(),
            diff_str,
            r.max_abs_diff,
            r.max_rel_diff,
        ));
    }
    out
}
