//! Tests for [`super::policy`]: the peer's announced encodings bound every choice.
//!
//! Also loopback and fixed modes, the build without compression, and the round trip that
//! proves frames of different encodings need no protocol change.

use super::{
    AdaptiveEncoder, AdaptiveEncoderPolicy, AdaptiveMode, BandwidthTier, DisplayRow,
    EncodingMatrix, PayloadRow, SIZE_CLASS_COUNT, SizeClass, TIER_COUNT,
};
use crate::{
    display::decode_rgba8,
    messages::{
        DEFAULT_SERVER_PREFERENCE, DisplayEncoding, LOOPBACK_SERVER_PREFERENCE, PayloadEncoding,
        negotiate,
    },
    radiance::{
        PayloadDecoder,
        test_support::{mixed_bits, smooth_payload, to_bytes},
    },
};

const RAW: PayloadEncoding = PayloadEncoding::Raw;
const LZ4: PayloadEncoding = PayloadEncoding::ShuffleLz4;
const ZSTD1: PayloadEncoding = PayloadEncoding::ShuffleZstd { level: 1 };
const ZSTD9: PayloadEncoding = PayloadEncoding::ShuffleZstd { level: 9 };

/// small -> zstd 9, medium -> LZ4, large -> Raw, on every tier; PNG for small and large.
const SMALL_CELL: &[PayloadEncoding] = &[ZSTD9, LZ4, RAW];
const MEDIUM_CELL: &[PayloadEncoding] = &[LZ4, RAW];
const LARGE_CELL: &[PayloadEncoding] = &[RAW];
const TEST_PAYLOAD: [PayloadRow; SIZE_CLASS_COUNT] = [
    [SMALL_CELL; TIER_COUNT],
    [MEDIUM_CELL; TIER_COUNT],
    [LARGE_CELL; TIER_COUNT],
];
const TEST_DISPLAY: [DisplayRow; SIZE_CLASS_COUNT] = [
    [DisplayEncoding::Png; TIER_COUNT],
    [DisplayEncoding::Rgba8; TIER_COUNT],
    [DisplayEncoding::Png; TIER_COUNT],
];
const TEST_MATRIX: EncodingMatrix = EncodingMatrix::new(&TEST_PAYLOAD, &TEST_DISPLAY);

/// Representative payload sizes of the three classes.
const SIZES: [usize; 3] = [36_864, 5_000_000, 100_000_000];

fn accept_sets() -> Vec<Vec<PayloadEncoding>> {
    vec![
        vec![],
        vec![RAW],
        vec![LZ4],
        vec![ZSTD1],
        vec![LZ4, RAW],
        vec![ZSTD1, LZ4, RAW],
    ]
}

#[test]
fn the_policy_never_picks_an_encoding_the_peer_did_not_announce() {
    for accepts in accept_sets() {
        for matrix in [EncodingMatrix::GENERATED, TEST_MATRIX] {
            let policy = AdaptiveEncoderPolicy::adaptive(&accepts, false).with_matrix(matrix);
            for size in SIZES {
                for tier in BandwidthTier::all() {
                    let chosen = policy.choose_payload(size, tier);
                    assert!(
                        chosen.is_supported(),
                        "{chosen:?} is not supported by this build"
                    );
                    assert!(
                        chosen == RAW || accepts.iter().any(|a| a.same_family(chosen)),
                        "{chosen:?} chosen for a peer announcing {accepts:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_matrix_level_is_used_for_an_announced_family() {
    let all = [ZSTD1, LZ4, RAW];
    let policy = AdaptiveEncoderPolicy::adaptive(&all, false).with_matrix(TEST_MATRIX);
    let tier = BandwidthTier::DEFAULT;
    let small = policy.choose_payload(SIZES[0], tier);
    let medium = policy.choose_payload(SIZES[1], tier);
    let large = policy.choose_payload(SIZES[2], tier);
    if cfg!(feature = "compression") {
        assert_eq!(
            small, ZSTD9,
            "the peer's level 1 is ignored, the matrix's 9 is used"
        );
        assert_eq!(medium, LZ4);
    } else {
        assert_eq!((small, medium), (RAW, RAW));
    }
    assert_eq!(large, RAW);

    let lz4_only = AdaptiveEncoderPolicy::adaptive(&[LZ4], false).with_matrix(TEST_MATRIX);
    let expected = if cfg!(feature = "compression") {
        LZ4
    } else {
        RAW
    };
    assert_eq!(
        lz4_only.choose_payload(SIZES[0], tier),
        expected,
        "zstd skipped, LZ4 next"
    );
}

#[test]
fn loopback_is_raw_for_payloads_and_pictures() {
    let policy = AdaptiveEncoderPolicy::adaptive(&[ZSTD1, LZ4, RAW], true).with_matrix(TEST_MATRIX);
    for size in SIZES {
        for tier in BandwidthTier::all() {
            assert_eq!(policy.choose_payload(size, tier), RAW);
            assert_eq!(policy.choose_display(64, 48, tier), DisplayEncoding::Rgba8);
        }
    }
}

#[test]
fn fixed_mode_is_todays_negotiation_whatever_the_link() {
    let accepts = [LZ4, RAW];
    let policy = AdaptiveEncoderPolicy::fixed(&DEFAULT_SERVER_PREFERENCE, &accepts);
    assert!(!policy.is_adaptive());
    assert!(matches!(policy.mode(), AdaptiveMode::Fixed(list) if list.len() == 3));
    let negotiated = negotiate(&DEFAULT_SERVER_PREFERENCE, &accepts);
    for size in SIZES {
        for tier in BandwidthTier::all() {
            assert_eq!(policy.choose_payload(size, tier), negotiated);
            assert_eq!(
                policy.choose_display(64, 48, tier),
                DisplayEncoding::for_payload_encoding(negotiated)
            );
        }
    }
    let raw_only = AdaptiveEncoderPolicy::fixed(&LOOPBACK_SERVER_PREFERENCE, &[ZSTD1, LZ4]);
    assert_eq!(
        raw_only.choose_payload(SIZES[2], BandwidthTier::LOWEST),
        RAW
    );
}

#[test]
fn a_build_without_compression_or_a_raw_only_peer_gets_raw_everything() {
    let raw_peer = AdaptiveEncoderPolicy::adaptive(&[RAW], false).with_matrix(TEST_MATRIX);
    for tier in BandwidthTier::all() {
        assert_eq!(raw_peer.choose_payload(SIZES[0], tier), RAW);
        assert_eq!(
            raw_peer.choose_display(64, 48, tier),
            DisplayEncoding::Rgba8
        );
    }
    if !cfg!(feature = "compression") {
        let full = AdaptiveEncoderPolicy::adaptive(&[ZSTD1, LZ4, RAW], false);
        for size in SIZES {
            for tier in BandwidthTier::all() {
                assert_eq!(full.choose_payload(size, tier), RAW);
            }
        }
    }
}

#[test]
fn pictures_use_png_only_for_a_peer_that_announced_compression() {
    let tier = BandwidthTier::LOWEST;
    let capable = AdaptiveEncoderPolicy::adaptive(&[ZSTD1, RAW], false).with_matrix(TEST_MATRIX);
    let expected = if cfg!(feature = "compression") {
        DisplayEncoding::Png
    } else {
        DisplayEncoding::Rgba8
    };
    assert_eq!(capable.choose_display(64, 48, tier), expected);
    assert_eq!(
        capable.choose_display(640, 480, tier),
        DisplayEncoding::Rgba8,
        "the medium row says raw"
    );
    assert_eq!(SizeClass::of_pixels(640 * 480), SizeClass::Medium);
}

/// A 64 x 48 RGBA8 picture: smooth, or noise PNG cannot shrink.
fn picture(noise: bool) -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32;
    (0..64 * 48 * 4)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if noise {
                (state >> 9) as u8
            } else {
                (i / 64) as u8
            }
        })
        .collect()
}

#[test]
fn display_frames_fall_back_to_rgba8_when_png_does_not_help() {
    let policy =
        AdaptiveEncoderPolicy::adaptive(&[ZSTD1, LZ4, RAW], false).with_matrix(TEST_MATRIX);
    let encoder = AdaptiveEncoder::new(policy);
    for noise in [false, true] {
        let rgba = picture(noise);
        let (encoding, bytes) = encoder
            .encode_display(64, 48, &rgba, BandwidthTier::LOWEST)
            .unwrap();
        assert_eq!(decode_rgba8(encoding, 64, 48, &bytes).unwrap(), rgba);
        if noise || !cfg!(feature = "compression") {
            assert_eq!(encoding, DisplayEncoding::Rgba8);
        } else {
            assert_eq!(encoding, DisplayEncoding::Png);
            assert!(bytes.len() < rgba.len());
        }
    }
}

/// The "no protocol change" proof: consecutive frames use different encodings (and one
/// falls back to Raw inside the encoder), a SINGLE decoder instance decodes all of them
/// from nothing but each frame's own encoding field, bit-identically.
#[test]
fn consecutive_frames_with_different_encodings_decode_on_one_decoder() {
    let policy = AdaptiveEncoderPolicy::adaptive(&PayloadEncoding::default_accept_list(), false)
        .with_matrix(TEST_MATRIX);
    let mut encoder = AdaptiveEncoder::new(policy);
    let mut decoder = PayloadDecoder::new();
    let smooth = |w: usize, h: usize| smooth_payload(w * h);
    let noisy = |w: usize, h: usize| to_bytes(&mixed_bits(w * h * 3, 7));
    let frames: Vec<(u32, u32, Vec<u8>)> = vec![
        (64, 48, smooth(64, 48)),
        (320, 300, smooth(320, 300)),
        (1200, 1200, smooth(1200, 1200)),
        (64, 48, noisy(64, 48)),
        (64, 48, smooth(64, 48)),
        (320, 300, noisy(320, 300)),
        (1200, 1200, smooth(1200, 1200)),
    ];
    let mut used: Vec<PayloadEncoding> = Vec::new();
    for (w, h, raw) in &frames {
        let encoded = encoder.encode(raw, BandwidthTier::DEFAULT);
        assert_eq!(encoded.raw_len as usize, raw.len());
        if !used.contains(&encoded.encoding) {
            used.push(encoded.encoding);
        }
        let decoded = decoder
            .decode_to_vec(encoded.encoding, encoded.raw_len, encoded.bytes, *w, *h)
            .unwrap();
        let bytes: &[u8] = bytemuck::cast_slice(&decoded);
        assert!(
            bytes == raw.as_slice(),
            "{:?} frame {w}x{h} differs",
            encoded.encoding
        );
    }
    if cfg!(feature = "compression") {
        assert!(used.len() >= 3, "frames used only {used:?}");
        assert!(
            used.iter()
                .any(|e| matches!(e, PayloadEncoding::ShuffleZstd { .. }))
        );
        assert!(used.contains(&LZ4) && used.contains(&RAW));
    } else {
        assert_eq!(used, [RAW]);
    }
}

#[test]
fn releasing_the_buffers_keeps_the_encoding_and_the_output_identical() {
    let raw = smooth_payload(64 * 64);
    let mut encoder = AdaptiveEncoder::new(
        AdaptiveEncoderPolicy::adaptive(&[ZSTD1, RAW], false).with_matrix(TEST_MATRIX),
    );
    let tier = BandwidthTier::DEFAULT;
    let before = {
        let encoded = encoder.encode(&raw, tier);
        (encoded.encoding, encoded.bytes.to_vec())
    };
    encoder.release_buffers();
    let encoded = encoder.encode(&raw, tier);
    assert_eq!((encoded.encoding, encoded.bytes.to_vec()), before);
}

#[test]
fn a_policy_reports_whether_it_is_adaptive_and_loopback() {
    assert!(AdaptiveEncoderPolicy::adaptive(&[RAW], true).is_loopback());
    assert!(!AdaptiveEncoderPolicy::adaptive(&[RAW], false).is_loopback());
    let fixed = AdaptiveEncoderPolicy::fixed(&DEFAULT_SERVER_PREFERENCE, &[RAW]);
    assert!(!fixed.is_adaptive() && !fixed.is_loopback());
}
