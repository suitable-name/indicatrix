use super::*;
use crate::{
    cli::ALL_INTERACTIVE_WORKERS,
    coordinator::{JobLanes, LaneNeed, LivenessConfig},
};
use indicatrix_dispatch::ChunkPolicy;
use indicatrix_net::messages::Backend;
use std::{
    collections::BTreeSet,
    net::{TcpListener, TcpStream},
};

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

/// The most pixels the fake workers of [`idle_registry`] accept.
const FAKE_MAX_PIXELS: u32 = 1 << 22;

fn gpu() -> Backend {
    Backend::Gpu {
        adapter: "test".to_string(),
    }
}

fn label_key(label: &str) -> LaneKey {
    LaneKey::Worker(WorkerIdentity::Label(label.to_string()))
}

/// A registry holding one idle fake connection per `(backend, label)`: a loopback pair
/// whose far ends are returned, to keep the connections open.
fn idle_registry(workers: &[(Backend, &str)]) -> (Arc<Registry>, Vec<TcpStream>) {
    let registry = Registry::new(LivenessConfig::default());
    let mut far_ends = Vec::new();
    for (backend, label) in workers {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        far_ends.push(listener.accept().unwrap().0);
        let info = WorkerInfo {
            worker_id: registry.allocate_id(),
            capability: RenderCapability {
                backend: backend.clone(),
                max_pixels: FAKE_MAX_PIXELS,
                min_cadence_ms: 100,
                hdr: false,
            },
            peer: None,
            label: Some((*label).to_string()),
            payload_encoding: PayloadEncoding::Raw,
        };
        registry.insert(info, Box::new(near), None);
    }
    (registry, far_ends)
}

/// A coordinator over `registry` with an own lane (never asked to render here) when
/// `own` is set.
fn coordinator_over(
    registry: Option<&Arc<Registry>>,
    own: bool,
    interactive_workers: u32,
) -> Coordinator {
    let own = own.then(|| OwnLaneSetup {
        gpu: Arc::new(GpuBackend::disabled()),
        threads: 1,
        compute_mode: ComputeMode::OnlyCpu,
    });
    Coordinator::new(registry.map(Arc::clone), own, interactive_workers, 1 << 30)
}

fn batch_ask(width: u32, height: u32, samples: u32) -> Ask {
    Ask {
        intent: RequestIntent::Batch,
        transfer_mode: TransferMode::FinalOnly,
        pixels: width * height,
        samples,
        hdr: false,
    }
}

fn live_ask() -> Ask {
    Ask {
        intent: RequestIntent::Interactive,
        transfer_mode: TransferMode::LiveProgressive,
        pixels: 160 * 160,
        samples: 64,
        hdr: false,
    }
}

fn planned_job(coordinator: &Coordinator, ask: Ask) -> JobPlan {
    match plan::plan(coordinator, ask) {
        Ok(Route::Job(job)) => job,
        other => panic!("expected a job route, got {other:?}"),
    }
}

/// A small `Batch` request with an idle GPU worker is one picture on the fastest worker,
/// outside the viewer's FIFO and without the own lane.
#[test]
fn a_small_batch_request_with_an_idle_gpu_worker_plans_whole_image() {
    let (registry, _far) = idle_registry(&[(gpu(), "a6000")]);
    let coordinator = coordinator_over(Some(&registry), true, ALL_INTERACTIVE_WORKERS);
    let planned = planned_job(&coordinator, batch_ask(160, 160, 256));
    assert!(planned.whole_image);
    assert_eq!(planned.workers, WorkerPick::Fastest(1));
    assert!(!planned.own, "the own lane is only the fallback");
    assert!(
        !planned.fifo,
        "a whole-image job takes a slot, not the FIFO"
    );
    assert_eq!(planned.pool, coordinator.job_config().batch);
}

/// A 1080p x 512 request is real work: it keeps the old fan-out over every lane and the
/// viewer's FIFO.
#[test]
fn a_large_batch_request_is_split_over_every_lane() {
    let (registry, _far) = idle_registry(&[(gpu(), "a6000")]);
    let coordinator = coordinator_over(Some(&registry), true, ALL_INTERACTIVE_WORKERS);
    let planned = planned_job(&coordinator, batch_ask(1920, 1080, 512));
    assert!(!planned.whole_image);
    assert_eq!(planned.workers, WorkerPick::All);
    assert!(planned.own && planned.fifo);
}

/// With no joined worker able to take the image there is no lane to render it whole on:
/// a small request is the plain own-lane route, or the capability refusal without an own
/// lane.
#[test]
fn without_an_eligible_worker_a_small_request_is_not_whole_image() {
    let (registry, _far) = idle_registry(&[(gpu(), "a6000")]);
    // 2500 x 2500 is more than FAKE_MAX_PIXELS, yet one sample is under the size limit.
    let too_big_for_the_worker = batch_ask(2500, 2500, 1);
    let with_own = coordinator_over(Some(&registry), true, ALL_INTERACTIVE_WORKERS);
    assert_eq!(
        plan::plan(&with_own, too_big_for_the_worker),
        Ok(Route::Direct)
    );
    let alone = coordinator_over(None, true, ALL_INTERACTIVE_WORKERS);
    assert_eq!(
        plan::plan(&alone, batch_ask(160, 160, 256)),
        Ok(Route::Direct)
    );
    let without_own = coordinator_over(Some(&registry), false, ALL_INTERACTIVE_WORKERS);
    let refusal = plan::plan(&without_own, too_big_for_the_worker).unwrap_err();
    assert_eq!(refusal.code, error_codes::UNSUPPORTED_REQUEST);
}

/// A measured rate replaces the size rule: a slow worker keeps even a small request
/// split, a fast one takes a request far past the size limit whole.
#[test]
fn a_measured_rate_decides_whole_image_instead_of_the_size_rule() {
    let (registry, _far) = idle_registry(&[(gpu(), "a6000")]);
    let coordinator = coordinator_over(Some(&registry), true, 0);
    let small = batch_ask(160, 160, 256);
    let huge = batch_ask(1920, 1080, 512);
    assert!(
        planned_job(&coordinator, small).whole_image,
        "no rate: size"
    );
    assert!(
        !planned_job(&coordinator, huge).whole_image,
        "no rate: size"
    );

    let set_rate = |samples_per_sec: f64| {
        coordinator
            .rates()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set(label_key("a6000"), 160 * 160, samples_per_sec);
    };
    // 256 samples at 50 samples/s take 5.12 s.
    set_rate(50.0);
    assert!(!planned_job(&coordinator, small).whole_image);
    // 256 samples at 1000 samples/s take 0.256 s; the same worker needs ~41 s for 1080p
    // x 512 (the book scales a rate by pixel count).
    set_rate(1000.0);
    assert!(planned_job(&coordinator, small).whole_image);
    assert!(!planned_job(&coordinator, huge).whole_image);
    // 1e9 pixel-samples per second renders 1080p x 512 in about 1.06 s.
    set_rate(1e9 / f64::from(160 * 160));
    assert!(planned_job(&coordinator, huge).whole_image);
}

/// `--whole-image-secs` and `--whole-image-pixel-samples` are the thresholds; zero for
/// both turns the routing off.
#[test]
fn the_thresholds_come_from_the_job_config() {
    let (registry, _far) = idle_registry(&[(gpu(), "a6000")]);
    let coordinator = coordinator_over(Some(&registry), false, 0);
    let ask = batch_ask(160, 160, 256);
    assert!(planned_job(&coordinator, ask).whole_image);
    let coordinator = coordinator.with_small_pictures(2.0, 6_000_000, 3);
    assert!(!planned_job(&coordinator, ask).whole_image, "6.55M > 6M");
    let coordinator = coordinator.with_small_pictures(0.0, 0, 0);
    assert!(!planned_job(&coordinator, ask).whole_image);
    assert_eq!(
        coordinator.job_config().jobs_per_viewer,
        1,
        "0 jobs per viewer means 1"
    );
}

/// The live view takes every idle joined worker besides the own lane by default; `0` is
/// the old own-lane-only route, and a count caps it.
#[test]
fn an_interactive_request_takes_every_idle_worker_unless_configured_otherwise() {
    let (registry, _far) = idle_registry(&[(gpu(), "a6000")]);
    let ask = live_ask();

    let all = coordinator_over(Some(&registry), true, ALL_INTERACTIVE_WORKERS);
    let planned = planned_job(&all, ask);
    assert_eq!(planned.workers, WorkerPick::Fastest(u32::MAX));
    assert!(planned.own && !planned.fifo && !planned.whole_image);
    assert_eq!(planned.pool, all.job_config().interactive);

    let none = coordinator_over(Some(&registry), true, 0);
    assert_eq!(plan::plan(&none, ask), Ok(Route::Direct));

    let three = coordinator_over(Some(&registry), true, 3);
    assert_eq!(planned_job(&three, ask).workers, WorkerPick::Fastest(3));

    // No worker at all: the own lane serves the live view directly whatever the setting.
    let alone = coordinator_over(None, true, ALL_INTERACTIVE_WORKERS);
    assert_eq!(plan::plan(&alone, ask), Ok(Route::Direct));
    // Without an own lane even `0` takes the single fastest worker.
    let render_less = coordinator_over(Some(&registry), false, 0);
    assert_eq!(
        planned_job(&render_less, ask).workers,
        WorkerPick::Fastest(1)
    );
}

/// A whole-image job takes the fastest idle joined worker; once none is idle the own lane
/// is its only lane; with neither there is nothing to take.
#[test]
fn a_whole_image_job_takes_the_fastest_idle_worker_then_the_own_lane() {
    let (registry, _far) =
        idle_registry(&[(Backend::Cpu { threads: 8 }, "cpu-box"), (gpu(), "a6000")]);
    let need = LaneNeed::pixels(160 * 160);
    let shared = Arc::new(JobLanes::new(need, None, LaneTimeouts::default()));
    let with_own = coordinator_over(Some(&registry), true, 0);
    let take = |coordinator: &Coordinator| {
        producer::whole_image_lane(coordinator, coordinator.rates(), &shared, need)
    };

    let (first, first_lane) = take(&with_own).expect("the GPU is idle");
    assert_eq!(first, label_key("a6000"), "the GPU ranks first");
    let (second, _second_lane) = take(&with_own).expect("the CPU box is idle");
    assert_eq!(second, label_key("cpu-box"));
    let (third, _own) = take(&with_own).expect("the own lane takes over");
    assert_eq!(third, LaneKey::Own, "every worker is busy");
    drop(first_lane);
    let (again, _again_lane) = take(&with_own).expect("the GPU is idle again");
    assert_eq!(again, label_key("a6000"));

    let without_own = coordinator_over(Some(&registry), false, 0);
    assert!(take(&without_own).is_none(), "nothing is idle, no own lane");
}

/// The second and later lane of one worker start out of phase; the first one, a lane
/// whose rate is unmeasured and another worker's lane do not.
#[test]
fn the_second_lane_of_one_worker_starts_half_a_chunk_out_of_phase() {
    let key = label_key("a6000");
    let other = label_key("cpu-box");
    let mut book = RateBook::default();
    book.set(key.clone(), 100, 40.0);
    let mut seeded = BTreeSet::new();

    let mut first = producer::lane_rate(&book, &key, 100, &mut seeded);
    assert_eq!(first.estimate(), Some(40.0));
    assert_eq!(
        first.take_first_chunk_scale(),
        None,
        "the first lane is in phase"
    );
    let mut second = producer::lane_rate(&book, &key, 100, &mut seeded);
    assert_eq!(second.estimate(), Some(40.0));
    assert_eq!(second.take_first_chunk_scale(), Some(0.5));
    assert_eq!(
        second.take_first_chunk_scale(),
        None,
        "only the first chunk"
    );
    let mut third = producer::lane_rate(&book, &key, 100, &mut seeded);
    assert_eq!(third.take_first_chunk_scale(), Some(0.5));

    for _ in 0..2 {
        let mut unmeasured = producer::lane_rate(&book, &other, 100, &mut seeded);
        assert!(!unmeasured.is_calibrated());
        assert_eq!(unmeasured.take_first_chunk_scale(), None);
    }
}

/// The log line's route fields: a direct request ran on the own lane, a job on the lanes
/// it checked out.
#[test]
fn the_served_line_reports_whole_image_and_lane_count() {
    let direct = served::Served::new(Route::Direct, 9);
    assert_eq!(
        (direct.direct, direct.whole_image, direct.lanes),
        (true, false, 1)
    );
    let (registry, _far) = idle_registry(&[(gpu(), "a6000")]);
    let coordinator = coordinator_over(Some(&registry), false, 0);
    let whole = planned_job(&coordinator, batch_ask(160, 160, 256));
    let whole_line = served::Served::new(Route::Job(whole), 1);
    assert_eq!(
        (whole_line.direct, whole_line.whole_image, whole_line.lanes),
        (false, true, 1)
    );
    let split = JobPlan {
        whole_image: false,
        ..whole
    };
    let split_line = served::Served::new(Route::Job(split), 3);
    assert_eq!(
        (split_line.direct, split_line.whole_image, split_line.lanes),
        (false, false, 3)
    );
}
