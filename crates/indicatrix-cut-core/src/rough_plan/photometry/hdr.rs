//! HDR merge of several exposures of one view.
//!
//! Each exposure `i` gives an estimate of the radiance per unit exposure, `r_i = x_i / e_i`,
//! with variance `var(x_i) / e_i^2` from the noise model. The merge is the inverse-variance
//! weighted mean of the unsaturated estimates (weights `e_i^2 / var(x_i)`, so long exposures
//! win where they are not clipped and short ones fill the highlights). The result is scaled
//! back to the units of the LONGEST exposure, so it reads like that frame, only without its
//! clipping.
//!
//! Where every exposure is saturated the shortest one is used and the pixel is flagged
//! [`flag::SATURATED`].

use super::{
    linear::{LinearImage, PhotometryError},
    mask::{PixelMask, flag},
    noise::NoiseModel,
};

/// One exposure of the merge.
#[derive(Debug, Clone, Copy)]
pub struct HdrInput<'a> {
    /// The exposure's linear image.
    pub image: &'a LinearImage,
    /// The relative exposure (any consistent unit). `None` takes it from the EXIF data: the
    /// exposure time, times the ISO when every input without a given exposure has one.
    pub exposure: Option<f64>,
}

/// Settings of [`merge_hdr`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrOptions {
    /// An exposure's value is excluded at or above this fraction of its full scale (default
    /// 0.98).
    pub saturation_fraction: f32,
}

impl Default for HdrOptions {
    fn default() -> Self {
        Self {
            saturation_fraction: 0.98,
        }
    }
}

/// The merged image and its flags.
#[derive(Debug, Clone, PartialEq)]
pub struct HdrMerged {
    /// The merge, in the units of the longest exposure, with its variance. Its `full_scale`
    /// is the longest exposure's times the ratio of the longest to the shortest exposure.
    pub image: LinearImage,
    /// [`flag::SATURATED`] where some channel was clipped in every exposure.
    pub mask: PixelMask,
    /// The exposures used, in input order.
    pub exposures: Vec<f64>,
}

/// The exposure of each input.
fn exposures_of(inputs: &[HdrInput<'_>]) -> Result<Vec<f64>, PhotometryError> {
    let use_iso = inputs
        .iter()
        .filter(|i| i.exposure.is_none())
        .all(|i| i.image.meta.iso.is_some());
    inputs
        .iter()
        .map(|input| {
            if let Some(given) = input.exposure {
                return Ok(given);
            }
            let time = input
                .image
                .meta
                .exposure_time_s
                .ok_or(PhotometryError::MissingExposure)?;
            let gain = if use_iso {
                f64::from(input.image.meta.iso.unwrap_or(1))
            } else {
                1.0
            };
            Ok(time * gain)
        })
        .collect::<Result<Vec<f64>, PhotometryError>>()
        .and_then(|list| {
            if list.iter().all(|e| e.is_finite() && *e > 0.0) {
                Ok(list)
            } else {
                Err(PhotometryError::MissingExposure)
            }
        })
}

/// Merges two or more exposures of the same view.
///
/// # Errors
///
/// [`PhotometryError::NoFrames`] for fewer than two inputs; [`PhotometryError::SizeMismatch`]
/// for mixed sizes; [`PhotometryError::MissingExposure`] when an exposure is unknown.
pub fn merge_hdr(
    inputs: &[HdrInput<'_>],
    noise: &NoiseModel,
    options: &HdrOptions,
) -> Result<HdrMerged, PhotometryError> {
    if inputs.len() < 2 {
        return Err(PhotometryError::NoFrames);
    }
    let first = inputs[0].image;
    for input in inputs {
        first.require_same_size(input.image)?;
    }
    let exposures = exposures_of(inputs)?;
    let (longest, shortest) = exposures
        .iter()
        .enumerate()
        .fold((0, 0), |(hi, lo), (i, e)| {
            (
                if *e > exposures[hi] { i } else { hi },
                if *e < exposures[lo] { i } else { lo },
            )
        });
    let reference = exposures[longest];
    let count = first.pixels.len();
    let mut pixels = Vec::with_capacity(count);
    let mut variance = Vec::with_capacity(count);
    let mut mask = PixelMask::new(first.width, first.height);
    for index in 0..count {
        let mut merged = [0.0_f32; 3];
        let mut var = [0.0_f32; 3];
        let mut clipped = false;
        for c in 0..3 {
            let (mut weight_sum, mut weighted) = (0.0_f64, 0.0_f64);
            for (input, exposure) in inputs.iter().zip(&exposures) {
                let x = input.image.pixels[index][c];
                if x >= input.image.full_scale * options.saturation_fraction {
                    continue;
                }
                let var_x = f64::from(noise.variance(c, x)).max(1e-12);
                let weight = exposure * exposure / var_x;
                weight_sum += weight;
                weighted += weight * f64::from(x) / exposure;
            }
            if weight_sum > 0.0 {
                merged[c] = (weighted / weight_sum * reference) as f32;
                var[c] = (reference * reference / weight_sum) as f32;
            } else {
                clipped = true;
                let x = inputs[shortest].image.pixels[index][c];
                let e = exposures[shortest];
                merged[c] = (f64::from(x) / e * reference) as f32;
                var[c] = (f64::from(noise.variance(c, x)) * (reference / e).powi(2)) as f32;
            }
        }
        if clipped {
            mask.set(index % first.width, index / first.width, flag::SATURATED);
        }
        pixels.push(merged);
        variance.push(var);
    }
    let mut image = inputs[longest].image.clone();
    image.pixels = pixels;
    image.variance = Some(variance);
    image.full_scale = inputs[longest].image.full_scale * (reference / exposures[shortest]) as f32;
    image.non_linear_source = inputs.iter().any(|i| i.image.non_linear_source);
    Ok(HdrMerged {
        image,
        mask,
        exposures,
    })
}
