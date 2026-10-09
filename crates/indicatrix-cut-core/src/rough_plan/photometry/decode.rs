//! Decoding photos into [`LinearImage`]s: JPEG, PNG and TIFF here, camera RAW in [`super::raw`].
//!
//! Rules:
//!
//! - 8-bit sources go through the inverse sRGB EOTF and carry `non_linear_source = true`.
//! - 16-bit sources are taken as linear when their ICC profile says so (a gamma of 1.0), else
//!   decoded with the sRGB curve and flagged like the 8-bit ones.
//! - Nothing is clamped, white-balanced or colour-converted.

use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageReader};

use super::{
    exif::parse_jpeg_exif,
    linear::{CaptureMeta, LinearImage, PhotometryError, SourceKind},
    raw::decode_raw_file,
    srgb::{srgb_to_linear, srgb8_table},
};

/// Decodes a photo file, choosing the decoder from its extension.
///
/// `jpg`, `jpeg`, `png`, `tif` and `tiff` go to the `image` crate, anything else is tried as a
/// camera RAW first and, if rawler refuses it, as an ordinary image.
///
/// # Errors
///
/// [`PhotometryError`] when the file cannot be read or decoded.
pub fn decode_file(path: &Path) -> Result<LinearImage, PhotometryError> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" | "png" | "tif" | "tiff" => decode_standard_file(path),
        _ => decode_raw_file(path)
            .or_else(|raw_error| decode_standard_file(path).map_err(|_| raw_error)),
    }
}

/// Decodes a JPEG, PNG or TIFF file.
///
/// # Errors
///
/// [`PhotometryError::Io`] or [`PhotometryError::Decode`].
pub fn decode_standard_file(path: &Path) -> Result<LinearImage, PhotometryError> {
    let bytes = std::fs::read(path).map_err(|e| PhotometryError::Io(e.to_string()))?;
    decode_standard_bytes(&bytes)
}

/// Decodes the bytes of a JPEG, PNG or TIFF file.
///
/// # Errors
///
/// [`PhotometryError::Decode`].
pub fn decode_standard_bytes(bytes: &[u8]) -> Result<LinearImage, PhotometryError> {
    let reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| PhotometryError::Io(e.to_string()))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| PhotometryError::Decode(e.to_string()))?;
    let icc = decoder.icc_profile().ok().flatten();
    let picture =
        DynamicImage::from_decoder(decoder).map_err(|e| PhotometryError::Decode(e.to_string()))?;
    let meta = parse_jpeg_exif(bytes);
    linear_from_dynamic(&picture, icc.as_deref(), meta)
}

/// The linear image of an already decoded picture, with the ICC profile it came with.
///
/// # Errors
///
/// [`PhotometryError::Invalid`] for an empty picture.
pub fn linear_from_dynamic(
    picture: &DynamicImage,
    icc: Option<&[u8]>,
    meta: CaptureMeta,
) -> Result<LinearImage, PhotometryError> {
    use image::ColorType;
    let (width, height) = (picture.width() as usize, picture.height() as usize);
    let sixteen = matches!(
        picture.color(),
        ColorType::L16 | ColorType::La16 | ColorType::Rgb16 | ColorType::Rgba16
    );
    let mut linear = if sixteen {
        let rgb = picture.to_rgb16();
        let linear_icc = icc.is_some_and(icc_is_linear);
        let mut out = from_rgb16(width, height, rgb.as_raw(), linear_icc)?;
        out.source = SourceKind::Tiff16;
        out
    } else {
        let rgb = picture.to_rgb8();
        from_srgb8(width, height, rgb.as_raw())?
    };
    linear.meta = meta;
    Ok(linear)
}

/// A [`LinearImage`] from interleaved 8-bit sRGB samples.
///
/// # Errors
///
/// [`PhotometryError::Invalid`] when the sample count does not match the size.
pub fn from_srgb8(
    width: usize,
    height: usize,
    samples: &[u8],
) -> Result<LinearImage, PhotometryError> {
    if samples.len() != width * height * 3 {
        return Err(PhotometryError::Invalid(format!(
            "{} samples for a {width}x{height} RGB image",
            samples.len()
        )));
    }
    let table = srgb8_table();
    let pixels = samples
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| {
            [
                table[usize::from(c[0])],
                table[usize::from(c[1])],
                table[usize::from(c[2])],
            ]
        })
        .collect();
    LinearImage::from_pixels(width, height, pixels, SourceKind::Rgb8)
}

/// A [`LinearImage`] from interleaved 16-bit samples. `linear` says the values already are
/// linear light; otherwise they are sRGB-encoded and the image is flagged non-linear.
///
/// # Errors
///
/// [`PhotometryError::Invalid`] when the sample count does not match the size.
pub fn from_rgb16(
    width: usize,
    height: usize,
    samples: &[u16],
    linear: bool,
) -> Result<LinearImage, PhotometryError> {
    if samples.len() != width * height * 3 {
        return Err(PhotometryError::Invalid(format!(
            "{} samples for a {width}x{height} RGB image",
            samples.len()
        )));
    }
    let decode = |code: u16| {
        let unit = f32::from(code) / 65535.0;
        if linear { unit } else { srgb_to_linear(unit) }
    };
    let pixels = samples
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [decode(c[0]), decode(c[1]), decode(c[2])])
        .collect();
    let mut image = LinearImage::from_pixels(width, height, pixels, SourceKind::Tiff16)?;
    image.non_linear_source = !linear;
    Ok(image)
}

/// Whether an ICC profile describes linear light: its tone curve (`rTRC`, or `kTRC` for a grey
/// profile) is the identity or a gamma of 1.0, or its text says "linear".
#[must_use]
pub fn icc_is_linear(profile: &[u8]) -> bool {
    let be32 = |at: usize| -> Option<u32> {
        let b: [u8; 4] = profile.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(u32::from_be_bytes(b))
    };
    let be16 = |at: usize| -> Option<u16> {
        let b: [u8; 2] = profile.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(u16::from_be_bytes(b))
    };
    if let Some(tags) = be32(128) {
        for i in 0..(tags as usize).min(256) {
            let entry = 132 + 12 * i;
            let (Some(sig), Some(offset)) = (be32(entry), be32(entry + 4)) else {
                break;
            };
            if sig != u32::from_be_bytes(*b"rTRC") && sig != u32::from_be_bytes(*b"kTRC") {
                continue;
            }
            let at = offset as usize;
            let Some(kind) = be32(at) else {
                break;
            };
            if kind == u32::from_be_bytes(*b"curv") {
                return match be32(at + 8) {
                    Some(0) => true,
                    // One entry: a u8Fixed8 gamma; 256 is 1.0.
                    Some(1) => be16(at + 12) == Some(256),
                    _ => false,
                };
            }
            if kind == u32::from_be_bytes(*b"para") {
                // Parametric curve: function type 0 is `Y = X^g` with g an s15Fixed16.
                let function = be16(at + 8);
                let gamma = be32(at + 12).map(|raw| f64::from(raw as i32) / 65536.0);
                return function == Some(0) && gamma.is_some_and(|g| (g - 1.0).abs() < 1e-3);
            }
            break;
        }
    }
    let lowered: Vec<u8> = profile.iter().map(u8::to_ascii_lowercase).collect();
    lowered.windows(6).any(|w| w == b"linear")
}
