//! Codec for the per-frame radiance buffer.
//!
//! `Vec<Vec3>`, one running XYZ sum per pixel -- the other half of the wire protocol
//! besides [`crate::scene::SceneState`].
//!
//! Raw POD bytes via `bytemuck::cast_slice`, not `postcard`: unlike `SceneState` (a few
//! kilobytes, sent once per camera change, where serialisation cost doesn't matter), the
//! radiance buffer is potentially megapixels of `Vec3` sent every sample batch on the
//! hot path, and is already a flat float array -- a serialization framework would only
//! add per-element framing and copies.
//!
//! Requires glam's `bytemuck` cargo feature (`indicatrix-net`'s `Cargo.toml` enables
//! it), since `glam::Vec3` isn't `bytemuck::Pod` otherwise. `Vec3` on this target is
//! exactly `3 * size_of::<f32>()` bytes, `f32`-aligned, no padding (unlike the
//! SIMD-aligned `Vec3A`), so the cast is a straight reinterpretation.
//!
//! # v14 payload encodings
//!
//! On the wire a payload may also be byte-shuffled and compressed losslessly
//! ([`payload`], [`shuffle`]); the functions in this file handle the raw form only.
//! A receiver decodes a `FRAME`/`PREVIEW` through [`payload::PayloadDecoder`] (as
//! `crate::client::Accumulator` does), which dispatches on the header's `encoding` and
//! enforces the bounded-decode rules documented there.

/// Bounded-decode payload codec (v14 lossless encodings) -- see this module's "v14
/// payload encodings" doc section.
pub mod payload;
#[cfg(test)]
mod payload_tests;
/// Byte-shuffles the raw `f32` planes before compression (v14) -- see this module's
/// "v14 payload encodings" doc section.
pub mod shuffle;
#[cfg(test)]
mod test_support;

pub use payload::{EncodedPayload, PayloadDecoder, PayloadEncoder, decode_payload};

use crate::messages::PayloadEncoding;
use glam::Vec3;

/// Byte size of one radiance sample on the wire (one pixel's running XYZ sum).
pub const BYTES_PER_PIXEL: usize = size_of::<Vec3>();

/// Reinterprets a radiance buffer as raw little-endian POD bytes -- no allocation, no copy.
///
/// Ready to hand straight to a writer as a `FRAME`/`PREVIEW` message's payload: just a
/// reinterpreted view over `buffer`'s own storage. The zero-copy counterpart of
/// [`encode`]: `encode` exists only as a thin `.to_vec()` wrapper over this now, kept
/// for callers (and tests) that genuinely need an owned buffer. A hot path writing a
/// fresh delta every tick (`stream_emit::emitter`) should call this directly instead --
/// `encode` there meant a full-frame `Vec<u8>` allocation and `memcpy` per tick purely
/// to hand the bytes to a writer and drop them immediately after.
#[must_use]
pub fn as_bytes(buffer: &[Vec3]) -> &[u8] {
    bytemuck::cast_slice(buffer)
}

/// Encodes a radiance buffer as raw little-endian POD bytes, ready to send as a
/// `FRAME` message's `xyz_bytes` payload. The inverse of [`decode`].
///
/// A thin `.to_vec()` wrapper over [`as_bytes`] -- see that function's doc comment for
/// the zero-copy alternative a hot path should prefer.
#[must_use]
pub fn encode(buffer: &[Vec3]) -> Vec<u8> {
    as_bytes(buffer).to_vec()
}

/// Why a decoded radiance payload was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadianceError {
    /// `bytes.len()` didn't equal `width * height` times [`BYTES_PER_PIXEL`]. Covers
    /// short, long, AND empty payloads uniformly (empty against nonzero dimensions is
    /// just `expected != 0, got == 0`).
    LengthMismatch {
        width: u32,
        height: u32,
        expected_bytes: usize,
        got_bytes: usize,
    },
    /// `bytes` was the right length but not aligned for a `&[Vec3]` reinterpretation.
    /// Rejected rather than worked around: reinterpreting misaligned bytes as `Vec3`
    /// would be UB.
    Misaligned,
    /// v14: `width * height * 12` exceeds `framing::MAX_FRAME_LEN`, the cap every raw
    /// payload is already held to -- rejected before allocating anything.
    TooLarge { width: u32, height: u32 },
    /// v14: the header's `raw_len` disagrees with `width * height * 12` (a lying or
    /// mismatched header) -- rejected before any decompression starts.
    RawLenMismatch {
        width: u32,
        height: u32,
        expected_bytes: usize,
        raw_len: u32,
    },
    /// v14: the header names an encoding this build cannot decode (built without the
    /// `compression` feature).
    UnsupportedEncoding(PayloadEncoding),
    /// v14: the codec rejected the payload -- corrupt data, or output that would exceed
    /// `raw_len` (a decompression bomb). The codec's own message is logged at `warn!`.
    DecompressFailed(PayloadEncoding),
    /// v14: the payload decompressed to fewer than `raw_len` bytes.
    ShortOutput {
        expected_bytes: usize,
        got_bytes: usize,
    },
    /// A `FRAME` whose `[first_sample, first_sample + samples)` lies outside the sample
    /// range `[start, end)` the request asked for -- a peer tracing indices it was never
    /// assigned.
    FrameOutsideRange {
        first_sample: u32,
        samples: u32,
        start: u32,
        end: u32,
    },
    /// A `FRAME` that would bring the epoch's cumulative sample count over the requested
    /// range's width -- an overlapping or over-reported contribution.
    SampleCountOverrun { cumulative: u32, range_len: u32 },
    /// A `FRAME` that claims `samples == 0` yet carries a payload.
    ZeroSampleFrame,
}

impl std::fmt::Display for RadianceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthMismatch {
                width,
                height,
                expected_bytes,
                got_bytes,
            } => write!(
                f,
                "radiance payload for {width}x{height} must be {expected_bytes} bytes, got {got_bytes}"
            ),
            Self::Misaligned => write!(f, "radiance payload is not f32-aligned"),
            Self::TooLarge { width, height } => write!(
                f,
                "a {width}x{height} radiance payload would exceed the {} byte frame cap",
                crate::framing::MAX_FRAME_LEN
            ),
            Self::RawLenMismatch {
                width,
                height,
                expected_bytes,
                raw_len,
            } => write!(
                f,
                "header raw_len {raw_len} disagrees with {width}x{height} = {expected_bytes} bytes"
            ),
            Self::UnsupportedEncoding(e) => {
                write!(f, "payload encoding {e:?} is not supported by this build")
            }
            Self::DecompressFailed(e) => write!(
                f,
                "{e:?} payload failed to decompress within its declared raw_len"
            ),
            Self::ShortOutput {
                expected_bytes,
                got_bytes,
            } => write!(
                f,
                "payload decompressed to {got_bytes} bytes, raw_len is {expected_bytes}"
            ),
            Self::FrameOutsideRange {
                first_sample,
                samples,
                start,
                end,
            } => write!(
                f,
                "FRAME [{first_sample}, +{samples}) lies outside the requested sample range [{start}, {end})"
            ),
            Self::SampleCountOverrun {
                cumulative,
                range_len,
            } => write!(
                f,
                "FRAME would bring the cumulative sample count to {cumulative}, over the requested range's width {range_len}"
            ),
            Self::ZeroSampleFrame => write!(f, "FRAME claims zero samples but carries a payload"),
        }
    }
}

impl std::error::Error for RadianceError {}

/// Decodes a `FRAME` message's raw `xyz_bytes` payload back into a radiance buffer.
/// Validates its length against the frame's declared `width * height` before
/// accumulating a single sample.
///
/// # Errors
///
/// Returns [`RadianceError::LengthMismatch`] if `bytes.len()` isn't exactly
/// `width * height` times [`BYTES_PER_PIXEL`], or [`RadianceError::Misaligned`] if
/// `bytes` is the right length but not aligned for reinterpretation as `&[Vec3]`.
pub fn decode(bytes: &[u8], width: u32, height: u32) -> Result<Vec<Vec3>, RadianceError> {
    decode_borrowed(bytes, width, height).map(<[Vec3]>::to_vec)
}

/// The zero-copy counterpart of [`decode`]: same validation, but returns a borrow.
///
/// Validates `bytes` identically to [`decode`] (same length checks, same zero-area
/// short-circuit, same [`RadianceError`] conditions) but returns a borrowed `&[Vec3]`
/// reinterpreting `bytes` in place rather than an owned, freshly allocated `Vec<Vec3>`.
/// Intended for a caller that only needs to read the decoded buffer once and
/// immediately (e.g. summing it into an accumulator) -- `decode`'s `to_vec()` is a full
/// extra copy of the whole frame that such a caller never needed in the first place.
///
/// # Errors
///
/// Identical conditions to [`decode`]: [`RadianceError::LengthMismatch`] or
/// [`RadianceError::Misaligned`].
pub fn decode_borrowed(bytes: &[u8], width: u32, height: u32) -> Result<&[Vec3], RadianceError> {
    let expected_pixels = width as usize * height as usize;
    let expected_bytes = expected_pixels * BYTES_PER_PIXEL;
    if bytes.len() != expected_bytes {
        return Err(RadianceError::LengthMismatch {
            width,
            height,
            expected_bytes,
            got_bytes: bytes.len(),
        });
    }
    if expected_pixels == 0 {
        // `try_cast_slice` on a zero-length slice can report a harmless alignment
        // mismatch on its dangling pointer; short-circuit to avoid masking a real
        // length-mismatch failure.
        return Ok(&[]);
    }
    bytemuck::try_cast_slice::<u8, Vec3>(bytes).map_or(Err(RadianceError::Misaligned), Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_pixel_matches_three_f32s() {
        assert_eq!(BYTES_PER_PIXEL, 12);
    }

    #[test]
    fn as_bytes_matches_encode() {
        let buffer = vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(-0.5, f32::MAX, 0.0)];
        assert_eq!(as_bytes(&buffer), encode(&buffer).as_slice());
    }

    #[test]
    fn round_trips_bit_exactly() {
        let buffer = vec![
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(-0.5, f32::MAX, 0.0),
            Vec3::ZERO,
        ];
        let bytes = encode(&buffer);
        let decoded = decode(&bytes, 3, 1).unwrap();
        assert_eq!(buffer, decoded);
    }

    #[test]
    fn decode_borrowed_agrees_with_decode() {
        let buffer = vec![
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(-0.5, f32::MAX, 0.0),
            Vec3::ZERO,
        ];
        let bytes = encode(&buffer);
        let owned = decode(&bytes, 3, 1).unwrap();
        let borrowed = decode_borrowed(&bytes, 3, 1).unwrap();
        assert_eq!(owned.as_slice(), borrowed);
    }

    #[test]
    fn decode_borrowed_rejects_short_payload() {
        let buffer = vec![Vec3::ONE; 4];
        let bytes = encode(&buffer);
        let short = &bytes[..bytes.len() - 1];
        assert!(matches!(
            decode_borrowed(short, 2, 2),
            Err(RadianceError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn decode_borrowed_accepts_empty_payload_for_zero_area() {
        assert_eq!(decode_borrowed(&[], 0, 0).unwrap(), &[] as &[Vec3]);
    }

    #[test]
    fn rejects_short_payload() {
        let buffer = vec![Vec3::ONE; 4];
        let bytes = encode(&buffer);
        let short = &bytes[..bytes.len() - 1];
        assert!(matches!(
            decode(short, 2, 2),
            Err(RadianceError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn rejects_long_payload() {
        let buffer = vec![Vec3::ONE; 4];
        let mut bytes = encode(&buffer);
        bytes.push(0);
        assert!(matches!(
            decode(&bytes, 2, 2),
            Err(RadianceError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn rejects_empty_payload_against_nonzero_dimensions() {
        assert!(matches!(
            decode(&[], 2, 2),
            Err(RadianceError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn accepts_empty_payload_for_zero_area() {
        assert_eq!(decode(&[], 0, 0).unwrap(), Vec::<Vec3>::new());
    }
}
