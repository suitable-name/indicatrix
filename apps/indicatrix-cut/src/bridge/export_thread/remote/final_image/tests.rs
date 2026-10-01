//! Pure decisions of the final-picture transfer and the picture -> PNG path.

use super::*;
use crate::bridge::export_thread::tonemap_png::{save_png, tonemap_accumulation};
use glam::Vec3;
use indicatrix_net::{
    messages::{
        DisplayEncoding, Done, FinalImageHeader, PayloadEncoding, PreviewHeader, Stats, StreamEvent,
    },
    radiance::{self, PayloadEncoder},
};

// ---- plan_export_transfer --------------------------------------------------------

#[test]
fn final_picture_is_used_only_when_asked_for_with_a_usable_remote() {
    use ComputeTarget::{Both, LocalOnly, RemoteOnly};
    use ExportTransfer::{FinalPicture, FullData};
    let plan = plan_export_transfer;
    assert_eq!(
        plan(FinalPicture, Both, true, false, false),
        TransferPlan::FinalPicture
    );
    assert_eq!(
        plan(FinalPicture, RemoteOnly, true, false, false),
        TransferPlan::FinalPicture
    );
    // Asked for full data, local-only, no remote, or an HDR scene: never.
    assert_eq!(
        plan(FullData, Both, true, false, false),
        TransferPlan::FullData
    );
    assert_eq!(
        plan(FinalPicture, LocalOnly, true, false, false),
        TransferPlan::FullData
    );
    assert_eq!(
        plan(FinalPicture, Both, false, false, false),
        TransferPlan::FullData
    );
    assert_eq!(
        plan(FinalPicture, Both, true, true, false),
        TransferPlan::FullData
    );
    // A remote that already refused is not asked again.
    assert_eq!(
        plan(FinalPicture, Both, true, false, true),
        TransferPlan::FullDataRefusedBefore
    );
    assert_eq!(
        plan(FullData, Both, true, false, true),
        TransferPlan::FullData
    );
}

// ---- final_picture_follow_up: the UNSUPPORTED_REQUEST / failure fallbacks ----------

#[test]
fn unsupported_request_falls_back_to_full_data_and_is_remembered() {
    for target in [ComputeTarget::Both, ComputeTarget::RemoteOnly] {
        let follow_up = final_picture_follow_up(
            FinalPictureOutcome::Unsupported("not a coordinator".to_string()),
            target,
        );
        let FinalPictureFollowUp::FallBackToFullData { note, remember } = follow_up else {
            panic!("expected a fallback, got {follow_up:?}");
        };
        assert!(
            remember,
            "a plain worker will not grow the feature on a retry"
        );
        assert!(note.contains("full data"), "{note}");
    }
}

#[test]
fn a_failure_falls_back_under_both_but_fails_under_remote_only() {
    let both = final_picture_follow_up(
        FinalPictureOutcome::Failed("All remote workers were lost".to_string()),
        ComputeTarget::Both,
    );
    assert!(matches!(
        both,
        FinalPictureFollowUp::FallBackToFullData { remember: false, ref note }
            if note.contains("All remote workers were lost")
    ));
    let remote_only = final_picture_follow_up(
        FinalPictureOutcome::Failed("boom".to_string()),
        ComputeTarget::RemoteOnly,
    );
    assert!(matches!(remote_only, FinalPictureFollowUp::Fail(ref m) if m.contains("boom")));
}

#[test]
fn a_picture_is_used_and_a_cancel_stays_a_cancel() {
    assert_eq!(
        final_picture_follow_up(
            FinalPictureOutcome::Completed {
                rgba: vec![1, 2, 3, 4],
                reclaimed_samples: 0,
            },
            ComputeTarget::Both
        ),
        FinalPictureFollowUp::Use(vec![1, 2, 3, 4])
    );
    assert_eq!(
        final_picture_follow_up(FinalPictureOutcome::Cancelled, ComputeTarget::RemoteOnly),
        FinalPictureFollowUp::Cancelled
    );
}

#[test]
fn refusals_are_remembered_per_address_until_forgotten() {
    let worker = WorkerSettings {
        address: "final-picture-refusal-test.invalid:7878".to_string(),
        ..WorkerSettings::default()
    };
    let other = WorkerSettings {
        address: "final-picture-other-test.invalid:7878".to_string(),
        ..WorkerSettings::default()
    };
    remember_final_picture_refused(&worker);
    assert!(final_picture_refused(&worker));
    assert!(!final_picture_refused(&other));
}

// ---- The picture -> RGBA -> PNG path ---------------------------------------------

/// A small deterministic accumulation buffer.
fn accumulation(width: u32, height: u32) -> Vec<Vec3> {
    (0..width * height)
        .map(|i| {
            let f = i as f32;
            Vec3::new(
                f.mul_add(0.11, 0.3),
                f.mul_add(0.07, 0.2),
                0.1 + (f * 0.13) % 2.0,
            )
        })
        .collect()
}

/// An accumulator holding `png` as request 1's `FINAL_IMAGE`, as the connection
/// thread leaves it before `DONE`.
fn accumulator_with_final_image(width: u32, height: u32, png: &[u8]) -> Accumulator {
    let mut acc = Accumulator::new(width, height);
    acc.begin_request(1);
    let header = FinalImageHeader {
        request_id: 1,
        width,
        height,
        samples_done: 16,
        encoding: DisplayEncoding::Png,
        payload_len: png.len() as u32,
    };
    acc.apply(&StreamEvent::FinalImage(header), Some(png))
        .expect("a well-formed picture applies");
    acc
}

/// The server tone-maps with the SAME function and sends a lossless PNG, so the
/// decoded picture equals the viewer's own tone-mapped bytes, and the file the viewer
/// writes from it (ICC included) is byte-identical to a local export of that RGBA --
/// for sRGB and a wide-gamut space alike.
#[test]
fn a_decoded_final_picture_writes_the_same_png_as_a_local_export() {
    let (width, height) = (7_u32, 5_u32);
    let accum = accumulation(width, height);
    let dir = std::env::temp_dir().join(format!("final_picture_png_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for color_space in [ColorSpace::Srgb, ColorSpace::DisplayP3] {
        let local = tonemap_accumulation(width, height, 16, &accum, color_space);
        let wire =
            indicatrix_net::display::encode_rgba8(DisplayEncoding::Png, width, height, &local)
                .unwrap();
        let acc = accumulator_with_final_image(width, height, &wire);
        let remote = decode_final_picture(&acc, width, height).expect("decodes");
        assert_eq!(
            remote, local,
            "{color_space:?}: lossless relative to the product"
        );

        let local_path = dir.join(format!("local_{color_space:?}.png"));
        let remote_path = dir.join(format!("remote_{color_space:?}.png"));
        save_png(&local_path, width, height, &local, color_space).unwrap();
        save_png(&remote_path, width, height, &remote, color_space).unwrap();
        assert_eq!(
            std::fs::read(&local_path).unwrap(),
            std::fs::read(&remote_path).unwrap(),
            "{color_space:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- on_update: RemoteUpdate::Preview -> a progress preview ----------------------

/// A `RemoteUpdate::Preview` -- the coordinator's forced periodic look at
/// a `FinalImageRequest` job (`coordinator::job::serve_final_image`) -- must reach
/// `on_progress` as a tone-mapped thumbnail instead of being silently ignored, which
/// left the export dialog showing no live preview at all for this transfer.
#[test]
fn a_preview_update_becomes_a_progress_preview() {
    let (width, height) = (4_u32, 3_u32);
    let accumulator = Mutex::new(Accumulator::new(width, height));
    accumulator.lock().unwrap().begin_request(1);

    let radiance_buf = accumulation(width, height);
    let mut encoder = PayloadEncoder::new(PayloadEncoding::Raw);
    let encoded = encoder.encode(radiance::as_bytes(&radiance_buf));
    let header = PreviewHeader::for_encoded(1, width, height, 7, &encoded);
    accumulator
        .lock()
        .unwrap()
        .apply(&StreamEvent::Preview(header), Some(encoded.bytes))
        .expect("a well-formed PREVIEW applies");

    let mut seen: Option<(u32, u32, Option<SharedPixelBuffer<Rgba8Pixel>>)> = None;
    let outcome = on_update(
        RemoteUpdate::Preview { request_id: 1 },
        &accumulator,
        width,
        height,
        0,
        &mut |samples_done, local_done, preview| seen = Some((samples_done, local_done, preview)),
    );

    assert!(outcome.is_none(), "a PREVIEW never ends the request");
    let (samples_done, local_done, preview) =
        seen.expect("on_progress must fire for a PREVIEW update");
    assert_eq!(
        samples_done, 7,
        "must report the snapshot's own samples_done"
    );
    assert_eq!(
        local_done, 0,
        "must pass the caller's own local_done through"
    );
    let preview = preview.expect("a PREVIEW update must carry a tone-mapped image");
    assert_eq!(preview.width(), width);
    assert_eq!(preview.height(), height);
}

// ---- v16: viewer contribution -- share sizing, the rate book, and reclaim reporting --

#[test]
fn viewer_share_is_proportional_to_rates_and_capped_at_half() {
    // A slower local machine gets a proportionally smaller share.
    assert_eq!(viewer_share(100, Some(100.0), Some(300.0)), 25);
    // A faster local machine would be proportionally larger, but never past the half
    // the server accepts (`FinalImageRequest::viewer_share_valid`).
    assert_eq!(viewer_share(100, Some(300.0), Some(100.0)), 50);
    assert_eq!(viewer_share(100, Some(1_000.0), Some(1.0)), 50);
}

#[test]
fn viewer_share_falls_back_to_ten_percent_without_rates() {
    assert_eq!(viewer_share(100, None, None), 10);
    assert_eq!(viewer_share(100, Some(5.0), None), 10);
    assert_eq!(viewer_share(100, None, Some(5.0)), 10);
    // Non-positive or non-finite rates are exactly as unusable as a missing one.
    assert_eq!(viewer_share(100, Some(0.0), Some(5.0)), 10);
    assert_eq!(viewer_share(100, Some(-1.0), Some(5.0)), 10);
    assert_eq!(viewer_share(100, Some(f64::NAN), Some(5.0)), 10);
}

#[test]
fn viewer_share_is_zero_for_a_one_sample_budget() {
    // `half = samples / 2 == 0` caps every share to zero, however lopsided the rates.
    assert_eq!(viewer_share(1, Some(1_000.0), Some(1.0)), 0);
    assert_eq!(viewer_share(0, None, None), 0);
}

#[test]
fn split_rates_are_pixel_normalised_and_forgotten_with_refusals() {
    let worker = WorkerSettings {
        address: "split-rate-test.invalid:7878".to_string(),
        ..WorkerSettings::default()
    };
    assert_eq!(
        split_rates(&worker, 100),
        (None, None),
        "nothing measured yet"
    );

    record_split_rates(&worker, 100, Some(50.0), Some(200.0));
    let (local, remote) = split_rates(&worker, 100);
    assert!((local.unwrap() - 50.0).abs() < 1e-9);
    assert!((remote.unwrap() - 200.0).abs() < 1e-9);

    // The SAME underlying device throughput reads back scaled at a different
    // resolution -- the whole point of pixel-normalising before storing.
    let (local_at_200, remote_at_200) = split_rates(&worker, 200);
    assert!((local_at_200.unwrap() - 25.0).abs() < 1e-9);
    assert!((remote_at_200.unwrap() - 100.0).abs() < 1e-9);

    forget_final_picture_refusals();
    assert_eq!(
        split_rates(&worker, 100),
        (None, None),
        "forgetting refusals also forgets the measured split"
    );
}

/// `on_update`'s `DONE` arm must unpack `Accumulator::done_stats().reclaimed_samples`
/// into `FinalPictureOutcome::Completed`, not just decode the picture -- this is the
/// data `worker::final_picture::final_picture` turns into a "the coordinator rendered N
/// of your samples itself" note.
#[test]
fn a_reclaim_in_done_stats_becomes_a_note() {
    let (width, height) = (2_u32, 2_u32);
    let rgba = vec![5_u8; (width * height * 4) as usize];
    let png =
        indicatrix_net::display::encode_rgba8(DisplayEncoding::Png, width, height, &rgba).unwrap();
    let acc = Mutex::new(accumulator_with_final_image(width, height, &png));
    acc.lock()
        .unwrap()
        .apply(
            &StreamEvent::Done(Done {
                request_id: 1,
                cancelled: false,
                stats: Stats {
                    samples_done: 16,
                    requested_cadence_ms: 0,
                    effective_cadence_ms: 0,
                    reclaimed_samples: 3,
                },
            }),
            None,
        )
        .expect("a well-formed DONE applies");

    let outcome = on_update(
        RemoteUpdate::Done {
            request_id: 1,
            cancelled: false,
        },
        &acc,
        width,
        height,
        0,
        &mut |_, _, _| {},
    );
    let Some(FinalPictureOutcome::Completed {
        reclaimed_samples, ..
    }) = outcome
    else {
        panic!("expected a completed picture carrying reclaimed_samples, got {outcome:?}");
    };
    assert_eq!(reclaimed_samples, 3);
}

#[test]
fn a_missing_or_mis_sized_picture_is_refused() {
    let acc = Accumulator::new(4, 4);
    assert!(
        decode_final_picture(&acc, 4, 4).is_err(),
        "no picture arrived"
    );

    let rgba = vec![9_u8; 2 * 2 * 4];
    let png = indicatrix_net::display::encode_rgba8(DisplayEncoding::Png, 2, 2, &rgba).unwrap();
    let mut acc = Accumulator::new(2, 2);
    acc.begin_request(1);
    let header = FinalImageHeader {
        request_id: 1,
        width: 2,
        height: 2,
        samples_done: 1,
        encoding: DisplayEncoding::Png,
        payload_len: png.len() as u32,
    };
    acc.apply(&StreamEvent::FinalImage(header), Some(&png))
        .unwrap();
    assert_eq!(decode_final_picture(&acc, 2, 2).unwrap(), rgba);
    assert!(
        decode_final_picture(&acc, 4, 4).is_err(),
        "a picture of another size never reaches the writer"
    );
}
