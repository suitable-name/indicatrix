//! Tests for [`super::Accumulator`]: epoch gating, FRAME/PREVIEW semantics, the
//! expected-range (containment) check, and the v14 payload encodings and events.

use super::*;
use crate::{
    messages::{FrameHeader, PreviewHeader, Stats},
    radiance,
};

fn frame_event(request_id: u32, value: f32, pixel_count: usize) -> (StreamEvent, Vec<u8>) {
    let buf = vec![Vec3::splat(value); pixel_count];
    let bytes = radiance::encode(&buf);
    let header = FrameHeader::for_payload(request_id, 0, 1, &bytes);
    (StreamEvent::Frame(header), bytes)
}

fn preview_event(
    request_id: u32,
    value: f32,
    width: u32,
    height: u32,
    samples_done: u32,
) -> (StreamEvent, Vec<u8>) {
    let buf = vec![Vec3::splat(value); (width * height) as usize];
    let bytes = radiance::encode(&buf);
    let header = PreviewHeader::for_payload(request_id, width, height, samples_done, &bytes);
    (StreamEvent::Preview(header), bytes)
}

#[test]
fn events_before_any_begin_request_are_all_dropped_as_stale() {
    let mut acc = Accumulator::new(2, 2);
    let (event, bytes) = frame_event(1, 1.0, 4);
    let outcome = acc.apply(&event, Some(&bytes)).unwrap();
    assert_eq!(outcome, ApplyOutcome::StaleDropped);
    assert!(acc.buffer().iter().all(|v| *v == Vec3::ZERO));
}

#[test]
fn frame_deltas_for_the_current_epoch_sum_into_the_buffer() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(1);

    let (event_a, bytes_a) = frame_event(1, 1.0, 4);
    acc.apply(&event_a, Some(&bytes_a)).unwrap();
    let (event_b, bytes_b) = frame_event(1, 2.0, 4);
    acc.apply(&event_b, Some(&bytes_b)).unwrap();

    for v in acc.buffer() {
        assert!((*v - Vec3::splat(3.0)).length() < 1e-6);
    }
}

#[test]
fn preview_snapshots_replace_rather_than_sum_and_never_touch_the_frame_buffer() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(1);

    let (frame, frame_bytes) = frame_event(1, 5.0, 4);
    acc.apply(&frame, Some(&frame_bytes)).unwrap();

    let (preview_a, bytes_a) = preview_event(1, 10.0, 1, 1, 4);
    let outcome = acc.apply(&preview_a, Some(&bytes_a)).unwrap();
    assert_eq!(outcome, ApplyOutcome::PreviewReplaced);
    assert_eq!(acc.last_preview().unwrap().buffer, vec![Vec3::splat(10.0)]);

    let (preview_b, bytes_b) = preview_event(1, 20.0, 1, 1, 8);
    acc.apply(&preview_b, Some(&bytes_b)).unwrap();
    // The SECOND preview replaced the first -- not summed with it (20.0, not 30.0).
    assert_eq!(acc.last_preview().unwrap().buffer, vec![Vec3::splat(20.0)]);
    assert_eq!(acc.last_preview().unwrap().samples_done, 8);

    // The frame buffer (a completely separate slot) is untouched by either preview.
    for v in acc.buffer() {
        assert!((*v - Vec3::splat(5.0)).length() < 1e-6);
    }
}

/// The invariant the whole module exists for: a delta for a just-superseded epoch,
/// still in flight when the next request begins, must never merge into the new
/// request's accumulation.
#[test]
fn a_frame_for_a_superseded_epoch_is_dropped_not_summed_into_the_new_one() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(1);
    let (frame1, bytes1) = frame_event(1, 100.0, 4);
    acc.apply(&frame1, Some(&bytes1)).unwrap();

    // The client moves on to a new request before this in-flight FRAME is read.
    acc.begin_request(2);
    assert_eq!(acc.current_request_id(), Some(2));
    assert!(
        acc.buffer().iter().all(|v| *v == Vec3::ZERO),
        "begin_request must zero the buffer"
    );

    // The stale epoch-1 frame arrives (or is finally processed) here.
    let stale_outcome = acc.apply(&frame1, Some(&bytes1)).unwrap();
    assert_eq!(stale_outcome, ApplyOutcome::StaleDropped);
    assert!(
        acc.buffer().iter().all(|v| *v == Vec3::ZERO),
        "a stale epoch-1 delta must never be summed into epoch 2's buffer"
    );

    // A legitimate epoch-2 frame DOES sum normally.
    let (frame2, bytes2) = frame_event(2, 7.0, 4);
    acc.apply(&frame2, Some(&bytes2)).unwrap();
    for v in acc.buffer() {
        assert!((*v - Vec3::splat(7.0)).length() < 1e-6);
    }
}

#[test]
fn a_preview_for_a_superseded_epoch_is_dropped_too() {
    let mut acc = Accumulator::new(4, 4);
    acc.begin_request(1);
    let (preview1, bytes1) = preview_event(1, 42.0, 2, 2, 4);
    acc.apply(&preview1, Some(&bytes1)).unwrap();
    assert!(acc.last_preview().is_some());

    acc.begin_request(2);
    assert!(
        acc.last_preview().is_none(),
        "begin_request must clear the preview slot"
    );

    let stale_outcome = acc.apply(&preview1, Some(&bytes1)).unwrap();
    assert_eq!(stale_outcome, ApplyOutcome::StaleDropped);
    assert!(acc.last_preview().is_none());
}

#[test]
fn progress_and_done_are_also_epoch_gated() {
    let mut acc = Accumulator::new(1, 1);
    acc.begin_request(5);

    let stale_progress = StreamEvent::Progress(Progress {
        request_id: 4,
        samples_done: 99,
    });
    assert_eq!(
        acc.apply(&stale_progress, None).unwrap(),
        ApplyOutcome::StaleDropped
    );

    let current_progress = StreamEvent::Progress(Progress {
        request_id: 5,
        samples_done: 12,
    });
    assert_eq!(
        acc.apply(&current_progress, None).unwrap(),
        ApplyOutcome::Progress { samples_done: 12 }
    );

    let stale_done = StreamEvent::Done(Done {
        request_id: 4,
        cancelled: true,
        stats: Stats {
            samples_done: 1,
            requested_cadence_ms: 0,
            effective_cadence_ms: 0,
            reclaimed_samples: 0,
        },
    });
    assert_eq!(
        acc.apply(&stale_done, None).unwrap(),
        ApplyOutcome::StaleDropped
    );

    let current_done = StreamEvent::Done(Done {
        request_id: 5,
        cancelled: false,
        stats: Stats {
            samples_done: 12,
            requested_cadence_ms: 0,
            effective_cadence_ms: 0,
            reclaimed_samples: 0,
        },
    });
    assert_eq!(
        acc.apply(&current_done, None).unwrap(),
        ApplyOutcome::Done { cancelled: false }
    );
}

#[test]
fn worker_error_with_no_request_id_is_reported_regardless_of_current_epoch() {
    let mut acc = Accumulator::new(1, 1);
    // No begin_request call at all -- current epoch is None.
    let event = StreamEvent::Error(ErrorMsg {
        code: 2,
        message: "validation failed".to_string(),
        request_id: None,
    });
    assert_eq!(acc.apply(&event, None).unwrap(), ApplyOutcome::WorkerError);
}

/// v15: an `ERROR` naming a `request_id` is epoch-gated exactly like `FRAME`/`DONE` -- a
/// late error for a request the client has already moved on from must not fail the
/// wrong (current) request.
#[test]
fn worker_error_with_a_stale_request_id_is_dropped_not_reported() {
    let mut acc = Accumulator::new(1, 1);
    acc.begin_request(5);
    let stale = StreamEvent::Error(ErrorMsg {
        code: 3,
        message: "internal error while tracing this request".to_string(),
        request_id: Some(4),
    });
    assert_eq!(acc.apply(&stale, None).unwrap(), ApplyOutcome::StaleDropped);

    let current = StreamEvent::Error(ErrorMsg {
        code: 3,
        message: "internal error while tracing this request".to_string(),
        request_id: Some(5),
    });
    assert_eq!(
        acc.apply(&current, None).unwrap(),
        ApplyOutcome::WorkerError
    );
}

#[test]
fn begin_request_resets_samples_done() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(1);
    let (frame, bytes) = frame_event(1, 1.0, 4);
    acc.apply(&frame, Some(&bytes)).unwrap();
    assert!(acc.samples_done() > 0);

    acc.begin_request(2);
    assert_eq!(acc.samples_done(), 0);
}

fn ranged_frame_event(
    request_id: u32,
    first_sample: u32,
    samples: u32,
    pixel_count: usize,
) -> (StreamEvent, Vec<u8>) {
    let buf = vec![Vec3::ONE; pixel_count];
    let bytes = radiance::encode(&buf);
    let header = FrameHeader::for_payload(request_id, first_sample, samples, &bytes);
    (StreamEvent::Frame(header), bytes)
}

#[test]
fn frame_within_range_is_inclusive_start_exclusive_end_and_overflow_safe() {
    assert!(frame_within_range(10, 5, 10, 15));
    assert!(frame_within_range(12, 3, 10, 15));
    assert!(!frame_within_range(9, 1, 10, 15), "starts before the range");
    assert!(!frame_within_range(14, 2, 10, 15), "ends past the range");
    assert!(
        !frame_within_range(u32::MAX, 2, 0, u32::MAX),
        "overflowing end"
    );
    assert!(
        frame_within_range(10, 0, 10, 15),
        "an empty frame at the start"
    );
}

#[test]
fn begin_request_for_range_records_the_range_and_begin_request_clears_it() {
    let mut acc = Accumulator::new(1, 1);
    acc.begin_request_for_range(3, 100, 20);
    assert_eq!(acc.expected_range(), Some((100, 120)));
    acc.begin_request(4);
    assert_eq!(acc.expected_range(), None);
    acc.begin_request_for_range(5, u32::MAX - 1, 10);
    assert_eq!(acc.expected_range(), Some((u32::MAX - 1, u32::MAX)));
}

/// Every FRAME whose range lies inside the declared chunk is summed normally,
/// including coalesced deltas that together cover the whole chunk.
#[test]
fn in_range_frames_are_summed_when_a_range_is_declared() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request_for_range(7, 64, 8);
    let (a, a_bytes) = ranged_frame_event(7, 64, 3, 4);
    let (b, b_bytes) = ranged_frame_event(7, 67, 5, 4);
    assert_eq!(
        acc.apply(&a, Some(&a_bytes)).unwrap(),
        ApplyOutcome::FrameSummed { samples_done: 3 }
    );
    assert_eq!(
        acc.apply(&b, Some(&b_bytes)).unwrap(),
        ApplyOutcome::FrameSummed { samples_done: 8 }
    );
    assert!(acc.buffer().iter().all(|v| *v == Vec3::splat(2.0)));
}

/// An out-of-range FRAME is refused with an error rather than summed: it means some
/// backend traced sample indices it was never assigned.
#[test]
fn an_out_of_range_frame_is_rejected() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request_for_range(7, 64, 8);
    let (event, bytes) = ranged_frame_event(7, 70, 4, 4);
    assert_eq!(
        acc.apply(&event, Some(&bytes)).unwrap_err(),
        RadianceError::FrameOutsideRange {
            first_sample: 70,
            samples: 4,
            start: 64,
            end: 72,
        }
    );
    assert_eq!(acc.samples_done(), 0);
    assert!(acc.buffer().iter().all(|v| *v == Vec3::ZERO));
}

/// Two FRAMEs that are each individually contained in the declared range can still
/// overlap each other, double-counting samples -- containment alone (checked per frame)
/// cannot see this, since it never compares one frame against another. The CUMULATIVE
/// check catches it: `[64, 72)` is 8 samples wide; a first FRAME of 5 samples
/// (`[64, 69)`, itself in range) leaves only 3 samples' worth of the range unaccounted
/// for, so a second FRAME of 5 more samples (`[67, 72)`, ALSO in range on its own) would
/// bring the cumulative total to 10 -- over the range's own width -- and is rejected even
/// though neither frame individually fails containment. Neither `samples_done` nor the
/// buffer reflects the rejected FRAME.
#[test]
fn a_cumulative_overrun_from_two_individually_in_range_frames_is_rejected() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request_for_range(7, 64, 8);
    let (first, first_bytes) = ranged_frame_event(7, 64, 5, 4);
    assert_eq!(
        acc.apply(&first, Some(&first_bytes)).unwrap(),
        ApplyOutcome::FrameSummed { samples_done: 5 }
    );
    let (second, second_bytes) = ranged_frame_event(7, 67, 5, 4);
    assert_eq!(
        acc.apply(&second, Some(&second_bytes)).unwrap_err(),
        RadianceError::SampleCountOverrun {
            cumulative: 10,
            range_len: 8,
        }
    );
    assert_eq!(
        acc.samples_done(),
        5,
        "the rejected FRAME must not be counted"
    );
    assert!(
        acc.buffer().iter().all(|v| *v == Vec3::ONE),
        "only the first FRAME's sum must land"
    );
}

/// A FRAME claiming zero samples but carrying a payload is refused, not summed.
#[test]
fn a_zero_sample_frame_with_a_payload_is_rejected() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(7);
    let (event, bytes) = ranged_frame_event(7, 0, 0, 4);
    assert_eq!(
        acc.apply(&event, Some(&bytes)).unwrap_err(),
        RadianceError::ZeroSampleFrame
    );
    assert!(acc.buffer().iter().all(|v| *v == Vec3::ZERO));
}

/// A pixel with a NaN, infinite or negative component is skipped and counted; the frame's
/// samples still count as done and the other pixels sum normally.
#[test]
fn invalid_radiance_pixels_are_skipped_but_the_samples_still_count() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(7);
    let pixels = [
        Vec3::ONE,
        Vec3::new(f32::NAN, 1.0, 1.0),
        Vec3::new(1.0, f32::INFINITY, 1.0),
        Vec3::new(1.0, 1.0, -1.0),
    ];
    let bytes = radiance::encode(&pixels);
    let header = FrameHeader::for_payload(7, 0, 2, &bytes);
    assert_eq!(
        acc.apply(&StreamEvent::Frame(header), Some(&bytes))
            .unwrap(),
        ApplyOutcome::FrameSummed { samples_done: 2 }
    );
    assert_eq!(acc.dropped_pixels(), 3);
    assert_eq!(
        acc.buffer(),
        &[Vec3::ONE, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO]
    );
}

/// Without a declared range, any FRAME range is accepted -- the pre-existing
/// `begin_request` contract is unchanged.
#[test]
fn without_a_declared_range_any_frame_range_is_accepted() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(7);
    let (event, bytes) = ranged_frame_event(7, 1_000_000, 4, 4);
    assert_eq!(
        acc.apply(&event, Some(&bytes)).unwrap(),
        ApplyOutcome::FrameSummed { samples_done: 4 }
    );
}

#[test]
fn a_malformed_frame_payload_is_a_hard_error_not_a_stale_drop() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(1);
    let header = FrameHeader {
        request_id: 1,
        first_sample: 0,
        samples: 1,
        payload_len: 3,
        encoding: crate::messages::PayloadEncoding::Raw,
        raw_len: 3,
    };
    let bad_bytes = vec![0u8; 3]; // not a multiple of BYTES_PER_PIXEL, and wrong length for 2x2
    let event = StreamEvent::Frame(header);
    assert!(acc.apply(&event, Some(&bad_bytes)).is_err());
}

/// A deterministic, smooth-ish delta (compresses) with a few special values mixed in.
fn delta_buffer(pixels: usize, seed: u32) -> Vec<Vec3> {
    let mut state = seed | 1;
    (0..pixels)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let v = f32::from_bits((1.0 + (i % 17) as f32 / 17.0).to_bits() ^ (state & 0xf));
            match i % 11 {
                0 => Vec3::new(-0.0, v, f32::MIN_POSITIVE / 4.0),
                _ => Vec3::new(v, v * 0.5, v * 0.25),
            }
        })
        .collect()
}

fn bits(values: &[Vec3]) -> Vec<u32> {
    values
        .iter()
        .flat_map(glam::Vec3::to_array)
        .map(f32::to_bits)
        .collect()
}

/// Every supported encoding sums into the accumulator bit-identically to `Raw`, over
/// several frames (the epoch's running sum, not just one delta).
#[test]
fn compressed_frames_sum_bit_identically_to_raw_frames() {
    use crate::messages::PayloadEncoding;
    let (w, h) = (23, 9);
    let pixels = (w * h) as usize;
    let deltas: Vec<Vec<Vec3>> = (0..3).map(|s| delta_buffer(pixels, 40 + s)).collect();

    let sum_with = |encoding: PayloadEncoding| {
        let mut acc = Accumulator::new(w, h);
        acc.begin_request_for_range(1, 0, 30);
        let mut encoder = radiance::PayloadEncoder::new(encoding);
        for (i, delta) in deltas.iter().enumerate() {
            let encoded = encoder.encode(radiance::as_bytes(delta));
            let header = FrameHeader::for_encoded(1, i as u32 * 10, 10, &encoded);
            let outcome = acc
                .apply(&StreamEvent::Frame(header), Some(encoded.bytes))
                .unwrap();
            assert!(matches!(outcome, ApplyOutcome::FrameSummed { .. }));
        }
        assert_eq!(acc.samples_done(), 30);
        bits(acc.buffer())
    };

    let reference = sum_with(PayloadEncoding::Raw);
    for encoding in [
        PayloadEncoding::ShuffleZstd { level: 1 },
        PayloadEncoding::ShuffleLz4,
    ]
    .into_iter()
    .filter(|e| e.is_supported())
    {
        assert_eq!(sum_with(encoding), reference, "{encoding:?}");
    }
}

/// A compressed frame whose `raw_len` lies is a hard error, and the buffer is untouched.
#[cfg(feature = "compression")]
#[test]
fn a_compressed_frame_with_a_lying_raw_len_is_a_hard_error() {
    use crate::messages::PayloadEncoding;
    let mut acc = Accumulator::new(4, 4);
    acc.begin_request(1);
    let delta = delta_buffer(16, 3);
    let mut encoder = radiance::PayloadEncoder::new(PayloadEncoding::ShuffleLz4);
    let encoded = encoder.encode(radiance::as_bytes(&delta));
    let mut header = FrameHeader::for_encoded(1, 0, 1, &encoded);
    header.raw_len *= 2;
    let err = acc
        .apply(&StreamEvent::Frame(header), Some(encoded.bytes))
        .unwrap_err();
    assert!(
        matches!(err, RadianceError::RawLenMismatch { .. }),
        "{err:?}"
    );
    assert!(acc.buffer().iter().all(|v| *v == Vec3::ZERO));
    assert_eq!(acc.samples_done(), 0);
}

/// A compressed PREVIEW decodes at its own header's size.
#[test]
fn a_compressed_preview_decodes_at_its_own_size() {
    use crate::messages::PayloadEncoding;
    let mut acc = Accumulator::new(8, 8);
    acc.begin_request(4);
    let preview = delta_buffer(6, 9);
    let mut encoder = radiance::PayloadEncoder::new(PayloadEncoding::DEFAULT_ZSTD);
    let encoded = encoder.encode(radiance::as_bytes(&preview));
    let header = PreviewHeader::for_encoded(4, 3, 2, 12, &encoded);
    assert_eq!(
        acc.apply(&StreamEvent::Preview(header), Some(encoded.bytes))
            .unwrap(),
        ApplyOutcome::PreviewReplaced
    );
    assert_eq!(bits(&acc.last_preview().unwrap().buffer), bits(&preview));
}

/// Coordinator FRAMEs (B6 semantics): each carries a SET of samples, reports the
/// request's own `first_sample`, and an exact count. Containment holds for each, so all
/// are summed and the count is exact.
#[test]
fn coordinator_style_set_frames_are_accepted_by_containment() {
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request_for_range(9, 1000, 64);
    for samples in [10, 30, 24] {
        let (event, bytes) = ranged_frame_event(9, 1000, samples, 4);
        assert!(matches!(
            acc.apply(&event, Some(&bytes)).unwrap(),
            ApplyOutcome::FrameSummed { .. }
        ));
    }
    assert_eq!(acc.samples_done(), 64);
    assert!(acc.buffer().iter().all(|v| *v == Vec3::splat(3.0)));
}

/// `DISPLAY_FRAME`/`FINAL_IMAGE` are epoch-gated, kept encoded, replaced by newer ones and
/// cleared by `begin_request`.
#[test]
fn display_frames_and_final_images_are_epoch_gated_and_kept_encoded() {
    use crate::messages::{DisplayEncoding, DisplayFrameHeader, FinalImageHeader};
    let rgba = vec![7u8; 2 * 2 * 4];
    let display = |request_id: u32, samples_done: u32| {
        StreamEvent::DisplayFrame(DisplayFrameHeader {
            request_id,
            samples_done,
            width: 2,
            height: 2,
            encoding: DisplayEncoding::Rgba8,
            payload_len: 16,
        })
    };
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(3);
    assert_eq!(
        acc.apply(&display(2, 5), Some(&rgba)).unwrap(),
        ApplyOutcome::StaleDropped
    );
    assert_eq!(
        acc.apply(&display(3, 5), Some(&rgba)).unwrap(),
        ApplyOutcome::DisplayFrameReplaced
    );
    assert_eq!(
        acc.apply(&display(3, 9), Some(&rgba)).unwrap(),
        ApplyOutcome::DisplayFrameReplaced
    );
    let shown = acc.last_display_frame().unwrap();
    assert_eq!(
        (shown.samples_done, shown.bytes.as_slice()),
        (9, rgba.as_slice())
    );

    let png_bytes = vec![1u8, 2, 3];
    let final_image = StreamEvent::FinalImage(FinalImageHeader {
        request_id: 3,
        width: 2,
        height: 2,
        samples_done: 9,
        encoding: DisplayEncoding::Png,
        payload_len: 3,
    });
    assert_eq!(
        acc.apply(&final_image, Some(&png_bytes)).unwrap(),
        ApplyOutcome::FinalImageReceived
    );
    assert_eq!(acc.final_image().unwrap().bytes, png_bytes);

    acc.begin_request(4);
    assert!(acc.last_display_frame().is_none() && acc.final_image().is_none());
}

/// A raw RGBA8 display frame of the wrong length is a hard error.
#[test]
fn a_raw_display_frame_of_the_wrong_length_is_rejected() {
    use crate::messages::{DisplayEncoding, DisplayFrameHeader};
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(1);
    let event = StreamEvent::DisplayFrame(DisplayFrameHeader {
        request_id: 1,
        samples_done: 1,
        width: 2,
        height: 2,
        encoding: DisplayEncoding::Rgba8,
        payload_len: 3,
    });
    assert!(matches!(
        acc.apply(&event, Some(&[0, 0, 0])),
        Err(RadianceError::LengthMismatch { .. })
    ));
}

/// `PONG` and `CAPABILITY_CHANGED` carry no `request_id` and are reported even before any
/// request started.
#[test]
fn pong_and_capability_changed_are_never_epoch_gated() {
    let mut acc = Accumulator::new(1, 1);
    assert_eq!(
        acc.apply(&StreamEvent::Pong { nonce: 42 }, None).unwrap(),
        ApplyOutcome::Pong { nonce: 42 }
    );
    assert_eq!(
        acc.apply(&StreamEvent::CapabilityChanged { render: None }, None)
            .unwrap(),
        ApplyOutcome::CapabilityChanged
    );
}

/// v16: `DONE.stats` is kept for the epoch it belongs to (e.g. so a caller can read
/// `reclaimed_samples` after the fact), and cleared the moment the next epoch begins --
/// exactly like `final_image`.
#[test]
fn done_stats_are_kept_for_the_epoch_and_cleared_on_begin_request() {
    let mut acc = Accumulator::new(1, 1);
    acc.begin_request(1);
    assert_eq!(acc.done_stats(), None);

    let stats = Stats {
        samples_done: 8,
        requested_cadence_ms: 0,
        effective_cadence_ms: 0,
        reclaimed_samples: 3,
    };
    let done = StreamEvent::Done(Done {
        request_id: 1,
        cancelled: false,
        stats,
    });
    assert_eq!(
        acc.apply(&done, None).unwrap(),
        ApplyOutcome::Done { cancelled: false }
    );
    assert_eq!(acc.done_stats(), Some(stats));

    acc.begin_request(2);
    assert_eq!(
        acc.done_stats(),
        None,
        "begin_request must clear the previous epoch's DONE stats"
    );
}

/// `NEED_ASSET` carries no `request_id` and is reported whatever the current epoch.
#[test]
fn need_asset_is_never_epoch_gated() {
    let mut acc = Accumulator::new(1, 1);
    acc.begin_request(3);
    assert_eq!(
        acc.apply(
            &StreamEvent::NeedAsset {
                content_hash: [9; 32]
            },
            None
        )
        .unwrap(),
        ApplyOutcome::NeedAsset {
            content_hash: [9; 32]
        }
    );
}
