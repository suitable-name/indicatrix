//! The sRGB transfer curve (IEC 61966-2-1) and its inverse.

/// The inverse sRGB EOTF: encoded value in `[0, 1]` to linear light.
#[must_use]
pub fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// The sRGB OETF: linear light to the encoded value in `[0, 1]`.
#[must_use]
pub fn linear_to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055_f32.mul_add(linear.powf(1.0 / 2.4), -0.055)
    }
}

/// The standard deviation, in the ENCODED (sRGB) domain, of the compression error of an 8-bit
/// source, in units of the encoded range: an assumed 2 codes of 255.
///
/// A conservative modelling constant, not a measurement: a quality 85 to 95 JPEG typically
/// reproduces a code within 1 to 3 codes RMS, and a lossless 8-bit PNG is over-estimated by it.
/// The white frame's local variance cannot replace it for the stone, because a flat patch
/// compresses almost perfectly while the textured stone does not; the stone's own compression
/// error is estimated from the stone frame by [`estimate_stone_compression_sigma`], and this
/// constant is only the fallback when that estimate is unavailable.
pub const COMPRESSION_SIGMA_ENCODED: f32 = 0.008;

/// The derivative of the inverse sRGB EOTF at the encoded value `encoded`.
fn srgb_to_linear_slope(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        1.0 / 12.92
    } else {
        2.4 / 1.055 * ((encoded + 0.055) / 1.055).powf(1.4)
    }
}

/// The variance, in linear light, that the encoding of a non-linear source adds to a pixel of
/// linear value `linear` (round 2, C4), with the ASSUMED compression term
/// [`COMPRESSION_SIGMA_ENCODED`].
///
/// See [`encoding_variance_with`].
#[must_use]
pub fn encoding_variance(source: super::linear::SourceKind, non_linear: bool, linear: f32) -> f32 {
    encoding_variance_with(source, non_linear, linear, COMPRESSION_SIGMA_ENCODED)
}

/// [`encoding_variance`] with the compression term `compression_sigma`.
///
/// `compression_sigma` is the standard deviation, in the encoded domain and in units of the
/// encoded range, of the compression error of an 8-bit source; for example the estimate of
/// [`estimate_compression_sigma`].
///
/// The encoded value has the quantisation variance `step^2 / 12` (`step` = 1/255 for 8 bits,
/// 1/65535 for a 16-bit file) and, for an 8-bit source, the compression variance
/// `compression_sigma^2`. Both are propagated through the derivative of the inverse sRGB EOTF at
/// the pixel's encoded value, `var_linear = f'(e)^2 (step^2 / 12 + sigma_c^2)`. Zero for a source
/// that was measured as linear (RAW).
#[must_use]
pub fn encoding_variance_with(
    source: super::linear::SourceKind,
    non_linear: bool,
    linear: f32,
    compression_sigma: f32,
) -> f32 {
    use super::linear::SourceKind;
    if !non_linear {
        return 0.0;
    }
    let (step, compression) = match source {
        SourceKind::Raw => return 0.0,
        SourceKind::Tiff16 => (1.0 / 65535.0, 0.0),
        SourceKind::Rgb8 => (1.0 / 255.0, compression_sigma.max(0.0)),
    };
    let encoded = linear_to_srgb(linear.clamp(0.0, 1.0));
    let slope = srgb_to_linear_slope(encoded);
    slope * slope * f32::mul_add(compression, compression, step * step / 12.0)
}

/// The largest compression sigma [`estimate_compression_sigma`] returns (four times the assumed
/// constant): a white frame with real texture (dust, a grainy diffuser) must not blow the noise
/// model up.
pub const COMPRESSION_SIGMA_MAX: f32 = 4.0 * COMPRESSION_SIGMA_ENCODED;

/// The half width of the box of [`estimate_compression_sigma`] (a 5 x 5 box).
const BOX_RADIUS: usize = 2;

/// Estimates the compression error of an 8-bit white frame from the frame itself (round 3, D4.2).
///
/// Per channel, the standard deviation in the ENCODED domain of the difference between a pixel
/// and the mean of its 5 x 5 box, with
///
/// * the box-filter factor `1 - 1/25` divided out (for independent noise the pixel differs from
///   the box mean, which contains the pixel, by `sigma sqrt(1 - 1/25)`),
/// * the sensor noise of `noise` (propagated to the encoded domain through the slope of the sRGB
///   curve) and the quantisation `1/255^2/12` subtracted in variance,
/// * pixels whose box holds a saturated value (`white.full_scale * saturation_fraction`) skipped.
///
/// What is left is the compression error plus any real texture of the frame, capped at
/// [`COMPRESSION_SIGMA_MAX`]. `None` for a source that is not an 8-bit file, a frame smaller than
/// 7 x 7 pixels, or fewer than 16 usable pixels in a channel: the caller falls back to the
/// constant [`COMPRESSION_SIGMA_ENCODED`].
///
/// A flat white frame compresses almost perfectly, so this is the right estimate for the WHITE
/// frame's own term only and a large under-estimate for the stone: the stone frame uses
/// [`estimate_stone_compression_sigma`] instead.
#[must_use]
pub fn estimate_compression_sigma(
    white: &super::linear::LinearImage,
    noise: &super::noise::NoiseModel,
    saturation_fraction: f32,
) -> Option<[f32; 3]> {
    use super::linear::SourceKind;
    if white.source != SourceKind::Rgb8 || !white.non_linear_source {
        return None;
    }
    let (width, height) = (white.width, white.height);
    if width < 2 * BOX_RADIUS + 3 || height < 2 * BOX_RADIUS + 3 {
        return None;
    }
    let limit = white.full_scale * saturation_fraction;
    let encoded: Vec<[f32; 3]> = white
        .pixels
        .iter()
        .map(|p| p.map(|v| linear_to_srgb(v.clamp(0.0, 1.0))))
        .collect();
    let quantisation = 1.0 / (255.0 * 255.0 * 12.0);
    let box_pixels = ((2 * BOX_RADIUS + 1) * (2 * BOX_RADIUS + 1)) as f64;
    let mut out = [0.0_f32; 3];
    for c in 0..3 {
        let (mut squares, mut sensor, mut count) = (0.0_f64, 0.0_f64, 0_u64);
        for y in BOX_RADIUS..height - BOX_RADIUS {
            for x in BOX_RADIUS..width - BOX_RADIUS {
                let (mut sum, mut saturated) = (0.0_f64, false);
                for dy in 0..=2 * BOX_RADIUS {
                    for dx in 0..=2 * BOX_RADIUS {
                        let i = (y + dy - BOX_RADIUS) * width + (x + dx - BOX_RADIUS);
                        saturated |= white.pixels[i][c] >= limit;
                        sum += f64::from(encoded[i][c]);
                    }
                }
                if saturated {
                    continue;
                }
                let i = y * width + x;
                let difference = f64::from(encoded[i][c]) - sum / box_pixels;
                squares = difference.mul_add(difference, squares);
                let slope = f64::from(srgb_to_linear_slope(encoded[i][c]));
                sensor += f64::from(noise.variance(c, white.pixels[i][c])) / (slope * slope);
                count += 1;
            }
        }
        if count < 16 {
            return None;
        }
        let n = count as f64;
        let variance = squares / n / (1.0 - 1.0 / box_pixels) - sensor / n - quantisation;
        out[c] = sigma_f32(variance);
    }
    Some(out)
}

/// The standard deviation of a (possibly negative, estimated) variance as `f32`, capped at
/// [`COMPRESSION_SIGMA_MAX`].
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the image-sigma storage precision; a sigma of 8-bit data is far inside f32 range"
)]
fn sigma_f32(variance: f64) -> f32 {
    (variance.max(0.0).sqrt() as f32).min(COMPRESSION_SIGMA_MAX)
}

/// The median of `values` (the upper middle element for an even count); reorders the slice.
/// `values` must not be empty.
fn median(values: &mut [f64]) -> f64 {
    let middle = values.len() / 2;
    *values.select_nth_unstable_by(middle, f64::total_cmp).1
}

/// The ratio of the median absolute deviation of a normal variable to its standard deviation.
const MAD_TO_SIGMA: f64 = 0.6745;

/// The variance gain of the 3 x 3 Laplacian `4 p - (left + right + up + down)` for independent
/// noise: `4^2 + 4 * 1^2`.
const LAPLACIAN_VARIANCE_GAIN: f64 = 20.0;

/// The fewest pixels a channel needs for [`estimate_stone_compression_sigma`].
const MIN_STONE_PIXELS: usize = 16;

/// The side of a JPEG transform block in pixels (the grid the block-mean estimate of
/// [`estimate_stone_compression`] is aligned to: pixel 0 of the decoded image).
pub const JPEG_BLOCK: usize = 8;

/// The fewest usable block-mean Laplacians a channel needs for the block-DC estimate; below this
/// the block-DC term is reported as zero.
const MIN_STONE_BLOCKS: usize = 9;

/// The compression error of an 8-bit stone frame in the ENCODED domain, in units of the encoded
/// range, split into the two parts that propagate differently (S2, root-cause analysis R4).
///
/// A JPEG's error on smooth content is the per-pixel AC quantisation error, which averages down
/// when pixels are averaged, plus one DC quantisation offset per 8 x 8 block, which is constant
/// inside a block and so does NOT average down over a region of one block or less.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoneCompression {
    /// The per-pixel (AC and high-frequency) standard deviation, per channel: what
    /// [`estimate_stone_compression_sigma`] returns.
    pub ac: [f32; 3],
    /// The standard deviation of the per-block DC offset, per channel (zero when the frame holds
    /// too few clear blocks to estimate it).
    pub block_dc: [f32; 3],
}

/// The linear-light variance of the block-DC error `block_dc_sigma` (encoded domain) at the pixel
/// of linear value `linear`: `f'(e)^2 sigma_dc^2`. Zero for a source that is not an 8-bit
/// non-linear file.
#[must_use]
pub(super) fn block_dc_variance(
    source: super::linear::SourceKind,
    non_linear: bool,
    linear: f32,
    block_dc_sigma: f32,
) -> f32 {
    if !non_linear || source != super::linear::SourceKind::Rgb8 {
        return 0.0;
    }
    let slope = srgb_to_linear_slope(linear_to_srgb(linear.clamp(0.0, 1.0)));
    let sigma = block_dc_sigma.max(0.0);
    slope * slope * sigma * sigma
}

/// The standard deviation of the per-block DC offset of channel `c`, from the robust Laplacian
/// of the 8 x 8 block means (`pixel_total` is the pixel-level variance estimate that the block
/// means also contain, divided by 64 and subtracted).
fn block_dc_sigma(
    encoded: &[[f32; 3]],
    clear: &[bool],
    c: usize,
    (width, height): (usize, usize),
    pixel_total: f64,
) -> f32 {
    let (nbx, nby) = (width / JPEG_BLOCK, height / JPEG_BLOCK);
    if nbx < 3 || nby < 3 {
        return 0.0;
    }
    let mut means = vec![0.0_f64; nbx * nby];
    let mut usable = vec![false; nbx * nby];
    let block_area = (JPEG_BLOCK * JPEG_BLOCK) as f64;
    for by in 0..nby {
        for bx in 0..nbx {
            let (mut sum, mut all_clear) = (0.0_f64, true);
            for y in by * JPEG_BLOCK..(by + 1) * JPEG_BLOCK {
                for x in bx * JPEG_BLOCK..(bx + 1) * JPEG_BLOCK {
                    all_clear &= clear[y * width + x];
                    sum += f64::from(encoded[y * width + x][c]);
                }
            }
            means[by * nbx + bx] = sum / block_area;
            usable[by * nbx + bx] = all_clear;
        }
    }
    let mut laplacians: Vec<f64> = Vec::new();
    for by in 1..nby - 1 {
        for bx in 1..nbx - 1 {
            let i = by * nbx + bx;
            let around = [i, i - 1, i + 1, i - nbx, i + nbx];
            if around.iter().all(|&k| usable[k]) {
                laplacians.push(4.0_f64.mul_add(
                    means[i],
                    -(means[i - 1] + means[i + 1] + means[i - nbx] + means[i + nbx]),
                ));
            }
        }
    }
    if laplacians.len() < MIN_STONE_BLOCKS {
        return 0.0;
    }
    let centre = median(&mut laplacians);
    let mut deviations: Vec<f64> = laplacians.iter().map(|l| (l - centre).abs()).collect();
    let spread = median(&mut deviations) / MAD_TO_SIGMA;
    let variance = spread * spread / LAPLACIAN_VARIANCE_GAIN - pixel_total / block_area;
    sigma_f32(variance)
}

/// Estimates the compression error of an 8-bit STONE frame from the frame itself (ZC-3), as the
/// per-pixel part only.
///
/// This is [`estimate_stone_compression`] without the block-DC part; see there for the method.
#[must_use]
pub fn estimate_stone_compression_sigma(
    image: &super::linear::LinearImage,
    noise: &super::noise::NoiseModel,
    mask: Option<&super::mask::PixelMask>,
    saturation_fraction: f32,
) -> Option<[f32; 3]> {
    estimate_stone_compression(image, noise, mask, saturation_fraction).map(|s| s.ac)
}

/// Estimates the compression error of an 8-bit STONE frame from the frame itself (ZC-3 and the
/// block-DC part of S2).
///
/// JPEG error is high frequency, while the stone's radiance is smooth at the working resolution
/// of the rig (about 24 pixels per mm), so a robust high-pass estimator separates the two. Per
/// channel, on the ENCODED frame:
///
/// 1. the 3 x 3 Laplacian `4 p - (left + right + up + down)` of every usable pixel (variance
///    `20 sigma^2` for independent noise, and zero for a locally linear radiance);
/// 2. the robust spread `MAD / 0.6745` of those values, which ignores the outline, inclusions and
///    facet edges that are a minority of the pixels, divided by `sqrt(20)`;
/// 3. the sensor noise of `noise` (propagated to the encoded domain through the slope of the sRGB
///    curve, median over the same pixels) and the quantisation `1/255^2/12` subtracted in
///    variance;
/// 4. the square root of what is left (zero when negative), capped at
///    [`COMPRESSION_SIGMA_MAX`]: [`StoneCompression::ac`].
///
/// The block-DC part ([`StoneCompression::block_dc`]) is what the Laplacian cannot see: the DC
/// quantisation offset of each 8 x 8 JPEG block is constant inside the block, so it shows only on
/// the one-pixel seams, which the robust spread discards. It is estimated from the 8 x 8 block
/// means of the encoded frame (blocks aligned to pixel 0, the JPEG grid of the decoded image,
/// every one of whose 64 pixels is clear), by the same robust Laplacian over the blocks whose four
/// neighbours are clear too: `sigma_block^2 = (MAD / 0.6745)^2 / 20`. The block mean also holds
/// the pixel-level noise averaged down by 64, so `total / 64` (the pixel-level variance estimate
/// of step 2, before the sensor and quantisation subtraction) is subtracted; what is left, clamped
/// at zero, is the DC variance, capped at [`COMPRESSION_SIGMA_MAX`]. Fewer than nine usable
/// blocks (or fewer than three blocks across or down) report zero: the stone frame is too small
/// to separate the block offsets from the image itself, which is the optimistic side. The
/// estimate assumes the stone's radiance is smooth over a block, as it is at the rig's working
/// resolution, and a crop of the decoded frame must keep the 8-pixel alignment to be meaningful.
///
/// `mask` (the stone's exclusion mask at the frame's resolution, [`super::PixelMask`]) removes every
/// pixel with a flag at it or at one of its four neighbours; without a mask every pixel is used.
/// Pixels whose neighbourhood holds a saturated value (`image.full_scale * saturation_fraction`)
/// are skipped as well.
///
/// `None` for a source that is not an 8-bit file, a frame smaller than 5 x 5 pixels, a mask of
/// another size, or fewer than 16 usable pixels in a channel: the caller falls back to the
/// constant [`COMPRESSION_SIGMA_ENCODED`].
///
/// This measures the excess high-frequency noise of the decoded frame. A codec that smooths the
/// sensor noise away (a strong JPEG on a noisy frame) leaves less than the noise model predicts
/// and the estimate then reads low (it never goes negative); chroma subsampling correlates the
/// error between neighbouring pixels, which also reads low. Both errors are on the optimistic
/// side by a bounded amount; the Birge ratio of the fit remains the backstop.
#[must_use]
pub fn estimate_stone_compression(
    image: &super::linear::LinearImage,
    noise: &super::noise::NoiseModel,
    mask: Option<&super::mask::PixelMask>,
    saturation_fraction: f32,
) -> Option<StoneCompression> {
    use super::linear::SourceKind;
    if image.source != SourceKind::Rgb8 || !image.non_linear_source {
        return None;
    }
    let (width, height) = (image.width, image.height);
    if width < 5 || height < 5 {
        return None;
    }
    if mask.is_some_and(|m| m.width() != width || m.height() != height) {
        return None;
    }
    let limit = image.full_scale * saturation_fraction;
    let encoded: Vec<[f32; 3]> = image
        .pixels
        .iter()
        .map(|p| p.map(|v| linear_to_srgb(v.clamp(0.0, 1.0))))
        .collect();
    let quantisation = 1.0 / (255.0 * 255.0 * 12.0);
    let mut out = StoneCompression {
        ac: [0.0_f32; 3],
        block_dc: [0.0_f32; 3],
    };
    for c in 0..3 {
        // A pixel is clear when it is unsaturated and carries no mask flag (the block-DC estimate
        // needs whole blocks of them).
        let clear: Vec<bool> = (0..width * height)
            .map(|i| {
                image.pixels[i][c] < limit && mask.is_none_or(|m| m.is_clear(i % width, i / width))
            })
            .collect();
        let mut laplacians: Vec<f64> = Vec::new();
        let mut sensors: Vec<f64> = Vec::new();
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let i = y * width + x;
                let around = [i, i - 1, i + 1, i - width, i + width];
                if around.iter().any(|&k| image.pixels[k][c] >= limit) {
                    continue;
                }
                if let Some(mask) = mask {
                    let blocked = [(x, y), (x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                        .iter()
                        .any(|&(px, py)| !mask.is_clear(px, py));
                    if blocked {
                        continue;
                    }
                }
                let neighbours = encoded[i - 1][c]
                    + encoded[i + 1][c]
                    + encoded[i - width][c]
                    + encoded[i + width][c];
                laplacians.push(f64::from(4.0_f32.mul_add(encoded[i][c], -neighbours)));
                let slope = f64::from(srgb_to_linear_slope(encoded[i][c]));
                sensors.push(f64::from(noise.variance(c, image.pixels[i][c])) / (slope * slope));
            }
        }
        if laplacians.len() < MIN_STONE_PIXELS {
            return None;
        }
        let centre = median(&mut laplacians);
        let mut deviations: Vec<f64> = laplacians.iter().map(|l| (l - centre).abs()).collect();
        let spread = median(&mut deviations) / MAD_TO_SIGMA;
        let total = spread * spread / LAPLACIAN_VARIANCE_GAIN;
        let variance = total - median(&mut sensors) - quantisation;
        out.ac[c] = sigma_f32(variance);
        out.block_dc[c] = block_dc_sigma(&encoded, &clear, c, (width, height), total);
    }
    Some(out)
}

/// The linear value of every 8-bit code.
#[must_use]
pub fn srgb8_table() -> [f32; 256] {
    let mut table = [0.0_f32; 256];
    for (code, slot) in table.iter_mut().enumerate() {
        *slot = srgb_to_linear(code as f32 / 255.0);
    }
    table
}
