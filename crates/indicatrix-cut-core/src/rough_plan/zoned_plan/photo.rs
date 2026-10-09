//! The cached working-resolution photos of a rough colour (owner decision 7).
//!
//! Packing the per-view [`ResampledImage`] the fit works on into a few byte blobs the vault stores, and
//! back, so a saved plan reopens its colour fit without the original RAW or JPEG files.
//!
//! # Layout
//!
//! One view is five blobs (all row-major, little-endian, at the working size):
//!
//! | kind | channels | encoding | content |
//! |---|---|---|---|
//! | `transmittance` | 3 | `f16le` | camera RGB relative to the empty rig |
//! | `variance` | 3 | `f32le` | its variance (infinite where nothing valid lay under a pixel) |
//! | `coverage` | 1 | `f16le` | the valid share of each footprint |
//! | `mask` | 1 | `u8` | the pixel flags (`photometry::flag`) |
//! | `grid` | - | `json` | the working grid (size, origin, scale) |
//!
//! The vault has no compressor (its only dependencies are SQLite and serde), so the blobs are
//! stored as they are: a 384 x 384 view is about 0.9 MB for the transmittance, 1.8 MB for the
//! variance and 0.3 MB for coverage, mask and grid, the "about 2 MB per view" of the owner
//! decision when the variance is dropped by the caller (the fit can rebuild it from the noise
//! model), or about 3 MB with it. Half precision costs at most 0.05 % relative error on a
//! transmittance in `[0, 1]`, far below the photo noise.

use crate::rough_plan::photometry::{PixelMask, ResampledImage, WorkingGrid};
use std::fmt;

/// The blob kinds of one view.
pub mod kind {
    /// Camera RGB relative to the empty rig.
    pub const TRANSMITTANCE: &str = "transmittance";
    /// The variance of the transmittance.
    pub const VARIANCE: &str = "variance";
    /// The valid share of each footprint.
    pub const COVERAGE: &str = "coverage";
    /// The pixel flags.
    pub const MASK: &str = "mask";
    /// The working grid.
    pub const GRID: &str = "grid";
}

/// How a blob's bytes are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoEncoding {
    /// IEEE half floats, little-endian.
    F16,
    /// IEEE single floats, little-endian.
    F32,
    /// One byte per sample.
    U8,
    /// UTF-8 JSON text.
    Json,
}

impl PhotoEncoding {
    /// The name the vault stores.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::F16 => "f16le",
            Self::F32 => "f32le",
            Self::U8 => "u8",
            Self::Json => "json",
        }
    }

    /// The encoding a stored name denotes.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "f16le" => Some(Self::F16),
            "f32le" => Some(Self::F32),
            "u8" => Some(Self::U8),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    /// Bytes per sample, `None` for text.
    const fn sample_bytes(self) -> Option<usize> {
        match self {
            Self::F16 => Some(2),
            Self::F32 => Some(4),
            Self::U8 => Some(1),
            Self::Json => None,
        }
    }
}

/// One stored blob of a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoBlob {
    /// What it is, one of [`kind`].
    pub kind: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How `data` is laid out.
    pub encoding: PhotoEncoding,
    /// The bytes.
    pub data: Vec<u8>,
}

/// Why a view's blobs could not be packed or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhotoCodecError {
    /// A blob of this kind is missing.
    Missing(&'static str),
    /// A blob has the wrong size for its pixels.
    Size {
        /// The blob.
        kind: &'static str,
        /// Bytes expected.
        expected: usize,
        /// Bytes found.
        found: usize,
    },
    /// A blob has an unexpected encoding.
    Encoding(&'static str),
    /// The grid text is not usable.
    Grid(String),
    /// The image is larger than a cache can hold.
    TooLarge,
}

impl fmt::Display for PhotoCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(kind) => write!(f, "the cached {kind} image is missing"),
            Self::Size {
                kind,
                expected,
                found,
            } => write!(
                f,
                "the cached {kind} image has {found} bytes, expected {expected}"
            ),
            Self::Encoding(kind) => write!(f, "the cached {kind} image has an unexpected encoding"),
            Self::Grid(message) => write!(f, "the cached working grid is not usable: {message}"),
            Self::TooLarge => write!(f, "the working image is too large to cache"),
        }
    }
}

impl std::error::Error for PhotoCodecError {}

/// The most pixels a cached view may have (a working grid is about 384 x 384).
pub const MAX_CACHED_PIXELS: usize = 4096 * 4096;

/// The IEEE binary16 bits of `value`, rounded to nearest even. Overflow gives infinity, and
/// infinities and `NaN`s survive.
#[must_use]
pub const fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;
    if exponent == 0xff {
        return sign | 0x7c00 | if mantissa == 0 { 0 } else { 0x0200 };
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7c00;
    }
    if unbiased >= -14 {
        let mut half = (((unbiased + 15) as u32) << 10) | (mantissa >> 13);
        let round = mantissa & 0x1fff;
        if round > 0x1000 || (round == 0x1000 && half & 1 == 1) {
            half += 1;
        }
        return sign | half as u16;
    }
    if unbiased >= -25 {
        let full = mantissa | 0x0080_0000;
        let shift = (-unbiased - 1) as u32;
        let mut half = full >> shift;
        let remainder = full & ((1_u32 << shift) - 1);
        let halfway = 1_u32 << (shift - 1);
        if remainder > halfway || (remainder == halfway && half & 1 == 1) {
            half += 1;
        }
        return sign | half as u16;
    }
    sign
}

/// The value of IEEE binary16 bits.
#[must_use]
pub fn f16_bits_to_f32(half: u16) -> f32 {
    let sign = u32::from(half & 0x8000) << 16;
    let exponent = u32::from((half >> 10) & 0x1f);
    let mantissa = u32::from(half & 0x03ff);
    match exponent {
        0 => {
            // 2^-24, the value of the smallest subnormal.
            let unit = f32::from_bits(0x3380_0000);
            let magnitude = mantissa as f32 * unit;
            if sign == 0 { magnitude } else { -magnitude }
        }
        31 => f32::from_bits(sign | 0x7f80_0000 | (mantissa << 13)),
        _ => f32::from_bits(sign | ((exponent + 112) << 23) | (mantissa << 13)),
    }
}

/// `samples` as bytes in `encoding` (`Json` is not a sample encoding and gives no bytes).
/// `U8` rounds and clamps to `0..=255`.
#[must_use]
pub fn encode_samples(samples: &[f32], encoding: PhotoEncoding) -> Vec<u8> {
    match encoding {
        PhotoEncoding::F16 => samples
            .iter()
            .flat_map(|&v| f32_to_f16_bits(v).to_le_bytes())
            .collect(),
        PhotoEncoding::F32 => samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
        PhotoEncoding::U8 => samples
            .iter()
            .map(|&v| v.round().clamp(0.0, 255.0) as u8)
            .collect(),
        PhotoEncoding::Json => Vec::new(),
    }
}

/// The samples of `bytes`, `count` of them.
///
/// # Errors
///
/// [`PhotoCodecError::Size`] when the byte count does not match, [`PhotoCodecError::Encoding`]
/// for `Json`.
pub fn decode_samples(
    bytes: &[u8],
    encoding: PhotoEncoding,
    count: usize,
    kind: &'static str,
) -> Result<Vec<f32>, PhotoCodecError> {
    let width = encoding
        .sample_bytes()
        .ok_or(PhotoCodecError::Encoding(kind))?;
    let expected = count.checked_mul(width).ok_or(PhotoCodecError::TooLarge)?;
    if bytes.len() != expected {
        return Err(PhotoCodecError::Size {
            kind,
            expected,
            found: bytes.len(),
        });
    }
    Ok(match encoding {
        PhotoEncoding::F16 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f16_bits_to_f32(u16::from_le_bytes([c[0], c[1]])))
            .collect(),
        PhotoEncoding::F32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        PhotoEncoding::U8 => bytes.iter().map(|&b| f32::from(b)).collect(),
        PhotoEncoding::Json => Vec::new(),
    })
}

/// The grid as JSON text.
fn grid_to_text(grid: &WorkingGrid) -> String {
    serde_json::json!({
        "width": grid.width,
        "height": grid.height,
        "origin": grid.origin,
        "scale": grid.scale,
    })
    .to_string()
}

/// The grid of its JSON text.
fn grid_from_text(text: &[u8]) -> Result<WorkingGrid, PhotoCodecError> {
    let bad = |what: &str| PhotoCodecError::Grid(what.to_string());
    let value: serde_json::Value =
        serde_json::from_slice(text).map_err(|e| PhotoCodecError::Grid(e.to_string()))?;
    let size = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| bad(key))
    };
    let number = |v: &serde_json::Value| v.as_f64().filter(|n| n.is_finite());
    let origin = value
        .get("origin")
        .and_then(serde_json::Value::as_array)
        .filter(|a| a.len() == 2)
        .and_then(|a| Some([number(&a[0])?, number(&a[1])?]))
        .ok_or_else(|| bad("origin"))?;
    let scale = value
        .get("scale")
        .and_then(number)
        .filter(|s| *s > 0.0)
        .ok_or_else(|| bad("scale"))?;
    Ok(WorkingGrid {
        width: size("width")?,
        height: size("height")?,
        origin,
        scale,
    })
}

fn dimension(value: usize) -> Result<u32, PhotoCodecError> {
    u32::try_from(value).map_err(|_| PhotoCodecError::TooLarge)
}

/// Packs one view's working-resolution image into its blobs (see the module table).
///
/// # Errors
///
/// [`PhotoCodecError::TooLarge`] for an image above [`MAX_CACHED_PIXELS`] and
/// [`PhotoCodecError::Size`] when the image's arrays do not match its grid.
pub fn blobs_from_resampled(image: &ResampledImage) -> Result<Vec<PhotoBlob>, PhotoCodecError> {
    let pixels = image
        .grid
        .width
        .checked_mul(image.grid.height)
        .filter(|&p| p <= MAX_CACHED_PIXELS)
        .ok_or(PhotoCodecError::TooLarge)?;
    let check = |kind: &'static str, found: usize| {
        if found == pixels {
            Ok(())
        } else {
            Err(PhotoCodecError::Size {
                kind,
                expected: pixels,
                found,
            })
        }
    };
    check(kind::TRANSMITTANCE, image.values.len())?;
    check(kind::VARIANCE, image.variance.len())?;
    check(kind::COVERAGE, image.coverage.len())?;
    check(kind::MASK, image.mask.bits().len())?;
    let width = dimension(image.grid.width)?;
    let height = dimension(image.grid.height)?;
    let flat = |rows: &[[f32; 3]]| -> Vec<f32> { rows.iter().flatten().copied().collect() };
    let blob = |kind: &str, encoding: PhotoEncoding, data: Vec<u8>| PhotoBlob {
        kind: kind.to_string(),
        width,
        height,
        encoding,
        data,
    };
    let mask: Vec<f32> = image.mask.bits().iter().map(|&b| f32::from(b)).collect();
    Ok(vec![
        blob(
            kind::TRANSMITTANCE,
            PhotoEncoding::F16,
            encode_samples(&flat(&image.values), PhotoEncoding::F16),
        ),
        blob(
            kind::VARIANCE,
            PhotoEncoding::F32,
            encode_samples(&flat(&image.variance), PhotoEncoding::F32),
        ),
        blob(
            kind::COVERAGE,
            PhotoEncoding::F16,
            encode_samples(&image.coverage, PhotoEncoding::F16),
        ),
        blob(
            kind::MASK,
            PhotoEncoding::U8,
            encode_samples(&mask, PhotoEncoding::U8),
        ),
        blob(
            kind::GRID,
            PhotoEncoding::Json,
            grid_to_text(&image.grid).into_bytes(),
        ),
    ])
}

/// The working-resolution image of one view's blobs.
///
/// # Errors
///
/// A missing blob, a wrong encoding or size, or an unusable grid.
pub fn resampled_from_blobs(blobs: &[PhotoBlob]) -> Result<ResampledImage, PhotoCodecError> {
    let find = |kind: &'static str| {
        blobs
            .iter()
            .find(|b| b.kind == kind)
            .ok_or(PhotoCodecError::Missing(kind))
    };
    let grid_blob = find(kind::GRID)?;
    if grid_blob.encoding != PhotoEncoding::Json {
        return Err(PhotoCodecError::Encoding(kind::GRID));
    }
    let grid = grid_from_text(&grid_blob.data)?;
    let pixels = grid
        .width
        .checked_mul(grid.height)
        .filter(|&p| p <= MAX_CACHED_PIXELS)
        .ok_or(PhotoCodecError::TooLarge)?;
    let samples = |kind: &'static str, expected: PhotoEncoding, channels: usize| {
        let blob = find(kind)?;
        if blob.encoding != expected {
            return Err(PhotoCodecError::Encoding(kind));
        }
        decode_samples(&blob.data, expected, pixels * channels, kind)
    };
    let rgb = |flat: Vec<f32>| -> Vec<[f32; 3]> {
        flat.as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect()
    };
    let values = rgb(samples(kind::TRANSMITTANCE, PhotoEncoding::F16, 3)?);
    let variance = rgb(samples(kind::VARIANCE, PhotoEncoding::F32, 3)?);
    let coverage = samples(kind::COVERAGE, PhotoEncoding::F16, 1)?;
    let mask_bytes = samples(kind::MASK, PhotoEncoding::U8, 1)?;
    let mut mask = PixelMask::new(grid.width, grid.height);
    for (index, &flags) in mask_bytes.iter().enumerate() {
        if flags > 0.0 {
            mask.set(index % grid.width, index / grid.width, flags as u8);
        }
    }
    Ok(ResampledImage {
        grid,
        values,
        variance,
        coverage,
        mask,
    })
}
