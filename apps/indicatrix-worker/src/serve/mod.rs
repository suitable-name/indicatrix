//! The `serve` subcommand -- the coordinator (renders itself only with `--render`;
//! viewers and joining workers on separate ports).
//!
//! Accepts `indicatrix_net` viewer connections over TCP and replies, and (on a `worker`
//! build) accepts joining render workers on a separate worker port.
//!
//! # What `serve` does
//!
//! - **Always** serves the read-only design-library protocol ([`library`], backed by
//!   [`indicatrix_vault`]) to viewers on `--bind` (default 7878).
//! - **With `--render`** (implied by `--only-gpu`/`--only-cpu`; `worker` builds only)
//!   renders viewers' `RenderRequest`s with its own CPU/GPU lane, exactly as a plain
//!   worker did before coordinator mode. Without it no GPU is acquired and no sample is
//!   traced here -- **`serve` no longer renders by default.**
//! - **On `worker` builds** opens the worker port (`--worker-bind`, default 7880) where
//!   `indicatrix-worker join` workers dial in and register in the coordinator's
//!   [`crate::coordinator::Registry`]; a viewer's request is then executed over them
//!   (and the own lane) by [`crate::coordinator::serve_render`] and its siblings.
//!
//! `WELCOME.render` follows [`crate::coordinator::viewer_render_capability`]: `None`
//! without `--render` and without joined workers (a library-only remote to the GUI),
//! the own lane's plain `Cpu`/`Gpu` backend with `--render` alone, and
//! `Backend::Coordinator{workers, threads, gpus}` as soon as a worker has joined.
//!
//! # Ports and trust
//!
//! Viewers use 7878 plus the viewer enrollment listener 7879; workers use 7880 plus the
//! worker enrollment listener 7881 ([`crate::enroll`]). Each TLS listener checks its own
//! client-certificate role and its own allowlist (`allowlist-viewers.txt`,
//! `allowlist-workers.txt`; see `crate::pki::role` and [`tls`]); the `HELLO` gate
//! ([`handshake`]) refuses a mismatched role with `ROLE_REFUSED`.
//!
//! **The library protocol sits behind the exact same mutual-TLS authentication as the
//! render protocol -- there is no separate check.** Both are message variants read off
//! the same already-authenticated stream.
//!
//! # TLS
//!
//! Connection handlers are generic over `Read + Write` and never name `TcpStream`
//! directly; only [`tls::accept_tls`] wraps an accepted socket in
//! `rustls::StreamOwned<ServerConnection, TcpStream>`. Mutual TLS answers "may this peer
//! talk to me at all", a different question from
//! `indicatrix_net::handshake::verify_compatible`'s physics-compatibility check. TLS
//! validates the certificate chain only; the role and allowlist check in
//! [`tls::accept_tls`] is the actual authorization decision (no CRL/OCSP, just a
//! fingerprint list). `--trust-any-client-cert` skips the allowlists (never the role
//! check) and must be an explicit flag. `--insecure-no-tls` skips TLS entirely, is
//! refused on a non-loopback `--bind`, disables the worker port and enrollment, and
//! every such connection logs a warning.
//!
//! # Robustness
//!
//! - Every `RenderRequest`/`LibraryRequest` is validated defensively; failures get an
//!   `Error`/`NotFound` reply, not a dropped connection.
//! - Tracing runs inside `catch_unwind` (worker builds); the accept loop wraps the
//!   whole per-connection handler in a second `catch_unwind` as backstop.
//! - Every listener binds to loopback only unless `--allow-remote` is passed,
//!   independent of TLS.
//! - An authenticated client can still ask for an absurd render or search:
//!   `crate::validate`'s caps bound the former, `Database::search_diagrams`'s 1000-row
//!   cap bounds the latter unconditionally.
//! - Every accepted connection gets `TCP_NODELAY` and TCP keepalive best-effort (see
//!   [`tune_accepted_socket`]), and [`HANDSHAKE_TIMEOUT`] until its first `HELLO`
//!   (slowloris), cleared by the connection handler once `HELLO` arrives.
//! - `--max-connections` (default 64, see [`ConnectionLimiter`]) bounds AUTHENTICATED
//!   connections, counted separately for viewers and joined workers. A connection over
//!   the cap is still accepted and given a definitive `<- ERROR` refusal
//!   ([`connection::refuse_for_capacity`]). For TLS the real slot is acquired only AFTER
//!   [`tls::accept_tls`] succeeds; a second, wider limiter (see
//!   [`limiter::PRE_AUTH_HANDSHAKE_MULTIPLIER`]) bounds bare, not-yet-authenticated
//!   connections in flight, refused silently. A per-source-address cap on those
//!   (`--max-preauth-per-ip`, default 8, checked first so one address cannot take the
//!   wider cap's slots) closes an over-cap connection at once with a logged reason.
//!   Both pre-authentication counts are released as soon as [`tls::accept_tls`]
//!   succeeds, so an authenticated connection counts only against `--max-connections`.

use std::{
    net::{SocketAddr, TcpListener},
    sync::Arc,
    thread,
};

#[cfg(feature = "worker")]
use crate::{cli::ComputeMode, coordinator};
use crate::{cli::ServeArgs, enroll};
use indicatrix_net::messages::PeerRole;

mod accept;
mod connection;
mod handshake;
pub mod library;
mod limiter;
mod socket;
#[cfg(feature = "worker")]
mod tilt;
mod tls;
// These tests drive a real render round trip, uncompilable without `render` support.
// `library`'s own tests cover the library protocol unconditionally.
// `pub(crate)` so other test modules (the coordinator's) share its temp-dir helper.
#[cfg(all(test, feature = "worker"))]
pub(crate) mod tests;

#[cfg(feature = "worker")]
pub(crate) use connection::refuse_for_capacity;
#[cfg(feature = "worker")]
pub(crate) use limiter::PRE_AUTH_HANDSHAKE_MULTIPLIER;
pub use limiter::{ConnectionLimiter, ConnectionSlot};
pub(crate) use socket::HANDSHAKE_TIMEOUT;
// Ungated (unlike the `worker`-only re-exports below): `crate::enroll`'s listener runs
// in every build, and needs these two to apply the same pre-HELLO deadline (F-06a) to
// its own TLS-handshake-then-one-message exchange.
pub(crate) use socket::{DeadlineIo, DeadlineSocket};
#[cfg(feature = "worker")]
pub(crate) use socket::{apply_handshake_timeout, tune_accepted_socket};
pub(crate) use tls::Auth;
#[cfg(feature = "worker")]
pub(crate) use tls::{TlsStream, accept_tls};

#[cfg(not(feature = "worker"))]
pub use connection::handle_connection;
#[cfg(feature = "worker")]
pub(crate) use connection::{
    LinkSettings, RequestContext, is_loopback_peer, local_render_capability,
    refuse_incompatible_handshake, serve_requests,
};
#[cfg(feature = "worker")]
pub use connection::{handle_connection, handle_connection_with_gpu};
#[cfg(feature = "worker")]
pub(crate) use handshake::{HelloCheck, read_and_check_hello, send_refusal};
#[cfg(feature = "worker")]
pub(crate) use socket::payload_preference_for;
#[cfg(feature = "worker")]
pub(crate) use tilt::{CancelPoll, handle_tilt_curves_request, poll_for_cancel};

/// What [`start`] brought up.
///
/// Every listener's actual bound address (useful with port `0`) and, on `worker`
/// builds, the coordinator's joined-worker registry. The listeners run on background
/// threads; [`Self::wait`] blocks on the viewer accept loop.
pub struct ServeHandle {
    /// The viewer listener (`--bind`).
    pub viewer_addr: SocketAddr,
    /// The viewer enrollment listener, if started.
    pub viewer_enroll_addr: Option<SocketAddr>,
    /// The worker port, if opened (`worker` builds with TLS and without `--no-workers`).
    pub worker_addr: Option<SocketAddr>,
    /// The worker enrollment listener, if started.
    pub worker_enroll_addr: Option<SocketAddr>,
    /// The joined-worker registry, present exactly when [`Self::worker_addr`] is.
    #[cfg(feature = "worker")]
    pub registry: Option<Arc<coordinator::Registry>>,
    /// The coordinator's job execution state (always present on `worker` builds).
    #[cfg(feature = "worker")]
    pub coordinator: Option<Arc<coordinator::Coordinator>>,
    accept_thread: thread::JoinHandle<()>,
}

impl ServeHandle {
    /// Blocks until the viewer accept loop ends (in practice: forever).
    pub fn wait(self) {
        if self.accept_thread.join().is_err() {
            tracing::error!("indicatrix-worker serve: the viewer accept loop panicked");
        }
    }
}

/// Runs the `serve` subcommand: [`start`], then serves forever.
///
/// # Errors
///
/// Whatever [`start`] returns.
pub fn run(args: &ServeArgs) -> Result<(), String> {
    start(args)?.wait();
    Ok(())
}

/// Brings up every listener `args` asks for and returns without blocking.
///
/// Refuses a non-loopback bind address (viewer, worker or enrollment) unless
/// `args.allow_remote` is set, independent of TLS. Builds the mutual-TLS server config
/// from `args.ca`/`args.cert`/`args.key` (required unless `args.insecure_no_tls`),
/// pre-loading the viewer allowlist to fail fast on a bad path (the per-connection check
/// re-reads it every time). Preflights the library database once. `worker` builds
/// acquire this process's `GpuBackend` only for `--render` without `--only-cpu`; open
/// the worker port with its own limiter, registry and liveness thread; and start the
/// worker enrollment listener.
///
/// # Errors
///
/// A human-readable message if an address doesn't parse or is non-loopback without
/// `--allow-remote`, if `--insecure-no-tls` is combined with a non-loopback bind, if TLS
/// is enabled but `--ca`/`--cert`/`--key` are missing or unloadable, if an allowlist
/// can't be loaded, if the design-library database can't be opened, or if a bind fails.
pub fn start(args: &ServeArgs) -> Result<ServeHandle, String> {
    start_inner(
        args,
        #[cfg(feature = "worker")]
        coordinator::LivenessConfig::default(),
    )
}

/// [`start`] with a custom joined-worker liveness schedule -- for tests that need a
/// silent worker dropped in milliseconds rather than 30 s.
///
/// # Errors
///
/// See [`start`].
#[cfg(all(test, feature = "worker"))]
pub(crate) fn start_with_liveness(
    args: &ServeArgs,
    liveness: coordinator::LivenessConfig,
) -> Result<ServeHandle, String> {
    start_inner(args, liveness)
}

fn start_inner(
    args: &ServeArgs,
    #[cfg(feature = "worker")] liveness: coordinator::LivenessConfig,
) -> Result<ServeHandle, String> {
    let bind_addr = checked_addr(&args.bind, "--bind", args.allow_remote)?;
    let transport = tls::build_transport(args, bind_addr)?;
    let db_path = socket::resolve_library_db_path(args);
    // Fail fast on a bad `--db` before binding a socket; each connection opens its own on
    // its first library request.
    drop(socket::open_library_database(&db_path)?);
    let limiter = ConnectionLimiter::new(args.max_connections);
    let handshake_limiter = ConnectionLimiter::new(
        args.max_connections
            .saturating_mul(limiter::PRE_AUTH_HANDSHAKE_MULTIPLIER),
    );

    let listener =
        TcpListener::bind(bind_addr).map_err(|e| format!("failed to bind {bind_addr}: {e}"))?;
    let viewer_addr = listener
        .local_addr()
        .map_err(|e| format!("viewer listener bound but local_addr() failed: {e}"))?;
    tracing::info!(
        "indicatrix-worker serve: viewers on {viewer_addr} ({}; {})",
        transport_summary(&transport),
        capability_summary(args),
    );

    let viewer_auth = match &transport {
        tls::Transport::Tls { auth, .. } => Some(auth.clone()),
        tls::Transport::Insecure => None,
    };
    let viewer_enroll_addr = start_enrollment(
        args,
        args.enroll_bind.as_deref(),
        bind_addr,
        1,
        viewer_auth.as_ref(),
    )?;

    #[cfg(feature = "worker")]
    let (worker, render) = start_worker_side(args, bind_addr, &transport, liveness, &db_path)?;
    #[cfg(not(feature = "worker"))]
    let worker = WorkerSide::default();

    let shared = Arc::new(accept::ViewerListener {
        transport,
        limiter,
        handshake_limiter,
        pre_auth_addresses: accept::PreAuthAddresses::new(args.max_preauth_per_ip),
        db_path,
        max_connections: args.max_connections,
        #[cfg(feature = "worker")]
        render,
    });
    let accept_thread = thread::spawn(move || accept::run_accept_loop(&listener, &shared));

    Ok(ServeHandle {
        viewer_addr,
        viewer_enroll_addr,
        worker_addr: worker.addr,
        worker_enroll_addr: worker.enroll_addr,
        #[cfg(feature = "worker")]
        registry: worker.registry,
        #[cfg(feature = "worker")]
        coordinator: worker.coordinator,
        accept_thread,
    })
}

/// The worker port's part of [`ServeHandle`].
#[derive(Default)]
struct WorkerSide {
    addr: Option<SocketAddr>,
    enroll_addr: Option<SocketAddr>,
    #[cfg(feature = "worker")]
    registry: Option<Arc<coordinator::Registry>>,
    #[cfg(feature = "worker")]
    coordinator: Option<Arc<coordinator::Coordinator>>,
}

/// `worker` builds: acquires the own lane's GPU (only for `--render`), and -- unless
/// `--no-workers` or `--insecure-no-tls` -- opens the worker port with its own limiter,
/// the registry and its liveness thread, plus the worker enrollment listener.
#[cfg(feature = "worker")]
fn start_worker_side(
    args: &ServeArgs,
    bind_addr: SocketAddr,
    transport: &tls::Transport,
    liveness: coordinator::LivenessConfig,
    db_path: &std::path::Path,
) -> Result<(WorkerSide, accept::RenderSetup), String> {
    use indicatrix::renderer::gpu_backend::GpuBackend;
    // Acquired once. A device lost later is re-acquired by the backend itself (cool-down
    // and hourly attempt budget in `indicatrix::renderer::gpu_backend`), and each
    // connection's `WELCOME` reads the backend's state when it is built
    // (`local_render_capability`), so nothing here re-acquires.
    let gpu = Arc::new(
        if args.render && args.compute_mode != ComputeMode::OnlyCpu {
            GpuBackend::acquire()
        } else {
            GpuBackend::disabled()
        },
    );
    // Makes the hybrid CPU/GPU split decision once, before the first real request,
    // rather than paying its 3-sample probe on whichever request happens to arrive
    // first. Only meaningful with a real adapter and the hybrid path (`OnlyGpu`/
    // `OnlyCpu` never calibrate a split at all -- see `render_core::hybrid::job_key`'s
    // doc comment).
    if args.render && args.compute_mode == ComputeMode::Hybrid && gpu.adapter_label().is_some() {
        crate::render_core::hybrid::calibrate_now(
            &gpu,
            &probe_scene(),
            args.threads,
            args.compute_mode,
        );
    }

    let mut side = WorkerSide::default();
    match transport {
        _ if args.no_workers => {
            tracing::info!("indicatrix-worker serve: --no-workers -- no worker port");
        }
        tls::Transport::Insecure => tracing::warn!(
            "indicatrix-worker serve: --insecure-no-tls -- no worker port (joining workers always need mutual TLS \
             and a worker certificate)"
        ),
        tls::Transport::Tls { config, .. } => {
            let worker_bind = derived_addr(
                args.worker_bind.as_deref(),
                bind_addr,
                crate::cli::WORKER_PORT_OFFSET,
                "--worker-bind",
                args.allow_remote,
            )?;
            let auth = tls::worker_auth(args)?;
            let worker_limiter = ConnectionLimiter::new(args.max_connections);
            let registry = coordinator::Registry::new(liveness);
            coordinator::Registry::spawn_liveness(&registry);
            let addr = coordinator::spawn_worker_listener(coordinator::WorkerListenerConfig {
                bind_addr: worker_bind,
                tls_config: Arc::clone(config),
                auth: auth.clone(),
                limiter: worker_limiter,
                max_connections: args.max_connections,
                registry: Arc::clone(&registry),
            })?;
            // Derived from the CONFIGURED worker address (an ephemeral port 0 stays 0).
            side.enroll_addr = start_enrollment(
                args,
                args.worker_enroll_bind.as_deref(),
                worker_bind,
                1,
                Some(&auth),
            )?;
            side.addr = Some(addr);
            side.registry = Some(registry);
        }
    }
    let own = args.render.then(|| coordinator::OwnLaneSetup {
        gpu: Arc::clone(&gpu),
        threads: args.threads,
        compute_mode: args.compute_mode,
    });
    // HDR maps: the own lane renders HDR scenes from this cache, and a coordinator with a
    // worker port holds every HDR job's map through it to forward it to joined workers
    // -- see `crate::assets` for its location and cap.
    let assets = if args.render || side.registry.is_some() {
        crate::assets::open_configured(db_path)
    } else {
        None
    };
    let coordinator = coordinator::Coordinator::new(
        side.registry.clone(),
        own,
        args.interactive_workers,
        u64::from(args.max_job_memory_mib) * 1024 * 1024,
    )
    .with_interactive_pin(args.pin_interactive_worker.clone())
    .with_small_pictures(
        args.whole_image_secs,
        args.whole_image_pixel_samples,
        args.jobs_per_viewer,
    )
    .with_assets(assets.clone());
    if let Some(pin) = coordinator.interactive_pin() {
        tracing::info!(
            "indicatrix-worker serve: live-view requests that take joined workers use worker {:?} \
             first while it is available (--pin-interactive-worker)",
            pin.label()
        );
    }
    let coordinator = Arc::new(coordinator);
    side.coordinator = Some(Arc::clone(&coordinator));
    let render = accept::RenderSetup {
        threads: args.threads,
        gpu,
        compute_mode: args.compute_mode,
        own_lane: args.render,
        registry: side.registry.clone(),
        coordinator,
        assets,
        payload: args.payload_encoding,
    };
    Ok((side, render))
}

/// A representative scene for [`crate::render_core::hybrid::calibrate_now`]'s start-up
/// probe: a real, traceable scene (so the probe measures genuine per-dispatch overhead,
/// not just call overhead) at a typical interactive/live-view resolution. Only its
/// resolution matters to the calibration decision (`render_core::hybrid::job_key`
/// buckets to the enclosing power of two), so the exact material/geometry/lighting
/// below are arbitrary.
#[cfg(feature = "worker")]
fn probe_scene() -> indicatrix_net::SceneState {
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    indicatrix_net::SceneState {
        width: 512,
        height: 512,
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
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
    }
}

/// Starts one token-enrollment listener (see [`crate::enroll`]) for `auth.role` at
/// `explicit`, else `base`'s host `offset` ports up. `None` (no listener) under
/// `--no-enroll` or `--insecure-no-tls` (no `auth`).
///
/// Builds its own dedicated pair of connection limiters (F-06b) rather than being
/// handed the matching viewer/worker listener's real `--max-connections` limiter --
/// see [`enroll::ENROLL_MAX_CONNECTIONS`]'s doc comment for why sharing it was a bug.
fn start_enrollment(
    args: &ServeArgs,
    explicit: Option<&str>,
    base: SocketAddr,
    offset: u16,
    auth: Option<&Auth>,
) -> Result<Option<SocketAddr>, String> {
    let Some(auth) = auth else {
        return Ok(None);
    };
    if args.no_enroll {
        return Ok(None);
    }
    let (Some(ca), Some(cert), Some(key)) = (&args.ca, &args.cert, &args.key) else {
        return Ok(None); // `tls::build_transport` already requires these.
    };
    let flag = match auth.role {
        PeerRole::Viewer => "--enroll-bind",
        PeerRole::Worker => "--worker-enroll-bind",
    };
    let bind = derived_addr(explicit, base, offset, flag, args.allow_remote)?;
    let config =
        enroll::EnrollConfig::build(bind, ca, cert, key, auth.allowlist.clone(), auth.role)?;
    enroll::spawn_enroll_listener(config).map(Some)
}

/// Parses `addr` for `flag` and refuses a non-loopback address without `--allow-remote`.
fn checked_addr(addr: &str, flag: &str, allow_remote: bool) -> Result<SocketAddr, String> {
    let parsed: SocketAddr = addr
        .parse()
        .map_err(|e| format!("invalid {flag} address {addr:?}: {e}"))?;
    if !parsed.ip().is_loopback() && !allow_remote {
        return Err(format!(
            "refusing to bind non-loopback address {parsed} ({flag}) without --allow-remote -- exposing this \
             server beyond localhost must be explicit (see --help)"
        ));
    }
    Ok(parsed)
}

/// `explicit` if given, else `base`'s host with the port `offset` higher (port `0` --
/// an ephemeral port -- stays `0`), then [`checked_addr`].
fn derived_addr(
    explicit: Option<&str>,
    base: SocketAddr,
    offset: u16,
    flag: &str,
    allow_remote: bool,
) -> Result<SocketAddr, String> {
    let addr = if let Some(explicit) = explicit {
        explicit.to_string()
    } else {
        let port = if base.port() == 0 {
            0
        } else {
            base.port().checked_add(offset).ok_or_else(|| {
                format!(
                    "{flag}: port {} + {offset} overflows; pass {flag} explicitly",
                    base.port()
                )
            })?
        };
        SocketAddr::new(base.ip(), port).to_string()
    };
    checked_addr(&addr, flag, allow_remote)
}

/// The transport half of the startup log line.
const fn transport_summary(transport: &tls::Transport) -> &'static str {
    match transport {
        tls::Transport::Tls {
            auth: Auth {
                allowlist: Some(_), ..
            },
            ..
        } => "mutual TLS, allowlist enforced",
        tls::Transport::Tls { .. } => "mutual TLS, ANY CA-signed client of the right role trusted",
        tls::Transport::Insecure => "PLAINTEXT, no TLS",
    }
}

/// One-line capability summary for the startup log line, matching what `WELCOME`
/// advertises before any worker joins.
#[cfg(feature = "worker")]
fn capability_summary(args: &ServeArgs) -> String {
    if !args.render {
        return "library + coordinator for joined workers; no own render lane (add --render to render here)"
            .to_string();
    }
    match args.compute_mode {
        ComputeMode::OnlyCpu => "library + render (CPU only, --only-cpu)".to_string(),
        ComputeMode::OnlyGpu => "library + render (GPU only, --only-gpu)".to_string(),
        ComputeMode::Hybrid => "library + render (--render)".to_string(),
    }
}

#[cfg(not(feature = "worker"))]
fn capability_summary(_args: &ServeArgs) -> String {
    "library only (no render capacity and no worker port -- build with --features worker to add them)"
        .to_string()
}
