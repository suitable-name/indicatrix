//! Tests driving a real `RenderRequest` over a real loopback `TcpStream`: correctly
//! sized reply buffers, CPU/GPU numeric parity, and the hybrid CPU+GPU split.

use super::fixtures::{final_only, read_stream_until_done, test_db, tiny_scene};
use crate::{render_core, serve::handle_connection};
use indicatrix_net::{
    handshake,
    messages::{Backend, ClientMessage, RenderRequest, StreamEvent, Welcome},
};
use std::{net::TcpListener, thread};

/// A real `RenderRequest` over a real loopback `TcpStream`, driven through
/// `handle_connection` directly (not `run`'s accept loop): request goes in, a
/// correctly-sized summed radiance buffer comes back.
#[test]
fn serve_round_trip_over_a_loopback_socket_returns_a_correctly_sized_buffer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, 2, &test_db()).unwrap();
    });

    let mut client = std::net::TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();
    assert!(matches!(
        welcome.render.as_ref().unwrap().backend,
        Backend::Cpu { .. }
    ));

    let scene = tiny_scene();
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 5,
        scene,
        first_sample: 10,
        samples: 3,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let events = read_stream_until_done(&mut client);
    let frames: Vec<_> = events
        .iter()
        .filter_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(frames.len(), 1);
    let (header, payload) = frames[0];
    assert_eq!(header.request_id, 5);
    assert_eq!(header.first_sample, 10);
    assert_eq!(header.samples, 3);
    assert_eq!(
        payload.len(),
        4 * 4 * indicatrix_net::radiance::BYTES_PER_PIXEL
    );

    drop(client); // close the connection so handle_connection's read loop sees EOF and returns
    server.join().unwrap();
}

#[test]
fn serve_round_trip_result_matches_tracing_the_same_range_directly() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let scene = tiny_scene();
    let scene_for_server = scene.clone();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, 2, &test_db()).unwrap();
    });

    let mut client = std::net::TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();

    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 6,
        scene: scene.clone(),
        first_sample: 0,
        samples: 4,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();
    let events = read_stream_until_done(&mut client);
    let (_header, payload) = events
        .iter()
        .find_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .unwrap();
    drop(client);
    server.join().unwrap();

    let over_the_wire =
        indicatrix_net::radiance::decode(payload, scene.width, scene.height).unwrap();
    let direct = render_core::trace_samples(&scene_for_server, 0, 4, 2);
    // Relative tolerance, not bit-exact: the server sub-batches internally
    // (`stream_emit::run_tracer`) and sums sub-batches, rather than tracing the whole
    // range in one call like `direct` does, and float addition isn't associative.
    for (a, b) in over_the_wire.iter().zip(&direct) {
        let diff = (*a - *b).abs();
        let scale = a.abs().max(b.abs()).max(glam::Vec3::splat(1e-6));
        assert!(
            (diff / scale).max_element() < 1e-3,
            "over_the_wire={a:?} direct={b:?}"
        );
    }
}

/// GPU counterpart to `serve_round_trip_over_a_loopback_socket_returns_a_correctly_sized_buffer`:
/// acquires a real [`indicatrix::renderer::gpu_backend::GpuBackend`] and drives
/// `handle_connection_with_gpu` over a real loopback socket. With a usable adapter this
/// proves `WELCOME` reports `Backend::Gpu` and the request still comes back correctly
/// sized. With no usable adapter, `GpuBackend::acquire` declines and this degrades to
/// re-checking CPU behavior rather than failing.
#[cfg(feature = "gpu")]
#[test]
fn serve_round_trip_over_gpu_reports_backend_gpu_and_a_correctly_sized_buffer() {
    use crate::{cli::ComputeMode, serve::handle_connection_with_gpu};
    use indicatrix::renderer::gpu_backend::GpuBackend;
    use std::{net::TcpStream, sync::Arc};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let scene = tiny_scene(); // diamond -- isotropic, so GpuBackend accepts it.

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let gpu = Arc::new(GpuBackend::acquire());
        handle_connection_with_gpu(
            stream,
            2,
            &gpu,
            &test_db(),
            ComputeMode::default(),
            &indicatrix_net::messages::LOOPBACK_SERVER_PREFERENCE,
        )
        .unwrap();
    });

    let mut client = TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();

    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 7,
        scene: scene.clone(),
        first_sample: 0,
        samples: 4,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();
    let events = read_stream_until_done(&mut client);
    let (header, payload) = events
        .iter()
        .find_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .unwrap();
    assert_eq!(header.samples, 4);
    assert_eq!(
        payload.len(),
        scene.width as usize * scene.height as usize * indicatrix_net::radiance::BYTES_PER_PIXEL
    );

    drop(client);
    server.join().unwrap();

    // Only asserted with a usable adapter -- a machine with none legitimately reports
    // Backend::Cpu, not a test failure.
    if let Some(Backend::Gpu { adapter }) = welcome.render.map(|r| r.backend) {
        assert!(!adapter.is_empty(), "adapter label must be non-empty");
    }
}

/// Exercises `stream_emit::tracer::run_tracer`'s hybrid CPU+GPU path over a real
/// loopback socket. `samples` here (32) is above `render_core::hybrid::HYBRID_MIN_SPP`
/// (8) -- unlike every other GPU test in this file -- so this actually triggers
/// `render_core::hybrid::calibrate` and a `hybrid_trace` split across both engines.
/// Proves the split doesn't change the per-pixel sum (same float-reordering tolerance
/// as `serve_round_trip_result_matches_tracing_the_same_range_directly`) and the total
/// sample count still matches what was requested. With no usable adapter, this
/// degrades to re-checking the plain CPU path, same as the GPU test above.
///
/// # Why this test's scene has no dispersion
///
/// The strict per-pixel tolerance only holds when CPU and GPU produce the same
/// per-sample radiance up to float-addition reordering. On a dispersive material they
/// occasionally don't: `refraction.rs`'s chromatic-termination decisions compare a dot
/// product against `DIRECTION_MATCH_COS_TOL = 1 - 1e-6`, so a cosine within a few ULPs
/// of that threshold can flip between the two engines' independently rounded paths.
/// Both outcomes are valid, unbiased estimators, but the flipped sample renormalises
/// over a different MIS family, moving pixel sums by up to several percent run to run
/// -- a property of the tolerance, not of the split-and-glue logic this test exists to
/// prove. Hence [`super::fixtures::tiny_scene_without_dispersion`], where no decision
/// can sit on the threshold. Dispersive CPU/GPU parity is covered statistically instead,
/// by the GPU equivalence harness's Tier 3 image comparisons
/// (`examples/gpu_equivalence_harness.rs`).
#[cfg(feature = "gpu")]
#[test]
fn hybrid_gpu_and_cpu_split_sums_to_the_same_result_as_tracing_the_range_directly() {
    use super::fixtures::tiny_scene_without_dispersion;
    use crate::{cli::ComputeMode, serve::handle_connection_with_gpu};
    use indicatrix::renderer::gpu_backend::GpuBackend;
    use std::{net::TcpStream, sync::Arc};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    // Diamond's index without its dispersion -- isotropic, so GpuBackend accepts it,
    // and knife-edge-free, see this test's own doc comment.
    let scene = tiny_scene_without_dispersion();
    let scene_for_direct = scene.clone();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let gpu = Arc::new(GpuBackend::acquire());
        // `Hybrid` explicitly: exercising `run_tracer`'s calibrate/hybrid_trace path
        // only runs for this mode.
        handle_connection_with_gpu(
            stream,
            2,
            &gpu,
            &test_db(),
            ComputeMode::Hybrid,
            &indicatrix_net::messages::LOOPBACK_SERVER_PREFERENCE,
        )
        .unwrap();
    });

    let mut client = TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();

    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 8,
        scene: scene.clone(),
        first_sample: 0,
        samples: 32,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();
    let events = read_stream_until_done(&mut client);
    let (header, payload) = events
        .iter()
        .find_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .unwrap();
    assert_eq!(header.samples, 32);

    drop(client);
    server.join().unwrap();

    let over_the_wire =
        indicatrix_net::radiance::decode(payload, scene.width, scene.height).unwrap();
    let direct = render_core::trace_samples(&scene_for_direct, 0, 32, 2);

    // Same tolerance as `serve_round_trip_result_matches_tracing_the_same_range_directly`;
    // holds here because the scene is non-dispersive (see this test's doc comment).
    for (a, b) in over_the_wire.iter().zip(&direct) {
        let diff = (*a - *b).abs();
        let scale = a.abs().max(b.abs()).max(glam::Vec3::splat(1e-6));
        let rel = (diff / scale).max_element();
        assert!(rel < 1e-3, "over_the_wire={a:?} direct={b:?} rel={rel}");
    }
}
