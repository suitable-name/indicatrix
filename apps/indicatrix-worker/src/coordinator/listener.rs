//! The worker port (default 7880, separate from the viewer port): where
//! `indicatrix-worker join` workers dial in. Mutual TLS with WORKER client certificates only
//! (`crate::serve::Auth` with role `Worker`, `allowlist-workers.txt`), a `HELLO` that
//! must say `PeerRole::Worker` and carry the worker's `RenderCapability`, the full
//! build-compatibility gate, then `WELCOME.registration = Some(worker_id)` and the
//! connection moves into the [`Registry`].

use super::registry::{Registry, WorkerInfo};
use crate::serve::{
    self, Auth, ConnectionLimiter, ConnectionSlot, HelloCheck, TlsStream, accept_tls,
};
use indicatrix_net::{
    handshake,
    messages::{NetError, PeerRole, Welcome, WorkerRegistration, negotiate},
};
use std::{
    net::{SocketAddr, TcpListener},
    sync::Arc,
    thread,
};

/// Everything the worker port needs, gathered by `crate::serve::start`.
pub struct WorkerListenerConfig {
    /// Where to listen (default: the viewer host, port 7880).
    pub bind_addr: SocketAddr,
    /// The same server certificate and CA as the viewer port.
    pub tls_config: Arc<rustls::ServerConfig>,
    /// Role `Worker` and the worker allowlist.
    pub auth: Auth,
    /// The worker port's own `--max-connections` cap (separate from the viewers').
    pub limiter: ConnectionLimiter,
    /// `--max-connections`, for the refusal message.
    pub max_connections: usize,
    /// Where joined workers are registered.
    pub registry: Arc<Registry>,
}

/// Binds the worker port and accepts joining workers on a background thread, one
/// short-lived thread per connection (TLS, `HELLO`, `WELCOME`, then hand-over to the
/// registry). Returns the bound address.
///
/// # Errors
///
/// A human-readable message if the bind fails.
pub fn spawn_worker_listener(config: WorkerListenerConfig) -> Result<SocketAddr, String> {
    let listener = TcpListener::bind(config.bind_addr)
        .map_err(|e| format!("failed to bind the worker port {}: {e}", config.bind_addr))?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("worker port bound but local_addr() failed: {e}"))?;
    tracing::info!(
        "indicatrix-worker serve: worker port on {bound} (`indicatrix-worker join {bound}`; worker certificates only)"
    );
    let config = Arc::new(config);
    // Bare, not-yet-authenticated connections, like the viewer port's second limiter.
    let handshake_limiter = ConnectionLimiter::new(
        config
            .max_connections
            .saturating_mul(serve::PRE_AUTH_HANDSHAKE_MULTIPLIER),
    );
    thread::spawn(move || {
        for incoming in listener.incoming() {
            match incoming {
                Ok(stream) => {
                    let peer = stream.peer_addr().ok();
                    serve::tune_accepted_socket(&stream, peer);
                    serve::apply_handshake_timeout(&stream, peer);
                    let Ok(handshake_slot) = handshake_limiter.try_acquire() else {
                        tracing::warn!(
                            "worker connection {peer:?}: refusing -- too many unauthenticated connections"
                        );
                        continue;
                    };
                    let config = Arc::clone(&config);
                    thread::spawn(move || {
                        let _handshake_slot = handshake_slot;
                        accept_worker(stream, peer, &config);
                    });
                }
                Err(e) => tracing::warn!("worker port accept error: {e}"),
            }
        }
    });
    Ok(bound)
}

/// TLS (role + allowlist), the connection cap, then [`register_worker`].
fn accept_worker(
    stream: std::net::TcpStream,
    peer: Option<SocketAddr>,
    config: &WorkerListenerConfig,
) {
    let Some((mut tls, cert_role)) = accept_tls(stream, &config.tls_config, &config.auth, peer)
    else {
        return; // accept_tls already logged why
    };
    let slot = match config.limiter.try_acquire() {
        Ok(slot) => slot,
        Err(active) => {
            serve::refuse_for_capacity(&mut tls, peer, active, config.max_connections);
            return;
        }
    };
    match register_worker(tls, peer, cert_role, slot, &config.registry) {
        Ok(Some(worker_id)) => {
            let capacity = config.registry.capacity();
            tracing::info!(
                "coordinator: worker #{worker_id} joined from {peer:?} -- now {} worker connection(s), {} CPU \
                 thread(s), {} GPU(s)",
                capacity.workers,
                capacity.threads,
                capacity.gpus
            );
        }
        Ok(None) => {} // refused; already logged
        Err(e) => tracing::warn!("worker connection {peer:?}: handshake failed: {e}"),
    }
}

/// The worker port's `HELLO`/`WELCOME`: role gate (worker `HELLO` and worker
/// certificate), build compatibility, a fresh `worker_id` in `WELCOME.registration`,
/// then the connection goes into `registry`, idle. `Ok(None)` if refused (the refusal
/// was sent).
///
/// # Errors
///
/// A transport failure reading `HELLO` or writing `WELCOME`.
fn register_worker(
    mut tls: TlsStream,
    peer: Option<SocketAddr>,
    cert_role: PeerRole,
    connection_slot: ConnectionSlot,
    registry: &Registry,
) -> Result<Option<u32>, NetError> {
    let hello = match serve::read_and_check_hello(&mut tls, PeerRole::Worker, Some(cert_role))? {
        HelloCheck::Accepted(hello) => hello,
        HelloCheck::Refused(refusal) => {
            serve::send_refusal(&mut tls, &refusal);
            return Ok(None);
        }
    };
    let local_hello = handshake::local_hello();
    if let Err(incompatible) = handshake::verify_compatible(&local_hello, &hello) {
        serve::refuse_incompatible_handshake(&mut tls, &local_hello, &hello, incompatible);
        return Ok(None);
    }
    let Some(capability) = hello.capability.clone() else {
        // Unreachable: the role gate only accepts a worker HELLO carrying one.
        return Ok(None);
    };

    // The WORKER encodes FRAMEs on this link and the coordinator decodes them; one build
    // runs on both ends, so every encoding the worker can decode it can also encode.
    let payload_encoding = negotiate(serve::payload_preference_for(peer), &hello.accept_encodings);
    // The certificate's `worker:<label>` -- the stable identity `--pin-interactive-worker`
    // names (the TLS layer already checked the role and the allowlist).
    let label = tls
        .conn
        .peer_certificates()
        .and_then(<[_]>::first)
        .and_then(|der| crate::pki::worker_label_of_certificate(der.as_ref()));
    let worker_id = registry.allocate_id();
    // A worker connection is not a viewer: nothing to advertise to it. A coordinator does
    // not forward zones to its joined workers (`Welcome::new` sets the zoning bit false).
    let welcome = Welcome::new(
        local_hello.build_hash,
        local_hello.source_hash,
        None,
        false,
        false,
        Some(WorkerRegistration { worker_id }),
        payload_encoding,
    );
    indicatrix_net::messages::write_message(&mut tls, &welcome)?;
    // The registry (liveness, request execution) sets its own deadlines from here on.
    let _ = tls.sock.set_read_timeout(None);
    let _ = tls.sock.set_write_timeout(None);

    registry.insert(
        WorkerInfo {
            worker_id,
            capability,
            peer,
            label,
            payload_encoding,
        },
        Box::new(tls),
        Some(connection_slot),
    );
    Ok(Some(worker_id))
}
