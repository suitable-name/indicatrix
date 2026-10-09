//! Camera RAW and DNG decoding through `rawler`.
//!
//! The recipe is fixed and deterministic:
//!
//! 1. the sensor values are scaled `(v - black) / (white - black)` with the per-position black
//!    and white levels of the file;
//! 2. a Bayer mosaic is demosaiced by plain **bilinear interpolation**: a missing colour at a
//!    pixel is the mean of the samples of that colour in the 3 by 3 neighbourhood that lie
//!    inside the image;
//! 3. the crop area the file recommends (else the active area) is cut out;
//! 4. nothing else: no white balance, no colour matrix, no tone curve. The pixels are
//!    camera-native linear RGB, and the file's colour data go to [`CameraColour`].
//!
//! Only 2 by 2 Bayer sensors (and files that already hold three colours per pixel) are
//! supported. Other mosaics (X-Trans) and four-colour sensors are refused with
//! [`PhotometryError::Unsupported`].

use std::path::Path;

use rawler::{
    decoders::RawDecodeParams,
    rawimage::{RawImage, RawPhotometricInterpretation},
    rawsource::RawSource,
};

use super::linear::{
    CameraColour, CaptureMeta, ColourMatrix, LinearImage, PhotometryError, SourceKind,
};

fn rawler_error(error: impl std::fmt::Display) -> PhotometryError {
    PhotometryError::Decode(error.to_string())
}

/// Decodes a camera RAW or DNG file.
///
/// # Errors
///
/// [`PhotometryError::Decode`] when rawler refuses the file, [`PhotometryError::Unsupported`]
/// for a sensor layout this module does not demosaic.
pub fn decode_raw_file(path: &Path) -> Result<LinearImage, PhotometryError> {
    let source = RawSource::new(path).map_err(rawler_error)?;
    let decoder = rawler::get_decoder(&source).map_err(rawler_error)?;
    let params = RawDecodeParams::default();
    let raw = decoder
        .raw_image(&source, &params, false)
        .map_err(rawler_error)?;
    let mut meta = capture_meta_of(&raw);
    if let Ok(metadata) = decoder.raw_metadata(&source, &params) {
        let exif = &metadata.exif;
        meta.exposure_time_s = exif
            .exposure_time
            .filter(|r| r.d != 0)
            .map(|r| f64::from(r.n) / f64::from(r.d));
        meta.f_number = exif
            .fnumber
            .filter(|r| r.d != 0)
            .map(|r| f64::from(r.n) / f64::from(r.d));
        meta.iso = exif
            .iso_speed
            .or_else(|| exif.iso_speed_ratings.map(u32::from));
        meta.white_balance_mode = exif.white_balance;
    }
    let mut image = linear_from_raw(&raw)?;
    image.meta = meta;
    Ok(image)
}

fn capture_meta_of(raw: &RawImage) -> CaptureMeta {
    CaptureMeta {
        camera_make: Some(raw.clean_make.clone()).filter(|s| !s.is_empty()),
        camera_model: Some(raw.clean_model.clone()).filter(|s| !s.is_empty()),
        ..CaptureMeta::default()
    }
}

/// The camera's own colour data from a decoded RAW.
fn colour_of(raw: &RawImage) -> CameraColour {
    let mut color_matrices: Vec<ColourMatrix> = raw
        .color_matrix
        .iter()
        .map(|(illuminant, values)| ColourMatrix {
            illuminant: u16::from(*illuminant),
            xyz_to_camera_or_forward: values.clone(),
        })
        .collect();
    color_matrices.sort_by_key(|m| m.illuminant);
    CameraColour {
        as_shot_wb: raw.wb_coeffs,
        xyz_to_cam: raw.xyz_to_cam,
        camera_to_xyz: raw.cam_to_xyz(),
        color_matrices,
        forward_matrices: Vec::new(),
    }
}

/// The value of a level list at sample position `index`; the last entry when the list is
/// shorter, 0 when it is empty.
fn level_at(levels: &[f32], index: usize) -> f32 {
    levels
        .get(index)
        .or_else(|| levels.last())
        .copied()
        .unwrap_or(0.0)
}

/// Linearises and demosaics a decoded RAW.
///
/// # Errors
///
/// [`PhotometryError::Unsupported`] for sensors other than a 2 by 2 Bayer mosaic or
/// three-colour data; [`PhotometryError::Invalid`] for inconsistent sizes.
pub fn linear_from_raw(raw: &RawImage) -> Result<LinearImage, PhotometryError> {
    let (width, height, cpp) = (raw.width, raw.height, raw.cpp);
    let data = raw.data.as_f32();
    if width == 0 || height == 0 || data.len() != width * height * cpp {
        return Err(PhotometryError::Invalid(format!(
            "RAW data of {} samples for {width}x{height} with {cpp} per pixel",
            data.len()
        )));
    }
    let pixels = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(config) => {
            if cpp != 1 {
                return Err(PhotometryError::Unsupported(format!(
                    "a CFA image with {cpp} samples per pixel"
                )));
            }
            let cfa = &config.cfa;
            if cfa.width != 2 || cfa.height != 2 {
                return Err(PhotometryError::Unsupported(format!(
                    "a {}x{} colour filter array ({}); only 2x2 Bayer is supported",
                    cfa.width, cfa.height, cfa.name
                )));
            }
            let colours: [[usize; 2]; 2] = [
                [cfa.color_at(0, 0), cfa.color_at(0, 1)],
                [cfa.color_at(1, 0), cfa.color_at(1, 1)],
            ];
            if colours.iter().flatten().any(|&c| c > 2) {
                return Err(PhotometryError::Unsupported(
                    "a four-colour sensor".to_owned(),
                ));
            }
            let black = raw.blacklevel.as_bayer_array();
            let white = raw.whitelevel.as_bayer_array();
            let mut plane = Vec::with_capacity(width * height);
            for y in 0..height {
                for x in 0..width {
                    let position = (y & 1) * 2 + (x & 1);
                    let (lo, hi) = (black[position], white[position]);
                    plane.push((data[y * width + x] - lo) / (hi - lo).max(1.0));
                }
            }
            demosaic_bilinear(&plane, width, height, &colours)
        }
        RawPhotometricInterpretation::LinearRaw | RawPhotometricInterpretation::BlackIsZero => {
            if cpp != 1 && cpp != 3 {
                return Err(PhotometryError::Unsupported(format!(
                    "{cpp} samples per pixel"
                )));
            }
            let black = raw.blacklevel.as_vec();
            let white = raw.whitelevel.as_vec();
            let mut pixels = Vec::with_capacity(width * height);
            for i in 0..width * height {
                let mut rgb = [0.0_f32; 3];
                for (c, slot) in rgb.iter_mut().enumerate() {
                    let sample = if cpp == 3 { c } else { 0 };
                    let (lo, hi) = (level_at(&black, sample), level_at(&white, sample));
                    *slot = (data[i * cpp + sample] - lo) / (hi - lo).max(1.0);
                }
                pixels.push(rgb);
            }
            pixels
        }
    };
    let mut image = LinearImage::from_pixels(width, height, pixels, SourceKind::Raw)?;
    image.non_linear_source = false;
    image.colour = Some(colour_of(raw));
    Ok(crop_to_recommended_area(image, raw))
}

/// Cuts the file's recommended crop (else its active area) out of `image`, when it fits.
fn crop_to_recommended_area(image: LinearImage, raw: &RawImage) -> LinearImage {
    let Some(area) = raw.crop_area.or(raw.active_area) else {
        return image;
    };
    let (x0, y0, w, h) = (area.x(), area.y(), area.width(), area.height());
    let fits = w > 0 && h > 0 && x0 + w <= image.width && y0 + h <= image.height;
    if !fits || (w == image.width && h == image.height) {
        return image;
    }
    let mut pixels = Vec::with_capacity(w * h);
    for y in y0..y0 + h {
        let row = y * image.width;
        pixels.extend_from_slice(&image.pixels[row + x0..row + x0 + w]);
    }
    LinearImage {
        width: w,
        height: h,
        pixels,
        ..image
    }
}

/// Bilinear demosaicing of a 2 by 2 mosaic. `colours[row % 2][col % 2]` is 0 for red, 1 for
/// green, 2 for blue.
///
/// A channel missing at a pixel is the mean of that channel's samples in the
/// 3 by 3 neighbourhood (inside the image), taken in a fixed scan order.
#[must_use]
pub fn demosaic_bilinear(
    plane: &[f32],
    width: usize,
    height: usize,
    colours: &[[usize; 2]; 2],
) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let own = colours[y & 1][x & 1];
            let mut sum = [0.0_f32; 3];
            let mut count = [0_u32; 3];
            for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let colour = colours[ny & 1][nx & 1];
                    sum[colour] += plane[ny * width + nx];
                    count[colour] += 1;
                }
            }
            let mut rgb = [0.0_f32; 3];
            for c in 0..3 {
                rgb[c] = if c == own {
                    plane[y * width + x]
                } else if count[c] > 0 {
                    sum[c] / count[c] as f32
                } else {
                    0.0
                };
            }
            out.push(rgb);
        }
    }
    out
}
