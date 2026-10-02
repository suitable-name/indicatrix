//! Shared fixtures for the coordinator tests: a throwaway PKI with role-specific client
//! bundles, a throwaway library database, a coordinator (`serve::start`) on ephemeral
//! loopback ports, TLS clients, and a CPU-only `join` setup. Nothing is mocked at the
//! `rustls` layer; no GPU is ever acquired.

use crate::{
    cli::{ComputeMode, ServeArgs},
    coordinator::JobConfig,
    join::WorkerSetup,
    serve::{self, ServeHandle},
};
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
    renderer::gpu_backend::GpuBackend,
};
use indicatrix_net::{
    SceneState,
    messages::{PeerRole, RenderCapability},
};
use rustls::pki_types::ServerName;
use std::{
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use super::super::LivenessConfig;

// A unique, freshly created temp directory, shared with `serve`'s tests.
pub use crate::serve::tests::fixtures::unique_temp_dir;

/// A CA plus a server certificate for `localhost`/`127.0.0.1`.
pub fn pki_with_server(label: &str) -> PathBuf {
    let dir = unique_temp_dir(&format!("{label}-pki"));
    crate::pki::init(&dir).unwrap();
    crate::pki::issue_server(
        &dir,
        &["localhost".to_string()],
        &["127.0.0.1".parse().unwrap()],
    )
    .unwrap();
    dir
}

/// A client bundle of `role` issued by hand (and allowlisted for that role).
pub fn bundle(pki: &Path, name: &str, role: PeerRole) -> PathBuf {
    let out = unique_temp_dir(&format!("bundle-{name}"));
    crate::pki::issue_client_with_role(pki, name, &out, role).unwrap();
    out
}

/// A fresh, empty library database file (tests never touch `facet_diagrams.sqlite`).
pub fn temp_db() -> PathBuf {
    let path = unique_temp_dir("db").join("library.sqlite");
    drop(indicatrix_vault::db::sqlite::Database::new(Some(path.to_str().unwrap())).unwrap());
    path
}

/// `serve` args for a coordinator on ephemeral loopback ports, TLS from `pki`, no own
/// render lane.
pub fn coordinator_args(pki: &Path) -> ServeArgs {
    // A viewer allowlist must exist for `serve` to start.
    let viewer_allowlist = crate::pki::viewer_allowlist_in(pki);
    if !viewer_allowlist.exists() {
        std::fs::write(&viewer_allowlist, "").unwrap();
    }
    ServeArgs {
        bind: "127.0.0.1:0".to_string(),
        threads: 2,
        allow_remote: false,
        ca: Some(pki.join(crate::pki::CA_CERT_FILE)),
        cert: Some(pki.join(crate::pki::SERVER_CERT_FILE)),
        key: Some(pki.join(crate::pki::SERVER_KEY_FILE)),
        allowlist: None,
        trust_any_client_cert: false,
        insecure_no_tls: false,
        compute_mode: ComputeMode::Hybrid,
        render: false,
        enroll_bind: Some("127.0.0.1:0".to_string()),
        no_enroll: false,
        worker_bind: Some("127.0.0.1:0".to_string()),
        worker_enroll_bind: Some("127.0.0.1:0".to_string()),
        no_workers: false,
        worker_allowlist: None,
        db: Some(temp_db()),
        max_connections: 16,
        max_preauth_per_ip: crate::cli::DEFAULT_MAX_PREAUTH_PER_IP,
        interactive_workers: 0,
        pin_interactive_worker: None,
        max_job_memory_mib: crate::cli::DEFAULT_MAX_JOB_MEMORY_MIB,
        whole_image_secs: crate::cli::DEFAULT_WHOLE_IMAGE_SECS,
        whole_image_pixel_samples: crate::cli::DEFAULT_WHOLE_IMAGE_PIXEL_SAMPLES,
        jobs_per_viewer: crate::cli::DEFAULT_JOBS_PER_VIEWER,
    }
}

/// The job config of a test coordinator that splits every request over all its lanes:
/// no request is little enough to render whole (see `JobConfig::whole_image_secs`), so a
/// test of the fan-out sees the fan-out. The whole-image tests set
/// [`JobConfig::default`] instead.
pub fn fan_out_config() -> JobConfig {
    JobConfig {
        whole_image_secs: 0.0,
        whole_image_pixel_samples: 0,
        ..JobConfig::default()
    }
}

/// Liveness fast enough for a test: ping after 100 ms idle, drop after 2 s silent.
///
/// `dead_after` was 600 ms; raised so a test run under load (a slow CI box, several
/// tests' threads contending for CPU) has real margin before a merely-slow-to-answer
/// PONG reads as "dead".
pub const FAST_LIVENESS: LivenessConfig = LivenessConfig {
    ping_interval: Duration::from_millis(100),
    dead_after: Duration::from_secs(2),
    tick: Duration::from_millis(20),
};

/// Starts a coordinator with `args` and `liveness`, splitting every request over all its
/// lanes ([`fan_out_config`]).
pub fn start(args: &ServeArgs, liveness: LivenessConfig) -> ServeHandle {
    let handle = serve::start_with_liveness(args, liveness).unwrap();
    if let Some(coordinator) = &handle.coordinator {
        coordinator.set_job_config(fan_out_config());
    }
    handle
}

/// A mutual-TLS client to `addr` presenting the bundle in `bundle_dir`.
pub fn tls_client(
    addr: SocketAddr,
    bundle_dir: &Path,
) -> rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    let ca = indicatrix_net::tls::load_ca(&bundle_dir.join(crate::pki::CA_CERT_FILE)).unwrap();
    let certs =
        indicatrix_net::tls::load_certs(&bundle_dir.join(crate::pki::CLIENT_CERT_FILE)).unwrap();
    let key = indicatrix_net::tls::load_private_key(&bundle_dir.join(crate::pki::CLIENT_KEY_FILE))
        .unwrap();
    let config = indicatrix_net::tls::client_config(ca, certs, key).unwrap();
    let tcp = TcpStream::connect(addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let conn = rustls::ClientConnection::new(
        config,
        ServerName::from(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)),
    )
    .unwrap();
    rustls::StreamOwned::new(conn, tcp)
}

/// A CPU-only worker (2 threads, GPU disabled) for `join_once`, with its own fresh HDR
/// asset cache (so it advertises `hdr`), like a production `join`.
pub fn cpu_worker_setup() -> Arc<WorkerSetup> {
    cpu_worker_setup_with_cache(true)
}

/// [`cpu_worker_setup`], with (`cache`) or without an HDR asset cache -- without one
/// the worker advertises `hdr: false`, like a `join` whose cache failed to open.
pub fn cpu_worker_setup_with_cache(cache: bool) -> Arc<WorkerSetup> {
    let gpu = Arc::new(GpuBackend::disabled());
    let assets = cache.then(|| {
        let dir = unique_temp_dir("worker-assets");
        Arc::new(crate::assets::AssetCache::open(&dir, 1 << 26).unwrap())
    });
    let capability = RenderCapability {
        hdr: assets.is_some(),
        ..serve::local_render_capability(&gpu, 2)
    };
    Arc::new(WorkerSetup {
        gpu,
        threads: 2,
        compute_mode: ComputeMode::OnlyCpu,
        capability,
        assets,
    })
}

/// Polls `condition` every 10 ms until it holds or `timeout` passes.
pub fn wait_for(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    condition()
}

/// A 4x4 diamond scene -- cheap enough to trace a few samples in a test.
pub fn tiny_scene() -> SceneState {
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
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
    }
}
