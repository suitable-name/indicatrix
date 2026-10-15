//! Reading the texels of a decoded map and box-filtering them down to at most
//! [`MAX_WIDTH`] x [`MAX_HEIGHT`].
//!
//! `EnvironmentMap` keeps its texels private, so [`read_texels`] reads each one back at its
//! own centre through the map's bilinear lookup (a texel centre returns that texel, up to a
//! float rounding of the direction round trip that is far below one percent). The box
//! filter weights the source texels by their exact overlap with each target texel, so the
//! mean radiance of the image is kept whatever the size ratio.

use indicatrix::renderer::env_map::EnvironmentMap;

/// The widest map kept, in texels.
pub const MAX_WIDTH: usize = 2048;
/// The tallest map kept, in texels.
pub const MAX_HEIGHT: usize = 1024;

/// The size a `width x height` source is kept at: unchanged when it fits, otherwise scaled
/// down by one factor (aspect ratio kept) until it fits [`MAX_WIDTH`] x [`MAX_HEIGHT`].
/// Never zero.
#[must_use]
pub fn target_size(width: usize, height: usize) -> (usize, usize) {
    let (width, height) = (width.max(1), height.max(1));
    if width <= MAX_WIDTH && height <= MAX_HEIGHT {
        return (width, height);
    }
    let scale = (MAX_WIDTH as f64 / width as f64).min(MAX_HEIGHT as f64 / height as f64);
    let fit = |size: usize, limit: usize| to_index((size as f64 * scale).round()).clamp(1, limit);
    (fit(width, MAX_WIDTH), fit(height, MAX_HEIGHT))
}

/// A non-negative, already rounded `f64` as an index. `as` saturates (and maps NaN to 0).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers pass floor/ceil/round of values in 0..=source size; `as` saturates"
)]
const fn to_index(value: f64) -> usize {
    value as usize
}

/// A box-filtered texel at the stored f32 precision.
#[expect(
    clippy::cast_possible_truncation,
    reason = "texels are stored as f32; filtered radiance is in the normal float range"
)]
const fn narrow_texel(sum: [f64; 3]) -> [f32; 3] {
    [sum[0] as f32, sum[1] as f32, sum[2] as f32]
}

/// For each of `dst` target indices, the source indices it covers with their weights (the
/// overlap with the target's span, normalized to sum to one).
fn axis_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f64)>> {
    let ratio = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            let start = i as f64 * ratio;
            let end = (i + 1) as f64 * ratio;
            let first = to_index(start.floor());
            let last = to_index(end.ceil()).min(src);
            let mut taps: Vec<(usize, f64)> = (first..last)
                .map(|j| {
                    let overlap = end.min((j + 1) as f64) - start.max(j as f64);
                    (j, overlap.max(0.0))
                })
                .filter(|&(_, weight)| weight > 0.0)
                .collect();
            let total: f64 = taps.iter().map(|&(_, weight)| weight).sum();
            if total > 0.0 {
                for tap in &mut taps {
                    tap.1 /= total;
                }
            } else {
                taps = vec![(first.min(src - 1), 1.0)];
            }
            taps
        })
        .collect()
}

/// Reads row `y` of `map`: one texel per column, at the texel centres.
fn read_row(map: &EnvironmentMap, y: usize, out: &mut Vec<[f32; 3]>) {
    let (width, height) = (map.width(), map.height());
    let v = (y as f32 + 0.5) / height as f32;
    out.clear();
    out.extend((0..width).map(|x| {
        let u = (x as f32 + 0.5) / width as f32;
        map.radiance_rgb(EnvironmentMap::uv_to_direction(u, v))
    }));
}

/// All texels of `map`, row-major (the tests compare it with the resampled copy).
#[cfg(test)]
#[must_use]
pub fn read_texels(map: &EnvironmentMap) -> Vec<[f32; 3]> {
    let mut texels = Vec::with_capacity(map.width() * map.height());
    let mut row = Vec::new();
    for y in 0..map.height() {
        read_row(map, y, &mut row);
        texels.extend_from_slice(&row);
    }
    texels
}

/// `map` box-filtered to `width x height` (each at least 1), row-major. Reads the source one
/// row at a time and filters horizontally on the fly, so only a `source rows x width` strip
/// is held besides the source.
#[must_use]
pub fn resample_map(map: &EnvironmentMap, width: usize, height: usize) -> Vec<[f32; 3]> {
    let (width, height) = (width.max(1), height.max(1));
    let (src_w, src_h) = (map.width(), map.height());
    let columns = axis_weights(src_w, width);
    let rows = axis_weights(src_h, height);

    // Horizontal pass: every source row reduced to `width` texels.
    let mut strip: Vec<[f32; 3]> = Vec::with_capacity(src_h * width);
    let mut row = Vec::with_capacity(src_w);
    for y in 0..src_h {
        read_row(map, y, &mut row);
        for taps in &columns {
            let mut sum = [0.0_f64; 3];
            for &(x, weight) in taps {
                for (acc, value) in sum.iter_mut().zip(row[x]) {
                    let term = weight * f64::from(value);
                    *acc += term;
                }
            }
            strip.push(narrow_texel(sum));
        }
    }

    // Vertical pass.
    let mut out = Vec::with_capacity(width * height);
    for taps in &rows {
        for x in 0..width {
            let mut sum = [0.0_f64; 3];
            for &(y, weight) in taps {
                for (acc, value) in sum.iter_mut().zip(strip[y * width + x]) {
                    let term = weight * f64::from(value);
                    *acc += term;
                }
            }
            out.push(narrow_texel(sum));
        }
    }
    out
}

/// The mean of each channel over `texels` (zeros for an empty slice).
#[must_use]
pub fn mean_radiance(texels: &[[f32; 3]]) -> [f64; 3] {
    if texels.is_empty() {
        return [0.0; 3];
    }
    let mut sum = [0.0_f64; 3];
    for texel in texels {
        for (acc, value) in sum.iter_mut().zip(texel) {
            *acc += f64::from(*value);
        }
    }
    let count = texels.len() as f64;
    [sum[0] / count, sum[1] / count, sum[2] / count]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_maps_keep_their_size() {
        assert_eq!(target_size(64, 32), (64, 32));
        assert_eq!(target_size(2048, 1024), (2048, 1024));
        assert_eq!(target_size(0, 0), (1, 1));
    }

    #[test]
    fn large_maps_shrink_with_their_aspect_ratio() {
        assert_eq!(target_size(4096, 2048), (2048, 1024));
        assert_eq!(target_size(8192, 4096), (2048, 1024));
        // The taller side decides: 3000 x 3000 -> 1024 x 1024.
        assert_eq!(target_size(3000, 3000), (1024, 1024));
        let (w, h) = target_size(4000, 1000);
        assert_eq!((w, h), (2048, 512));
    }

    #[test]
    fn axis_weights_are_normalized_and_cover_the_source() {
        for (src, dst) in [(10, 4), (7, 7), (9, 2), (5, 1)] {
            let weights = axis_weights(src, dst);
            assert_eq!(weights.len(), dst);
            for taps in &weights {
                let total: f64 = taps.iter().map(|&(_, w)| w).sum();
                assert!((total - 1.0).abs() < 1e-12, "{src}->{dst}: {total}");
                assert!(taps.iter().all(|&(i, _)| i < src));
            }
        }
    }
}
