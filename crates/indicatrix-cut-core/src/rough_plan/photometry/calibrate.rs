//! White and dark frames, and the transmittance image they turn a stone photo into.
//!
//! `t = (stone - dark) / (white - dark)` per channel and pixel is the stone's transmittance
//! relative to the empty backlight. It removes vignetting, backlight non-uniformity and the
//! camera gain in one step.
//!
//! Error propagation (independent noise, `D = white - dark`):
//! `var(t) = (var_s + (1 - t)^2 var_d / n_d + t^2 var_w / n_w) / D^2`, with the frame variances
//! from the view's [`NoiseModel`] and `n_w`, `n_d` the numbers of averaged frames.
//!
//! # The JPEG block-DC error (S2)
//!
//! The stone frame's compression error of an 8-bit JPEG has two parts (see
//! [`estimate_stone_compression`]): the per-pixel AC error, independent from pixel to pixel, and
//! the DC quantisation offset of each 8 x 8 block, constant inside a block. The photo-resolution
//! [`TransmittanceImage::variance`] holds both (the marginal variance of one pixel), and
//! [`TransmittanceImage::block_variance`] the DC part alone. Averaging onto the working grid
//! ([`TransmittanceImage::to_working`]) treats the AC part as independent (variance divided by the
//! pixel count `N = scale^2` under a working pixel) and the DC part as fully correlated within a
//! block: for a working pixel of `N` photo pixels the DC variance is `var_dc * min(1, 64 / N)` (a
//! working pixel of one block or less does not average it down, many blocks average by the block
//! count). The correlation BETWEEN working pixels that share a block (working pixels smaller than
//! a block) is not modelled, and the white frame's own block offsets are not estimated (the
//! flat white frame is averaged over several exposures).

use super::{
    linear::{LinearImage, PhotometryError, SourceKind},
    mask::{PixelMask, flag},
    noise::{NoiseModel, NoiseOptions, estimate_noise},
    resample::{ResampleOptions, ResampleSource, ResampledImage, WorkingGrid, resample},
    srgb::{
        COMPRESSION_SIGMA_ENCODED, JPEG_BLOCK, StoneCompression, block_dc_variance,
        encoding_variance_with, estimate_compression_sigma, estimate_stone_compression,
    },
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
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the pixel storage precision; the per-channel mean is a normal-range float"
)]
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
    /// The part of [`variance`](Self::variance) that is the stone frame's JPEG block-DC error
    /// (zero for every source but an 8-bit one, and where no block estimate exists): constant
    /// inside an 8 x 8 block, so it is propagated onto the working grid as fully correlated.
    pub block_variance: Vec<[f32; 3]>,
    /// [`flag::SATURATED`] (stone or white frame) and [`flag::BELOW_NOISE`].
    pub mask: PixelMask,
}

impl TransmittanceImage {
    /// Area-averages the transmittance onto the working grid; masked pixels do not enter the
    /// average (see [`resample`]).
    ///
    /// The per-pixel part of the variance is averaged as independent noise; the block-DC part
    /// ([`block_variance`](Self::block_variance)) as fully correlated within a block: a working
    /// pixel of `N = scale^2` photo pixels carries `var_dc * min(1, 64 / N)` of it (module notes).
    ///
    /// # Errors
    ///
    /// As [`resample`].
    #[expect(
        clippy::float_cmp,
        reason = "an all-zero block-variance plane is the exact 'no block data' marker, never a computed value"
    )]
    pub fn to_working(
        &self,
        grid: &WorkingGrid,
        options: &ResampleOptions,
    ) -> Result<ResampledImage, PhotometryError> {
        let source = |variance: &[[f32; 3]]| {
            resample(
                &ResampleSource {
                    width: self.width,
                    height: self.height,
                    values: &self.values,
                    variance: Some(variance),
                    mask: Some(&self.mask),
                },
                grid,
                options,
            )
        };
        if self.block_variance.iter().all(|v| *v == [0.0; 3]) {
            return source(&self.variance);
        }
        let per_pixel: Vec<[f32; 3]> = self
            .variance
            .iter()
            .zip(&self.block_variance)
            .map(|(total, block)| [0, 1, 2].map(|c| (total[c] - block[c]).max(0.0)))
            .collect();
        let mut image = source(&per_pixel)?;
        let block = source(&self.block_variance)?;
        // The independent average of the block variance is `var_dc / N`; the correlated one is
        // `var_dc * min(1, 64 / N)`, i.e. a factor `min(N, 64)` on top.
        let photo_pixels = grid.scale * grid.scale;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the factor is in 1..=64; f32 is the image-variance storage precision"
        )]
        let factor = photo_pixels.min((JPEG_BLOCK * JPEG_BLOCK) as f64).max(1.0) as f32;
        for (total, dc) in image.variance.iter_mut().zip(&block.variance) {
            for c in 0..3 {
                total[c] = dc[c].mul_add(factor, total[c]);
            }
        }
        Ok(image)
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
        self.transmittance_masked(stone, options, None)
    }

    /// [`transmittance`](Self::transmittance) with the stone's exclusion mask, at the stone
    /// photo's resolution, for the estimate of the stone's compression error (the pixels the
    /// mask flags, outline and edge included, do not enter it). Without a mask every pixel does.
    ///
    /// # Errors
    ///
    /// As [`transmittance`](Self::transmittance), and [`PhotometryError::SizeMismatch`] when the
    /// mask and the stone photo differ in size.
    #[allow(
        clippy::too_many_lines,
        reason = "one straight-line pipeline (validate, linearise, divide, propagate variance); splitting adds plumbing only"
    )]
    pub fn transmittance_masked(
        &self,
        stone: &LinearImage,
        options: &TransmittanceOptions,
        stone_mask: Option<&PixelMask>,
    ) -> Result<TransmittanceImage, PhotometryError> {
        let (white, n_white) = self.frames.mean_white()?;
        let (dark, n_dark) = self.frames.mean_dark()?;
        stone.require_same_size(&white)?;
        white.require_same_size(&dark)?;
        if let Some(mask) =
            stone_mask.filter(|m| m.width() != stone.width || m.height() != stone.height)
        {
            return Err(PhotometryError::SizeMismatch {
                expected: [stone.width, stone.height],
                found: [mask.width(), mask.height()],
            });
        }
        let (n_w, n_d) = (n_white as f32, n_dark as f32);
        // The compression error of an 8-bit source (ZC-3). The stone's own term comes from the
        // stone frame (a robust Laplacian: a flat frame compresses almost perfectly, a textured
        // stone does not); the white and dark frames use the local variance of the first white
        // frame, which is correctly tiny for a flat frame. The assumed constant only where the
        // frame offers no usable estimate.
        let compression_white = self
            .frames
            .white
            .first()
            .and_then(|frame| {
                estimate_compression_sigma(frame, &self.noise, options.saturation_fraction)
            })
            .unwrap_or([COMPRESSION_SIGMA_ENCODED; 3]);
        let compression_stone = if stone.variance.is_some() {
            StoneCompression {
                ac: compression_white,
                block_dc: [0.0; 3],
            }
        } else {
            estimate_stone_compression(stone, &self.noise, stone_mask, options.saturation_fraction)
                .unwrap_or(StoneCompression {
                    ac: [COMPRESSION_SIGMA_ENCODED; 3],
                    block_dc: [0.0; 3],
                })
        };
        let count = stone.pixels.len();
        let mut values = Vec::with_capacity(count);
        let mut variance = Vec::with_capacity(count);
        let mut block_variance = Vec::with_capacity(count);
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
            let mut block = [0.0_f32; 3];
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
                                compression_stone.ac[c],
                            )
                    },
                    |v| v[i][c],
                );
                let var_w = (self.noise.variance(c, w[c])
                    + encoding_variance_with(
                        white.source,
                        white.non_linear_source,
                        w[c],
                        compression_white[c],
                    ))
                    / n_w;
                let var_d = (self.noise.variance(c, d[c])
                    + encoding_variance_with(
                        dark.source,
                        dark.non_linear_source,
                        d[c],
                        compression_white[c],
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
                    // The stone frame's block-DC error (S2), divided by the span like the rest.
                    let dc = block_dc_variance(
                        stone.source,
                        stone.non_linear_source,
                        s[c],
                        compression_stone.block_dc[c],
                    ) / (span * span);
                    block[c] = dc;
                    var[c] += dc;
                }
            }
            if bits != 0 {
                mask.set(x, y, bits);
            }
            values.push(t);
            variance.push(var);
            block_variance.push(block);
        }
        Ok(TransmittanceImage {
            width: stone.width,
            height: stone.height,
            values,
            variance,
            block_variance,
            mask,
        })
    }
}
