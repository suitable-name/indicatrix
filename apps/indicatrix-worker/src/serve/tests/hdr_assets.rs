//! HDR assets end to end through the real request loop: an HDR scene's map is asked for
//! once (`NEED_ASSET` -> `ASSET`), verified, cached and used; the next request naming
//! the same hash is not asked again; the render equals rendering the same samples with
//! the map decoded from the locally held bytes; bad bytes and a server without an asset
//! cache are refused clearly.

use super::fixtures::{DuplexHalf, final_only, read_stream_until_done, test_db, tiny_scene};
use crate::{
    assets::{self, AssetCache},
    cli::ComputeMode,
    render_core,
    serve::connection::{ViewerContext, handle_viewer_connection},
};
use indicatrix::renderer::{
    env_map::{HdrLimits, environment_from_hdr_bytes},
    gpu_backend::GpuBackend,
};
use indicatrix_net::{
    SceneState, framing, handshake,
    messages::{
        AssetHeader, ClientMessage, LOOPBACK_SERVER_PREFERENCE, RenderRequest, RequestIntent,
        StreamEvent, Welcome, content_hash, error_codes, write_asset_message,
    },
    radiance,
    scene::{HdrEnvironment, SceneEnvironment},
};
use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// A synthetic `.hdr` file unique to this call (the decoded registry is process-wide).
fn unique_hdr(width: u32, height: u32) -> Vec<u8> {
    // An integer tag in the first texel: exactly representable in RGBE's 8-bit
    // mantissa, so no two calls encode to the same bytes.
    let tag = (UNIQUE.fetch_add(1, Ordering::Relaxed) % 250 + 1) as f32;
    let pixels: Vec<image::Rgb<f32>> = (0..width * height)
        .map(|i| {
            let t = i as f32 / (width * height) as f32;
            let first = if i == 0 { tag } else { t.mul_add(5.0, 0.2) };
            image::Rgb([first, 0.5, (t * 13.0).cos().abs()])
        })
        .collect();
    let mut out = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut out)
        .encode(&pixels, width as usize, height as usize)
        .unwrap();
    out
}

fn hdr_scene(bytes: &[u8], width: u32, height: u32) -> SceneState {
    SceneState {
        environment: SceneEnvironment::Hdr(HdrEnvironment {
            content_hash: content_hash(bytes),
            width,
            height,
        }),
        ..tiny_scene()
    }
}

fn request(request_id: u32, scene: &SceneState, first_sample: u32, samples: u32) -> ClientMessage {
    ClientMessage::RenderRequest(Box::new(RenderRequest {
        request_id,
        scene: scene.clone(),
        first_sample,
        samples,
        stream: final_only(0),
        intent: RequestIntent::Batch,
    }))
}

fn write(buf: &mut Vec<u8>, msg: &ClientMessage) {
    indicatrix_net::messages::write_message(buf, msg).unwrap();
}

fn temp_cache(name: &str) -> (std::path::PathBuf, AssetCache) {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-serve-hdr-{name}-{}-{}",
        std::process::id(),
        UNIQUE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = AssetCache::open(&dir, 1 << 26).unwrap();
    (dir, cache)
}

/// Runs one scripted viewer connection (CPU only, own lane, plain worker) and returns
/// everything the server wrote after its `WELCOME`, plus the `WELCOME`.
fn run(input: Vec<u8>, assets: Option<&AssetCache>) -> (Welcome, Cursor<Vec<u8>>) {
    let mut duplex = DuplexHalf::new(input);
    let db = test_db();
    handle_viewer_connection(
        &mut duplex,
        &ViewerContext {
            threads: 2,
            gpu: &Arc::new(GpuBackend::disabled()),
            db: &db,
            compute_mode: ComputeMode::OnlyCpu,
            encodings: &LOOPBACK_SERVER_PREFERENCE,
            link: None,
            own_lane: true,
            registry: None,
            cert_role: None,
            coordinator: None,
            assets,
        },
    )
    .unwrap();
    let mut out = Cursor::new(duplex.out);
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut out).unwrap();
    (welcome, out)
}

/// The summed radiance of a request's single `FinalOnly` `FRAME`.
fn frame_sum(events: &[(StreamEvent, Option<Vec<u8>>)]) -> Vec<glam::Vec3> {
    let (_, payload) = events
        .iter()
        .find(|(event, _)| matches!(event, StreamEvent::Frame(_)))
        .expect("a FRAME");
    radiance::decode(payload.as_deref().unwrap(), 4, 4).unwrap()
}

#[test]
fn an_hdr_scene_asks_for_its_map_once_and_renders_it_like_the_local_bytes() {
    let bytes = unique_hdr(32, 16);
    let scene = hdr_scene(&bytes, 32, 16);
    let (dir, cache) = temp_cache("once");
    // Two connections (a request scripted right behind a streaming one would be
    // pipelined, cancelling it): the first uploads, the second must not be asked.
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    write(&mut input, &request(1, &scene, 0, 2));
    write_asset_message(&mut input, &bytes).unwrap();
    let (welcome, mut out) = run(input, Some(&cache));
    assert!(
        welcome.render.unwrap().hdr,
        "an own lane with a cache advertises HDR"
    );
    let first = read_stream_until_done(&mut out);

    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    write(&mut input, &request(2, &scene, 2, 2));
    let (_, mut out) = run(input, Some(&cache));
    assert_eq!(
        first[0].0,
        StreamEvent::NeedAsset {
            content_hash: content_hash(&bytes)
        }
    );
    assert!(
        matches!(first.last().unwrap().0, StreamEvent::Done(d) if d.request_id == 1 && !d.cancelled)
    );
    let second = read_stream_until_done(&mut out);
    assert!(
        second
            .iter()
            .all(|(event, _)| !matches!(event, StreamEvent::NeedAsset { .. })),
        "the same hash must not be asked for twice: {second:?}"
    );
    assert!(
        matches!(second.last().unwrap().0, StreamEvent::Done(d) if d.request_id == 2 && !d.cancelled)
    );
    assert!(
        cache.contains(&content_hash(&bytes)),
        "received bytes are cached"
    );

    // The transferred (cached) bytes are the viewer's, and decode to the local map bit
    // for bit.
    let transferred = cache.get(&content_hash(&bytes)).unwrap();
    assert_eq!(transferred, bytes);
    let local = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap();
    let remote = environment_from_hdr_bytes(&transferred, HdrLimits::DEFAULT).unwrap();
    assert!(remote.bitwise_eq(&local));

    // Rendering with the locally decoded map (registered under a separate key, an
    // independent decode of the viewer's bytes) gives the same samples.
    let local_key = HdrEnvironment {
        content_hash: [0x4C; 32],
        width: 32,
        height: 16,
    };
    let _pin = assets::decode_and_register(&local_key, &bytes).unwrap();
    let local_scene = SceneState {
        environment: SceneEnvironment::Hdr(local_key),
        ..scene
    };
    let reference = render_core::trace_samples(&local_scene, 0, 2, 2);
    let served = frame_sum(&first);
    assert_eq!(served.len(), reference.len());
    for (a, b) in served.iter().zip(&reference) {
        let scale = a.abs().max(b.abs()).max(glam::Vec3::splat(1e-6));
        assert!(
            ((*a - *b).abs() / scale).max_element() <= 1e-6,
            "served={a:?} local={b:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bytes_that_do_not_match_the_hash_are_refused_and_the_connection_stays_usable() {
    let bytes = unique_hdr(16, 8);
    let scene = hdr_scene(&bytes, 16, 8);
    let (dir, cache) = temp_cache("mismatch");
    let wrong = unique_hdr(16, 8);
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    write(&mut input, &request(1, &scene, 0, 1));
    // Claims the scene's hash, carries other bytes.
    write(
        &mut input,
        &ClientMessage::Asset(AssetHeader {
            content_hash: content_hash(&bytes),
            len: wrong.len() as u32,
        }),
    );
    framing::write_frame(&mut input, &wrong).unwrap();
    write(&mut input, &request(2, &tiny_scene(), 0, 1));

    let (_, mut out) = run(input, Some(&cache));
    let first = read_stream_until_done(&mut out);
    let (StreamEvent::Error(error), _) = first.last().unwrap() else {
        panic!("expected ASSET_FAILED, got {first:?}");
    };
    assert_eq!(error.code, error_codes::ASSET_FAILED);
    assert!(!cache.contains(&content_hash(&bytes)));
    let second = read_stream_until_done(&mut out);
    assert!(matches!(second.last().unwrap().0, StreamEvent::Done(d) if d.request_id == 2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_server_without_an_asset_cache_refuses_hdr_scenes_and_does_not_advertise_them() {
    let bytes = unique_hdr(8, 4);
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    write(&mut input, &request(1, &hdr_scene(&bytes, 8, 4), 0, 1));
    let (welcome, mut out) = run(input, None);
    assert!(!welcome.render.unwrap().hdr);
    let events = read_stream_until_done(&mut out);
    let [(StreamEvent::Error(error), None)] = events.as_slice() else {
        panic!("expected one refusal, got {events:?}");
    };
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
}

#[test]
fn an_over_limit_hdr_map_is_refused_by_validation_without_a_transfer() {
    let (dir, cache) = temp_cache("limits");
    let mut scene = tiny_scene();
    scene.environment = SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: [9; 32],
        width: 16_385,
        height: 16,
    });
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    write(&mut input, &request(1, &scene, 0, 1));
    let (_, mut out) = run(input, Some(&cache));
    let events = read_stream_until_done(&mut out);
    let [(StreamEvent::Error(error), None)] = events.as_slice() else {
        panic!("expected one validation error, got {events:?}");
    };
    assert_eq!(error.code, error_codes::VALIDATION_FAILED);
    assert!(error.message.contains("HDR map"), "{}", error.message);
    let _ = std::fs::remove_dir_all(&dir);
}
