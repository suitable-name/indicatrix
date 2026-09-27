//! Pure decisions of the final-picture transfer and the picture -> PNG path.

use super::*;
use crate::bridge::export_thread::tonemap_png::{save_png, tonemap_accumulation};
use glam::Vec3;
use indicatrix_net::messages::{DisplayEncoding, FinalImageHeader, StreamEvent};

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
            FinalPictureOutcome::Completed(vec![1, 2, 3, 4]),
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
