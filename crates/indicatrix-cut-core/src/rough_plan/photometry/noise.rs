//! The sensor noise model: per channel, `variance = a + b * signal`.
//!
//! `a` is the signal-independent part (read noise, quantisation), `b` the shot-noise slope
//! (variance proportional to the signal), both in the pixel units of the frames they were
//! estimated from. The model is estimated from flat patches of the white and dark frames.
//!
//! # Estimator
//!
//! Each frame is cut into non-overlapping blocks. In every block a plane `p + q x + r y` is
//! fitted by least squares (so a gradient of the backlight does not count as noise) and the
//! residual variance with `n - 3` degrees of freedom is the block's variance estimate, paired
//! with the block's mean. Blocks that touch the saturation level are dropped. The pairs are
//! sorted by mean and split into equal-count bins; the **median** variance of each bin is
//! taken (an edge or dust speck inflates a few blocks only), corrected for the median of a
//! chi-squared variable lying below its mean, and a straight line `a + b m` is fitted through
//! the bins' (mean of means, median variance) points.

use super::linear::{LinearImage, PhotometryError};

/// A per-channel noise model, `variance = a + b * max(signal, 0)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseModel {
    /// The signal-independent variance per channel.
    pub a: [f32; 3],
    /// The variance per unit of signal per channel.
    pub b: [f32; 3],
}

impl NoiseModel {
    /// The variance of a measured `signal` in channel `channel`.
    #[must_use]
    pub const fn variance(&self, channel: usize, signal: f32) -> f32 {
        self.b[channel].mul_add(signal.max(0.0), self.a[channel])
    }

    /// The variance of every channel of a pixel.
    #[must_use]
    pub const fn variance_rgb(&self, pixel: [f32; 3]) -> [f32; 3] {
        [
            self.variance(0, pixel[0]),
            self.variance(1, pixel[1]),
            self.variance(2, pixel[2]),
        ]
    }
}

/// Settings of [`estimate_noise`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseOptions {
    /// The side of a block in pixels (default 8).
    pub block: usize,
    /// How many mean-ordered bins the block pairs are split into (default 12).
    pub bins: usize,
    /// Blocks holding a value at or above this fraction of the frame's full scale are dropped
    /// (default 0.95).
    pub saturation_fraction: f32,
}

impl Default for NoiseOptions {
    fn default() -> Self {
        Self {
            block: 8,
            bins: 12,
            saturation_fraction: 0.95,
        }
    }
}

/// The residual variance (after a plane fit) and the mean of one square block.
fn block_statistics(
    frame: &LinearImage,
    channel: usize,
    x0: usize,
    y0: usize,
    side: usize,
    limit: f32,
) -> Option<(f64, f64)> {
    let n = (side * side) as f64;
    let centre = (side as f64 - 1.0) * 0.5;
    let (mut sum, mut sum_x, mut sum_y) = (0.0_f64, 0.0_f64, 0.0_f64);
    for y in 0..side {
        for x in 0..side {
            let v = frame.pixels[(y0 + y) * frame.width + x0 + x][channel];
            if v >= limit {
                return None;
            }
            let v = f64::from(v);
            sum += v;
            sum_x = f64::mul_add(v, x as f64 - centre, sum_x);
            sum_y = f64::mul_add(v, y as f64 - centre, sum_y);
        }
    }
    let mean = sum / n;
    // The centred coordinates are orthogonal to the constant and to each other.
    let spread: f64 = (0..side).map(|i| (i as f64 - centre).powi(2)).sum::<f64>() * side as f64;
    let (slope_x, slope_y) = (sum_x / spread, sum_y / spread);
    let mut residual = 0.0_f64;
    for y in 0..side {
        for x in 0..side {
            let v = f64::from(frame.pixels[(y0 + y) * frame.width + x0 + x][channel]);
            let fit = slope_y.mul_add(y as f64 - centre, slope_x.mul_add(x as f64 - centre, mean));
            residual += (v - fit).powi(2);
        }
    }
    Some((mean, residual / (n - 3.0)))
}

/// The median of an already sorted slice.
const fn median_sorted(values: &[f64]) -> f64 {
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        f64::midpoint(values[n / 2 - 1], values[n / 2])
    }
}

/// Estimates the noise model from flat frames (white and dark frames, any number, all of one
/// view's camera settings).
///
/// # Errors
///
/// [`PhotometryError::NoFrames`] for an empty list; [`PhotometryError::InsufficientData`]
/// when a channel has fewer than four blocks per bin; [`PhotometryError::Invalid`] for a block
/// smaller than 3 pixels or no bins.
pub fn estimate_noise(
    frames: &[&LinearImage],
    options: &NoiseOptions,
) -> Result<NoiseModel, PhotometryError> {
    if frames.is_empty() {
        return Err(PhotometryError::NoFrames);
    }
    if options.block < 3 || options.bins == 0 {
        return Err(PhotometryError::Invalid(
            "noise block must be at least 3 pixels and bins at least 1".to_owned(),
        ));
    }
    let side = options.block;
    let dof = (side * side) as f64 - 3.0;
    // The median of chi-squared(k)/k is about (1 - 2/(9k))^3.
    let median_bias = (1.0 - 2.0 / (9.0 * dof)).powi(3);
    let mut model = NoiseModel {
        a: [0.0; 3],
        b: [0.0; 3],
    };
    for channel in 0..3 {
        let mut pairs: Vec<(f64, f64)> = Vec::new();
        for frame in frames {
            let limit = frame.full_scale * options.saturation_fraction;
            for by in 0..frame.height / side {
                for bx in 0..frame.width / side {
                    if let Some(pair) =
                        block_statistics(frame, channel, bx * side, by * side, side, limit)
                    {
                        pairs.push(pair);
                    }
                }
            }
        }
        if pairs.len() < options.bins * 4 {
            return Err(PhotometryError::InsufficientData(format!(
                "{} flat blocks for {} bins in channel {channel}",
                pairs.len(),
                options.bins
            )));
        }
        pairs.sort_by(|l, r| l.0.total_cmp(&r.0).then(l.1.total_cmp(&r.1)));
        let mut points: Vec<(f64, f64)> = Vec::with_capacity(options.bins);
        for bin in 0..options.bins {
            let start = bin * pairs.len() / options.bins;
            let end = (bin + 1) * pairs.len() / options.bins;
            let group = &pairs[start..end];
            let mean = group.iter().map(|p| p.0).sum::<f64>() / group.len() as f64;
            let mut variances: Vec<f64> = group.iter().map(|p| p.1).collect();
            variances.sort_by(f64::total_cmp);
            points.push((mean, median_sorted(&variances) / median_bias));
        }
        let (a, b) = fit_line(&points);
        model.a[channel] = a as f32;
        model.b[channel] = b as f32;
    }
    Ok(model)
}

/// Least-squares `v = a + b m` with `a, b >= 0`.
fn fit_line(points: &[(f64, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    let mean_m = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_v = points.iter().map(|p| p.1).sum::<f64>() / n;
    let spread: f64 = points.iter().map(|p| (p.0 - mean_m).powi(2)).sum();
    if spread < 1e-18 {
        return (mean_v.max(0.0), 0.0);
    }
    let cross: f64 = points.iter().map(|p| (p.0 - mean_m) * (p.1 - mean_v)).sum();
    let slope = cross / spread;
    let intercept = slope.mul_add(-mean_m, mean_v);
    if slope <= 0.0 {
        return (mean_v.max(0.0), 0.0);
    }
    if intercept < 0.0 {
        // Refit through the origin.
        let through_origin = points.iter().map(|p| p.0 * p.1).sum::<f64>()
            / points.iter().map(|p| p.0 * p.0).sum::<f64>().max(1e-18);
        return (0.0, through_origin.max(0.0));
    }
    (intercept, slope)
}
