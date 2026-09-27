//! The VIEWER listener's accept loop: per-socket tuning, the two connection caps, the
//! transport (mutual TLS or `--insecure-no-tls`), and one thread per connection running
//! the viewer handler. See `crate::serve`'s "Robustness" section for why the caps are
//! acquired in this order.

use super::{
    ConnectionLimiter, ConnectionSlot,
    connection::{self, report_connection_result},
    socket::{apply_handshake_timeout, open_connection_database, tune_accepted_socket},
    tls::{Transport, accept_tls},
};
use indicatrix_net::messages::PeerRole;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::Arc,
    thread,
};

/// Everything the viewer listener's connection threads share, built once by
/// [`super::start`] and handed to every thread behind one `Arc` -- one argument instead
/// of a positional parameter per setting.
pub struct ViewerListener {
    /// Mutual TLS (with the viewer [`super::tls::Auth`]) or plaintext.
    pub transport: Transport,
    /// The real `--max-connections` cap for authenticated viewer connections.
    pub limiter: ConnectionLimiter,
    /// The wider cap on bare, not-yet-authenticated connections (see
    /// [`super::limiter::PRE_AUTH_HANDSHAKE_MULTIPLIER`]).
    pub handshake_limiter: ConnectionLimiter,
    /// The library database every connection opens its own read-only handle from.
    pub db_path: PathBuf,
    /// `--max-connections`, for the refusal message.
    pub max_connections: usize,
    /// The render side of a `worker` build.
    #[cfg(feature = "worker")]
    pub render: RenderSetup,
}

/// The render side of the viewer listener (`worker` builds): the own lane (`--render`) and the
/// coordinator's joined-worker registry.
#[cfg(feature = "worker")]
pub struct RenderSetup {
    /// CPU tracer threads for the own lane.
    pub threads: usize,
    /// The process's GPU backend -- [`indicatrix::renderer::gpu_backend::GpuBackend::disabled`]
    /// unless `--render` without `--only-cpu`.
    pub gpu: Arc<indicatrix::renderer::gpu_backend::GpuBackend>,
    /// `--only-gpu`/`--only-cpu`/hybrid.
    pub compute_mode: crate::cli::ComputeMode,
    /// `serve --render`.
    pub own_lane: bool,
    /// The joined-worker registry (`None` with `--no-workers` or `--insecure-no-tls`).
    pub registry: Option<Arc<crate::coordinator::Registry>>,
    /// Request execution over the own lane and joined workers.
    pub coordinator: Arc<crate::coordinator::Coordinator>,
    /// The HDR asset cache (v14): opened with `--render` or a worker port (the
    /// coordinator holds HDR jobs' maps through it), `None` otherwise (and when
    /// it cannot be opened), in which case HDR scenes are refused.
    pub assets: Option<Arc<crate::assets::AssetCache>>,
}

/// What TLS established about a viewer: its certificate's role and the certificate
/// itself (both `None` without TLS).
type PeerAuth = (
    Option<PeerRole>,
    Option<rustls::pki_types::CertificateDer<'static>>,
);

/// The identity a viewer's jobs queue under (one active job per viewer certificate):
/// the client certificate's SHA-256 fingerprint, or -- without TLS -- the peer's IP
/// address.
#[cfg(feature = "worker")]
fn viewer_key(
    certificate: Option<&rustls::pki_types::CertificateDer<'_>>,
    peer: Option<SocketAddr>,
) -> String {
    certificate.map_or_else(
        || {
            format!(
                "address:{}",
                peer.map_or_else(|| "unknown".to_string(), |p| p.ip().to_string())
            )
        },
        |cert| {
            format!(
                "certificate:{}",
                indicatrix_net::tls::fingerprint_to_hex(&indicatrix_net::tls::fingerprint(cert))
            )
        },
    )
}

/// Accepts viewer connections on `listener` forever, one thread per connection.
pub fn run_accept_loop(listener: &TcpListener, shared: &Arc<ViewerListener>) {
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let peer = stream.peer_addr().ok();
                tune_accepted_socket(&stream, peer);
                apply_handshake_timeout(&stream, peer);

                // Bounds bare, not-yet-authenticated connections regardless of
                // transport -- checked before spawning a thread at all.
                let Ok(handshake_slot) = shared.handshake_limiter.try_acquire() else {
                    tracing::warn!(
                        "connection {peer:?}: refusing -- too many not-yet-authenticated connections in \
                         flight (see --max-connections)"
                    );
                    continue;
                };

                // `Transport::Insecure` (loopback only) has no authentication step to wait
                // for, so the REAL slot is reserved right here; for `Transport::Tls` it is
                // acquired inside the spawned thread, only after `accept_tls` succeeds.
                let pre_acquired_slot = matches!(shared.transport, Transport::Insecure)
                    .then(|| shared.limiter.try_acquire());

                spawn_connection_handler(stream, peer, handshake_slot, pre_acquired_slot, shared);
            }
            Err(e) => tracing::warn!("accept error: {e}"),
        }
    }
}

/// Dispatches one just-accepted `stream` on the listener's transport and spawns its own
/// thread running the viewer handler.
///
/// `handshake_slot` is held for the whole spawned thread's lifetime regardless of
/// transport or outcome, bounding bare connections in flight. `pre_acquired_slot` is
/// the REAL cap's verdict, decided in the accept loop -- but only for
/// `Transport::Insecure` (`Some`). For `Transport::Tls` (`None` here), the real cap is
/// acquired INSIDE the thread, only after [`accept_tls`] itself has succeeded (handshake
/// AND allowlist): reserving it any earlier would let a burst of bare TCP connects hold
/// every real slot for up to `HANDSHAKE_TIMEOUT`, locking out certificate-holding
/// viewers. `Err(active)` goes to [`connection::refuse_for_capacity`] instead.
fn spawn_connection_handler(
    stream: TcpStream,
    peer: Option<SocketAddr>,
    handshake_slot: ConnectionSlot,
    pre_acquired_slot: Option<Result<ConnectionSlot, usize>>,
    shared_ref: &Arc<ViewerListener>,
) {
    let shared = Arc::clone(shared_ref);
    match &shared_ref.transport {
        Transport::Insecure => {
            tracing::warn!(
                "--insecure-no-tls: accepted PLAINTEXT connection from {peer:?} -- no TLS, no \
                 authentication"
            );
            let slot = pre_acquired_slot.expect(
                "the accept loop always pre-acquires the real slot for Transport::Insecure",
            );
            thread::spawn(move || {
                let _handshake_slot = handshake_slot; // held for this thread's whole lifetime
                let mut stream = stream;
                match slot {
                    // `_slot` releases this connection's counted slot when the arm ends.
                    Ok(_slot) => serve_accepted(stream, peer, (None, None), &shared),
                    Err(active) => connection::refuse_for_capacity(
                        &mut stream,
                        peer,
                        active,
                        shared.max_connections,
                    ),
                }
            });
        }
        Transport::Tls { config, auth } => {
            let config = Arc::clone(config);
            let auth = auth.clone();
            thread::spawn(move || {
                let _handshake_slot = handshake_slot; // held for this thread's whole lifetime
                let Some((mut tls_stream, cert_role)) = accept_tls(stream, &config, &auth, peer)
                else {
                    return; // accept_tls already logged why
                };
                // The real slot is acquired only now that accept_tls has succeeded.
                let certificate = tls_stream
                    .conn
                    .peer_certificates()
                    .and_then(<[_]>::first)
                    .map(|c| c.clone().into_owned());
                match shared.limiter.try_acquire() {
                    Ok(_slot) => {
                        serve_accepted(tls_stream, peer, (Some(cert_role), certificate), &shared);
                    }
                    Err(active) => connection::refuse_for_capacity(
                        &mut tls_stream,
                        peer,
                        active,
                        shared.max_connections,
                    ),
                }
            });
        }
    }
}

/// Opens the connection's database and runs the viewer handler inside `catch_unwind`
/// (`worker` builds: the render-capable handler with the coordinator's `WELCOME`
/// advertisement).
#[cfg(feature = "worker")]
fn serve_accepted<S>(
    stream: S,
    peer: Option<SocketAddr>,
    (cert_role, certificate): PeerAuth,
    shared: &ViewerListener,
) where
    S: Read + Write + crate::stream_emit::TimeoutRead + crate::stream_emit::TimeoutWrite,
{
    let Some(db) = open_connection_database(&shared.db_path, peer) else {
        return;
    };
    let render = &shared.render;
    let ctx = connection::ViewerContext {
        threads: render.threads,
        gpu: &render.gpu,
        db: &db,
        compute_mode: render.compute_mode,
        encodings: super::socket::payload_preference_for(peer),
        own_lane: render.own_lane,
        registry: render.registry.as_ref(),
        cert_role,
        coordinator: Some((&render.coordinator, viewer_key(certificate.as_ref(), peer))),
        assets: render.assets.as_deref(),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        connection::handle_viewer_connection(stream, &ctx)
    }));
    report_connection_result(peer, result);
}

/// Library-only counterpart of the `worker` build's `serve_accepted`.
#[cfg(not(feature = "worker"))]
fn serve_accepted<S: Read + Write + connection::ClearHandshakeTimeout>(
    stream: S,
    peer: Option<SocketAddr>,
    (cert_role, _certificate): PeerAuth,
    shared: &ViewerListener,
) {
    let Some(db) = open_connection_database(&shared.db_path, peer) else {
        return;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        connection::handle_connection(stream, &db, cert_role)
    }));
    report_connection_result(peer, result);
}
