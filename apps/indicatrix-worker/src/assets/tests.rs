//! Unit tests for `crate::assets`: the HDR request policy, the decoded-map registry, and the
//! resolve order (decoded, then disk cache, then the client). The end-to-end
//! `NEED_ASSET` -> `ASSET` exchange through the real request loop lives in
//! `crate::serve`'s `tests::hdr_assets`.

use super::*;
use crate::{
    cli::ComputeMode,
    coordinator::{Coordinator, OwnLaneSetup, ViewerSession},
};
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
    renderer::{
        env_map::{EnvironmentMap, HdrLimits, environment_from_hdr_bytes},
        gpu_backend::GpuBackend,
    },
};
use indicatrix_net::{
    SceneState,
    messages::{
        Backend, PayloadEncoding, RenderCapability, StreamEvent, content_hash, error_codes,
    },
    scene::{HdrEnvironment, SceneEnvironment},
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// A synthetic Radiance `.hdr` file whose content is unique to this call (the decoded
/// registry is process-wide, so tests must never share a hash by accident).
pub fn unique_hdr(width: u32, height: u32) -> Vec<u8> {
    // An integer tag in the first texel: exactly representable in RGBE's 8-bit
    // mantissa, so no two calls encode to the same bytes.
    let tag = (UNIQUE.fetch_add(1, Ordering::Relaxed) % 250 + 1) as f32;
    let pixels: Vec<image::Rgb<f32>> = (0..width * height)
        .map(|i| {
            let t = i as f32 / (width * height) as f32;
            let first = if i == 0 { tag } else { t.mul_add(3.0, 0.1) };
            image::Rgb([first, 0.2, (t * 9.0).sin().abs()])
        })
        .collect();
    let mut out = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut out)
        .encode(&pixels, width as usize, height as usize)
        .unwrap();
    out
}

/// The `HdrEnvironment` naming `bytes` (a `width x height` map).
pub fn hdr_env(bytes: &[u8], width: u32, height: u32) -> HdrEnvironment {
    HdrEnvironment {
        content_hash: content_hash(bytes),
        width,
        height,
    }
}

/// A fresh, empty directory under the system temp dir.
pub fn temp_cache_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-assets-{name}-{}-{}",
        std::process::id(),
        UNIQUE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A `Read + Write` double: scripted input (then `WouldBlock` while a read timeout is
/// set, EOF otherwise) and captured output.
#[derive(Default)]
struct Scripted {
    input: std::io::Cursor<Vec<u8>>,
    out: Vec<u8>,
    timeout: bool,
}

impl std::io::Read for Scripted {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.input.read(buf)?;
        if n == 0 && self.timeout {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        Ok(n)
    }
}

impl std::io::Write for Scripted {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl crate::stream_emit::TimeoutRead for Scripted {
    fn set_read_timeout(&mut self, duration: Option<std::time::Duration>) -> std::io::Result<()> {
        self.timeout = duration.is_some();
        Ok(())
    }
}

impl crate::stream_emit::TimeoutWrite for Scripted {
    fn set_write_timeout(&mut self, _: Option<std::time::Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

fn scene(environment: SceneEnvironment) -> SceneState {
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
        environment,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
    }
}

/// A coordinator viewer session: an own lane (`own`), a worker port whose registry
/// holds one idle fake worker advertising `worker_hdr` (`Some`) or none (`None`), and
/// the coordinator's asset cache (`cache`). The far end of the fake worker's
/// connection is returned to keep it open.
fn session(
    own: bool,
    worker_hdr: Option<bool>,
    cache: Option<&Arc<AssetCache>>,
) -> (ViewerSession, Option<std::net::TcpStream>) {
    let own = own.then(|| OwnLaneSetup {
        gpu: Arc::new(GpuBackend::disabled()),
        threads: 1,
        compute_mode: ComputeMode::OnlyCpu,
    });
    let registry = crate::coordinator::Registry::new(crate::coordinator::LivenessConfig::default());
    let far = worker_hdr.map(|hdr| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let near = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let worker_id = registry.allocate_id();
        let info = crate::coordinator::WorkerInfo {
            worker_id,
            capability: RenderCapability {
                backend: Backend::Cpu { threads: 2 },
                max_pixels: 1 << 20,
                min_cadence_ms: 100,
                hdr,
            },
            peer: None,
            label: None,
            payload_encoding: PayloadEncoding::Raw,
        };
        registry.insert(info, Box::new(near), None);
        listener.accept().unwrap().0
    });
    let coordinator = Coordinator::new(Some(registry), own, 0, 1 << 30).with_assets(cache.cloned());
    let session = ViewerSession {
        coordinator: Arc::new(coordinator),
        viewer: Arc::from("test"),
        payload_encoding: PayloadEncoding::Raw,
        link: None,
        advertised: None,
        own_capability: None,
        rates: Arc::default(),
    };
    (session, far)
}

/// The HDR request policy: a plain/joined worker serves HDR exactly with a cache; a
/// coordinator needs its own cache AND a lane that renders HDR (own lane, or a joined
/// worker advertising `hdr`).
#[test]
fn the_policy_serves_hdr_where_a_lane_can_render_it_and_the_map_can_be_held() {
    let dir = temp_cache_dir("policy");
    let cache = Arc::new(AssetCache::open(&dir, 1 << 20).unwrap());
    let hdr = scene(SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: [1; 32],
        width: 8,
        height: 4,
    }));
    let studio = scene(SceneEnvironment::Studio);

    assert_eq!(hdr_route(&studio, None, None), HdrRoute::NotHdr);
    assert_eq!(hdr_route(&hdr, Some(&cache), None), HdrRoute::Serve);
    for (own, worker_hdr) in [(true, None), (true, Some(false)), (false, Some(true))] {
        let (viewer_session, _far) = session(own, worker_hdr, Some(&cache));
        assert_eq!(
            hdr_route(&hdr, Some(&cache), Some(&viewer_session)),
            HdrRoute::Serve,
            "own={own} worker_hdr={worker_hdr:?}"
        );
        // Studio scenes never need anything.
        assert_eq!(
            hdr_route(&studio, None, Some(&viewer_session)),
            HdrRoute::NotHdr
        );
    }
    let refusals = [
        // A plain worker without a cache.
        (None, None),
        // No own lane and no joined worker at all.
        (Some(session(false, None, Some(&cache))), Some(&cache)),
        // Only a joined worker whose cache is disabled.
        (
            Some(session(false, Some(false), Some(&cache))),
            Some(&cache),
        ),
        // Lanes that could render it, but the coordinator keeps no cache.
        (Some(session(true, Some(true), None)), None),
    ];
    for (viewer_session, assets) in refusals {
        let refused = hdr_route(
            &hdr,
            assets.map(Arc::as_ref),
            viewer_session.as_ref().map(|(s, _)| s),
        );
        let HdrRoute::Refuse(error) = refused else {
            panic!("expected a refusal, got {refused:?}");
        };
        assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a coordinator advertises: `hdr` with a cache and an HDR-capable lane (own, or a
/// joined worker advertising it); never without its own cache.
#[test]
fn a_coordinator_advertises_hdr_when_it_holds_assets_and_a_lane_renders_them() {
    use crate::coordinator::{Capacity, viewer_render_capability};
    let own = |hdr| RenderCapability {
        backend: Backend::Cpu { threads: 4 },
        max_pixels: 100,
        min_cadence_ms: 100,
        hdr,
    };
    let workers = |hdr_workers| Capacity {
        workers: 2,
        threads: 8,
        gpus: 0,
        max_pixels: 100,
        hdr_workers,
    };
    assert!(coordinator_advertises_hdr(
        Some(&own(true)),
        workers(0),
        true
    ));
    assert!(coordinator_advertises_hdr(None, workers(1), true));
    assert!(coordinator_advertises_hdr(
        Some(&own(false)),
        workers(2),
        true
    ));
    assert!(!coordinator_advertises_hdr(
        Some(&own(false)),
        workers(0),
        true
    ));
    assert!(!coordinator_advertises_hdr(None, workers(0), true));
    assert!(!coordinator_advertises_hdr(None, workers(2), false));
    assert!(!coordinator_advertises_hdr(
        Some(&own(true)),
        workers(2),
        false
    ));
    // And the viewer-facing capability follows it.
    let bare = viewer_render_capability(None, Some(workers(1)), true).unwrap();
    assert!(bare.hdr);
    let no_hdr_workers = viewer_render_capability(None, Some(workers(0)), true).unwrap();
    assert!(!no_hdr_workers.hdr);
    let with_own = viewer_render_capability(Some(&own(true)), Some(workers(0)), true).unwrap();
    assert!(with_own.hdr);
    let alone = viewer_render_capability(Some(&own(true)), None, true).unwrap();
    assert!(alone.hdr);
}

/// The registry decodes with the shared builder: the result equals a local decode of
/// the same bytes bit for bit, and a wrong declared size is refused.
#[test]
fn a_registered_map_equals_a_local_decode_bit_for_bit() {
    let bytes = unique_hdr(32, 16);
    let env = hdr_env(&bytes, 32, 16);
    let registered = decoded::decode_and_register(&env, &bytes).unwrap();
    let local: EnvironmentMap = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap();
    assert!(registered.bitwise_eq(&local));
    assert!(Arc::ptr_eq(
        &lookup(&env.content_hash).unwrap(),
        &registered
    ));

    let lying = HdrEnvironment { width: 31, ..env };
    assert!(decoded::decode_and_register(&lying, &bytes).is_err());
    assert!(decoded::decode_and_register(&env, b"not an hdr file").is_err());
}

/// An HDR scene whose map nobody resolved panics in the tracer instead of rendering
/// with the studio rig.
#[test]
fn tracing_an_unresolved_hdr_scene_never_falls_back_to_the_studio_rig() {
    let unresolved = scene(SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: [0xEE; 32],
        width: 8,
        height: 4,
    }));
    let result = std::panic::catch_unwind(|| resolved_hdr_map(&unresolved));
    assert!(result.is_err());
    assert!(resolved_hdr_map(&scene(SceneEnvironment::Studio)).is_none());
}

/// A map already in the on-disk cache (but not decoded in this process) is resolved
/// without asking the client: nothing but at most heartbeats is written.
#[test]
fn a_map_in_the_disk_cache_is_resolved_without_need_asset() {
    let dir = temp_cache_dir("disk");
    let cache = AssetCache::open(&dir, 1 << 24).unwrap();
    let bytes = unique_hdr(16, 8);
    let env = hdr_env(&bytes, 16, 8);
    cache.put(&env.content_hash, &bytes).unwrap();

    let mut stream = Scripted::default();
    let fetched = ensure_environment(
        &mut stream,
        &cache,
        Pending {
            request_id: 5,
            cadence_ms: 100,
            hdr: &env,
        },
    )
    .unwrap();
    let Fetched::Ready(map) = fetched else {
        panic!("expected the cached map to resolve");
    };
    assert_eq!((map.width(), map.height()), (16, 8));
    let mut written = std::io::Cursor::new(stream.out);
    while (written.position() as usize) < written.get_ref().len() {
        let (event, _) = indicatrix_net::messages::read_stream_event(&mut written).unwrap();
        assert!(
            matches!(event, StreamEvent::Progress(_)),
            "unexpected {event:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The studio environment a worker builds carries the scene's surface glare, and an
/// HDR map ignores it (reads back as the unscaled `1.0`).
#[test]
fn environment_source_carries_the_scenes_surface_glare() {
    let mut studio = scene(SceneEnvironment::Studio);
    studio.surface_glare = 0.0;
    let env = environment_source(&studio, None);
    assert_eq!(env.surface_glare().to_bits(), 0.0f32.to_bits());

    studio.surface_glare = 0.35;
    let env = environment_source(&studio, None);
    assert_eq!(env.surface_glare().to_bits(), 0.35f32.to_bits());

    studio.surface_glare = 1.0;
    let env = environment_source(&studio, None);
    assert_eq!(env.surface_glare().to_bits(), 1.0f32.to_bits());

    let bytes = unique_hdr(16, 8);
    let map = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap();
    studio.surface_glare = 0.0;
    let hdr = environment_source(&studio, Some(&map));
    assert_eq!(hdr.surface_glare().to_bits(), 1.0f32.to_bits());
}

/// The studio environment a worker builds carries the scene's head-shadow radius.
#[test]
fn environment_source_carries_the_scenes_head_shadow() {
    let mut studio = scene(SceneEnvironment::Studio);
    assert_eq!(studio.head_shadow_deg.to_bits(), 16.0f32.to_bits());
    assert_eq!(
        environment_source(&studio, None)
            .head_shadow_deg()
            .to_bits(),
        16.0f32.to_bits()
    );
    studio.head_shadow_deg = 0.0;
    assert_eq!(
        environment_source(&studio, None)
            .head_shadow_deg()
            .to_bits(),
        0.0f32.to_bits()
    );
    studio.head_shadow_deg = 24.0;
    assert_eq!(
        environment_source(&studio, None)
            .head_shadow_deg()
            .to_bits(),
        24.0f32.to_bits()
    );
}
