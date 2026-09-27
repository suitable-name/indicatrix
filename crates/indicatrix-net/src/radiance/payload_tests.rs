//! Tests for the v14 payload codecs (`super::payload`): bit-exact round trips for every
//! encoding, the per-payload `Raw` fallback, and the bounded-decode rejections (bombs,
//! short output, lying headers, oversize dimensions).

use super::{
    RadianceError,
    payload::{PayloadDecoder, PayloadEncoder, decode_payload},
    test_support::{mixed_bits, smooth_payload, to_bytes},
};
use crate::messages::PayloadEncoding;
use glam::Vec3;

const RAW: PayloadEncoding = PayloadEncoding::Raw;

/// Every encoding this build supports.
fn supported_encodings() -> Vec<PayloadEncoding> {
    [
        RAW,
        PayloadEncoding::ShuffleZstd { level: 1 },
        PayloadEncoding::ShuffleZstd { level: 3 },
        PayloadEncoding::ShuffleLz4,
    ]
    .into_iter()
    .filter(|e| e.is_supported())
    .collect()
}

fn bits_of(values: &[Vec3]) -> Vec<u32> {
    values
        .iter()
        .flat_map(Vec3::to_array)
        .map(f32::to_bits)
        .collect()
}

/// Encode then `decode_to_vec` returns the exact input bits -- NaN payloads, signed
/// zeros, subnormals and infinities included -- for every encoding and both a
/// compressible and an incompressible payload.
#[test]
fn every_encoding_round_trips_bit_exactly() {
    let (width, height) = (37, 11);
    let pixels = (width * height) as usize;
    let payloads = [smooth_payload(pixels), to_bytes(&mixed_bits(pixels * 3, 7))];
    for encoding in supported_encodings() {
        let mut encoder = PayloadEncoder::new(encoding);
        let mut decoder = PayloadDecoder::new();
        for raw in &payloads {
            let encoded = encoder.encode(raw);
            assert_eq!(encoded.raw_len as usize, raw.len());
            let decoded = decoder
                .decode_to_vec(
                    encoded.encoding,
                    encoded.raw_len,
                    encoded.bytes,
                    width,
                    height,
                )
                .unwrap();
            let decoded_bytes: Vec<u8> = bytemuck::cast_slice(&decoded).to_vec();
            assert_eq!(&decoded_bytes, raw, "{encoding:?} is not bit-exact");
        }
    }
}

/// The accumulator path: summing a compressed delta gives exactly the bits summing the
/// raw delta gives, starting from the same (special-value-laden) buffer.
#[test]
fn decode_and_add_is_bit_identical_to_the_raw_path() {
    let (width, height) = (29, 13);
    let pixels = (width * height) as usize;
    let raw = to_bytes(&mixed_bits(pixels * 3, 21));
    let start: Vec<Vec3> = bytemuck::cast_slice(&to_bytes(&mixed_bits(pixels * 3, 22))).to_vec();

    let mut reference = start.clone();
    PayloadDecoder::new()
        .decode_and_add(RAW, raw.len() as u32, &raw, width, height, &mut reference)
        .unwrap();

    for encoding in supported_encodings() {
        let mut encoder = PayloadEncoder::new(encoding);
        let encoded = encoder.encode(&raw);
        let mut summed = start.clone();
        PayloadDecoder::new()
            .decode_and_add(
                encoded.encoding,
                encoded.raw_len,
                encoded.bytes,
                width,
                height,
                &mut summed,
            )
            .unwrap();
        assert_eq!(bits_of(&summed), bits_of(&reference), "{encoding:?}");
    }
}

/// A compressible payload really is compressed by the compressed encodings, and an
/// incompressible one falls back to `Raw` in its header rather than growing.
#[cfg(feature = "compression")]
#[test]
fn compressible_payloads_shrink_and_incompressible_ones_go_raw() {
    let smooth = smooth_payload(4096);
    let noise: Vec<u8> = {
        let mut state = 0x1234_5678_u32;
        (0..smooth.len())
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state.to_le_bytes()[0]
            })
            .collect()
    };
    for encoding in [
        PayloadEncoding::ShuffleZstd { level: 1 },
        PayloadEncoding::ShuffleLz4,
    ] {
        let mut encoder = PayloadEncoder::new(encoding);
        let encoded = encoder.encode(&smooth);
        assert_eq!(encoded.encoding, encoding);
        assert!(
            encoded.bytes.len() < smooth.len(),
            "{encoding:?} did not shrink"
        );

        let encoded = encoder.encode(&noise);
        assert_eq!(
            encoded.encoding, RAW,
            "{encoding:?} should fall back to Raw"
        );
        assert_eq!(encoded.bytes, noise.as_slice());
    }
}

/// A `Raw` encoder is zero-copy: it hands back the very slice it was given.
#[test]
fn a_raw_encoder_is_zero_copy() {
    let raw = smooth_payload(8);
    let mut encoder = PayloadEncoder::new(RAW);
    let encoded = encoder.encode(&raw);
    assert_eq!(encoded.encoding, RAW);
    assert!(std::ptr::eq(encoded.bytes, raw.as_slice()));
}

/// The header's `raw_len` must equal `width * height * 12`, checked before any decode.
#[test]
fn a_lying_raw_len_is_rejected_before_decoding() {
    let raw = smooth_payload(16);
    for encoding in supported_encodings() {
        let mut encoder = PayloadEncoder::new(encoding);
        let encoded = encoder.encode(&raw);
        let err = decode_payload(encoded.encoding, encoded.raw_len * 64, encoded.bytes, 4, 4)
            .unwrap_err();
        assert!(
            matches!(err, RadianceError::RawLenMismatch { .. }),
            "{encoding:?}: {err:?}"
        );
    }
}

/// Dimensions whose raw size exceeds `MAX_FRAME_LEN` are refused outright, before any
/// allocation, even with a matching `raw_len`.
#[test]
fn oversize_dimensions_are_refused_before_allocating() {
    let err = decode_payload(RAW, u32::MAX, &[], 65_536, 65_536).unwrap_err();
    assert_eq!(
        err,
        RadianceError::TooLarge {
            width: 65_536,
            height: 65_536
        }
    );
}

/// Expansion bomb: a tiny payload that inflates to 64x the declared `raw_len` fails to
/// decode -- the output buffer is exactly `raw_len`, never grown.
#[cfg(feature = "compression")]
#[test]
fn an_expansion_bomb_is_rejected() {
    let (width, height) = (64, 64);
    let raw_len = width * height * 12;
    let huge = vec![0_u8; raw_len as usize * 64];
    for encoding in [
        PayloadEncoding::ShuffleZstd { level: 1 },
        PayloadEncoding::ShuffleLz4,
    ] {
        let mut encoder = PayloadEncoder::new(encoding);
        let bomb = encoder.encode(&huge);
        assert_eq!(bomb.encoding, encoding, "zeros must compress");
        assert!(
            bomb.bytes.len() < raw_len as usize,
            "the bomb must be small"
        );
        let err = decode_payload(encoding, raw_len, bomb.bytes, width, height).unwrap_err();
        assert_eq!(
            err,
            RadianceError::DecompressFailed(encoding),
            "{encoding:?}"
        );
    }
}

/// A payload that decodes to fewer bytes than `raw_len` is rejected as short.
#[cfg(feature = "compression")]
#[test]
fn a_short_payload_is_rejected() {
    let (width, height) = (16, 16);
    let raw_len = width * height * 12;
    let half = smooth_payload((width * height / 2) as usize);
    for encoding in [
        PayloadEncoding::ShuffleZstd { level: 1 },
        PayloadEncoding::ShuffleLz4,
    ] {
        let mut encoder = PayloadEncoder::new(encoding);
        let short = encoder.encode(&half);
        assert_eq!(short.encoding, encoding);
        let err = decode_payload(encoding, raw_len, short.bytes, width, height).unwrap_err();
        assert!(
            matches!(
                err,
                RadianceError::ShortOutput { .. } | RadianceError::DecompressFailed(_)
            ),
            "{encoding:?}: {err:?}"
        );
    }
}

/// Garbage bytes claiming to be compressed are rejected, not decoded into noise.
#[cfg(feature = "compression")]
#[test]
fn corrupt_compressed_bytes_are_rejected() {
    let garbage = [0xFF_u8; 64];
    for encoding in [
        PayloadEncoding::ShuffleZstd { level: 1 },
        PayloadEncoding::ShuffleLz4,
    ] {
        assert!(decode_payload(encoding, 48, &garbage, 2, 2).is_err());
    }
}

/// Without the `compression` feature a compressed header is refused cleanly.
#[cfg(not(feature = "compression"))]
#[test]
fn a_build_without_compression_refuses_compressed_payloads() {
    let encoding = PayloadEncoding::ShuffleLz4;
    assert_eq!(
        decode_payload(encoding, 48, &[1, 2, 3], 2, 2).unwrap_err(),
        RadianceError::UnsupportedEncoding(encoding)
    );
    assert_eq!(PayloadEncoder::new(encoding).encoding(), RAW);
}

/// A zero-area payload decodes to nothing under every encoding.
#[test]
fn zero_area_payloads_decode_to_nothing() {
    for encoding in supported_encodings() {
        assert_eq!(
            decode_payload(encoding, 0, &[], 0, 0).unwrap(),
            Vec::<Vec3>::new()
        );
    }
}

/// `decode_and_add` refuses an accumulator of the wrong size and leaves it untouched.
#[test]
fn decode_and_add_rejects_a_mismatched_accumulator() {
    let raw = smooth_payload(4);
    let mut acc = vec![Vec3::ONE; 3];
    let err = PayloadDecoder::new()
        .decode_and_add(RAW, raw.len() as u32, &raw, 2, 2, &mut acc)
        .unwrap_err();
    assert!(matches!(err, RadianceError::LengthMismatch { .. }));
    assert!(acc.iter().all(|v| *v == Vec3::ONE));
}
