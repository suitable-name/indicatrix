//! One `join` connection: dial the coordinator's worker port over mutual TLS, `HELLO`
//! as a worker, expect `WELCOME.registration`, then serve the coordinator's requests
//! with the same loop a viewer connection uses (`crate::serve::serve_requests`) until
//! the connection ends.

use crate::{
    assets::AssetCache,
    cli::ComputeMode,
    pki,
    serve::{self, RequestContext},
    stream_emit::{TimeoutRead, TimeoutWrite},
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::{
    client::{ClientError, handshake_with_hello},
    handshake,
    messages::{ErrorMsg, Hello, NetError, RenderCapability, error_codes},
};
use rustls::pki_types::ServerName;
use std::{
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    path::Path,
    sync::Arc,
    time::Duration,
};

/// How long a joined connection may sit idle between requests before reconnecting.
///
/// The coordinator pings idle connections every 10 s and drops them after 30 s of
/// silence, so 45 s of nothing means it is gone.
pub const JOIN_IDLE_TIMEOUT: Duration = Duration::from_secs(45);

/// How long one TCP connect attempt may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where to join and with which certificate: the coordinator's worker address and a
/// ready TLS client config built from a certificate bundle.
pub struct JoinTarget {
    /// The coordinator's worker port, `host:port`.
    pub addr: String,
    server_name: ServerName<'static>,
    client_config: Arc<rustls::ClientConfig>,
}

impl JoinTarget {
    /// Loads the bundle in `cert_dir` (`ca.pem`, `client.pem`, `client.key`) for
    /// dialling `addr`. Does not check the certificate's role -- see [`bundle_role`].
    ///
    /// # Errors
    ///
    /// A human-readable message if `addr` isn't `host:port` or a bundle file can't be
    /// loaded.
    pub fn from_bundle(addr: &str, cert_dir: &Path) -> Result<Self, String> {
        let (host, _port) = addr
            .rsplit_once(':')
            .ok_or_else(|| format!("coordinator address {addr:?} must be host:port"))?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let server_name = ServerName::try_from(host.to_string())
            .map_err(|e| format!("coordinator address {addr:?}: invalid host for TLS: {e}"))?;
        let ca_path = cert_dir.join(pki::CA_CERT_FILE);
        let cert_path = cert_dir.join(pki::CLIENT_CERT_FILE);
        let key_path = cert_dir.join(pki::CLIENT_KEY_FILE);
        let ca = indicatrix_net::tls::load_ca(&ca_path).map_err(|e| e.to_string())?;
        let certs = indicatrix_net::tls::load_certs(&cert_path).map_err(|e| e.to_string())?;
        let key = indicatrix_net::tls::load_private_key(&key_path).map_err(|e| e.to_string())?;
        let client_config = indicatrix_net::tls::client_config(ca, certs, key)
            .map_err(|e| format!("failed to build the TLS client config: {e}"))?;
        Ok(Self {
            addr: addr.to_string(),
            server_name,
            client_config,
        })
    }
}

/// The role of the client certificate in `cert_dir` (`crate::pki::role`).
///
/// # Errors
///
/// A human-readable message if the certificate can't be loaded or parsed.
pub fn bundle_role(cert_dir: &Path) -> Result<indicatrix_net::messages::PeerRole, String> {
    let certs = indicatrix_net::tls::load_certs(&cert_dir.join(pki::CLIENT_CERT_FILE))
        .map_err(|e| e.to_string())?;
    pki::role_of_certificate(&certs[0])
}

/// What this worker contributes: its render engine and the capability it reports.
pub struct WorkerSetup {
    /// The process's GPU backend (shared by every slot; FIFO turns).
    pub gpu: Arc<GpuBackend>,
    /// CPU tracer threads (`0` = all cores).
    pub threads: usize,
    /// `--only-gpu`/`--only-cpu`/hybrid.
    pub compute_mode: ComputeMode,
    /// What the `HELLO` reports ([`serve::local_render_capability`]); `hdr` exactly when
    /// [`Self::assets`] is `Some`.
    pub capability: RenderCapability,
    /// The HDR asset cache (shared by every slot): HDR scenes are resolved
    /// through it, missing maps asked of the coordinator. `None` (it could not be
    /// opened) refuses HDR scenes -- and the `HELLO` then says `hdr: false`, so the
    /// coordinator never sends any.
    pub assets: Option<Arc<AssetCache>>,
}

/// Why a join attempt did not get as far as registering.
#[derive(Debug)]
pub enum JoinError {
    /// No TCP connection (DNS, refused, unreachable, timeout).
    Connect(String),
    /// The TLS handshake failed (wrong CA, SAN, a certificate the port rejected).
    Tls(String),
    /// The coordinator refused the `HELLO` in place of `WELCOME` (role, build, capacity).
    Refused(ErrorMsg),
    /// Any other handshake failure (malformed reply, build mismatch found locally).
    Handshake(String),
    /// A `WELCOME` without a registration: the peer is not a coordinator's worker port.
    NotACoordinator,
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "could not connect: {e}"),
            Self::Tls(e) => write!(f, "TLS handshake failed: {e}"),
            Self::Refused(e) if e.code == error_codes::ROLE_REFUSED => write!(
                f,
                "refused by the coordinator (ROLE_REFUSED): {} -- `join` needs a WORKER certificate on the \
                 coordinator's WORKER port",
                e.message
            ),
            Self::Refused(e) => write!(
                f,
                "refused by the coordinator (code {}): {}",
                e.code, e.message
            ),
            Self::Handshake(e) => write!(f, "handshake failed: {e}"),
            Self::NotACoordinator => write!(
                f,
                "the peer answered WELCOME without a worker registration -- that is a viewer port or a plain \
                 worker, not a coordinator's worker port (default 7880)"
            ),
        }
    }
}

impl std::error::Error for JoinError {}

/// A registered session that has ended: which id the coordinator had assigned, and how
/// the request loop finished (`Ok` for a clean close by the coordinator).
#[derive(Debug)]
pub struct Session {
    /// `WELCOME.registration.worker_id`.
    pub worker_id: u32,
    /// The request loop's result.
    pub ended: Result<(), NetError>,
}

/// One connection's whole life: dial, register, serve until it ends.
///
/// # Errors
///
/// A [`JoinError`] if the connection never registered.
pub fn join_once(target: &JoinTarget, setup: &WorkerSetup) -> Result<Session, JoinError> {
    let mut tls = dial(target)?;
    let hello = handshake::local_worker_hello(setup.capability.clone());
    let welcome = handshake_with_hello(&mut tls, &hello).map_err(|e| match e {
        ClientError::Refused(refusal) => JoinError::Refused(refusal),
        other => JoinError::Handshake(other.to_string()),
    })?;
    let registration = welcome.registration.ok_or(JoinError::NotACoordinator)?;
    // `handshake_with_hello` only re-checks build compatibility when WELCOME advertises
    // render capacity, which a coordinator's worker WELCOME never does -- check here.
    let remote = Hello::viewer(
        welcome.protocol_version,
        welcome.build_hash,
        welcome.source_hash,
    );
    handshake::verify_compatible(&hello, &remote)
        .map_err(|e| JoinError::Handshake(format!("refusing to serve this coordinator: {e}")))?;
    tracing::info!(
        "indicatrix-worker join: joined coordinator {} as worker #{} (payload encoding {:?})",
        target.addr,
        registration.worker_id,
        welcome.payload_encoding
    );

    // Back to "blocking" -- which on this stream means the idle deadline (see
    // `IdleDeadlineStream`).
    let _ = tls.set_read_timeout(None);
    let _ = tls.set_write_timeout(None);
    let ended = serve::serve_requests(
        &mut tls,
        &RequestContext {
            threads: setup.threads,
            gpu: &setup.gpu,
            db: None,
            compute_mode: setup.compute_mode,
            payload_encoding: welcome.payload_encoding,
            own_lane: true,
            session: None,
            // The coordinator answers this worker's `NEED_ASSET`.
            assets: setup.assets.as_deref(),
        },
    );
    Ok(Session {
        worker_id: registration.worker_id,
        ended,
    })
}

/// A TLS client stream to the coordinator.
type JoinStream = rustls::StreamOwned<rustls::ClientConnection, IdleDeadlineStream>;

/// TCP connect (first address that answers), socket tuning, the handshake deadline,
/// and the TLS handshake.
fn dial(target: &JoinTarget) -> Result<JoinStream, JoinError> {
    let addrs: Vec<SocketAddr> = target
        .addr
        .to_socket_addrs()
        .map_err(|e| JoinError::Connect(format!("{}: {e}", target.addr)))?
        .collect();
    let mut last_error = format!("{}: no address", target.addr);
    let tcp = addrs
        .iter()
        .find_map(|addr| {
            TcpStream::connect_timeout(addr, CONNECT_TIMEOUT)
                .map_err(|e| last_error = format!("{addr}: {e}"))
                .ok()
        })
        .ok_or_else(|| JoinError::Connect(last_error.clone()))?;
    let peer = tcp.peer_addr().ok();
    serve::tune_accepted_socket(&tcp, peer);
    serve::apply_handshake_timeout(&tcp, peer);

    let conn = rustls::ClientConnection::new(
        Arc::clone(&target.client_config),
        target.server_name.clone(),
    )
    .map_err(|e| JoinError::Tls(e.to_string()))?;
    let mut tls = rustls::StreamOwned::new(
        conn,
        IdleDeadlineStream {
            tcp,
            idle: JOIN_IDLE_TIMEOUT,
        },
    );
    tls.conn
        .complete_io(&mut tls.sock)
        .map_err(|e| JoinError::Tls(e.to_string()))?;
    Ok(tls)
}

/// The joined connection's socket: "no read timeout" means [`JOIN_IDLE_TIMEOUT`].
///
/// So a coordinator that vanished without a FIN (and stopped pinging) is noticed while
/// this worker waits for its next request. Short timeouts the stream emitter sets while
/// streaming pass through unchanged.
pub struct IdleDeadlineStream {
    tcp: TcpStream,
    idle: Duration,
}

impl Read for IdleDeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.tcp.read(buf)
    }
}

impl Write for IdleDeadlineStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tcp.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

impl TimeoutRead for IdleDeadlineStream {
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        self.tcp
            .set_read_timeout(Some(duration.unwrap_or(self.idle)))
    }
}

impl TimeoutWrite for IdleDeadlineStream {
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        self.tcp.set_write_timeout(duration)
    }
}
