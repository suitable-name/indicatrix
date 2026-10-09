//! White and dark frames, and the transmittance image they turn a stone photo into.
//!
//! `t = (stone - dark) / (white - dark)` per channel and pixel is the stone's transmittance
//! relative to the empty backlight. It removes vignetting, backlight non-uniformity and the
//! camera gain in one step.
//!
//! Error propagation (independent noise, `D = white - dark`):
//! `var(t) = (var_s + (1 - t)^2 var_d / n_d + t^2 var_w / n_w) / D^2`, with the frame variances
//! from the view's [`NoiseModel`] and `n_w`, `n_d` the numbers of averaged frames.

use super::{
    linear::{LinearImage, PhotometryError, SourceKind},
    mask::{PixelMask, flag},
    noise::{NoiseModel, NoiseOptions, estimate_noise},
    resample::{ResampleOptions, ResampleSource, ResampledImage, WorkingGrid, resample},
    srgb::{COMPRESSION_SIGMA_ENCODED, encoding_variance_with, estimate_compression_sigma},
};

/// The empty-rig frames of one view: the backlit white frames and the backlight-off dark frames.
/// Several of each are averaged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalibrationFrames {
    /// White frames: the empty rig with the backlight on.
    pub white: Vec<LinearImage>,
    /// Dark frames: the backlight off.
    pub dark: Vec<LinearImage>,
}

/// The mean of several frames of one size, and how many there were.
fn average(frames: &[LinearImage]) -> Result<(LinearImage, usize), PhotometryError> {
    let first = frames.first().ok_or(PhotometryError::NoFrames)?;
    let mut sum = vec![[0.0_f64; 3]; first.pixels.len()];
    for frame in frames {
        first.require_same_size(frame)?;
        for (acc, pixel) in sum.iter_mut().zip(&frame.pixels) {
            for c in 0..3 {
                acc[c] += f64::from(pixel[c]);
            }
        }
    }
    let n = frames.len() as f64;
    let mut mean = first.clone();
    mean.variance = None;
    mean.pixels = sum
        .iter()
        .map(|s| [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32])
        .collect();
    Ok((mean, frames.len()))
}

impl CalibrationFrames {
    /// The mean white frame and the number of frames averaged.
    ///
    /// # Errors
    ///
    /// [`PhotometryError::NoFrames`] without a white frame; a size error for mixed sizes.
    pub fn mean_white(&self) -> Result<(LinearImage, usize), PhotometryError> {
        average(&self.white)
    }

    /// The mean dark frame and the number of frames averaged; an all-zero frame of the white
    /// frame's size (counted as one frame) when there is no dark frame, since RAW black levels
    /// are already removed.
    ///
    /// # Errors
    ///
    /// As [`mean_white`](Self::mean_white), and a size error between dark and white frames.
    pub fn mean_dark(&self) -> Result<(LinearImage, usize), PhotometryError> {
        if self.dark.is_empty() {
            let (white, _) = self.mean_white()?;
            let zero = LinearImage::filled(white.width, white.height, [0.0; 3], SourceKind::Raw)?;
            return Ok((zero, 1));
        }
        average(&self.dark)
    }

    /// All the frames, white first, for the noise estimate.
    #[must_use]
    pub fn all_frames(&self) -> Vec<&LinearImage> {
        self.white.iter().chain(&self.dark).collect()
    }
}

/// Settings of [`ViewCalibration::transmittance`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransmittanceOptions {
    /// A pixel is masked when `white - dark` is below this many standard deviations of its own
    /// noise in any channel (default 3).
    pub min_snr: f32,
    /// Saturation limit as a fraction of each frame's full scale (default 0.98).
    pub saturation_fraction: f32,
}

impl Default for TransmittanceOptions {
    fn default() -> Self {
        Self {
            min_snr: 3.0,
            saturation_fraction: 0.98,
        }
    }
}

/// The transmittance of a stone relative to its backlight, at the photo's resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct TransmittanceImage {
    /// The width in pixels.
    pub width: usize,
    /// The height in pixels.
    pub height: usize,
    /// `(stone - dark) / (white - dark)` per channel; not clamped. Zero where `white - dark`
    /// is not positive.
    pub values: Vec<[f32; 3]>,
    /// The propagated variance per channel; infinite where `white - dark` is not positive.
    pub variance: Vec<[f32; 3]>,
    /// [`flag::SATURATED`] (stone or white frame) and [`flag::BELOW_NOISE`].
    pub mask: PixelMask,
}

impl TransmittanceImage {
    /// Area-averages the transmittance onto the working grid; masked pixels do not enter the
    /// average (see [`resample`]).
    ///
    /// # Errors
    ///
    /// As [`resample`].
    pub fn to_working(
        &self,
        grid: &WorkingGrid,
        options: &ResampleOptions,
    ) -> Result<ResampledImage, PhotometryError> {
        resample(
            &ResampleSource {
                width: self.width,
                height: self.height,
                values: &self.values,
                variance: Some(&self.variance),
                mask: Some(&self.mask),
            },
            grid,
            options,
        )
    }
}

/// The calibration of one view: its frames and the noise model estimated from them.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewCalibration {
    /// The white and dark frames.
    pub frames: CalibrationFrames,
    /// The noise model estimated from them, stored per view.
    pub noise: NoiseModel,
}

impl ViewCalibration {
    /// The calibration of a view, estimating its noise from the frames.
    ///
    /// # Errors
    ///
    /// [`PhotometryError::NoFrames`] without a white frame, and the errors of
    /// [`estimate_noise`].
    pub fn new(frames: CalibrationFrames, options: &NoiseOptions) -> Result<Self, PhotometryError> {
        if frames.white.is_empty() {
            return Err(PhotometryError::NoFrames);
        }
        let noise = estimate_noise(&frames.all_frames(), options)?;
        Ok(Self { frames, noise })
    }

    /// The transmittance image of `stone` for this view.
    ///
    /// # Errors
    ///
    /// [`PhotometryError::NoFrames`] without a white frame, [`PhotometryError::SizeMismatch`]
    /// when the stone photo and the frames differ in size.
    pub fn transmittance(
        &self,
        stone: &LinearImage,
        options: &TransmittanceOptions,
    ) -> Result<TransmittanceImage, PhotometryError> {
        let (white, n_white) = self.frames.mean_white()?;
        let (dark, n_dark) = self.frames.mean_dark()?;
        stone.require_same_size(&white)?;
        white.require_same_size(&dark)?;
        let (n_w, n_d) = (n_white as f32, n_dark as f32);
        // The compression error of an 8-bit source, estimated from the first white frame itself
        // (round 3, D4.2); the assumed constant only without a usable white frame.
        let compression = self
            .frames
            .white
            .first()
            .and_then(|frame| {
                estimate_compression_sigma(frame, &self.noise, options.saturation_fraction)
            })
            .unwrap_or([COMPRESSION_SIGMA_ENCODED; 3]);
        let count = stone.pixels.len();
        let mut values = Vec::with_capacity(count);
        let mut variance = Vec::with_capacity(count);
        let mut mask = PixelMask::new(stone.width, stone.height);
        let stone_limit = stone.full_scale * options.saturation_fraction;
        let white_limit = white.full_scale * options.saturation_fraction;
        for (i, ((s, w), d)) in stone
            .pixels
            .iter()
            .zip(&white.pixels)
            .zip(&dark.pixels)
            .enumerate()
        {
            let (x, y) = (i % stone.width, i / stone.width);
            let mut t = [0.0_f32; 3];
            let mut var = [f32::INFINITY; 3];
            let mut bits = 0_u8;
            for c in 0..3 {
                if s[c] >= stone_limit || w[c] >= white_limit {
                    bits |= flag::SATURATED;
                }
                let span = w[c] - d[c];
                // The sensor noise plus, for a source that went through an assumed transfer
                // curve, the quantisation and compression variance of the encoding (round 2, C4).
                let var_s = stone.variance.as_ref().map_or_else(
                    || {
                        self.noise.variance(c, s[c])
                            + encoding_variance_with(
                                stone.source,
                                stone.non_linear_source,
                                s[c],
                                compression[c],
                            )
                    },
                    |v| v[i][c],
                );
                let var_w = (self.noise.variance(c, w[c])
                    + encoding_variance_with(
                        white.source,
                        white.non_linear_source,
                        w[c],
                        compression[c],
                    ))
                    / n_w;
                let var_d = (self.noise.variance(c, d[c])
                    + encoding_variance_with(
                        dark.source,
                        dark.non_linear_source,
                        d[c],
                        compression[c],
                    ))
                    / n_d;
                let var_span = var_w + var_d;
                if span <= 0.0 || span < options.min_snr * var_span.sqrt() {
                    bits |= flag::BELOW_NOISE;
                }
                if span > 0.0 {
                    t[c] = (s[c] - d[c]) / span;
                    let one_minus = 1.0 - t[c];
                    var[c] = (one_minus * one_minus)
                        .mul_add(var_d, (t[c] * t[c]).mul_add(var_w, var_s))
                        / (span * span);
                }
            }
            if bits != 0 {
                mask.set(x, y, bits);
            }
            values.push(t);
            variance.push(var);
        }
        Ok(TransmittanceImage {
            width: stone.width,
            height: stone.height,
            values,
            variance,
            mask,
        })
    }
}
