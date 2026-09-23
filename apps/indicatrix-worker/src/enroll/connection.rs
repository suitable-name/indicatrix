//! The enrollment listener's per-connection handling: a bare (no client-certificate
//! requirement) TLS accept, then exactly one [`indicatrix_net::enroll::EnrollRequest`]/
//! [`indicatrix_net::enroll::EnrollResponse`] exchange -- see `crate::enroll`'s module doc
//! comment for why this listener structurally cannot reach `RenderRequest` handling.

use super::registry::{EnrollRegistry, ZeroizeLocal};
use indicatrix_net::enroll::{EnrollRequest, EnrollResponse};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::Arc,
};

pub(super) type EnrollTlsStream = rustls::StreamOwned<rustls::ServerConnection, TcpStream>;

/// Hard cap on the encoded size of an [`EnrollRequest`], enforced by
/// [`handle_enroll_connection`]'s [`indicatrix_net::messages::read_message_bounded`]
/// call. An `EnrollRequest` is at most a `Claim`'s
/// [`indicatrix_net::token::SECRET_LEN`]-byte secret plus a handful of bytes of postcard
/// overhead -- comfortably under 4 KiB even with slack for a generously long `--name`.
const MAX_ENROLL_REQUEST_LEN: u32 = 4096;

/// Completes the TLS handshake for one just-accepted enrollment connection. No
/// client-certificate check follows (unlike `crate::serve::accept_tls`) -- this listener
/// never requires one; see the module doc comment.
///
/// Applies `crate::serve::HANDSHAKE_TIMEOUT` to the raw socket's read/write timeouts
/// BEFORE `complete_io`: unlike the render listener's mutual-TLS accept,
/// this listener requires no client certificate at all, so without a deadline here a
/// peer that opens a connection and never completes the TLS handshake pins this
/// connection's thread open indefinitely. Left in place afterward (not cleared, unlike
/// `serve::handle_connection_with_gpu`'s post-`HELLO` reset): [`handle_enroll_connection`]
/// handles exactly one request/response and returns, so there is no long-lived loop this
/// deadline would wrongly bound.
pub(super) fn accept_enroll_tls(
    stream: TcpStream,
    config: &Arc<rustls::ServerConfig>,
    peer: Option<SocketAddr>,
) -> Option<EnrollTlsStream> {
    if let Err(e) = stream.set_read_timeout(Some(crate::serve::HANDSHAKE_TIMEOUT)) {
        tracing::debug!(
            "enrollment connection {peer:?}: failed to set handshake read timeout: {e}"
        );
    }
    if let Err(e) = stream.set_write_timeout(Some(crate::serve::HANDSHAKE_TIMEOUT)) {
        tracing::debug!(
            "enrollment connection {peer:?}: failed to set handshake write timeout: {e}"
        );
    }

    let conn = match rustls::ServerConnection::new(Arc::clone(config)) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("enrollment connection {peer:?}: could not start TLS: {e}");
            return None;
        }
    };
    let mut tls_stream = rustls::StreamOwned::new(conn, stream);
    if let Err(e) = tls_stream.conn.complete_io(&mut tls_stream.sock) {
        tracing::warn!("enrollment connection {peer:?}: TLS handshake failed: {e}");
        return None;
    }
    Some(tls_stream)
}

/// Handles exactly one [`EnrollRequest`]/[`EnrollResponse`] exchange on `stream`, then
/// returns -- not a loop, since both `Issue` and `Claim` are one-shot.
///
/// Generic over `Read + Write` so this crate's tests can drive it over an in-memory
/// duplex, like `crate::serve::handle_connection` does.
///
/// Reads the request via [`indicatrix_net::messages::read_message_bounded`] with
/// [`MAX_ENROLL_REQUEST_LEN`] rather than plain [`indicatrix_net::messages::read_message`]:
/// this listener accepts a connection from anyone (no client-certificate
/// requirement -- see the module doc comment), so a peer that never authenticates at all
/// must not be able to make this process attempt a
/// [`indicatrix_net::framing::MAX_FRAME_LEN`]-sized (512 MiB) allocation with a single
/// 4-byte length prefix.
///
/// # Errors
///
/// A human-readable message for a transport-level failure decoding the request or
/// encoding the response. An `Issue` refused for being non-loopback, or a `Claim` that
/// doesn't match a pending enrollment, are NOT error returns -- both are reported to the
/// peer as a normal [`EnrollResponse`], since neither means the connection is broken.
pub(super) fn handle_enroll_connection<S: Read + Write>(
    mut stream: S,
    registry: &EnrollRegistry,
    pki_dir: &Path,
    allowlist_path: Option<&Path>,
    peer: Option<SocketAddr>,
) -> Result<(), String> {
    let request: EnrollRequest =
        indicatrix_net::messages::read_message_bounded(&mut stream, MAX_ENROLL_REQUEST_LEN)
            .map_err(|e| e.to_string())?;

    match request {
        EnrollRequest::Issue { name } => {
            let is_loopback = peer.is_some_and(|p| p.ip().is_loopback());
            let response = if is_loopback {
                match registry.issue(pki_dir, &name) {
                    Ok((token, expires_in_secs)) => EnrollResponse::Issued {
                        token,
                        expires_in_secs,
                    },
                    // EnrollIssueError -> String at the wire boundary: `reason` travels
                    // to the claiming peer as plain text, not a value it ever matches on.
                    Err(reason) => EnrollResponse::IssueRefused {
                        reason: reason.to_string(),
                    },
                }
            } else {
                tracing::warn!(
                    "enrollment connection {peer:?}: refused Issue from a non-loopback peer"
                );
                EnrollResponse::IssueRefused {
                    reason: "issuing enrollment tokens is only permitted from loopback".to_string(),
                }
            };
            indicatrix_net::messages::write_message(&mut stream, &response)
                .map_err(|e| e.to_string())
        }
        EnrollRequest::Claim { mut secret } => {
            let claimed = registry.claim(&secret);
            secret.zeroize_local();

            let response = if let Some(claimed) = claimed {
                let allowlisted = allowlist_path.map_or(Ok(()), |path| {
                    indicatrix_net::tls::append_to_allowlist(
                        path,
                        &claimed.bundle.client_fingerprint,
                        &claimed.name,
                    )
                    .map_err(|e| e.to_string())
                });
                match allowlisted {
                    Ok(()) => EnrollResponse::Claimed {
                        ca_pem: claimed.bundle.ca_pem,
                        client_cert_pem: claimed.bundle.client_cert_pem,
                        client_key_pem: claimed.bundle.client_key_pem.to_string(),
                    },
                    Err(e) => {
                        // The certificate must not be handed out without also being
                        // allowlisted -- an unlisted cert would just fail the render
                        // listener's check later, confusingly far from this cause.
                        tracing::error!(
                            "enrollment connection {peer:?}: claim succeeded but appending to the allowlist \
                             failed, so it is being reported as a failed claim instead: {e}"
                        );
                        EnrollResponse::ClaimFailed
                    }
                }
            } else {
                tracing::debug!(
                    "enrollment connection {peer:?}: claim did not match any pending enrollment (wrong \
                     secret, already claimed, or expired)"
                );
                EnrollResponse::ClaimFailed
            };
            indicatrix_net::messages::write_message(&mut stream, &response)
                .map_err(|e| e.to_string())
        }
    }
}
