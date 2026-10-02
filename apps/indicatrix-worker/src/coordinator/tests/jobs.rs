//! Job execution end to end over real TLS on loopback, CPU only: a coordinator, real
//! `join` workers (or fake ones that die), and a test viewer.

use super::{
    fixtures::{bundle, coordinator_args, fan_out_config, pki_with_server, start, wait_for},
    support::{close, collect, render, scene, send, spawn_dying_worker, spawn_worker, viewer},
};
use crate::{
    coordinator::{JobConfig, LivenessConfig},
    render_core::trace_samples,
};
use indicatrix_dispatch::PoolConfig;
use indicatrix_net::messages::{
    Backend, Cancel, ClientMessage, PeerRole, RenderCapability, RequestIntent, StreamEvent,
    TransferMode, error_codes,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(20);

fn cpu_capability(max_pixels: u32) -> RenderCapability {
    RenderCapability {
        backend: Backend::Cpu { threads: 2 },
        max_pixels,
        min_cadence_ms: 100,
        hdr: false,
    }
}

/// A coordinator without `--render` and two joined CPU workers render a `Batch` request:
/// exactly the requested samples arrive, one DONE, and the image equals a single-machine
/// render of the same range (to 1e-5 relative -- only the float summation order differs).
#[test]
fn two_joined_workers_render_a_batch_request_like_a_single_worker() {
    let pki = pki_with_server("jobs-two");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();
    spawn_worker(worker_addr, &worker_bundle);
    spawn_worker(worker_addr, &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(8, 6);
    for (id, mode) in [
        (1, TransferMode::FinalOnly),
        (2, TransferMode::LiveProgressive),
    ] {
        send(
            &mut client,
            &render(id, image.clone(), (3, 40), mode, RequestIntent::Batch),
        );
        let transcript = collect(&mut client, (8, 6), |_, _| {});
        let done = transcript.done.expect("DONE");
        assert!(!done.cancelled && done.request_id == id);
        assert_eq!(done.stats.samples_done, 40);
        assert_eq!(transcript.frame_samples, 40, "{mode:?}: exact sample count");
        let reference = trace_samples(&image, 3, 40, 2);
        assert!(
            close(&transcript.sum, &reference),
            "{mode:?}: merged image differs"
        );
    }
    assert!(wait_for(WAIT, || registry
        .workers()
        .iter()
        .all(|(_, idle)| *idle)));
}

/// A worker that dies on its chunk is dropped and its samples go to the other lane:
/// the job still completes with every sample.
#[test]
fn a_worker_dying_mid_job_does_not_stop_the_job() {
    let pki = pki_with_server("jobs-dying");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();
    let renders = Arc::new(AtomicU32::new(0));
    spawn_dying_worker(
        worker_addr,
        &worker_bundle,
        cpu_capability(1 << 20),
        &renders,
    );
    spawn_worker(worker_addr, &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(8, 6);
    send(
        &mut client,
        &render(
            4,
            image.clone(),
            (0, 40),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        ),
    );
    let transcript = collect(&mut client, (8, 6), |_, _| {});
    let done = transcript
        .done
        .expect("the job must complete on the surviving worker");
    assert_eq!(done.stats.samples_done, 40);
    assert_eq!(transcript.frame_samples, 40);
    assert!(close(&transcript.sum, &trace_samples(&image, 0, 40, 2)));
    assert_eq!(
        renders.load(Ordering::SeqCst),
        1,
        "the dying worker got (and lost) a chunk"
    );
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
}

/// Every worker lost: the stream ends with `ERROR(ALL_WORKERS_LOST)` and no `DONE` (the
/// next event on the connection is the answer to a fresh `PING`).
#[test]
fn losing_every_worker_ends_the_stream_with_all_workers_lost() {
    let pki = pki_with_server("jobs-lost");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let fast_failures = PoolConfig {
        backoff_initial: Duration::from_millis(10),
        backoff_max: Duration::from_millis(40),
        ..JobConfig::default().batch
    };
    handle
        .coordinator
        .as_ref()
        .unwrap()
        .set_job_config(JobConfig {
            batch: fast_failures,
            ..fan_out_config()
        });
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();
    let renders = Arc::new(AtomicU32::new(0));
    for _ in 0..2 {
        spawn_dying_worker(
            worker_addr,
            &worker_bundle,
            cpu_capability(1 << 20),
            &renders,
        );
    }
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    send(
        &mut client,
        &render(
            5,
            scene(8, 6),
            (0, 40),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        ),
    );
    let transcript = collect(&mut client, (8, 6), |_, _| {});
    let error = transcript.error.expect("an ERROR, not a DONE");
    assert_eq!(
        error.code,
        error_codes::ALL_WORKERS_LOST,
        "{}",
        error.message
    );
    assert_eq!(renders.load(Ordering::SeqCst), 2);
    send(&mut client, &ClientMessage::Ping { nonce: 77 });
    loop {
        match indicatrix_net::messages::read_stream_event(&mut client)
            .unwrap()
            .0
        {
            StreamEvent::Pong { nonce: 77 } => break,
            // Losing both workers also changes what the coordinator can offer.
            StreamEvent::CapabilityChanged { render } => assert_eq!(render, None),
            other => panic!("nothing but the PONG may follow the ERROR, got {other:?}"),
        }
    }
}

/// A viewer `CANCEL` mid-job: `DONE { cancelled: true }`, both workers are checked back
/// in (idle), and they serve the next request.
#[test]
fn a_viewer_cancel_mid_job_answers_done_cancelled_and_returns_the_workers() {
    let pki = pki_with_server("jobs-cancel");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();
    spawn_worker(worker_addr, &worker_bundle);
    spawn_worker(worker_addr, &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let big = scene(32, 32);
    send(
        &mut client,
        &render(
            6,
            big,
            (0, 60_000),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        ),
    );
    let mut cancel_sent = false;
    let transcript = collect(&mut client, (32, 32), |client, samples_done| {
        if !cancel_sent && samples_done > 0 {
            send(client, &ClientMessage::Cancel(Cancel { request_id: 6 }));
            cancel_sent = true;
        }
    });
    assert!(cancel_sent, "the job made progress before the cancel");
    let done = transcript.done.expect("DONE after CANCEL");
    assert!(done.cancelled && done.request_id == 6);
    assert_eq!(transcript.frame_samples, 0, "no FRAME after a cancel");
    assert!(wait_for(WAIT, || registry.capacity().workers == 2
        && registry.workers().iter().all(|(_, idle)| *idle)));

    let small = scene(4, 4);
    send(
        &mut client,
        &render(
            7,
            small.clone(),
            (0, 6),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        ),
    );
    let transcript = collect(&mut client, (4, 4), |_, _| {});
    assert_eq!(transcript.done.expect("DONE").stats.samples_done, 6);
    assert!(close(&transcript.sum, &trace_samples(&small, 0, 6, 2)));
}

/// `Interactive` on a `--render` coordinator started with `--interactive-workers 0` runs
/// on its own lane only: a joined worker never sees the request.
#[test]
fn an_interactive_request_with_render_uses_only_the_own_lane() {
    let pki = pki_with_server("jobs-interactive");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let mut args = coordinator_args(&pki);
    args.render = true;
    args.compute_mode = crate::cli::ComputeMode::OnlyCpu;
    let handle = start(&args, LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let renders = Arc::new(AtomicU32::new(0));
    spawn_dying_worker(
        handle.worker_addr.unwrap(),
        &worker_bundle,
        cpu_capability(1 << 20),
        &renders,
    );
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (welcome, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    assert!(matches!(
        welcome.render.unwrap().backend,
        Backend::Coordinator { workers: 1, .. }
    ));
    let image = scene(8, 6);
    let mode = TransferMode::LiveProgressive;
    send(
        &mut client,
        &render(8, image.clone(), (0, 12), mode, RequestIntent::Interactive),
    );
    let transcript = collect(&mut client, (8, 6), |_, _| {});
    assert_eq!(transcript.done.expect("DONE").stats.samples_done, 12);
    assert_eq!(transcript.frame_samples, 12);
    assert!(close(&transcript.sum, &trace_samples(&image, 0, 12, 2)));
    assert_eq!(renders.load(Ordering::SeqCst), 0, "no worker took part");
    assert_eq!(registry.capacity().workers, 1);
}

/// A request larger than every joined worker's `max_pixels` (and no own lane) is refused
/// with a clear `UNSUPPORTED_REQUEST`; a job past the memory cap with
/// `CONNECTION_LIMIT_REACHED`.
#[test]
fn requests_no_lane_can_take_are_refused_clearly() {
    let pki = pki_with_server("jobs-refused");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let mut args = coordinator_args(&pki);
    args.max_job_memory_mib = 1;
    let handle = start(&args, LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let renders = Arc::new(AtomicU32::new(0));
    let cap = 150 * 150;
    spawn_dying_worker(
        handle.worker_addr.unwrap(),
        &worker_bundle,
        cpu_capability(cap),
        &renders,
    );
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    send(
        &mut client,
        &render(
            9,
            scene(160, 160),
            (0, 4),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        ),
    );
    let error = collect(&mut client, (160, 160), |_, _| {})
        .error
        .expect("a refusal");
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    assert!(error.message.contains("max_pixels"), "{}", error.message);

    // 150 x 150 fits the worker, but a job is charged 150 * 150 * 48 bytes (~1.03 MiB),
    // more than the whole 1 MiB budget.
    assert!(crate::coordinator::job_bytes(150, 150) > 1024 * 1024);
    send(
        &mut client,
        &render(
            10,
            scene(150, 150),
            (0, 4),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        ),
    );
    let error = collect(&mut client, (150, 150), |_, _| {})
        .error
        .expect("a refusal");
    assert_eq!(
        error.code,
        error_codes::CONNECTION_LIMIT_REACHED,
        "{}",
        error.message
    );
    assert!(
        error.message.contains("--max-job-memory-mib"),
        "{}",
        error.message
    );
    assert_eq!(renders.load(Ordering::SeqCst), 0);
}

/// Interactive-worker pin end to end: a render-less coordinator started with
/// `--pin-interactive-worker slowbox` hands a live-view request to the pinned worker
/// first, although the fastest-first rule would pick the 2-thread `box` over its 1
/// thread (the fake pinned worker counts the request, then dies on it). With the pinned
/// worker gone, the next live-view request falls back to `box` and completes.
#[test]
fn a_pinned_worker_takes_the_live_view_first_and_falls_back_when_gone() {
    let pki = pki_with_server("jobs-pin");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let fast_bundle = bundle(&pki, "box", PeerRole::Worker);
    let pinned_bundle = bundle(&pki, "slowbox", PeerRole::Worker);
    let mut args = coordinator_args(&pki);
    args.pin_interactive_worker = Some("slowbox".to_string());
    let handle = start(&args, LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();
    let renders = Arc::new(AtomicU32::new(0));
    let one_thread = RenderCapability {
        backend: Backend::Cpu { threads: 1 },
        ..cpu_capability(1 << 20)
    };
    spawn_dying_worker(worker_addr, &pinned_bundle, one_thread, &renders);
    spawn_worker(worker_addr, &fast_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(8, 6);
    let live = |id| {
        render(
            id,
            image.clone(),
            (0, 4),
            TransferMode::LiveProgressive,
            RequestIntent::Interactive,
        )
    };
    send(&mut client, &live(5));
    let _ = collect(&mut client, (8, 6), |_, _| {});
    assert_eq!(
        renders.load(Ordering::SeqCst),
        1,
        "the pinned worker got the live-view request first"
    );

    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    send(&mut client, &live(6));
    let transcript = collect(&mut client, (8, 6), |_, _| {});
    let done = transcript.done.expect("the fallback worker completes it");
    assert_eq!((done.request_id, done.stats.samples_done), (6, 4));
    assert_eq!(renders.load(Ordering::SeqCst), 1);
}
