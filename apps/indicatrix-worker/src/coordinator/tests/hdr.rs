//! HDR jobs end to end over real TLS on loopback, CPU only: a coordinator without
//! `--render` holds an HDR scene's map for the job (asking the viewer once) and forwards
//! it to each joined worker that asks; workers without an asset cache are left out of
//! HDR jobs; bad bytes from the viewer fail the job cleanly.

use super::{
    fixtures::{
        bundle, coordinator_args, cpu_worker_setup_with_cache, fan_out_config, pki_with_server,
        start, tls_client, wait_for,
    },
    support::{ViewerStream, render, scene, send, spawn_dying_worker, viewer},
};
use crate::{
    assets,
    coordinator::{JobConfig, LivenessConfig, Registry},
    join::{JoinTarget, WorkerSetup, join_once},
    render_core::trace_samples,
    serve::ServeHandle,
};
use glam::Vec3;
use indicatrix::renderer::tonemap::tonemap_accumulation;
use indicatrix_dispatch::{ChunkPolicy, PoolConfig};
use indicatrix_net::{
    SceneState,
    client::handshake_with_hello,
    display, framing, handshake,
    messages::{
        AssetHeader, Backend, ClientMessage, DisplayEncoding, Done, ErrorMsg, FinalImageHeader,
        FinalImageRequest, FinalOutput, PeerRole, RenderCapability, RequestIntent, StreamEvent,
        TransferMode, WireColorSpace, content_hash, error_codes,
    },
    radiance::PayloadDecoder,
    scene::{HdrEnvironment, SceneEnvironment},
};
use std::{
    net::SocketAddr,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(20);

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// A synthetic `.hdr` file unique to this call (the decoded registry is process-wide).
fn unique_hdr(width: u32, height: u32) -> Vec<u8> {
    // An integer tag in the first texel: exactly representable in RGBE's 8-bit
    // mantissa, so no two calls encode to the same bytes.
    let tag = (UNIQUE.fetch_add(1, Ordering::Relaxed) % 250 + 1) as f32;
    let pixels: Vec<image::Rgb<f32>> = (0..width * height)
        .map(|i| {
            let t = i as f32 / (width * height) as f32;
            let first = if i == 0 { tag } else { t.mul_add(7.0, 0.3) };
            image::Rgb([first, 0.8, (t * 5.0).sin().abs()])
        })
        .collect();
    let mut out = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut out)
        .encode(&pixels, width as usize, height as usize)
        .unwrap();
    out
}

/// A `width x height` diamond scene lit by the 32x16 map `bytes`.
fn hdr_scene(bytes: &[u8], width: u32, height: u32) -> SceneState {
    SceneState {
        environment: SceneEnvironment::Hdr(HdrEnvironment {
            content_hash: content_hash(bytes),
            width: 32,
            height: 16,
        }),
        ..scene(width, height)
    }
}

/// The same scene rendered on this machine: the map decoded from `bytes` independently
/// and registered under a key of its own (kept alive by the returned pin).
fn local_reference(
    scene: &SceneState,
    bytes: &[u8],
    (first_sample, samples): (u32, u32),
) -> Vec<Vec3> {
    let mut key = content_hash(bytes);
    key[0] ^= 0xFF;
    let local = HdrEnvironment {
        content_hash: key,
        width: 32,
        height: 16,
    };
    let _pin = assets::decode_and_register(&local, bytes).unwrap();
    let local_scene = SceneState {
        environment: SceneEnvironment::Hdr(local),
        ..scene.clone()
    };
    trace_samples(&local_scene, first_sample, samples, 2)
}

/// Whether `a` and `b` agree to 1e-6 relative (with a 1e-6 floor near zero).
fn within_1e6(a: &[Vec3], b: &[Vec3]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let scale = x.abs().max(y.abs()).max(Vec3::splat(1e-6));
            ((*x - *y).abs() / scale).max_element() <= 1e-6
        })
}

/// A coordinator without `--render` whose batch jobs use fixed 4-sample chunks (so the
/// partition, and a 2-sample final image's single chunk, never depend on timing).
fn coordinator(label: &str) -> (ServeHandle, Arc<Registry>, std::path::PathBuf) {
    let pki = pki_with_server(label);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    handle
        .coordinator
        .as_ref()
        .unwrap()
        .set_job_config(JobConfig {
            batch: PoolConfig {
                policy: ChunkPolicy::fixed(4),
                ..JobConfig::default().batch
            },
            ..fan_out_config()
        });
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    (handle, registry, pki)
}

/// Runs a real CPU `join` worker with (`cache`) or without an asset cache; returns its
/// setup (to inspect the cache).
fn spawn_joined(worker_addr: SocketAddr, bundle_dir: &Path, cache: bool) -> Arc<WorkerSetup> {
    let target = JoinTarget::from_bundle(&worker_addr.to_string(), bundle_dir).unwrap();
    let setup = cpu_worker_setup_with_cache(cache);
    let running = Arc::clone(&setup);
    thread::spawn(move || {
        let _ = join_once(&target, &running);
    });
    setup
}

/// Everything one request produced, up to its `DONE` or `ERROR`.
struct Run {
    sum: Vec<Vec3>,
    frame_samples: u32,
    final_image: Option<(FinalImageHeader, Vec<u8>)>,
    done: Option<Done>,
    error: Option<ErrorMsg>,
    /// `NEED_ASSET`s the coordinator sent this viewer.
    need_assets: u32,
}

/// Sends `message` and reads its events, answering every `NEED_ASSET` with an `ASSET`
/// that claims to be `claimed` and carries `sent` (the same bytes for an honest viewer).
fn run(
    client: &mut ViewerStream,
    message: &ClientMessage,
    (width, height): (u32, u32),
    (claimed, sent): (&[u8], &[u8]),
) -> Run {
    send(client, message);
    let mut out = Run {
        sum: vec![Vec3::ZERO; width as usize * height as usize],
        frame_samples: 0,
        final_image: None,
        done: None,
        error: None,
        need_assets: 0,
    };
    let mut decoder = PayloadDecoder::new();
    loop {
        let (event, payload) = indicatrix_net::messages::read_stream_event(client).unwrap();
        match event {
            StreamEvent::NeedAsset { content_hash: hash } => {
                assert_eq!(hash, content_hash(claimed), "asked for the scene's map");
                out.need_assets += 1;
                let header = AssetHeader {
                    content_hash: hash,
                    len: u32::try_from(sent.len()).unwrap(),
                };
                send(client, &ClientMessage::Asset(header));
                framing::write_frame(client, sent).unwrap();
            }
            StreamEvent::Frame(h) => {
                let bytes = payload.unwrap();
                decoder
                    .decode_and_add(h.encoding, h.raw_len, &bytes, width, height, &mut out.sum)
                    .unwrap();
                out.frame_samples += h.samples;
            }
            StreamEvent::FinalImage(h) => out.final_image = Some((h, payload.unwrap())),
            StreamEvent::Done(done) => {
                out.done = Some(done);
                return out;
            }
            StreamEvent::Error(error) => {
                out.error = Some(error);
                return out;
            }
            _ => {}
        }
    }
}

/// A `Batch` + `FinalOnly` render request.
fn batch(request_id: u32, scene: &SceneState, range: (u32, u32)) -> ClientMessage {
    render(
        request_id,
        scene.clone(),
        range,
        TransferMode::FinalOnly,
        RequestIntent::Batch,
    )
}

/// The acceptance run: the coordinator asks the viewer ONCE, both workers get the map
/// from the coordinator (each asks once), and the merged image matches a local render of
/// the same bytes over the same range to 1e-6; a second request with the same hash asks
/// nobody; a `FinalImageRequest` of the HDR scene works through the workers too.
#[test]
fn an_hdr_job_asks_the_viewer_once_and_forwards_the_map_to_each_worker() {
    let (handle, registry, pki) = coordinator("hdr-forward");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let worker_addr = handle.worker_addr.unwrap();
    let workers = [
        spawn_joined(worker_addr, &worker_bundle, true),
        spawn_joined(worker_addr, &worker_bundle, true),
    ];
    assert!(wait_for(WAIT, || registry.capacity().hdr_workers == 2));

    let (welcome, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    assert!(
        welcome.render.unwrap().hdr,
        "joined HDR workers: HDR advertised"
    );
    let bytes = unique_hdr(32, 16);
    let hash = content_hash(&bytes);
    let image = hdr_scene(&bytes, 8, 6);

    let first = run(
        &mut client,
        &batch(1, &image, (3, 8)),
        (8, 6),
        (&bytes, &bytes),
    );
    let done = first
        .done
        .unwrap_or_else(|| panic!("DONE, got {:?}", first.error));
    assert_eq!((done.request_id, done.stats.samples_done), (1, 8));
    assert_eq!(first.frame_samples, 8);
    assert_eq!(first.need_assets, 1, "the coordinator asks the viewer once");
    assert_eq!(registry.assets_forwarded(), 2, "one ASSET per worker");
    for setup in &workers {
        assert!(
            setup.assets.as_ref().unwrap().contains(&hash),
            "every worker got the map from the coordinator"
        );
    }
    assert!(within_1e6(
        &first.sum,
        &local_reference(&image, &bytes, (3, 8))
    ));

    let second = run(
        &mut client,
        &batch(2, &image, (11, 8)),
        (8, 6),
        (&bytes, &bytes),
    );
    assert_eq!(second.done.expect("DONE").stats.samples_done, 8);
    assert_eq!(second.need_assets, 0, "the coordinator's cache has the map");
    assert_eq!(registry.assets_forwarded(), 2, "no worker asked again");
    assert!(within_1e6(
        &second.sum,
        &local_reference(&image, &bytes, (11, 8))
    ));

    let final_request = ClientMessage::FinalImageRequest(Box::new(FinalImageRequest {
        request_id: 3,
        scene: image.clone(),
        first_sample: 5,
        samples: 2,
        width: 8,
        height: 6,
        color_space: WireColorSpace::Srgb,
        output: FinalOutput::PngRgba8,
        viewer_samples: 0,
    }));
    let picture = run(&mut client, &final_request, (8, 6), (&bytes, &bytes));
    assert_eq!(picture.done.expect("DONE").stats.samples_done, 2);
    assert_eq!(picture.need_assets, 0);
    let (header, png) = picture.final_image.expect("one FINAL_IMAGE");
    assert_eq!((header.request_id, header.samples_done), (3, 2));
    let pixels = display::decode_rgba8(DisplayEncoding::Png, 8, 6, &png).unwrap();
    let sum = local_reference(&image, &bytes, (5, 2));
    let expected = tonemap_accumulation(8, 6, 2, &sum, WireColorSpace::Srgb.into());
    assert_eq!(pixels, expected, "the GUI tone-map of the same HDR-lit sum");
    assert_eq!(registry.capacity().workers, 2, "no connection was dropped");
}

/// A worker whose asset cache is disabled advertises `hdr: false`: a coordinator with
/// only that worker refuses HDR scenes (without asking the viewer for the map) but
/// serves studio scenes on it; once an HDR worker joins, HDR jobs run on it alone (the
/// non-HDR fake worker never sees an HDR chunk), and studio jobs still use every worker.
#[test]
fn workers_without_an_asset_cache_are_left_out_of_hdr_jobs_only() {
    let (handle, registry, pki) = coordinator("hdr-exclude");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let worker_addr = handle.worker_addr.unwrap();
    spawn_joined(worker_addr, &worker_bundle, false);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (welcome, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    assert!(!welcome.render.unwrap().hdr, "no lane renders HDR");
    let bytes = unique_hdr(32, 16);
    let lit = hdr_scene(&bytes, 8, 6);
    let refused = run(
        &mut client,
        &batch(1, &lit, (0, 4)),
        (8, 6),
        (&bytes, &bytes),
    );
    let error = refused.error.expect("a refusal");
    assert_eq!(
        error.code,
        error_codes::UNSUPPORTED_REQUEST,
        "{}",
        error.message
    );
    assert_eq!(
        refused.need_assets, 0,
        "a refused scene's map is not fetched"
    );
    let studio = scene(8, 6);
    let served = run(
        &mut client,
        &batch(2, &studio, (0, 4)),
        (8, 6),
        (&bytes, &bytes),
    );
    assert_eq!(served.done.expect("DONE").stats.samples_done, 4);
    assert!(within_1e6(&served.sum, &trace_samples(&studio, 0, 4, 2)));

    let renders = Arc::new(AtomicU32::new(0));
    let no_hdr = RenderCapability {
        backend: Backend::Cpu { threads: 2 },
        max_pixels: 1 << 20,
        min_cadence_ms: 100,
        hdr: false,
    };
    spawn_dying_worker(worker_addr, &worker_bundle, no_hdr, &renders);
    spawn_joined(worker_addr, &worker_bundle, true);
    assert!(wait_for(WAIT, || registry.capacity().workers == 3
        && registry.capacity().hdr_workers == 1));

    let lit_run = run(
        &mut client,
        &batch(3, &lit, (0, 8)),
        (8, 6),
        (&bytes, &bytes),
    );
    let done = lit_run
        .done
        .unwrap_or_else(|| panic!("DONE, got {:?}", lit_run.error));
    assert_eq!(done.stats.samples_done, 8);
    assert_eq!(lit_run.need_assets, 1);
    assert_eq!(
        renders.load(Ordering::SeqCst),
        0,
        "the non-HDR worker got no HDR chunk"
    );
    assert!(within_1e6(
        &lit_run.sum,
        &local_reference(&lit, &bytes, (0, 8))
    ));

    let studio_run = run(
        &mut client,
        &batch(4, &studio, (0, 16)),
        (8, 6),
        (&bytes, &bytes),
    );
    assert_eq!(studio_run.done.expect("DONE").stats.samples_done, 16);
    assert_eq!(
        renders.load(Ordering::SeqCst),
        1,
        "studio jobs use non-HDR workers"
    );
}

/// Bytes that do not hash to the scene's map: the job fails with `ASSET_FAILED`, nothing
/// reaches a worker, and both the viewer connection and the worker keep working (a
/// studio request, then the HDR request with the right bytes).
#[test]
fn wrong_bytes_from_the_viewer_fail_the_job_and_leave_connections_usable() {
    let (handle, registry, pki) = coordinator("hdr-mismatch");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    spawn_joined(handle.worker_addr.unwrap(), &worker_bundle, true);
    assert!(wait_for(WAIT, || registry.capacity().hdr_workers == 1));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let bytes = unique_hdr(32, 16);
    let wrong = unique_hdr(32, 16);
    let lit = hdr_scene(&bytes, 8, 6);
    let failed = run(
        &mut client,
        &batch(1, &lit, (0, 4)),
        (8, 6),
        (&bytes, &wrong),
    );
    let error = failed.error.expect("ASSET_FAILED");
    assert_eq!(error.code, error_codes::ASSET_FAILED, "{}", error.message);
    assert_eq!(registry.assets_forwarded(), 0);

    let studio = scene(8, 6);
    let served = run(
        &mut client,
        &batch(2, &studio, (0, 4)),
        (8, 6),
        (&bytes, &bytes),
    );
    assert_eq!(served.done.expect("DONE").stats.samples_done, 4);
    let retried = run(
        &mut client,
        &batch(3, &lit, (0, 4)),
        (8, 6),
        (&bytes, &bytes),
    );
    assert_eq!(retried.done.expect("DONE").stats.samples_done, 4);
    assert_eq!(retried.need_assets, 1, "the bad bytes were never cached");
    assert_eq!(registry.assets_forwarded(), 1);
    assert_eq!(registry.capacity().workers, 1);
}

/// A worker asking for an asset that is not the job's map is a protocol violation: its
/// connection is discarded and the job completes on the other worker.
#[test]
fn a_worker_asking_for_a_foreign_asset_is_discarded() {
    let (handle, registry, pki) = coordinator("hdr-foreign");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let worker_addr = handle.worker_addr.unwrap();
    let closed = Arc::new(AtomicBool::new(false));
    spawn_confused_worker(worker_addr, &worker_bundle, &closed);
    spawn_joined(worker_addr, &worker_bundle, true);
    assert!(wait_for(WAIT, || registry.capacity().hdr_workers == 2));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let bytes = unique_hdr(32, 16);
    let lit = hdr_scene(&bytes, 8, 6);
    let result = run(
        &mut client,
        &batch(1, &lit, (0, 8)),
        (8, 6),
        (&bytes, &bytes),
    );
    let done = result
        .done
        .unwrap_or_else(|| panic!("DONE, got {:?}", result.error));
    assert_eq!(done.stats.samples_done, 8);
    assert!(within_1e6(
        &result.sum,
        &local_reference(&lit, &bytes, (0, 8))
    ));
    assert!(wait_for(WAIT, || closed.load(Ordering::SeqCst)));
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    assert_eq!(
        registry.assets_forwarded(),
        1,
        "only the honest worker got the map"
    );
}

/// A fake HDR-capable worker that answers `PING`s and answers every `RenderRequest` with
/// `NEED_ASSET` for a hash no scene names; sets `closed` when its connection ends.
fn spawn_confused_worker(worker_addr: SocketAddr, bundle_dir: &Path, closed: &Arc<AtomicBool>) {
    let mut tls = tls_client(worker_addr, bundle_dir);
    let closed = Arc::clone(closed);
    let capability = RenderCapability {
        backend: Backend::Cpu { threads: 2 },
        max_pixels: 1 << 20,
        min_cadence_ms: 100,
        hdr: true,
    };
    thread::spawn(move || {
        if handshake_with_hello(&mut tls, &handshake::local_worker_hello(capability)).is_err() {
            return;
        }
        let _ = tls.sock.set_read_timeout(Some(Duration::from_secs(300)));
        loop {
            let reply = match indicatrix_net::messages::read_message::<_, ClientMessage>(&mut tls) {
                Ok(ClientMessage::Ping { nonce }) => StreamEvent::Pong { nonce },
                Ok(ClientMessage::RenderRequest(_)) => StreamEvent::NeedAsset {
                    content_hash: [0xAB; 32],
                },
                Ok(_) => continue,
                Err(_) => break,
            };
            if indicatrix_net::messages::write_stream_event(&mut tls, &reply, None).is_err() {
                break;
            }
        }
        closed.store(true, Ordering::SeqCst);
    });
}
