//! TLS transport setup for the render listener: [`Transport`]/[`Auth`] (decided once at
//! [`crate::serve::run`] startup), [`build_transport`], and the per-connection
//! [`accept_tls`]/[`check_auth`] pair -- see `crate::serve`'s module docs (the "TLS"
//! section) for the full picture of how this fits together with [`crate::handshake`].

use crate::cli::ServeArgs;
use std::{
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::Arc,
};

/// How [`accept_tls`] decides whether a client whose certificate chains to the
/// configured CA is actually trusted -- the authorization decision that stands in for a
/// password in this design. See the module doc comment.
#[derive(Debug, Clone)]
pub(super) enum Auth {
    /// Check the connected client certificate's SHA-256 fingerprint against the
    /// allowlist file at this path, RE-READ on every connection (not cached at
    /// startup) -- so revoking a client by deleting its line from the file takes
    /// effect immediately, no restart required.
    Allowlist(PathBuf),
    /// `--trust-any-client-cert`: skip the fingerprint check, trusting any client
    /// certificate that chains to the configured CA. Never the silent default -- see
    /// [`run`]'s doc comment on why this has to be an explicit, visible flag.
    AnyCaSignedClient,
}

/// How [`run`]'s accept loop wraps each accepted `TcpStream`, decided once at startup
/// from `ServeArgs` rather than per-connection.
#[derive(Debug)]
pub(super) enum Transport {
    Tls {
        config: Arc<rustls::ServerConfig>,
        auth: Auth,
    },
    /// `--insecure-no-tls`. See the module doc comment.
    Insecure,
}

/// Builds this process's [`Transport`] from `args`, per [`run`]'s doc comment.
pub(super) fn build_transport(
    args: &ServeArgs,
    bind_addr: SocketAddr,
) -> Result<Transport, String> {
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

    let auth = if args.trust_any_client_cert {
        tracing::warn!(
            "indicatrix-worker serve: --trust-any-client-cert -- the fingerprint allowlist is DISABLED; any client \
             certificate signed by {} will be accepted",
            ca_path.display()
        );
        Auth::AnyCaSignedClient
    } else {
        let allowlist_path = args
            .allowlist
            .clone()
            .unwrap_or_else(|| crate::pki::default_allowlist_path(&ca_path));
        // Loaded once here purely to fail fast at startup on a bad/missing path --
        // accept_tls re-loads it on every connection; see Auth::Allowlist.
        let preflight = indicatrix_net::tls::Allowlist::load(&allowlist_path).map_err(|e| {
            format!(
                "--allowlist {}: {e} (run `indicatrix-worker cert issue-client` to trust a client, or pass \
                 --trust-any-client-cert to skip this check)",
                allowlist_path.display()
            )
        })?;
        tracing::info!(
            "indicatrix-worker serve: {} trusted client certificate(s) in {}",
            preflight.len(),
            allowlist_path.display()
        );
        Auth::Allowlist(allowlist_path)
    };

    Ok(Transport::Tls { config, auth })
}

/// Completes the TLS handshake for one just-accepted `stream` and checks the resulting
/// peer certificate against `auth`. Returns `None` for anything short of a fully
/// authenticated connection -- a handshake failure (wrong CA, expired certificate,
/// clock skew, a SAN that doesn't match how the peer connected: see `indicatrix_net::tls`'s
/// doc comment and this crate's top-level docs) or a client certificate that isn't on
/// the allowlist -- having already logged specifically why via `peer`.
pub(super) fn accept_tls(
    stream: TcpStream,
    config: &Arc<rustls::ServerConfig>,
    auth: &Auth,
    peer: Option<SocketAddr>,
) -> Option<TlsStream> {
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
    // `stream` carries `serve::HANDSHAKE_TIMEOUT` (applied by `serve::run`'s accept loop
    // before calling this function), so a client that connects and never completes the
    // handshake -- a slowloris attempt, not a real protocol failure -- surfaces here as
    // an `io::Error` of kind `WouldBlock`/`TimedOut` rather than hanging forever. Logged
    // at `debug`, not `warn`: unlike an actual handshake failure (wrong CA, expired
    // certificate, clock skew, no matching SAN), a timeout is an expected, routine
    // outcome of exposing a socket to the network at all.
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

    if let Err(msg) = check_auth(&tls_stream, auth) {
        tracing::warn!("connection {peer:?}: {msg}");
        return None;
    }

    Some(tls_stream)
}

pub(super) type TlsStream = rustls::StreamOwned<rustls::ServerConnection, TcpStream>;

/// The authorization decision described in the module doc comment: CA-chain validity
/// (already established by the completed handshake) is necessary but not sufficient,
/// so this is where [`Auth::Allowlist`] actually gets checked.
///
/// # Errors
///
/// A human-readable message (naming the offending fingerprint, for `Auth::Allowlist`)
/// if the peer presented no certificate at all (should be unreachable -- the server
/// config requires one, so a bare TLS handshake success without one would itself be a
/// bug) or its fingerprint isn't on the allowlist.
fn check_auth(stream: &TlsStream, auth: &Auth) -> Result<(), String> {
    let Auth::Allowlist(path) = auth else {
        return Ok(()); // Auth::AnyCaSignedClient: CA-chain validity alone is enough.
    };

    let peer_certs = stream
        .conn
        .peer_certificates()
        .filter(|certs| !certs.is_empty())
        .ok_or_else(|| {
            "TLS handshake succeeded but the peer presented no client certificate".to_string()
        })?;
    let fingerprint = indicatrix_net::tls::fingerprint(&peer_certs[0]);
    let fingerprint_hex = indicatrix_net::tls::fingerprint_to_hex(&fingerprint);

    let allowlist = indicatrix_net::tls::Allowlist::load(path).map_err(|e| {
        format!(
            "rejecting client certificate {fingerprint_hex}: could not (re-)load allowlist {}: {e}",
            path.display()
        )
    })?;

    if allowlist.contains(&fingerprint) {
        Ok(())
    } else {
        Err(format!(
            "rejecting client certificate {fingerprint_hex}: not present in {} (run `indicatrix-worker cert \
             issue-client` to trust it, or add this exact fingerprint by hand)",
            path.display()
        ))
    }
}
