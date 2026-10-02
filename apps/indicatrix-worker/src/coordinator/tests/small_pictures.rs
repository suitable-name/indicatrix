//! Small pictures end to end over real TLS on loopback: a small `Batch` request is one
//! picture on one joined worker, several of one viewer certificate's run at once, and the
//! live view takes the joined workers by default.

use super::{
    fixtures::{bundle, coordinator_args, pki_with_server, start, tls_client, wait_for},
    support::{close, collect, render, scene, send, spawn_dying_worker, spawn_worker, viewer},
};
use crate::{
    cli::{ALL_INTERACTIVE_WORKERS, ComputeMode},
    coordinator::{JobConfig, LivenessConfig},
    render_core::trace_samples,
};
use indicatrix_dispatch::PoolConfig;
use indicatrix_net::{
    client::handshake_with_hello,
    handshake,
    messages::{
        Backend, ClientMessage, PeerRole, RenderCapability, RequestIntent, StreamEvent,
        TransferMode, error_codes,
    },
};
use std::{
    net::{Shutdown, SocketAddr},
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(20);

/// How long a gated fake worker waits for the others before giving up (a failed test, not
/// a hung one).
const GATE_TIMEOUT: Duration = Duration::from_secs(10);

fn cpu_capability() -> RenderCapability {
    RenderCapability {
        backend: Backend::Cpu { threads: 2 },
        max_pixels: 1 << 20,
        min_cadence_ms: 100,
        hdr: false,
    }
}

/// The `first_sample` of every `RenderRequest` the gated fake workers received.
type Seen = Arc<Mutex<Vec<u32>>>;

/// A fake worker that answers `PING`s; on its first `RenderRequest` it records the
/// request's `first_sample` in `seen`, counts itself in `arrivals`, waits (up to
/// [`GATE_TIMEOUT`]) until `gate` workers have counted themselves -- so it only goes on
/// when that many requests are being served at the same time -- and closes its connection.
fn spawn_gated_worker(
    worker_addr: SocketAddr,
    bundle_dir: &Path,
    seen: &Seen,
    arrivals: &Arc<AtomicU32>,
    gate: u32,
) {
    let mut tls = tls_client(worker_addr, bundle_dir);
    let (seen, arrivals) = (Arc::clone(seen), Arc::clone(arrivals));
    thread::spawn(move || {
        if handshake_with_hello(&mut tls, &handshake::local_worker_hello(cpu_capability())).is_err()
        {
            return;
        }
        let _ = tls.sock.set_read_timeout(Some(Duration::from_secs(300)));
        loop {
            match indicatrix_net::messages::read_message::<_, ClientMessage>(&mut tls) {
                Ok(ClientMessage::Ping { nonce }) => {
                    let _ = indicatrix_net::messages::write_stream_event(
                        &mut tls,
                        &StreamEvent::Pong { nonce },
                        None,
                    );
                }
                Ok(ClientMessage::RenderRequest(request)) => {
                    seen.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(request.first_sample);
                    arrivals.fetch_add(1, Ordering::SeqCst);
                    let deadline = Instant::now() + GATE_TIMEOUT;
                    while arrivals.load(Ordering::SeqCst) < gate && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(10));
                    }
                    let _ = tls.sock.shutdown(Shutdown::Both);
                    return;
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
}

/// Two requests of ONE viewer certificate, on two connections, are each a whole picture on
/// their own joined worker, and they run at the same time: both workers hold their
/// request until the other one has received its, which a FIFO of one active job per
/// viewer (or a split of one picture over both workers) could never satisfy. Each worker
/// sees the FIRST sample of a different request: 0 and 1000.
#[test]
fn two_small_requests_of_one_viewer_run_at_once_each_whole_on_its_own_worker() {
    let pki = pki_with_server("small-concurrent");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    // Whole-image routing as shipped; only the failure backoff is short, so the jobs end
    // quickly once the fake workers hang up.
    handle
        .coordinator
        .as_ref()
        .unwrap()
        .set_job_config(JobConfig {
            batch: PoolConfig {
                backoff_initial: Duration::from_millis(10),
                backoff_max: Duration::from_millis(40),
                ..JobConfig::default().batch
            },
            ..JobConfig::default()
        });
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let seen = Seen::default();
    let arrivals = Arc::new(AtomicU32::new(0));
    for _ in 0..2 {
        spawn_gated_worker(
            handle.worker_addr.unwrap(),
            &worker_bundle,
            &seen,
            &arrivals,
            2,
        );
    }
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));

    let image = scene(8, 6);
    let viewers: Vec<_> = [(1, 0), (2, 1000)]
        .into_iter()
        .map(|(id, first_sample)| {
            let (addr, bundle_dir, image) =
                (handle.viewer_addr, viewer_bundle.clone(), image.clone());
            thread::spawn(move || {
                let (_, mut client) = viewer(addr, &bundle_dir);
                let request = render(
                    id,
                    image,
                    (first_sample, 64),
                    TransferMode::FinalOnly,
                    RequestIntent::Batch,
                );
                send(&mut client, &request);
                collect(&mut client, (8, 6), |_, _| {})
            })
        })
        .collect();
    for viewer in viewers {
        // Both ended: the fake workers hung up on their first chunk, so the jobs fail --
        // what matters here is what the workers saw.
        let transcript = viewer.join().unwrap();
        assert!(transcript.done.is_none());
        let error = transcript.error.expect("the job ends with an error");
        assert_eq!(
            error.code,
            error_codes::ALL_WORKERS_LOST,
            "{}",
            error.message
        );
    }
    let mut firsts = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
    firsts.sort_unstable();
    assert_eq!(
        firsts,
        [0, 1000],
        "each worker got the start of a different request"
    );
}

/// A small `Batch` picture on a coordinator with two real workers comes out right:
/// exactly the requested samples, one `DONE`, and the image of a single-machine render of
/// the same range.
#[test]
fn a_small_batch_picture_matches_a_local_render() {
    let pki = pki_with_server("small-whole");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    handle
        .coordinator
        .as_ref()
        .unwrap()
        .set_job_config(JobConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();
    spawn_worker(worker_addr, &worker_bundle);
    spawn_worker(worker_addr, &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(8, 6);
    for id in 1..=3 {
        let request = render(
            id,
            image.clone(),
            (5, 40),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        );
        send(&mut client, &request);
        let transcript = collect(&mut client, (8, 6), |_, _| {});
        let done = transcript.done.expect("DONE");
        assert!(!done.cancelled && done.request_id == id);
        assert_eq!(done.stats.samples_done, 40);
        assert_eq!(transcript.frame_samples, 40);
        assert!(close(&transcript.sum, &trace_samples(&image, 5, 40, 2)));
    }
    assert!(wait_for(WAIT, || registry
        .workers()
        .iter()
        .all(|(_, idle)| *idle)));
}

/// By default a live-view request takes every idle joined worker besides the own lane
/// (`--interactive-workers all`): a worker that dies on its chunk proves it was handed
/// one, and the own lane still completes the request.
#[test]
fn an_interactive_request_uses_the_joined_workers_by_default() {
    let pki = pki_with_server("small-interactive");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let mut args = coordinator_args(&pki);
    args.render = true;
    args.compute_mode = ComputeMode::OnlyCpu;
    args.interactive_workers = ALL_INTERACTIVE_WORKERS;
    let handle = start(&args, LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let renders = Arc::new(AtomicU32::new(0));
    spawn_dying_worker(
        handle.worker_addr.unwrap(),
        &worker_bundle,
        cpu_capability(),
        &renders,
    );
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    send(
        &mut client,
        &render(
            8,
            scene(8, 6),
            (0, 600),
            TransferMode::LiveProgressive,
            RequestIntent::Interactive,
        ),
    );
    let transcript = collect(&mut client, (8, 6), |_, _| {});
    let done = transcript.done.expect("the own lane completes the request");
    assert_eq!(done.stats.samples_done, 600);
    assert_eq!(transcript.frame_samples, 600);
    assert!(
        wait_for(WAIT, || renders.load(Ordering::SeqCst) == 1),
        "the joined worker was handed a chunk of the live view"
    );
}
