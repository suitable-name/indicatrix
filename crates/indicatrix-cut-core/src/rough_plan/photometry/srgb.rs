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
/// The white frame's local variance cannot replace it, because a flat patch compresses
/// almost perfectly while the textured stone does not.
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
/// A flat white frame compresses better than a textured stone, so this is a lower estimate for
/// the stone; the solver's Birge ratio protects the rest.
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
        out[c] = (variance.max(0.0).sqrt() as f32).min(COMPRESSION_SIGMA_MAX);
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
