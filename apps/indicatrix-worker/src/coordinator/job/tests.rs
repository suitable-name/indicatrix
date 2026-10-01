use super::*;
use indicatrix_dispatch::ChunkPolicy;
use indicatrix_net::messages::Backend;

/// A bare, otherwise-irrelevant `WorkerInfo` for `LaneKey`/`RateBook` tests.
fn worker_info(worker_id: u32, label: Option<&str>) -> WorkerInfo {
    WorkerInfo {
        worker_id,
        capability: RenderCapability {
            backend: Backend::Cpu { threads: 1 },
            max_pixels: 1_000,
            min_cadence_ms: 100,
            hdr: false,
        },
        peer: None,
        label: label.map(str::to_string),
        payload_encoding: PayloadEncoding::Raw,
    }
}

/// A worker with a certificate label keys by that label -- the identity that
/// survives `join --slots K` and reconnects; one without a label (no TLS) falls
/// back to its ephemeral per-registration id.
#[test]
fn for_worker_prefers_the_certificate_label_over_the_ephemeral_id() {
    let labelled = worker_info(7, Some("a100"));
    assert_eq!(
        LaneKey::for_worker(&labelled),
        LaneKey::Worker(WorkerIdentity::Label("a100".to_string()))
    );

    let unlabelled = worker_info(7, None);
    assert_eq!(
        LaneKey::for_worker(&unlabelled),
        LaneKey::Worker(WorkerIdentity::Id(7))
    );
}

/// Task: coordinator load balancing -- the book behind `Coordinator::rates` is one
/// shared instance, not a fresh one per caller: a rate set through one clone (as one
/// viewer connection would hold) is visible through another (as a later viewer
/// connection, or a GUI export's next one-shot connection, would hold). The old
/// per-connection `Arc::default()` started every connection from an empty book.
#[test]
fn the_coordinators_rate_book_is_shared_across_connections() {
    let coordinator = Coordinator::new(None, None, 0, 1 << 30);
    let first_connection = Arc::clone(coordinator.rates());
    let second_connection = Arc::clone(coordinator.rates());
    let key = LaneKey::for_worker(&worker_info(1, Some("a100")));

    first_connection
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set(key.clone(), 1024, 900.0);

    assert_eq!(
        second_connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key, 1024),
        Some(900.0)
    );
}

/// A rate calibrated on one image size, reused for a very differently sized
/// job, must read back roughly proportional to the pixel-count ratio -- not the
/// raw samples/sec figure, which would size a 3840x2160 job's chunks as if it were
/// as cheap per sample as a 512x512 one.
#[test]
fn a_rate_calibrated_at_one_resolution_scales_down_for_a_much_larger_job() {
    let small_pixels = 512 * 512;
    let large_pixels = 3840 * 2160;
    let mut book = RateBook::default();
    let measured_rate = 40.0; // samples/sec, calibrated on the 512x512 job.
    let key = LaneKey::Worker(WorkerIdentity::Id(1));
    book.set(key.clone(), small_pixels, measured_rate);

    let read_back = book.get(&key, large_pixels).unwrap();
    let expected = measured_rate * (f64::from(small_pixels) / f64::from(large_pixels));
    assert!(
        (read_back - expected).abs() < 1e-9,
        "read_back={read_back} expected={expected}"
    );

    // Reused for chunk sizing at the large resolution: roughly
    // target_secs * rate * (small_pixels / large_pixels) samples, not
    // target_secs * rate (which would be ~85x too large here).
    let policy = ChunkPolicy::EXPORT;
    let chunk_samples = policy.samples_for_rate(read_back);
    let naive_chunk_samples = policy.samples_for_rate(measured_rate);
    assert!(
        chunk_samples < naive_chunk_samples,
        "a job 32x the calibrated resolution must get smaller chunks from the \
         normalised rate, not the same ones the raw rate would produce: \
         chunk_samples={chunk_samples} naive={naive_chunk_samples}"
    );
}

/// A `FinalImageRequest` job's forced `PREVIEW` never upsamples an image already at
/// or under the 360px long-edge cap -- e.g. the tiny scenes several other tests in
/// this crate render at.
#[test]
fn final_image_preview_config_never_upsamples_a_small_request() {
    let cfg = final_image_preview_config(8, 6);
    assert_eq!(
        cfg,
        PreviewConfig {
            width: 8,
            height: 6
        }
    );
}

/// A request past the cap is downsampled so its long edge lands exactly at 360,
/// with the short edge scaled proportionally (never zero, per
/// [`final_image_preview_config`]'s own `.max(1)`).
#[test]
fn final_image_preview_config_caps_the_long_edge_at_360() {
    let cfg = final_image_preview_config(3840, 2160);
    assert_eq!(cfg.width, 360);
    // 2160 * (360 / 3840) == 202.5 exactly; `f64::round` breaks the tie away from
    // zero, giving 203.
    assert_eq!(cfg.height, 203);

    // A portrait request caps its (long) height instead, scaling width down with it.
    let portrait = final_image_preview_config(2160, 3840);
    assert_eq!(portrait.height, 360);
    assert_eq!(portrait.width, 203);
}
