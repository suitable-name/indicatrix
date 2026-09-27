//! Lossless encode/decode of the v14 8-bit picture payloads.
//!
//! `DISPLAY_FRAME` (live view, `TransferMode::DisplayOnly`) and `FINAL_IMAGE` (the reply
//! to a `FinalImageRequest`).
//!
//! The pixels are RGBA8, row-major, top row first, `width * height * 4` bytes -- already
//! tone-mapped, so nothing here touches radiance. On the wire they travel as
//! [`DisplayEncoding::Rgba8`] (raw) or [`DisplayEncoding::Png`] (the `png` crate already
//! in the workspace via `image`; no QOI crate is in `Cargo.lock`). PNG is lossless, so the
//! decoded pixels equal the encoded ones byte for byte.
//!
//! # Bounded decoding
//!
//! [`decode_rgba8`] is given the dimensions from the event header and refuses anything
//! else: a PNG whose own `IHDR` disagrees with them, that is not 8-bit RGBA, or whose
//! `width * height * 4` exceeds `framing::MAX_FRAME_LEN`. The output buffer is exactly
//! `width * height * 4` bytes, allocated only after those checks, and the decoder's own
//! working memory is capped by `png::Limits`.

use crate::{framing::MAX_FRAME_LEN, messages::DisplayEncoding};

/// Why a display payload could not be encoded or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayError {
    /// `width * height * 4` exceeds `framing::MAX_FRAME_LEN`.
    TooLarge { width: u32, height: u32 },
    /// The raw RGBA8 bytes are not `width * height * 4` long.
    LengthMismatch {
        expected_bytes: usize,
        got_bytes: usize,
    },
    /// The PNG's own header disagrees with the event header, or is not 8-bit RGBA.
    HeaderMismatch(String),
    /// The PNG codec failed (corrupt data, limits exceeded).
    Codec(String),
    /// This build has no PNG codec (built without the `compression` feature).
    Unsupported(DisplayEncoding),
}

impl std::fmt::Display for DisplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { width, height } => write!(
                f,
                "a {width}x{height} RGBA8 picture would exceed the {MAX_FRAME_LEN} byte frame cap"
            ),
            Self::LengthMismatch {
                expected_bytes,
                got_bytes,
            } => write!(
                f,
                "RGBA8 picture must be {expected_bytes} bytes, got {got_bytes}"
            ),
            Self::HeaderMismatch(m) => write!(f, "PNG header mismatch: {m}"),
            Self::Codec(m) => write!(f, "PNG codec error: {m}"),
            Self::Unsupported(e) => {
                write!(f, "display encoding {e:?} is not supported by this build")
            }
        }
    }
}

impl std::error::Error for DisplayError {}

/// `width * height * 4`, refusing anything over `framing::MAX_FRAME_LEN`.
///
/// # Errors
///
/// [`DisplayError::TooLarge`] past the cap.
pub fn rgba8_len(width: u32, height: u32) -> Result<usize, DisplayError> {
    let len = u64::from(width) * u64::from(height) * 4;
    if len > u64::from(MAX_FRAME_LEN) {
        return Err(DisplayError::TooLarge { width, height });
    }
    Ok(len as usize)
}

/// Encodes `rgba` (`width * height * 4` bytes) as `encoding`.
///
/// # Errors
///
/// [`DisplayError::LengthMismatch`]/[`DisplayError::TooLarge`] for a wrongly sized
/// input, [`DisplayError::Codec`] if the PNG encoder fails, or
/// [`DisplayError::Unsupported`] for PNG on a build without `compression`.
pub fn encode_rgba8(
    encoding: DisplayEncoding,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<Vec<u8>, DisplayError> {
    let expected = rgba8_len(width, height)?;
    if rgba.len() != expected {
        return Err(DisplayError::LengthMismatch {
            expected_bytes: expected,
            got_bytes: rgba.len(),
        });
    }
    match encoding {
        DisplayEncoding::Rgba8 => Ok(rgba.to_vec()),
        DisplayEncoding::Png => encode_png(width, height, rgba),
    }
}

/// Decodes a display payload to exactly `width * height * 4` RGBA8 bytes -- see the
/// module doc comment's "Bounded decoding".
///
/// # Errors
///
/// Any [`DisplayError`] from those checks, or [`DisplayError::Unsupported`] for PNG on a
/// build without `compression`.
pub fn decode_rgba8(
    encoding: DisplayEncoding,
    width: u32,
    height: u32,
    bytes: &[u8],
) -> Result<Vec<u8>, DisplayError> {
    let expected = rgba8_len(width, height)?;
    match encoding {
        DisplayEncoding::Rgba8 => {
            if bytes.len() == expected {
                Ok(bytes.to_vec())
            } else {
                Err(DisplayError::LengthMismatch {
                    expected_bytes: expected,
                    got_bytes: bytes.len(),
                })
            }
        }
        DisplayEncoding::Png => decode_png(width, height, expected, bytes),
    }
}

#[cfg(feature = "compression")]
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, DisplayError> {
    let codec = |e: png::EncodingError| DisplayError::Codec(e.to_string());
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(codec)?;
        writer.write_image_data(rgba).map_err(codec)?;
        writer.finish().map_err(codec)?;
    }
    Ok(out)
}

#[cfg(not(feature = "compression"))]
const fn encode_png(_width: u32, _height: u32, _rgba: &[u8]) -> Result<Vec<u8>, DisplayError> {
    Err(DisplayError::Unsupported(DisplayEncoding::Png))
}

#[cfg(feature = "compression")]
fn decode_png(
    width: u32,
    height: u32,
    expected: usize,
    bytes: &[u8],
) -> Result<Vec<u8>, DisplayError> {
    let codec = |e: png::DecodingError| DisplayError::Codec(e.to_string());
    // Working memory beyond the output buffer: a few rows plus chunk buffers.
    let limits = png::Limits {
        bytes: (expected / 4).clamp(1 << 20, 64 << 20),
    };
    let mut decoder = png::Decoder::new_with_limits(std::io::Cursor::new(bytes), limits);
    let info = decoder.read_header_info().map_err(codec)?;
    if info.width != width
        || info.height != height
        || info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
        || info.interlaced
    {
        return Err(DisplayError::HeaderMismatch(format!(
            "expected {width}x{height} 8-bit RGBA non-interlaced, got {}x{} {:?} {:?} interlaced={}",
            info.width, info.height, info.color_type, info.bit_depth, info.interlaced
        )));
    }
    let mut reader = decoder.read_info().map_err(codec)?;
    let mut out = vec![0_u8; expected];
    let frame = reader.next_frame(&mut out).map_err(codec)?;
    if frame.buffer_size() != expected {
        return Err(DisplayError::LengthMismatch {
            expected_bytes: expected,
            got_bytes: frame.buffer_size(),
        });
    }
    Ok(out)
}

#[cfg(not(feature = "compression"))]
const fn decode_png(
    _width: u32,
    _height: u32,
    _expected: usize,
    _bytes: &[u8],
) -> Result<Vec<u8>, DisplayError> {
    Err(DisplayError::Unsupported(DisplayEncoding::Png))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .flat_map(|i| {
                let v = (i % 251) as u8;
                [v, v.wrapping_mul(3), 255 - v, (i % 7) as u8 * 36]
            })
            .collect()
    }

    #[test]
    fn raw_rgba8_round_trips_and_checks_its_length() {
        let rgba = gradient(5, 3);
        let wire = encode_rgba8(DisplayEncoding::Rgba8, 5, 3, &rgba).unwrap();
        assert_eq!(
            decode_rgba8(DisplayEncoding::Rgba8, 5, 3, &wire).unwrap(),
            rgba
        );
        assert!(matches!(
            decode_rgba8(DisplayEncoding::Rgba8, 5, 4, &wire),
            Err(DisplayError::LengthMismatch { .. })
        ));
    }

    #[cfg(feature = "compression")]
    #[test]
    fn png_round_trips_byte_for_byte() {
        let rgba = gradient(97, 41);
        let png = encode_rgba8(DisplayEncoding::Png, 97, 41, &rgba).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            decode_rgba8(DisplayEncoding::Png, 97, 41, &png).unwrap(),
            rgba
        );
    }

    /// A PNG whose own header disagrees with the event header is refused before the
    /// output buffer is sized from untrusted data.
    #[cfg(feature = "compression")]
    #[test]
    fn a_png_with_other_dimensions_is_refused() {
        let png = encode_rgba8(DisplayEncoding::Png, 8, 8, &gradient(8, 8)).unwrap();
        assert!(matches!(
            decode_rgba8(DisplayEncoding::Png, 16, 16, &png),
            Err(DisplayError::HeaderMismatch(_))
        ));
    }

    #[cfg(feature = "compression")]
    #[test]
    fn corrupt_png_bytes_are_refused() {
        assert!(decode_rgba8(DisplayEncoding::Png, 2, 2, &[0x89, b'P', b'N', b'G']).is_err());
    }

    #[test]
    fn oversize_dimensions_are_refused_before_allocating() {
        assert_eq!(
            decode_rgba8(DisplayEncoding::Rgba8, 100_000, 100_000, &[]),
            Err(DisplayError::TooLarge {
                width: 100_000,
                height: 100_000
            })
        );
    }

    #[cfg(not(feature = "compression"))]
    #[test]
    fn png_is_unsupported_without_compression() {
        assert_eq!(
            encode_rgba8(DisplayEncoding::Png, 1, 1, &[0; 4]),
            Err(DisplayError::Unsupported(DisplayEncoding::Png))
        );
    }
}
