//! TLS transport setup for the viewer and worker listeners: [`Transport`]/[`Auth`]
//! (decided once at [`crate::serve::run`] startup), [`build_transport`], and the
//! per-connection [`accept_tls`]/[`check_auth`] pair -- see `crate::serve`'s module docs
//! (the "TLS" section) for the full picture.
//!
//! # Roles
//!
//! Every listener expects one client-certificate role ([`Auth::role`]): the viewer port
//! viewers, the worker port joining workers (see `crate::pki::role` for how a role is
//! recorded). [`check_auth`] reads the presented certificate's role first:
//!
//! - the port's own role: the certificate must ALSO be on the port's allowlist (unless
//!   `--trust-any-client-cert`), else the connection is dropped after logging why;
//! - the other role: the allowlist is not consulted, and the connection is handed on with
//!   its real role so the `HELLO` gate refuses it with `ROLE_REFUSED` and a message that
//!   names the mix-up -- never served.

use crate::{cli::ServeArgs, pki};
use indicatrix_net::messages::PeerRole;
use std::{
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::Arc,
};

/// How [`accept_tls`] decides whether a client whose certificate chains to the
/// configured CA is actually trusted on one listener -- the authorization decision that
/// stands in for a password in this design. See the module doc comment.
#[derive(Debug, Clone)]
pub struct Auth {
    /// The client-certificate role this listener serves.
    pub role: PeerRole,
    /// The fingerprint allowlist for [`Self::role`], RE-READ on every connection (not
    /// cached at startup) -- so revoking a client by deleting its line from the file
    /// takes effect immediately, no restart required. `None` is
    /// `--trust-any-client-cert`: any certificate that chains to the CA and carries the
    /// right role is trusted. Never the silent default.
    pub allowlist: Option<PathBuf>,
}

/// How [`crate::serve::run`]'s accept loop wraps each accepted `TcpStream`, decided
/// once at startup from `ServeArgs` rather than per-connection.
#[derive(Debug)]
pub enum Transport {
    /// Mutual TLS with the viewer listener's [`Auth`].
    Tls {
        config: Arc<rustls::ServerConfig>,
        auth: Auth,
    },
    /// `--insecure-no-tls`. See the module doc comment.
    Insecure,
}

/// Builds this process's viewer [`Transport`] from `args`, per [`crate::serve::run`]'s
/// doc comment.
pub fn build_transport(args: &ServeArgs, bind_addr: SocketAddr) -> Result<Transport, String> {
    if args.insecure_no_tls {
        if !bind_addr.ip().is_loopback() {
            return Err(format!(
                "refusing --insecure-no-tls on non-loopback bind address {bind_addr} -- serving plaintext with no \
                 TLS and no authentication beyond localhost is not allowed; see --help"
            ));
        }
        tracing::warn!(
            "indicatrix-worker serve: --insecure-no-tls -- serving PLAINTEXT with no TLS and no authentication. For \
             local debugging only; every connection accepted this way is logged."
        );
        return Ok(Transport::Insecure);
    }

    let ca_path = args.ca.clone().ok_or_else(|| {
        "\"serve\" requires --ca <path> unless --insecure-no-tls is set (see --help)".to_string()
    })?;
    let cert_path = args.cert.clone().ok_or_else(|| {
        "\"serve\" requires --cert <path> unless --insecure-no-tls is set (see --help)".to_string()
    })?;
    let key_path = args.key.clone().ok_or_else(|| {
        "\"serve\" requires --key <path> unless --insecure-no-tls is set (see --help)".to_string()
    })?;

    let ca = indicatrix_net::tls::load_ca(&ca_path)
        .map_err(|e| format!("--ca {}: {e}", ca_path.display()))?;
    let cert_chain = indicatrix_net::tls::load_certs(&cert_path)
        .map_err(|e| format!("--cert {}: {e}", cert_path.display()))?;
    let key = indicatrix_net::tls::load_private_key(&key_path)
        .map_err(|e| format!("--key {}: {e}", key_path.display()))?;
    let config = indicatrix_net::tls::server_config(ca, cert_chain, key)
        .map_err(|e| format!("failed to build TLS config: {e}"))?;

    let allowlist = if args.trust_any_client_cert {
        tracing::warn!(
            "indicatrix-worker serve: --trust-any-client-cert -- the fingerprint allowlists are DISABLED; any client \
             certificate signed by {} (and carrying the port's role) will be accepted",
            ca_path.display()
        );
        None
    } else {
        let allowlist_path = args
            .allowlist
            .clone()
            .unwrap_or_else(|| pki::default_viewer_allowlist_path(&ca_path));
        // Loaded once here purely to fail fast at startup on a bad/missing path --
        // accept_tls re-loads it on every connection; see Auth::allowlist.
        let preflight = indicatrix_net::tls::Allowlist::load(&allowlist_path).map_err(|e| {
            format!(
                "--allowlist {}: {e} (run `indicatrix-worker cert issue-client` to trust a viewer, or pass \
                 --trust-any-client-cert to skip this check)",
                allowlist_path.display()
            )
        })?;
        tracing::info!(
            "indicatrix-worker serve: {} trusted viewer certificate(s) in {}",
            preflight.len(),
            allowlist_path.display()
        );
        Some(allowlist_path)
    };

    Ok(Transport::Tls {
        config,
        auth: Auth {
            role: PeerRole::Viewer,
            allowlist,
        },
    })
}

/// The worker port's [`Auth`]: role [`PeerRole::Worker`], the `--worker-allowlist`
/// (default `allowlist-workers.txt` next to `--ca`), or none under
/// `--trust-any-client-cert`.
///
/// Unlike the viewer allowlist, a MISSING worker allowlist is not a startup error --
/// most coordinators start before any worker is enrolled -- it only means every worker
/// is refused until one is (logged at `info`). A malformed one still fails fast.
#[cfg(feature = "worker")]
pub fn worker_auth(args: &ServeArgs) -> Result<Auth, String> {
    if args.trust_any_client_cert {
        return Ok(Auth {
            role: PeerRole::Worker,
            allowlist: None,
        });
    }
    let path = match (&args.worker_allowlist, &args.ca) {
        (Some(path), _) => path.clone(),
        (None, Some(ca)) => pki::default_worker_allowlist_path(ca),
        (None, None) => return Err("\"serve\" requires --ca <path> for the worker port".into()),
    };
    if path.exists() {
        let list = indicatrix_net::tls::Allowlist::load(&path)
            .map_err(|e| format!("--worker-allowlist {}: {e}", path.display()))?;
        tracing::info!(
            "indicatrix-worker serve: {} trusted worker certificate(s) in {}",
            list.len(),
            path.display()
        );
    } else {
        tracing::info!(
            "indicatrix-worker serve: no worker allowlist at {} yet -- every joining worker is refused until one \
             is enrolled (`cert issue-client --role worker` or `cert issue-token --role worker`)",
            path.display()
        );
    }
    Ok(Auth {
        role: PeerRole::Worker,
        allowlist: Some(path),
    })
}

/// Completes the TLS handshake for one just-accepted `stream` and checks the resulting
/// peer certificate against `auth`, returning the stream with the certificate's role.
///
/// `None` for anything short of a usable connection -- a handshake failure (wrong CA,
/// expired certificate, clock skew, a SAN that doesn't match how the peer connected: see
/// `indicatrix_net::tls`'s doc comment) or a certificate of the port's own role that isn't
/// on the allowlist -- having already logged specifically why via `peer`. A certificate
/// of the OTHER role comes back `Some` with that role, for the `HELLO` gate to refuse
/// (see the module doc comment).
pub fn accept_tls(
    stream: TcpStream,
    config: &Arc<rustls::ServerConfig>,
    auth: &Auth,
    peer: Option<SocketAddr>,
) -> Option<(TlsStream, PeerRole)> {
    let conn = match rustls::ServerConnection::new(Arc::clone(config)) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("connection {peer:?}: could not start TLS: {e}");
            return None;
        }
    };
    let mut tls_stream = rustls::StreamOwned::new(conn, stream);

    // Force the handshake to complete now (not lazily on first read/write) so a
    // failure is diagnosed here with the peer address in hand, rustls's own error text
    // naming the actual reason (expired, clock skew, wrong CA, no matching SAN).
    //
    // `stream` carries `serve::HANDSHAKE_TIMEOUT` (applied by the accept loop before
    // calling this function), so a client that connects and never completes the
    // handshake -- a slowloris attempt, not a real protocol failure -- surfaces here as
    // an `io::Error` of kind `WouldBlock`/`TimedOut` rather than hanging forever. Logged
    // at `debug`, not `warn`: a timeout is an expected, routine outcome of exposing a
    // socket to the network at all.
    if let Err(e) = tls_stream.conn.complete_io(&mut tls_stream.sock) {
        if matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            tracing::debug!(
                "connection {peer:?}: TLS handshake timed out after {:?} -- dropping (see \
                 serve::HANDSHAKE_TIMEOUT)",
                super::HANDSHAKE_TIMEOUT
            );
        } else {
            tracing::warn!("connection {peer:?}: TLS handshake failed: {e}");
        }
        return None;
    }

    match check_auth(&tls_stream, auth) {
        Ok(role) => Some((tls_stream, role)),
        Err(msg) => {
            tracing::warn!("connection {peer:?}: {msg}");
            None
        }
    }
}

/// A server-side mutual-TLS stream over an accepted socket.
pub type TlsStream = rustls::StreamOwned<rustls::ServerConnection, TcpStream>;

/// The authorization decision described in the module doc comment: CA-chain validity
/// (already established by the completed handshake) is necessary but not sufficient,
/// so this is where the role and the allowlist actually get checked. Returns the
/// certificate's role.
///
/// # Errors
///
/// A human-readable message (naming the offending fingerprint) if the peer presented no
/// certificate at all (should be unreachable -- the server config requires one), its
/// subject can't be parsed, or a certificate of the port's own role isn't on the
/// allowlist.
fn check_auth(stream: &TlsStream, auth: &Auth) -> Result<PeerRole, String> {
    let peer_certs = stream
        .conn
        .peer_certificates()
        .filter(|certs| !certs.is_empty())
        .ok_or_else(|| {
            "TLS handshake succeeded but the peer presented no client certificate".to_string()
        })?;
    let fingerprint = indicatrix_net::tls::fingerprint(&peer_certs[0]);
    let fingerprint_hex = indicatrix_net::tls::fingerprint_to_hex(&fingerprint);
    let role = pki::role_of_certificate(&peer_certs[0])
        .map_err(|e| format!("rejecting client certificate {fingerprint_hex}: {e}"))?;

    if role != auth.role {
        // Refused at HELLO with ROLE_REFUSED (see the module doc comment).
        return Ok(role);
    }
    let Some(path) = &auth.allowlist else {
        return Ok(role); // --trust-any-client-cert: CA chain + role are enough.
    };

    let allowlist = indicatrix_net::tls::Allowlist::load(path).map_err(|e| {
        format!(
            "rejecting {} certificate {fingerprint_hex}: could not (re-)load allowlist {}: {e}",
            pki::role::role_name(role),
            path.display()
        )
    })?;

    if allowlist.contains(&fingerprint) {
        Ok(role)
    } else {
        Err(format!(
            "rejecting {} certificate {fingerprint_hex}: not present in {} (run `indicatrix-worker cert \
             issue-client --role {}` to trust it, or add this exact fingerprint by hand)",
            pki::role::role_name(role),
            path.display(),
            pki::role::role_name(role)
        ))
    }
}
