use crate::serve::{
    connection::{BUILD_MISMATCH_CODE, NO_RENDER_CAPACITY_CODE, VALIDATION_FAILED_CODE},
    handle_connection,
};
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_net::{
    SceneState, handshake,
    messages::{
        Backend, Cancel, ClientMessage, Done, ErrorMsg, Hello, RenderRequest, StreamEvent, Welcome,
    },
};
use indicatrix_vault::db::sqlite::Database;
use std::{
    io::{Cursor, Read, Write},
    net::TcpListener,
    thread,
};

use crate::{render_core, stream_emit::TimeoutRead};

/// A fresh, empty, throwaway temp database, just to satisfy `handle_connection`'s
/// signature -- none of these tests exercise the library protocol itself. Tests never
/// touch `facet_diagrams.sqlite`, only their own throwaway temp files.
fn test_db() -> Database {
    let path = std::env::temp_dir().join(format!(
        "indicatrix-worker-serve-test-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    Database::new(Some(path.to_str().unwrap())).unwrap()
}

fn tiny_scene() -> SceneState {
    SceneState {
        width: 4,
        height: 4,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 4,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
    }
}

/// [`tiny_scene`] with dispersion removed: a Cauchy model with `b = c = 0` at
/// diamond's `n_d` (2.417), so every spectral channel refracts along the same
/// direction and no chromatic-termination decision can sit on its
/// `DIRECTION_MATCH_COS_TOL` knife edge -- see
/// `hybrid_gpu_and_cpu_split_sums_to_the_same_result_as_tracing_the_range_directly`.
#[cfg(feature = "gpu")]
fn tiny_scene_without_dispersion() -> SceneState {
    let mut scene = tiny_scene();
    scene.material.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
        a: 2.417,
        b: 0.0,
        c: 0.0,
    };
    scene
}

/// A `Read + Write` over two independent in-memory buffers, standing in for one end
/// of a duplex connection: writes go to `out`, reads come from `in_`. Lets
/// `handle_connection` be driven with hand-assembled request bytes, no networking.
///
/// `TimeoutRead`-aware, to exercise `stream_emit::poll_for_client_message`'s polling
/// loop: with no timeout set (the default), exhausting `in_` reports `Ok(0)` (EOF).
/// With a timeout set (as `run_stream`'s emitter does while streaming), exhausting
/// `in_` reports `WouldBlock` instead -- "nothing new yet", letting a test simulate
/// "connection still open, no `CANCEL` sent" by simply not writing one.
struct DuplexHalf {
    in_: Cursor<Vec<u8>>,
    out: Vec<u8>,
    timeout_active: bool,
}

impl DuplexHalf {
    const fn new(input: Vec<u8>) -> Self {
        Self {
            in_: Cursor::new(input),
            out: Vec::new(),
            timeout_active: false,
        }
    }
}

impl Read for DuplexHalf {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.in_.read(buf)?;
        if n == 0 && self.timeout_active {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "DuplexHalf: no more scripted input (yet)",
            ));
        }
        Ok(n)
    }
}

impl Write for DuplexHalf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutRead for DuplexHalf {
    fn set_read_timeout(&mut self, duration: Option<std::time::Duration>) -> std::io::Result<()> {
        self.timeout_active = duration.is_some();
        Ok(())
    }
}

/// A no-op: `DuplexHalf::write` goes to an unbounded `Vec` and can never actually
/// block. See [`BackpressureDuplex`] below for a double that genuinely models a peer
/// who stops draining.
impl crate::stream_emit::TimeoutWrite for DuplexHalf {
    fn set_write_timeout(&mut self, _duration: Option<std::time::Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

/// A `Read + Write` double modeling a peer that stops draining: `write()` succeeds
/// while `out` stays within `capacity`, then returns [`std::io::ErrorKind::TimedOut`]
/// as if a write-timeout deadline had already elapsed, so a test doesn't need a real
/// multi-second sleep. Only enforced while a write timeout is set
/// (`write_timeout_active`): the `WELCOME` handshake write always succeeds regardless
/// of `capacity`, like a real unbounded-by-default socket.
///
/// [`DuplexHalf`]'s unbounded-`Vec` `Write` side can never model this backpressure, so
/// it can't reproduce the real bug (the emitter blocking inside an unbounded
/// `write()`) that `repro_slow_reader_blocks_the_emitter_and_delays_cancel` shows
/// against real sockets. `BackpressureDuplex` closes that gap deterministically.
struct BackpressureDuplex {
    in_: Cursor<Vec<u8>>,
    out: Vec<u8>,
    capacity: usize,
    read_timeout_active: bool,
    write_timeout_active: bool,
    /// The `io::ErrorKind` a write past `capacity` reports once `write_timeout_active`.
    /// Defaults to `TimedOut` (a raw socket's kind) via [`Self::new`]; see
    /// [`Self::new_with_kind`] to simulate rustls' `WriteZero` instead (see
    /// `crate::stream_emit::is_stream_timeout`).
    timeout_kind: std::io::ErrorKind,
}

impl BackpressureDuplex {
    const fn new(input: Vec<u8>, capacity: usize) -> Self {
        Self::new_with_kind(input, capacity, std::io::ErrorKind::TimedOut)
    }

    /// Like [`Self::new`], but a write past `capacity` reports `kind` instead of always
    /// `TimedOut` -- lets a test simulate rustls' `WriteZero`-on-timeout behavior too.
    const fn new_with_kind(input: Vec<u8>, capacity: usize, kind: std::io::ErrorKind) -> Self {
        Self {
            in_: Cursor::new(input),
            out: Vec::new(),
            capacity,
            read_timeout_active: false,
            write_timeout_active: false,
            timeout_kind: kind,
        }
    }
}

impl Read for BackpressureDuplex {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.in_.read(buf)?;
        if n == 0 && self.read_timeout_active {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "BackpressureDuplex: no more scripted input (yet)",
            ));
        }
        Ok(n)
    }
}

impl Write for BackpressureDuplex {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.write_timeout_active && self.out.len() + buf.len() > self.capacity {
            return Err(std::io::Error::new(
                self.timeout_kind,
                "BackpressureDuplex: peer isn't draining -- write timed out",
            ));
        }
        self.out.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutRead for BackpressureDuplex {
    fn set_read_timeout(&mut self, duration: Option<std::time::Duration>) -> std::io::Result<()> {
        self.read_timeout_active = duration.is_some();
        Ok(())
    }
}

impl crate::stream_emit::TimeoutWrite for BackpressureDuplex {
    fn set_write_timeout(&mut self, duration: Option<std::time::Duration>) -> std::io::Result<()> {
        self.write_timeout_active = duration.is_some();
        Ok(())
    }
}

const fn live_progressive(cadence_ms: u32) -> indicatrix_net::messages::StreamConfig {
    indicatrix_net::messages::StreamConfig {
        transfer_mode: indicatrix_net::messages::TransferMode::LiveProgressive,
        cadence_ms,
        preview: None,
    }
}

const fn final_only(cadence_ms: u32) -> indicatrix_net::messages::StreamConfig {
    indicatrix_net::messages::StreamConfig {
        transfer_mode: indicatrix_net::messages::TransferMode::FinalOnly,
        cadence_ms,
        preview: None,
    }
}

/// Reads [`indicatrix_net::messages::StreamEvent`]s from `reader` (pairing each with
/// its raw payload, for `Frame`/`Preview`) until -- and including -- a `Done` or
/// `Error`, the two terminal variants for one `RENDER` reply.
fn read_stream_until_done<R: Read>(
    reader: &mut R,
) -> Vec<(indicatrix_net::messages::StreamEvent, Option<Vec<u8>>)> {
    let mut events = Vec::new();
    loop {
        let (event, payload) = indicatrix_net::messages::read_stream_event(reader).unwrap();
        let terminal = matches!(
            event,
            indicatrix_net::messages::StreamEvent::Done(_)
                | indicatrix_net::messages::StreamEvent::Error(_)
        );
        events.push((event, payload));
        if terminal {
            break;
        }
    }
    events
}

#[test]
fn handle_connection_refuses_a_mismatched_build_hash() {
    let mut input = Vec::new();
    let bad_hello = Hello {
        protocol_version: indicatrix_net::messages::PROTOCOL_VERSION,
        build_hash: [0xAB; 8],
        source_hash: handshake::UNKNOWN_BUILD_HASH,
    };
    indicatrix_net::messages::write_message(&mut input, &bad_hello).unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let err: ErrorMsg = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    assert_eq!(err.code, BUILD_MISMATCH_CODE);
    assert!(err.message.contains("refusing to pair"), "{}", err.message);
}

#[test]
fn handle_connection_accepts_a_matching_build_hash_and_sends_welcome() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    // No RenderRequest follows -- the reader hits EOF, which handle_connection
    // treats as the peer having closed the connection after the handshake.

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    assert_eq!(welcome.build_hash, handshake::local_hello().build_hash);
    assert!(matches!(
        welcome.render.as_ref().unwrap().backend,
        Backend::Cpu { .. }
    ));
}

/// A `HELLO` declaring no `indicatrix` build at all
/// (`build_hash == UNKNOWN_BUILD_HASH`, e.g. a library-only client) must not be refused
/// by `verify_compatible`'s "an unknown build is never compatible with anything" rule --
/// it is paired as library-only instead: `WELCOME::render` is `None`, and a later
/// `RenderRequest` on that same connection is refused with `NO_RENDER_CAPACITY_CODE`
/// rather than reaching the tracer.
#[test]
fn handle_connection_pairs_an_unknown_build_hash_as_library_only_and_refuses_a_later_render() {
    let mut input = Vec::new();
    let library_only_hello = Hello {
        protocol_version: indicatrix_net::messages::PROTOCOL_VERSION,
        build_hash: handshake::UNKNOWN_BUILD_HASH,
        source_hash: handshake::UNKNOWN_BUILD_HASH,
    };
    indicatrix_net::messages::write_message(&mut input, &library_only_hello).unwrap();
    let request = RenderRequest {
        request_id: 1,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 2,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    assert!(
        welcome.render.is_none(),
        "an unknown-build-hash peer must be welcomed with no render capacity"
    );
    assert!(welcome.library, "the library protocol stays available");
    assert!(!welcome.tilt_curves);

    let events = read_stream_until_done(&mut out_cursor);
    assert_eq!(
        events.len(),
        1,
        "a RenderRequest on a library-only-paired connection gets exactly one reply"
    );
    let StreamEvent::Error(err) = &events[0].0 else {
        panic!("expected StreamEvent::Error, got {:?}", events[0].0);
    };
    assert_eq!(err.code, NO_RENDER_CAPACITY_CODE);
}

#[test]
fn handle_connection_rejects_a_request_that_fails_validation_but_keeps_the_connection_open() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();

    let mut bad_scene = tiny_scene();
    bad_scene.planes[0].normal = [f32::NAN, 0.0, 0.0];
    let bad_request = RenderRequest {
        request_id: 1,
        scene: bad_scene,
        first_sample: 0,
        samples: 4,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(bad_request)),
    )
    .unwrap();

    // A second, well-formed request right after the bad one -- proves the
    // connection is still alive and serving requests after a validation failure.
    let good_request = RenderRequest {
        request_id: 2,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 2,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(good_request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();

    // The bad request's reply: a single StreamEvent::Error, nothing else.
    let bad_events = read_stream_until_done(&mut out_cursor);
    assert_eq!(bad_events.len(), 1);
    let StreamEvent::Error(err) = &bad_events[0].0 else {
        panic!("expected StreamEvent::Error, got {:?}", bad_events[0].0);
    };
    assert_eq!(err.code, VALIDATION_FAILED_CODE);

    // The good request's reply: FinalOnly, so exactly one Frame (with correct
    // request_id and sample count) followed by Done { cancelled: false }.
    let good_events = read_stream_until_done(&mut out_cursor);
    let frames: Vec<_> = good_events
        .iter()
        .filter_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(frames.len(), 1);
    let (header, payload) = frames[0];
    assert_eq!(header.request_id, 2);
    assert_eq!(header.samples, 2);
    assert_eq!(
        payload.len(),
        4 * 4 * indicatrix_net::radiance::BYTES_PER_PIXEL
    );
    assert!(matches!(
        good_events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));
}

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
        handle_connection_with_gpu(stream, 2, &gpu, &test_db(), ComputeMode::default()).unwrap();
    });

    let mut client = TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();

    let request = RenderRequest {
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
/// prove. Hence [`tiny_scene_without_dispersion`], where no decision can sit on the
/// threshold. Dispersive CPU/GPU parity is covered statistically instead, by the GPU
/// equivalence harness's Tier 3 image comparisons (`examples/gpu_equivalence_harness.rs`).
#[cfg(feature = "gpu")]
#[test]
fn hybrid_gpu_and_cpu_split_sums_to_the_same_result_as_tracing_the_range_directly() {
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
        handle_connection_with_gpu(stream, 2, &gpu, &test_db(), ComputeMode::Hybrid).unwrap();
    });

    let mut client = TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();

    let request = RenderRequest {
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

// ---- Progressive streaming ---------------------------------------------------

/// A scene with enough per-sample work that a several-dozen-sample request takes long
/// enough for `run_stream`'s emitter to get multiple chances to poll/emit before the
/// tracer finishes -- unlike `tiny_scene`, which can finish before the emitter's first
/// loop iteration runs. Used by tests that need to observe more than one emission
/// deterministically.
fn heavier_scene() -> SceneState {
    SceneState {
        width: 24,
        height: 24,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 6,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
    }
}

/// The `request_id` carried by one `StreamEvent` -- every variant but `Error`
/// carries one; see `indicatrix_net::messages`' docs on why that's what makes a stale
/// reply mechanically identifiable.
fn event_request_id(event: &StreamEvent) -> u32 {
    match event {
        StreamEvent::Frame(h) => h.request_id,
        StreamEvent::Preview(h) => h.request_id,
        StreamEvent::Progress(p) => p.request_id,
        StreamEvent::Done(d) => d.request_id,
        StreamEvent::Error(e) => panic!("unexpected StreamEvent::Error: {e:?}"),
    }
}

#[test]
fn delta_frames_tile_the_requested_sample_range_with_no_gaps_or_overlaps() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        request_id: 11,
        scene: heavier_scene(),
        first_sample: 100,
        samples: 64,
        stream: live_progressive(0), // due on every emitter tick
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    let mut ranges: Vec<(u32, u32)> = events
        .iter()
        .filter_map(|(e, _)| match e {
            StreamEvent::Frame(h) => Some((h.first_sample, h.samples)),
            _ => None,
        })
        .collect();
    assert!(!ranges.is_empty(), "expected at least one FRAME delta");
    ranges.sort_by_key(|r| r.0);

    let mut cursor = 100u32;
    for (first, samples) in &ranges {
        assert_eq!(
            *first, cursor,
            "gap or overlap: expected next delta to start at {cursor}, got {first}"
        );
        cursor += samples;
    }
    assert_eq!(
        cursor,
        100 + 64,
        "deltas must tile the whole requested range exactly"
    );

    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));
}

#[test]
fn final_only_transfer_mode_still_emits_progress() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        request_id: 12,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    let frame_count = events
        .iter()
        .filter(|(e, _)| matches!(e, StreamEvent::Frame(_)))
        .count();
    assert_eq!(
        frame_count, 1,
        "FinalOnly must send exactly one FRAME, covering the whole request"
    );

    let progress_count = events
        .iter()
        .filter(|(e, _)| matches!(e, StreamEvent::Progress(_)))
        .count();
    assert!(
        progress_count >= 1,
        "FinalOnly must still emit PROGRESS on the cadence even though FRAME only arrives once"
    );

    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));
}

#[test]
fn cancel_mid_stream_produces_done_cancelled_and_no_further_payload() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        request_id: 13,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::Cancel(Cancel { request_id: 13 }),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    for (event, _) in &events {
        assert_eq!(event_request_id(event), 13);
    }
    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: true,
            ..
        })
    ));

    // No further payload: handle_connection's outer loop went straight back to
    // reading the next RenderRequest and hit EOF, writing nothing more.
    assert_eq!(
        out_cursor.position(),
        out_cursor.get_ref().len() as u64,
        "no bytes may follow DONE{{cancelled: true, ..}}"
    );
}

/// Why this passes while the real bug (`repro_slow_reader_blocks_the_emitter_and_delays_cancel`,
/// further down) doesn't reproduce on `DuplexHalf`: `CANCEL` is already sitting in the
/// scripted input at time zero, and `DuplexHalf::write`'s unbounded `Vec` can never
/// block. The real bug needs `CANCEL` arriving *while* the emitter is stuck inside a
/// blocked `write()` -- `BackpressureDuplex` supplies that.
#[test]
fn a_peer_that_never_drains_ends_the_connection_rather_than_hanging_forever() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        request_id: 21,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    // Tiny on purpose: the WELCOME handshake write always gets through regardless of
    // capacity (see BackpressureDuplex's doc comment), so this only needs to be
    // smaller than the first FRAME/PROGRESS write once streaming starts.
    let mut duplex = BackpressureDuplex::new(input, 8);
    let result = handle_connection(&mut duplex, 2, &test_db());

    // The bound this closes: without WRITE_TIMEOUT this would hang forever; instead
    // it returns an error within one WRITE_TIMEOUT.
    let err = result.expect_err(
        "a peer that never drains must end the connection with an error, not hang forever",
    );
    assert!(
        matches!(
            err,
            indicatrix_net::messages::NetError::Framing(indicatrix_net::framing::FramingError::Io(ref e))
                if e.kind() == std::io::ErrorKind::TimedOut
        ),
        "expected the write-timeout error to surface as-is, got {err:?}"
    );
}

/// The same bound as the test above, but against a peer that reports `WriteZero`
/// instead of `TimedOut` -- what a real `rustls` `StreamOwned` reports for an
/// underlying write timeout (see `crate::stream_emit::is_stream_timeout`). Nothing in
/// `run_stream`'s write path classifies by kind before propagating, so this must
/// behave identically: an error, not a hang, with `WriteZero` preserved as-is.
#[test]
fn a_peer_that_never_drains_ends_the_connection_rather_than_hanging_forever_write_zero() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        request_id: 23,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = BackpressureDuplex::new_with_kind(input, 8, std::io::ErrorKind::WriteZero);
    let result = handle_connection(&mut duplex, 2, &test_db());

    let err = result.expect_err(
        "a peer that never drains must end the connection with an error, not hang forever, \
         regardless of which io::ErrorKind the write failure carries",
    );
    assert!(
        matches!(
            err,
            indicatrix_net::messages::NetError::Framing(indicatrix_net::framing::FramingError::Io(ref e))
                if e.kind() == std::io::ErrorKind::WriteZero
        ),
        "expected the WriteZero error to surface as-is, got {err:?}"
    );
}

/// The positive case: a peer with plenty of room to drain into never sees a write
/// timeout, and `CANCEL` still produces `DONE{cancelled: true}` and no further
/// payload -- proving `WRITE_TIMEOUT` bounds the pathological case above without
/// punishing a merely-finite, still-draining peer.
#[test]
fn cancel_still_completes_cleanly_against_a_finite_but_sufficient_peer() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        request_id: 22,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::Cancel(Cancel { request_id: 22 }),
    )
    .unwrap();

    // Generous: comfortably more than everything this tiny 24x24 request could
    // possibly produce before CANCEL is observed.
    let mut duplex = BackpressureDuplex::new(input, 1_000_000);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: true,
            ..
        })
    ));
}

#[test]
fn stale_request_id_frames_are_identifiable() {
    // Two separate connections: a client that moved on to a new request_id (via any
    // means) can mechanically recognize a reply carrying the old one as stale. The
    // pipelined-on-one-connection scenario gets its own test below.
    fn run_one_request(request_id: u32) -> Vec<(StreamEvent, Option<Vec<u8>>)> {
        let mut input = Vec::new();
        indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
        let request = RenderRequest {
            request_id,
            scene: tiny_scene(),
            first_sample: 0,
            samples: 4,
            stream: final_only(0),
        };
        indicatrix_net::messages::write_message(
            &mut input,
            &ClientMessage::RenderRequest(Box::new(request)),
        )
        .unwrap();

        let mut duplex = DuplexHalf::new(input);
        handle_connection(&mut duplex, 1, &test_db()).unwrap();

        let mut out_cursor = Cursor::new(duplex.out);
        let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
        read_stream_until_done(&mut out_cursor)
    }

    let first_events = run_one_request(21);
    let second_events = run_one_request(22);

    // A client tracking "current epoch = 22" can identify every event belonging to
    // the first, now-stale request_id as one to drop.
    assert_ne!(first_events.len(), 0);
    for (event, _) in &first_events {
        assert_eq!(event_request_id(event), 21);
        assert_ne!(event_request_id(event), 22);
    }
    assert_ne!(second_events.len(), 0);
    for (event, _) in &second_events {
        assert_eq!(event_request_id(event), 22);
    }
}

/// The scenario `stream_emit::run_stream`'s pipelining support exists for: a client
/// writes its next `RenderRequest` right behind the first, on the same connection,
/// without reading `DONE` for the first one. Without that support this is the failure
/// mode `poll_for_client_message` warns about (spurious error, connection torn down) --
/// `handle_connection` returning `Ok(())` is itself part of what this test proves.
#[test]
fn pipelined_render_request_on_one_connection_is_queued_after_an_implicit_cancel() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();

    // Heavy enough that the emitter gets multiple chances to poll (and observe the
    // pipelined request) before the tracer finishes on its own.
    let first_request = RenderRequest {
        request_id: 41,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(first_request)),
    )
    .unwrap();

    // Written immediately behind the first -- no DONE read in between. A different
    // (smaller) scene so a leaked buffer from the first request would show up as a
    // payload-size mismatch rather than silently passing.
    let second_request = RenderRequest {
        request_id: 42,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 4,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(second_request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();

    // The first request's reply: implicitly cancelled, like an explicit CANCEL --
    // every event carries request_id 41, ending in DONE { cancelled: true }.
    let first_events = read_stream_until_done(&mut out_cursor);
    for (event, _) in &first_events {
        assert_eq!(event_request_id(event), 41);
    }
    assert!(
        matches!(
            first_events.last().unwrap().0,
            StreamEvent::Done(Done {
                cancelled: true,
                ..
            })
        ),
        "the superseded request must still get a proper DONE {{ cancelled: true }}, \
         not be silently dropped"
    );

    // The pipelined request's reply, immediately following on the same connection.
    // FinalOnly, so exactly one FRAME sized for its own scene, then DONE { cancelled: false }.
    let second_events = read_stream_until_done(&mut out_cursor);
    let frames: Vec<_> = second_events
        .iter()
        .filter_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(frames.len(), 1);
    let (header, payload) = frames[0];
    assert_eq!(header.request_id, 42);
    assert_eq!(header.samples, 4);
    // 4x4 (tiny_scene), not 24x24 -- proves accumulation started fresh, not
    // inheriting the cancelled first request's buffers.
    assert_eq!(
        payload.len(),
        4 * 4 * indicatrix_net::radiance::BYTES_PER_PIXEL
    );
    for (event, _) in &second_events {
        assert_eq!(event_request_id(event), 42);
    }
    assert!(matches!(
        second_events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));

    // Nothing beyond the two requests' worth of events was written.
    assert_eq!(
        out_cursor.position(),
        out_cursor.get_ref().len() as u64,
        "no bytes may follow the second request's DONE"
    );
}

#[test]
fn preview_frames_never_enter_the_full_resolution_accumulator_path() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();

    let scene = tiny_scene(); // 4x4
    let mut stream_cfg = live_progressive(0);
    stream_cfg.preview = Some(indicatrix_net::messages::PreviewConfig {
        width: 2,
        height: 2,
    });
    let request = RenderRequest {
        request_id: 31,
        scene: scene.clone(),
        first_sample: 0,
        samples: 8,
        stream: stream_cfg,
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    let previews: Vec<_> = events
        .iter()
        .filter_map(|(e, p)| match e {
            StreamEvent::Preview(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .collect();
    assert!(
        !previews.is_empty(),
        "expected at least one PREVIEW with a preview configured"
    );

    for (header, payload) in &previews {
        assert_eq!(header.width, 2);
        assert_eq!(header.height, 2);

        // Decoding it against the scene's full resolution must fail with a length
        // mismatch -- a PREVIEW payload can never be silently fed into the FRAME path.
        let full_res_attempt = indicatrix_net::radiance::decode(payload, scene.width, scene.height);
        assert!(
            matches!(
                full_res_attempt,
                Err(indicatrix_net::radiance::RadianceError::LengthMismatch { .. })
            ),
            "{full_res_attempt:?}"
        );

        // It DOES decode correctly at its own declared (reduced) resolution -- it's
        // valid radiance data, just never for the full-resolution accumulator.
        assert!(indicatrix_net::radiance::decode(payload, header.width, header.height).is_ok());
    }
}

// ---- TILT_CURVES leaves the connection usable afterward ---------------------------

/// `serve::tilt::handle_tilt_curves_request` must restore the short read timeout
/// `poll_for_cancel` applies to the socket
/// (`CANCEL_POLL_TIMEOUT`/`crate::stream_emit::FRAME_REMAINDER_TIMEOUT`) before
/// returning -- otherwise this connection's next `read_message` call fails unless the
/// next frame happens to already be sitting in the socket buffer. Driven over a REAL
/// loopback `TcpStream`
/// (unlike this file's `DuplexHalf`-based tests, which can never actually time out a
/// read) with the second message written only after the first reply comes back --
/// exactly the case a leftover short timeout would break.
#[test]
fn tilt_curves_request_then_library_request_both_get_replies_over_a_real_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, 2, &test_db()).unwrap();
    });

    let mut client = std::net::TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();
    assert!(welcome.tilt_curves);

    let tilt_request = indicatrix_net::messages::TiltCurvesRequest {
        request_id: 1,
        scene: tiny_scene(),
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::TiltCurvesRequest(Box::new(tilt_request)),
    )
    .unwrap();
    let tilt_response: indicatrix_net::messages::TiltCurvesResponse =
        indicatrix_net::messages::read_message(&mut client).unwrap();
    assert!(
        matches!(
            tilt_response,
            indicatrix_net::messages::TiltCurvesResponse::Curves(_)
        ),
        "{tilt_response:?}"
    );

    // Sent only now -- not already buffered when the TILT_CURVES reply above went out
    // -- so this proves the read timeout the emitter left on the socket was actually
    // restored to blocking, not that a lucky pre-buffered read papered over the bug.
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::Library(Box::new(
            indicatrix_net::library::LibraryRequest::FilterOptions,
        )),
    )
    .unwrap();
    let library_response: indicatrix_net::library::LibraryResponse =
        indicatrix_net::messages::read_message(&mut client).unwrap();
    assert!(
        matches!(
            library_response,
            indicatrix_net::library::LibraryResponse::FilterOptions { .. }
        ),
        "{library_response:?}"
    );

    drop(client);
    server.join().unwrap();
}

// ---- Real-socket repro: does the emitter block on a slow reader? -----------

/// Manual repro (real `TcpStream`, real OS socket buffers -- `DuplexHalf`'s unbounded
/// `Vec` can never model this): starts a real `serve` connection, sends a
/// `RenderRequest` big enough that `FRAME` payloads fill the OS send buffer if the peer
/// never reads, then never reads for a while before sending `CANCEL`. If the emitter
/// thread blocks inside `write_all` once the socket backs up -- the same thread that
/// polls for `CANCEL` -- then `CANCEL` sits unread until this test drains the read
/// side again.
///
/// `WRITE_TIMEOUT` bounds any single blocked write, including the cancelled `DONE`
/// write and the `StreamEvent::Progress` heartbeat while waiting for the tracer to
/// stop, so against a peer that truly never drains, expect `DONE{cancelled:true}` (or
/// a transport error) within roughly one to two `WRITE_TIMEOUT`s of `CANCEL` being
/// sent, never unbounded.
#[test]
#[ignore = "manual repro: real sockets + sleeps, prints timing to demonstrate the write-blocks-the-poll-loop theory"]
fn repro_slow_reader_blocks_the_emitter_and_delays_cancel() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, 0, &test_db())
    });

    let mut client = std::net::TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();

    // Large enough that LiveProgressive FRAME deltas add up to several MB before the
    // request finishes -- past a default OS socket-buffer size if nothing reads them.
    let scene = SceneState {
        width: 1200,
        height: 1200,
        max_bounces: 2,
        ..tiny_scene()
    };
    let request = RenderRequest {
        request_id: 99,
        scene,
        first_sample: 0,
        samples: 65_536,
        stream: indicatrix_net::messages::StreamConfig {
            transfer_mode: indicatrix_net::messages::TransferMode::LiveProgressive,
            cadence_ms: 100,
            preview: None,
        },
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    // Deliberately never read. Give the tracer/emitter time to produce and attempt
    // to write several cadence ticks' worth of FRAME data.
    eprintln!("not reading for 8s -- letting FRAME writes pile up unread");
    thread::sleep(std::time::Duration::from_secs(8));

    eprintln!("sending CANCEL now, still not reading FRAME payload");
    let cancel_sent_at = std::time::Instant::now();
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::Cancel(Cancel { request_id: 99 }),
    )
    .unwrap();

    // Now start draining and time how long DONE{cancelled: true} takes to show up.
    // If the emitter were never blocked, this arrives promptly (~100-200ms); a long
    // delay confirms the write-blocks-the-poll-loop theory.
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let drain_start = std::time::Instant::now();
    let mut frame_count = 0u32;
    let mut progress_count = 0u32;
    loop {
        let event: Result<(StreamEvent, Option<Vec<u8>>), _> =
            indicatrix_net::messages::read_stream_event(&mut client);
        match event {
            Ok((StreamEvent::Done(done), _)) => {
                eprintln!(
                    "DONE{{cancelled={}, samples_done={}, effective_cadence_ms={}}} arrived \
                     {:?} after CANCEL was sent ({:?} to drain from when reading resumed); \
                     saw {frame_count} FRAMEs and {progress_count} PROGRESSes total",
                    done.cancelled,
                    done.stats.samples_done,
                    done.stats.effective_cadence_ms,
                    cancel_sent_at.elapsed(),
                    drain_start.elapsed()
                );
                break;
            }
            Ok((StreamEvent::Frame(_), _)) => frame_count += 1,
            Ok((StreamEvent::Progress(_), _)) => progress_count += 1,
            Ok(_) => {}
            Err(e) => {
                eprintln!("stream ended before DONE: {e:?}");
                break;
            }
        }
    }

    drop(client);
    let _ = server.join();
}

// ---- Mutual-TLS tests -------------------------------------------------------
//
// Each test builds a real throwaway private CA and issues real certificates via
// `crate::pki`, then drives a real TLS handshake over a real loopback `TcpStream` --
// nothing is mocked at the `rustls` layer, to catch what a mock would paper over (a
// missing SAN, a wrong trust anchor, an allowlist that isn't actually consulted).
mod mtls;
