//! The v14 lossless payload codecs for `FRAME`/`PREVIEW` radiance.
//!
//! [`PayloadEncoder`] on the sending side, [`PayloadDecoder`] on the receiving side. See
//! `crate::messages::encoding` for how a connection picks its [`PayloadEncoding`].
//!
//! # Encoding
//!
//! `ShuffleZstd`/`ShuffleLz4` byte-shuffle the raw `f32` bytes (`super::shuffle`), then
//! compress the planes with one zstd frame (the one-shot `bulk` API, one reused
//! compression context per encoder) or one LZ4 block. An encoder falls back to sending a
//! single payload `Raw` whenever compression would not make it smaller (or, which never
//! happens with the bounded output buffers used here, a codec reports an error), and
//! says so in the header -- a receiver dispatches on each header's own `encoding`.
//!
//! # Bounded decoding (the coordinator is internet-facing)
//!
//! Every decode runs these checks, in this order, before trusting a byte:
//!
//! 1. The decoded size the RECEIVER expects, `width * height * 12` (its own frame size
//!    for a FRAME, the header's dimensions for a PREVIEW), must not exceed
//!    `framing::MAX_FRAME_LEN` -- the same cap a raw payload is held to
//!    ([`RadianceError::TooLarge`]).
//! 2. The header's `raw_len` must equal that expected size
//!    ([`RadianceError::RawLenMismatch`]); a lying header is rejected before any
//!    decompression starts.
//! 3. The payload is decompressed into a buffer of EXACTLY `raw_len` bytes. zstd's
//!    one-shot `decompress_to_buffer` and `lz4_flex`'s safe `decompress_into` both fail
//!    rather than write past it, so an expansion bomb fails
//!    ([`RadianceError::DecompressFailed`]) with memory bounded by `raw_len`, whatever
//!    window size or content size a hostile frame declares.
//! 4. Output shorter than `raw_len` is rejected ([`RadianceError::ShortOutput`]).
//!
//! `framing::MAX_FRAME_LEN` still bounds the compressed bytes themselves, since they
//! arrive as an ordinary frame.

use super::{BYTES_PER_PIXEL, RadianceError, decode_borrowed, shuffle};
use crate::{framing::MAX_FRAME_LEN, messages::PayloadEncoding};
use glam::Vec3;

/// One payload as it goes on the wire: its encoding, its decoded size, and the bytes to
/// send. Built by [`PayloadEncoder::encode`]; turned into a header by
/// `FrameHeader::for_encoded`/`PreviewHeader::for_encoded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedPayload<'a> {
    /// The encoding of [`Self::bytes`] -- the encoder's configured one, or `Raw` when it
    /// fell back for this payload.
    pub encoding: PayloadEncoding,
    /// The payload's decoded size in bytes.
    pub raw_len: u32,
    /// The bytes to send (`payload_len` is their length).
    pub bytes: &'a [u8],
}

/// Encodes radiance payloads with one [`PayloadEncoding`], reusing its scratch buffers
/// and compression context across calls (one per emitter, not per frame).
pub struct PayloadEncoder {
    encoding: PayloadEncoding,
    #[cfg(feature = "compression")]
    planes: Vec<u8>,
    wire: Vec<u8>,
    #[cfg(feature = "compression")]
    zstd: Option<zstd::bulk::Compressor<'static>>,
}

impl std::fmt::Debug for PayloadEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayloadEncoder")
            .field("encoding", &self.encoding)
            .finish_non_exhaustive()
    }
}

impl PayloadEncoder {
    /// An encoder for `encoding`. An encoding this build does not support
    /// ([`PayloadEncoding::is_supported`]) becomes `Raw` -- a negotiated encoding is
    /// always supported, so this only matters for a caller passing one by hand.
    #[must_use]
    pub const fn new(encoding: PayloadEncoding) -> Self {
        let encoding = if encoding.is_supported() {
            encoding
        } else {
            PayloadEncoding::Raw
        };
        Self {
            encoding,
            #[cfg(feature = "compression")]
            planes: Vec::new(),
            wire: Vec::new(),
            #[cfg(feature = "compression")]
            zstd: None,
        }
    }

    /// The encoding this encoder was built for (individual payloads may still go `Raw`).
    #[must_use]
    pub const fn encoding(&self) -> PayloadEncoding {
        self.encoding
    }

    /// Encodes `raw` (little-endian `f32` bytes, e.g. `radiance::as_bytes`). Zero-copy
    /// for `Raw`; otherwise the returned bytes borrow this encoder's scratch buffer until
    /// the next call. Never fails: see the module doc comment on the per-payload `Raw`
    /// fallback.
    pub fn encode<'a>(&'a mut self, raw: &'a [u8]) -> EncodedPayload<'a> {
        let raw_len = raw.len() as u32;
        match self.try_compress(raw) {
            Some(len) => EncodedPayload {
                encoding: self.encoding,
                raw_len,
                bytes: &self.wire[..len],
            },
            None => EncodedPayload {
                encoding: PayloadEncoding::Raw,
                raw_len,
                bytes: raw,
            },
        }
    }

    /// Shuffles and compresses `raw` into `self.wire`, returning the compressed length,
    /// or `None` to send `raw` as is (a `Raw` encoder, a partial trailing value, a codec
    /// error, or no size win).
    #[cfg(feature = "compression")]
    fn try_compress(&mut self, raw: &[u8]) -> Option<usize> {
        if self.encoding == PayloadEncoding::Raw
            || raw.is_empty()
            || !raw.len().is_multiple_of(shuffle::PLANES)
        {
            return None;
        }
        self.planes.resize(raw.len(), 0);
        shuffle::shuffle_into(raw, &mut self.planes);
        let result = match self.encoding {
            PayloadEncoding::Raw => return None,
            PayloadEncoding::ShuffleZstd { level } => {
                self.wire
                    .resize(zstd::zstd_safe::compress_bound(raw.len()), 0);
                if self.zstd.is_none() {
                    self.zstd = Some(zstd::bulk::Compressor::new(i32::from(level)).ok()?);
                }
                self.zstd
                    .as_mut()?
                    .compress_to_buffer(&self.planes, self.wire.as_mut_slice())
                    .map_err(|e| e.to_string())
            }
            PayloadEncoding::ShuffleLz4 => {
                self.wire
                    .resize(lz4_flex::block::get_maximum_output_size(raw.len()), 0);
                lz4_flex::block::compress_into(&self.planes, &mut self.wire)
                    .map_err(|e| e.to_string())
            }
        };
        match result {
            Ok(len) if len < raw.len() => Some(len),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(
                    "compressing a {} B payload with {:?} failed ({e}); sending it raw",
                    raw.len(),
                    self.encoding
                );
                None
            }
        }
    }

    /// Without the `compression` feature every encoder is `Raw` (see [`Self::new`]).
    #[cfg(not(feature = "compression"))]
    const fn try_compress(&self, _raw: &[u8]) -> Option<usize> {
        debug_assert!(matches!(self.encoding, PayloadEncoding::Raw));
        None
    }
}

/// A payload after the bounds checks: either the raw bytes reinterpreted in place, or
/// the decompressed (still shuffled) planes in the decoder's scratch buffer.
enum Prepared<'a> {
    Raw(&'a [Vec3]),
    Planes(&'a [u8]),
}

/// Decodes radiance payloads of any supported encoding, reusing its scratch buffer and
/// decompression context across calls (one per client connection or accumulator).
#[derive(Default)]
pub struct PayloadDecoder {
    planes: Vec<u8>,
    #[cfg(feature = "compression")]
    zstd: Option<zstd::bulk::Decompressor<'static>>,
}

impl std::fmt::Debug for PayloadDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayloadDecoder")
            .field("scratch_bytes", &self.planes.len())
            .finish_non_exhaustive()
    }
}

impl PayloadDecoder {
    /// A decoder with empty scratch buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes a `width * height` payload and adds it onto `acc` element-wise -- the
    /// `FRAME` delta path. For a compressed payload this sums straight from the
    /// decompressed planes ([`shuffle::add_unshuffled`]), with no second full-frame
    /// buffer; the result is bit-identical to decoding first and adding after.
    ///
    /// # Errors
    ///
    /// Any [`RadianceError`] from the module doc comment's bounded-decode checks, or
    /// [`RadianceError::LengthMismatch`] if `acc.len() != width * height`. `acc` is left
    /// untouched on error.
    ///
    /// A pixel with a non-finite or negative component is not summed (it adds nothing);
    /// the returned count is how many pixels were skipped that way. The caller still
    /// counts the frame's samples as done, matching the tracer's own rule.
    pub fn decode_and_add(
        &mut self,
        encoding: PayloadEncoding,
        raw_len: u32,
        wire: &[u8],
        width: u32,
        height: u32,
        acc: &mut [Vec3],
    ) -> Result<u32, RadianceError> {
        let pixels = width as usize * height as usize;
        if acc.len() != pixels {
            return Err(RadianceError::LengthMismatch {
                width,
                height,
                expected_bytes: pixels * BYTES_PER_PIXEL,
                got_bytes: acc.len() * BYTES_PER_PIXEL,
            });
        }
        let dropped = match self.prepare(encoding, raw_len, wire, width, height)? {
            Prepared::Raw(delta) => {
                let mut dropped = 0u32;
                for (a, d) in acc.iter_mut().zip(delta) {
                    if shuffle::is_valid_sample(*d) {
                        *a += *d;
                    } else {
                        dropped = dropped.saturating_add(1);
                    }
                }
                dropped
            }
            Prepared::Planes(planes) => shuffle::add_unshuffled_valid(planes, acc),
        };
        Ok(dropped)
    }

    /// Decodes a `width * height` payload into a fresh buffer -- the `PREVIEW` path.
    ///
    /// # Errors
    ///
    /// Any [`RadianceError`] from the module doc comment's bounded-decode checks.
    pub fn decode_to_vec(
        &mut self,
        encoding: PayloadEncoding,
        raw_len: u32,
        wire: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<Vec3>, RadianceError> {
        match self.prepare(encoding, raw_len, wire, width, height)? {
            Prepared::Raw(values) => Ok(values.to_vec()),
            Prepared::Planes(planes) => {
                let mut out = vec![Vec3::ZERO; planes.len() / BYTES_PER_PIXEL];
                shuffle::unshuffle_into(planes, bytemuck::cast_slice_mut(&mut out));
                Ok(out)
            }
        }
    }

    /// Runs the module doc comment's checks 1-4 and decompresses when needed.
    fn prepare<'a>(
        &'a mut self,
        encoding: PayloadEncoding,
        raw_len: u32,
        wire: &'a [u8],
        width: u32,
        height: u32,
    ) -> Result<Prepared<'a>, RadianceError> {
        let expected = u64::from(width) * u64::from(height) * BYTES_PER_PIXEL as u64;
        if expected > u64::from(MAX_FRAME_LEN) {
            return Err(RadianceError::TooLarge { width, height });
        }
        let expected = expected as usize;
        if raw_len as usize != expected {
            return Err(RadianceError::RawLenMismatch {
                width,
                height,
                expected_bytes: expected,
                raw_len,
            });
        }
        if encoding == PayloadEncoding::Raw {
            return decode_borrowed(wire, width, height).map(Prepared::Raw);
        }
        if !encoding.is_supported() {
            return Err(RadianceError::UnsupportedEncoding(encoding));
        }
        if expected == 0 {
            return Ok(Prepared::Raw(&[]));
        }
        self.planes.resize(expected, 0);
        let got = self.decompress_exact(encoding, wire)?;
        if got != expected {
            return Err(RadianceError::ShortOutput {
                expected_bytes: expected,
                got_bytes: got,
            });
        }
        Ok(Prepared::Planes(&self.planes))
    }

    /// Decompresses `wire` into exactly `self.planes` (already sized to `raw_len`),
    /// never more, returning how many bytes it produced.
    #[cfg(feature = "compression")]
    fn decompress_exact(
        &mut self,
        encoding: PayloadEncoding,
        wire: &[u8],
    ) -> Result<usize, RadianceError> {
        let result = match encoding {
            PayloadEncoding::Raw => return Err(RadianceError::UnsupportedEncoding(encoding)),
            PayloadEncoding::ShuffleZstd { .. } => {
                if self.zstd.is_none() {
                    self.zstd = Some(
                        zstd::bulk::Decompressor::new()
                            .map_err(|_| RadianceError::DecompressFailed(encoding))?,
                    );
                }
                self.zstd
                    .as_mut()
                    .ok_or(RadianceError::DecompressFailed(encoding))?
                    .decompress_to_buffer(wire, self.planes.as_mut_slice())
                    .map_err(|e| e.to_string())
            }
            PayloadEncoding::ShuffleLz4 => {
                lz4_flex::block::decompress_into(wire, &mut self.planes).map_err(|e| e.to_string())
            }
        };
        result.map_err(|e| {
            tracing::warn!(
                "rejecting a {encoding:?} payload ({} B on the wire, raw_len {}): {e}",
                wire.len(),
                self.planes.len()
            );
            RadianceError::DecompressFailed(encoding)
        })
    }

    /// Without the `compression` feature nothing but `Raw` reaches here (see
    /// [`Self::prepare`]'s `is_supported` check).
    #[cfg(not(feature = "compression"))]
    fn decompress_exact(
        &mut self,
        encoding: PayloadEncoding,
        _wire: &[u8],
    ) -> Result<usize, RadianceError> {
        self.planes.clear();
        Err(RadianceError::UnsupportedEncoding(encoding))
    }
}

/// Decodes one payload into a fresh buffer with a throwaway [`PayloadDecoder`] -- for a
/// caller that decodes rarely; a hot path keeps its own decoder.
///
/// # Errors
///
/// See [`PayloadDecoder::decode_to_vec`].
pub fn decode_payload(
    encoding: PayloadEncoding,
    raw_len: u32,
    wire: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<Vec3>, RadianceError> {
    PayloadDecoder::new().decode_to_vec(encoding, raw_len, wire, width, height)
}
