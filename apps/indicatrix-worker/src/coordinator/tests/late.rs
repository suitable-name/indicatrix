//! Workers that register while a job is running join it, and one that registers after
//! the job is over gets nothing from it. Real TLS on loopback, CPU only; the late workers
//! are fakes that die on their first chunk, so that a chunk received is counted.

use super::{
    fixtures::{bundle, coordinator_args, fan_out_config, pki_with_server, start, wait_for},
    support::{collect, render, scene, send, spawn_dying_worker, spawn_worker, viewer},
};
use crate::coordinator::{JobConfig, LivenessConfig};
use indicatrix_dispatch::{ChunkPolicy, PoolConfig};
use indicatrix_net::messages::{
    Backend, Cancel, ClientMessage, PeerRole, RenderCapability, RequestIntent, TransferMode,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(20);

fn cpu_capability() -> RenderCapability {
    RenderCapability {
        backend: Backend::Cpu { threads: 2 },
        max_pixels: 1 << 20,
        min_cadence_ms: 100,
        hdr: false,
    }
}

/// Short fixed chunks, so a running job keeps offering work to a lane that joins it.
fn short_chunks() -> JobConfig {
    JobConfig {
        batch: PoolConfig {
            policy: ChunkPolicy::fixed(40),
            ..JobConfig::default().batch
        },
        ..fan_out_config()
    }
}

/// A worker registering after the job started is handed one of its chunks within a few
/// ticks; its connection dying on that chunk costs the job nothing (the lane is removed,
/// the range goes back to the real worker). A second fake registering after the first one
/// died -- a worker that rejoined under a new id -- is handed a chunk of the same job.
#[test]
fn workers_registering_mid_job_receive_chunks_of_the_running_job() {
    let pki = pki_with_server("late-join");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    handle
        .coordinator
        .as_ref()
        .unwrap()
        .set_job_config(short_chunks());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();
    spawn_worker(worker_addr, &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    send(
        &mut client,
        &render(
            11,
            scene(32, 32),
            (0, 60_000),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        ),
    );
    let first = Arc::new(AtomicU32::new(0));
    let second = Arc::new(AtomicU32::new(0));
    let (mut spawned_first, mut spawned_second, mut cancelled) = (false, false, false);
    let transcript = collect(&mut client, (32, 32), |client, _| {
        if !spawned_first {
            spawned_first = true;
            spawn_dying_worker(worker_addr, &worker_bundle, cpu_capability(), &first);
        } else if first.load(Ordering::SeqCst) >= 1 && !spawned_second {
            spawned_second = true;
            spawn_dying_worker(worker_addr, &worker_bundle, cpu_capability(), &second);
        } else if second.load(Ordering::SeqCst) >= 1 && !cancelled {
            cancelled = true;
            send(client, &ClientMessage::Cancel(Cancel { request_id: 11 }));
        }
    });
    let done = transcript.done.expect("DONE after CANCEL");
    assert!(done.cancelled && done.request_id == 11);
    assert_eq!(first.load(Ordering::SeqCst), 1, "the first late worker");
    assert_eq!(second.load(Ordering::SeqCst), 1, "the rejoined worker");
    assert!(
        wait_for(WAIT, || registry.capacity().workers == 1),
        "both dead connections were dropped from the registry"
    );
}

/// Once a job has finished it takes no more workers: one registering afterwards is never
/// handed a chunk of it (and stays idle in the registry).
#[test]
fn a_worker_registering_after_the_job_finished_gets_nothing_from_it() {
    let pki = pki_with_server("late-after");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    handle
        .coordinator
        .as_ref()
        .unwrap()
        .set_job_config(short_chunks());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();
    spawn_worker(worker_addr, &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    send(
        &mut client,
        &render(
            12,
            scene(4, 4),
            (0, 80),
            TransferMode::FinalOnly,
            RequestIntent::Batch,
        ),
    );
    let transcript = collect(&mut client, (4, 4), |_, _| {});
    assert_eq!(transcript.done.expect("DONE").stats.samples_done, 80);

    let renders = Arc::new(AtomicU32::new(0));
    spawn_dying_worker(worker_addr, &worker_bundle, cpu_capability(), &renders);
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(renders.load(Ordering::SeqCst), 0);
    assert_eq!(registry.capacity().workers, 2, "it was never checked out");
}
