//! Registry and advertisement unit tests: no TLS, connections are loopback
//! `TcpStream` pairs standing in for joined workers.

use crate::coordinator::{
    Capacity, LivenessConfig, Registry, WorkerInfo, viewer_render_capability,
};
use indicatrix_net::messages::{Backend, PayloadEncoding, RenderCapability};
use std::{
    net::{TcpListener, TcpStream},
    time::Duration,
};

fn capability(backend: Backend) -> RenderCapability {
    RenderCapability {
        backend,
        max_pixels: 1_000,
        min_cadence_ms: 100,
        hdr: false,
    }
}

/// Registers one fake worker with `backend`; returns its id and the far end of its
/// connection (kept alive by the caller).
fn register(registry: &Registry, backend: Backend) -> (u32, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (far, _) = listener.accept().unwrap();
    let worker_id = registry.allocate_id();
    registry.insert(
        WorkerInfo {
            worker_id,
            capability: capability(backend),
            peer: None,
            label: None,
            payload_encoding: PayloadEncoding::Raw,
        },
        Box::new(near),
        None,
    );
    (worker_id, far)
}

#[test]
fn checkout_lends_each_connection_once_and_drop_returns_it() {
    let registry = Registry::new(LivenessConfig::default());
    let (cpu, _far_cpu) = register(&registry, Backend::Cpu { threads: 8 });
    let (gpu, _far_gpu) = register(
        &registry,
        Backend::Gpu {
            adapter: "test".to_string(),
        },
    );

    let first = Registry::checkout(&registry, |_| true).unwrap();
    assert_eq!(first.info().worker_id, cpu, "lowest id first");
    let second = Registry::checkout(&registry, |_| true).unwrap();
    assert_eq!(second.info().worker_id, gpu);
    assert!(
        Registry::checkout(&registry, |_| true).is_none(),
        "both busy"
    );
    assert_eq!(registry.capacity().workers, 2, "busy workers still count");

    drop(first);
    let again = Registry::checkout(&registry, |w| {
        matches!(w.capability.backend, Backend::Cpu { .. })
    })
    .unwrap();
    assert_eq!(again.info().worker_id, cpu);
    assert!(
        Registry::checkout(&registry, |w| matches!(
            w.capability.backend,
            Backend::Gpu { .. }
        ))
        .is_none(),
        "the GPU worker is still checked out"
    );
    drop(second);
    drop(again);
    assert!(registry.workers().iter().all(|(_, idle)| *idle));
}

#[test]
fn discard_unregisters_and_notifies_subscribers() {
    let registry = Registry::new(LivenessConfig::default());
    let changes = registry.subscribe();
    let (_cpu, _far) = register(&registry, Backend::Cpu { threads: 4 });
    let (_gpu, _far_gpu) = register(
        &registry,
        Backend::Gpu {
            adapter: "test".to_string(),
        },
    );
    assert_eq!(
        changes
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .workers,
        1
    );
    assert_eq!(
        changes.recv_timeout(Duration::from_secs(1)).unwrap(),
        Capacity {
            workers: 2,
            threads: 4,
            gpus: 1,
            max_pixels: 1_000,
            hdr_workers: 0,
        }
    );

    let handle = Registry::checkout(&registry, |w| {
        matches!(w.capability.backend, Backend::Gpu { .. })
    })
    .unwrap();
    handle.discard("test: stream broke");
    assert_eq!(
        changes.recv_timeout(Duration::from_secs(1)).unwrap(),
        Capacity {
            workers: 1,
            threads: 4,
            gpus: 0,
            max_pixels: 1_000,
            hdr_workers: 0,
        }
    );
    assert_eq!(registry.workers().len(), 1);
}

#[test]
fn viewer_advertisement_follows_o5() {
    let own_cpu = capability(Backend::Cpu { threads: 16 });
    let own_gpu = capability(Backend::Gpu {
        adapter: "rtx".to_string(),
    });
    let none = Capacity::default();
    let two = Capacity {
        workers: 2,
        threads: 12,
        gpus: 1,
        max_pixels: 2_000,
        hdr_workers: 0,
    };

    assert_eq!(viewer_render_capability(None, None, false), None);
    assert_eq!(viewer_render_capability(None, Some(none), false), None);
    assert_eq!(
        viewer_render_capability(Some(&own_cpu), Some(none), false),
        Some(own_cpu.clone())
    );
    assert_eq!(
        viewer_render_capability(Some(&own_gpu), None, false),
        Some(own_gpu.clone())
    );

    let bare = viewer_render_capability(None, Some(two), false).unwrap();
    assert_eq!(
        bare.backend,
        Backend::Coordinator {
            workers: 2,
            threads: 12,
            gpus: 1
        }
    );
    assert_eq!(bare.max_pixels, 2_000);

    let with_own = viewer_render_capability(Some(&own_cpu), Some(two), false).unwrap();
    assert_eq!(
        with_own.backend,
        Backend::Coordinator {
            workers: 2,
            threads: 28,
            gpus: 1
        }
    );
    let with_gpu = viewer_render_capability(Some(&own_gpu), Some(two), false).unwrap();
    assert_eq!(
        with_gpu.backend,
        Backend::Coordinator {
            workers: 2,
            threads: 12,
            gpus: 2
        }
    );
}
