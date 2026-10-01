//! `-> CONTRIBUTION` (v16): the viewer's own share of a [`super::final_image`] export.
//!
//! When a `FinalImageRequest` reserves a tail of `viewer_samples` for itself
//! ([`super::final_image::FinalImageRequest::reserved_range`]), the viewer renders that
//! tail locally and uploads the result as one `CONTRIBUTION`: a small `postcard` header
//! (this module's [`ContributionHeader`]), then the float-XYZ sum as one raw payload
//! frame -- the same two-frame shape as `FRAME`/`PREVIEW`
//! ([`crate::messages::stream::wire`]) and `ASSET` ([`super::asset`]).
//!
//! The payload is encoded with the connection's negotiated [`PayloadEncoding`] (or `Raw`
//! per payload, see [`crate::radiance::payload`]) and decoded the same bounded way every
//! other radiance payload is: the receiver's expected size (`width * height * 12`) is
//! checked against the header's declared `raw_len` and the frame cap
//! ([`crate::framing::MAX_FRAME_LEN`]) BEFORE anything is allocated -- mirroring
//! [`super::asset::read_asset_payload`]'s `read_frame_bounded(reader, header.len)`
//! pattern.

use super::{NetError, PayloadEncoding, codec};
use crate::{
    framing::{self, FramingError, MAX_FRAME_LEN},
    radiance::{
        self, BYTES_PER_PIXEL, EncodedPayload, PayloadDecoder, PayloadEncoder, RadianceError,
    },
};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// The header half of a `-> CONTRIBUTION` message: the viewer's float-XYZ sum of
/// exactly its reserved range, `samples` samples. The payload follows as one raw frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionHeader {
    /// The `FinalImageRequest` this contribution answers.
    pub request_id: u32,
    /// First absolute sample index of the contributed range -- `==
    /// request.first_sample + request.server_samples()`.
    pub first_sample: u32,
    /// Number of samples contributed -- `== request.viewer_samples`.
    pub samples: u32,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// How the payload is encoded -- the WELCOME-negotiated encoding, or `Raw` when
    /// this particular payload fell back (see [`PayloadEncoder::encode`]).
    pub encoding: PayloadEncoding,
    /// On-wire byte length of the payload frame that follows, as sent. Must be `<=
    /// raw_len`.
    pub payload_len: u32,
    /// Decoded byte length of the payload -- `== width * height * 12`.
    pub raw_len: u32,
}

impl ContributionHeader {
    /// Builds a header for an already-encoded payload (see
    /// [`PayloadEncoder::encode`]), the same pattern as `FrameHeader::for_encoded`.
    #[must_use]
    pub const fn for_encoded(
        request_id: u32,
        first_sample: u32,
        samples: u32,
        width: u32,
        height: u32,
        encoded: &EncodedPayload<'_>,
    ) -> Self {
        Self {
            request_id,
            first_sample,
            samples,
            width,
            height,
            encoding: encoded.encoding,
            payload_len: encoded.bytes.len() as u32,
            raw_len: encoded.raw_len,
        }
    }
}

/// Why a `CONTRIBUTION` payload could not be used.
#[derive(Debug)]
pub enum ContributionError {
    /// `payload_len > raw_len`, or `raw_len` disagrees with `width * height * 12`, or
    /// exceeds [`MAX_FRAME_LEN`] -- refused BEFORE reading the payload frame; the
    /// stream is NOT in sync (the caller must drop the connection).
    TooLarge { payload_len: u32, raw_len: u32 },
    /// The header doesn't match what the receiver expected (wrong request, range or
    /// dimensions). The frame WAS consumed first, so the stream stays in sync.
    Mismatch(String),
    /// Bounded decode of the (already-consumed) frame failed.
    Radiance(RadianceError),
    /// A transport-level failure reading the frame; the stream is NOT in sync.
    Framing(FramingError),
}

impl std::fmt::Display for ContributionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge {
                payload_len,
                raw_len,
            } => write!(
                f,
                "contribution payload_len={payload_len} raw_len={raw_len} is refused before reading"
            ),
            Self::Mismatch(msg) => write!(f, "unexpected contribution: {msg}"),
            Self::Radiance(e) => write!(f, "contribution payload failed to decode: {e}"),
            Self::Framing(e) => write!(f, "reading the contribution payload failed: {e}"),
        }
    }
}

impl std::error::Error for ContributionError {}

/// What the receiver expects a `CONTRIBUTION` to carry, from its own `FinalImageRequest`.
#[derive(Debug, Clone, Copy)]
pub struct ExpectedContribution {
    /// The request this contribution must answer.
    pub request_id: u32,
    /// The reserved range's first absolute sample index.
    pub first_sample: u32,
    /// The reserved range's sample count.
    pub samples: u32,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
}

/// Encodes `sum` (a `width * height` radiance buffer) with `encoder` and writes one
/// `-> CONTRIBUTION`: the tagged [`super::ClientMessage::Contribution`] header, then the
/// payload as one raw frame.
///
/// `range` is `(first_sample, samples)` -- the same shape
/// [`super::final_image::FinalImageRequest::reserved_range`] returns -- collapsed into
/// one parameter to keep this function's arity under clippy's default limit.
///
/// # Errors
///
/// Returns [`NetError`] if writing fails.
pub fn write_contribution_message<W: std::io::Write>(
    w: &mut W,
    request_id: u32,
    range: (u32, u32),
    width: u32,
    height: u32,
    sum: &[Vec3],
    encoder: &mut PayloadEncoder,
) -> Result<(), NetError> {
    let (first_sample, samples) = range;
    let raw = radiance::as_bytes(sum);
    let encoded = encoder.encode(raw);
    let header =
        ContributionHeader::for_encoded(request_id, first_sample, samples, width, height, &encoded);
    codec::write_message(w, &super::ClientMessage::Contribution(header))?;
    framing::write_frame(w, encoded.bytes)?;
    Ok(())
}

/// Reads the payload frame that follows a [`ContributionHeader`] (already read),
/// bounding the read BEFORE allocating, then checks it against `expect` and decodes it.
///
/// Order (matches [`ContributionError`]'s own doc comments): the size bound is checked
/// first ([`ContributionError::TooLarge`], stream not in sync if it fails); then the
/// frame is read via [`framing::read_frame_bounded`] with `header.payload_len` as the
/// cap (the frame is consumed from here on, whatever happens next); then the header is
/// checked against `expect` ([`ContributionError::Mismatch`]); then the frame is
/// decoded with `decoder` ([`ContributionError::Radiance`]).
///
/// # Errors
///
/// See [`ContributionError`].
pub fn read_contribution_payload<R: std::io::Read>(
    r: &mut R,
    header: &ContributionHeader,
    expect: ExpectedContribution,
    decoder: &mut PayloadDecoder,
) -> Result<Vec<Vec3>, ContributionError> {
    let expected_raw = u64::from(header.width) * u64::from(header.height) * BYTES_PER_PIXEL as u64;
    if u64::from(header.payload_len) > u64::from(header.raw_len)
        || u64::from(header.raw_len) != expected_raw
        || u64::from(header.raw_len) > u64::from(MAX_FRAME_LEN)
    {
        return Err(ContributionError::TooLarge {
            payload_len: header.payload_len,
            raw_len: header.raw_len,
        });
    }
    let bytes =
        framing::read_frame_bounded(r, header.payload_len).map_err(ContributionError::Framing)?;

    if header.request_id != expect.request_id
        || header.first_sample != expect.first_sample
        || header.samples != expect.samples
        || header.width != expect.width
        || header.height != expect.height
    {
        return Err(ContributionError::Mismatch(format!(
            "expected request {} [{}, +{}) {}x{}, got request {} [{}, +{}) {}x{}",
            expect.request_id,
            expect.first_sample,
            expect.samples,
            expect.width,
            expect.height,
            header.request_id,
            header.first_sample,
            header.samples,
            header.width,
            header.height
        )));
    }

    decoder
        .decode_to_vec(
            header.encoding,
            header.raw_len,
            &bytes,
            header.width,
            header.height,
        )
        .map_err(ContributionError::Radiance)
}

/// Consumes an unwanted `CONTRIBUTION`'s payload frame, keeping the stream in sync.
///
/// Bounded by `min(payload_len, MAX_FRAME_LEN)` without trusting `raw_len`. Mirrors
/// `apps/indicatrix-worker`'s `assets::discard_asset`.
///
/// # Errors
///
/// Returns [`NetError`] if the frame cannot be read in sync.
pub fn discard_contribution_payload<R: std::io::Read>(
    r: &mut R,
    header: &ContributionHeader,
) -> Result<(), NetError> {
    let max = header.payload_len.min(MAX_FRAME_LEN);
    framing::read_frame_bounded(r, max)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::ClientMessage;

    fn sum(pixels: usize, value: f32) -> Vec<Vec3> {
        vec![Vec3::splat(value); pixels]
    }

    fn expect_for(header: &ContributionHeader) -> ExpectedContribution {
        ExpectedContribution {
            request_id: header.request_id,
            first_sample: header.first_sample,
            samples: header.samples,
            width: header.width,
            height: header.height,
        }
    }

    /// Every supported encoding round-trips a contribution bit-identically.
    #[test]
    fn a_contribution_round_trips_bit_identically_for_every_encoding() {
        let (w, h) = (5, 3);
        let values = sum((w * h) as usize, 2.5);
        for encoding in [
            PayloadEncoding::Raw,
            PayloadEncoding::ShuffleZstd { level: 1 },
            PayloadEncoding::ShuffleLz4,
        ]
        .into_iter()
        .filter(|e| e.is_supported())
        {
            let mut encoder = PayloadEncoder::new(encoding);
            let mut buf = Vec::new();
            write_contribution_message(&mut buf, 9, (100, 12), w, h, &values, &mut encoder)
                .unwrap();

            let mut cursor = std::io::Cursor::new(buf);
            let msg: ClientMessage = codec::read_message(&mut cursor).unwrap();
            let ClientMessage::Contribution(header) = msg else {
                panic!("expected ClientMessage::Contribution, got {msg:?}");
            };
            let mut decoder = PayloadDecoder::new();
            let decoded =
                read_contribution_payload(&mut cursor, &header, expect_for(&header), &mut decoder)
                    .unwrap();
            assert_eq!(decoded, values, "{encoding:?}");
        }
    }

    #[test]
    fn a_payload_longer_than_raw_len_is_refused_before_reading() {
        let header = ContributionHeader {
            request_id: 1,
            first_sample: 0,
            samples: 1,
            width: 2,
            height: 2,
            encoding: PayloadEncoding::Raw,
            payload_len: 999,
            raw_len: 48, // 2*2*12
        };
        let mut decoder = PayloadDecoder::new();
        // No bytes follow -- a read would hang/error if the bound check didn't fire
        // first.
        let err = read_contribution_payload(
            &mut std::io::Cursor::new(Vec::new()),
            &header,
            expect_for(&header),
            &mut decoder,
        )
        .unwrap_err();
        assert!(matches!(err, ContributionError::TooLarge { .. }), "{err:?}");
    }

    #[test]
    fn a_mismatched_range_or_size_is_consumed_and_reported() {
        let (w, h) = (2, 2);
        let values = sum((w * h) as usize, 1.0);
        let mut encoder = PayloadEncoder::new(PayloadEncoding::Raw);
        let mut buf = Vec::new();
        write_contribution_message(&mut buf, 1, (10, 4), w, h, &values, &mut encoder).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let msg: ClientMessage = codec::read_message(&mut cursor).unwrap();
        let ClientMessage::Contribution(header) = msg else {
            panic!("expected ClientMessage::Contribution, got {msg:?}");
        };
        let mut decoder = PayloadDecoder::new();
        let wrong_expect = ExpectedContribution {
            request_id: 1,
            first_sample: 99, // does not match header.first_sample == 10
            samples: 4,
            width: w,
            height: h,
        };
        let err = read_contribution_payload(&mut cursor, &header, wrong_expect, &mut decoder)
            .unwrap_err();
        assert!(matches!(err, ContributionError::Mismatch(_)), "{err:?}");
        // The frame was consumed: nothing left to read, and the stream stayed usable
        // (proven by reaching EOF cleanly rather than desyncing mid-frame).
        assert_eq!(cursor.position() as usize, cursor.get_ref().len());
    }

    #[test]
    fn discard_keeps_the_stream_in_sync() {
        let (w, h) = (3, 2);
        let values = sum((w * h) as usize, 4.0);
        let mut encoder = PayloadEncoder::new(PayloadEncoding::Raw);
        let mut buf = Vec::new();
        write_contribution_message(&mut buf, 5, (0, 6), w, h, &values, &mut encoder).unwrap();
        // A PING written right after, to prove the reader lands exactly on it.
        codec::write_message(&mut buf, &ClientMessage::Ping { nonce: 42 }).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let msg: ClientMessage = codec::read_message(&mut cursor).unwrap();
        let ClientMessage::Contribution(header) = msg else {
            panic!("expected ClientMessage::Contribution, got {msg:?}");
        };
        discard_contribution_payload(&mut cursor, &header).unwrap();

        let next: ClientMessage = codec::read_message(&mut cursor).unwrap();
        assert_eq!(next, ClientMessage::Ping { nonce: 42 });
    }
}
