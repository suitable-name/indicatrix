//! The enrollment listener's per-connection handling: a bare (no client-certificate
//! requirement) TLS accept, then exactly one [`indicatrix_net::enroll::EnrollRequest`]/
//! [`indicatrix_net::enroll::EnrollResponse`] exchange -- see `crate::enroll`'s module doc
//! comment for why this listener structurally cannot reach `RenderRequest` handling.

use super::registry::{EnrollRegistry, ZeroizeLocal};
use indicatrix_net::{
    enroll::{EnrollRequest, EnrollResponse},
    messages::PeerRole,
};
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
    // F-06a: a real TOTAL deadline for the handshake, re-armed before every read --
    // see `crate::serve::socket::DeadlineSocket`'s doc comment and
    // `crate::serve::tls::accept_tls`'s identical use of it. This listener needs it at
    // least as much as the render listener's `accept_tls`: it has NO client-certificate
    // check at all (see the module doc comment), so a bare TLS handshake is the whole of
    // its authentication story.
    let deadline = std::time::Instant::now() + crate::serve::HANDSHAKE_TIMEOUT;
    let handshake_result = {
        let mut io = crate::serve::DeadlineSocket::new(&tls_stream.sock, deadline);
        tls_stream.conn.complete_io(&mut io)
    };
    if let Err(e) = handshake_result {
        tracing::warn!("enrollment connection {peer:?}: TLS handshake failed: {e}");
        return None;
    }
    Some(tls_stream)
}

/// The certificate label an `Issue { name }` asks for, checked against the listener's
/// role (see `super`'s module doc comment): a worker listener needs the
/// `worker:<label>` form `cert issue-token --role worker` sends and returns the bare
/// label; a viewer listener refuses that form.
///
/// # Errors
///
/// The refusal reason for an `Issue` of the other role.
fn issue_label_for_role(name: &str, role: PeerRole) -> Result<&str, String> {
    match (role, name.strip_prefix(crate::pki::WORKER_CN_PREFIX)) {
        (PeerRole::Viewer, None) => Ok(name),
        (PeerRole::Worker, Some(label)) => Ok(label),
        (PeerRole::Viewer, Some(_)) => Err(
            "this is the VIEWER enrollment listener -- a worker token must be issued on the coordinator's \
             WORKER enrollment listener (default port 7881)"
                .to_string(),
        ),
        (PeerRole::Worker, None) => Err(
            "this is the WORKER enrollment listener -- pass `cert issue-token --role worker` for a worker \
             token, or use the viewer enrollment listener (default port 7879) for a viewer"
                .to_string(),
        ),
    }
}

/// The reply to an `Issue { name }`: refused unless the peer is on loopback AND presented
/// the operator secret (`authorised`); otherwise a freshly minted token.
///
/// Loopback alone is not authority: any local user, or anything forwarding a port to the
/// loopback listener, appears as a loopback peer. The operator secret is readable only by
/// whoever can read the PKI directory, which is the same party that can read `ca.key`.
fn issue_response(
    name: &str,
    authorised: bool,
    registry: &EnrollRegistry,
    pki_dir: &Path,
    peer: Option<SocketAddr>,
) -> EnrollResponse {
    if !peer.is_some_and(|p| p.ip().is_loopback()) {
        tracing::warn!("enrollment connection {peer:?}: refused Issue from a non-loopback peer");
        return EnrollResponse::IssueRefused {
            reason: "issuing enrollment tokens is only permitted from loopback".to_string(),
        };
    }
    if !authorised {
        tracing::warn!("enrollment connection {peer:?}: refused Issue without the operator secret");
        return EnrollResponse::IssueRefused {
            reason: format!(
                "issuing enrollment tokens needs the operator secret ({} in the PKI directory)",
                indicatrix_net::enroll::OPERATOR_SECRET_FILE
            ),
        };
    }
    match issue_label_for_role(name, registry.role()) {
        Ok(label) => match registry.issue(pki_dir, label) {
            Ok((token, expires_in_secs)) => EnrollResponse::Issued {
                token,
                expires_in_secs,
            },
            // EnrollIssueError -> String at the wire boundary: `reason` travels to the
            // peer as plain text, never matched on.
            Err(reason) => EnrollResponse::IssueRefused {
                reason: reason.to_string(),
            },
        },
        Err(reason) => EnrollResponse::IssueRefused { reason },
    }
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
/// encoding the response. An `Issue` refused for being non-loopback or lacking the
/// operator secret, or a `Claim` that doesn't match a pending enrollment, are NOT error
/// returns -- all are reported to the peer as a normal [`EnrollResponse`], since none
/// means the connection is broken.
pub(super) fn handle_enroll_connection<S: Read + Write>(
    mut stream: S,
    registry: &EnrollRegistry,
    pki_dir: &Path,
    allowlist_path: Option<&Path>,
    peer: Option<SocketAddr>,
) -> Result<(), String> {
    // F-06a: the same real, TOTAL deadline as the TLS handshake this connection already
    // completed (a fresh `HANDSHAKE_TIMEOUT` budget for this second phase -- see
    // `socket::HANDSHAKE_TIMEOUT`'s doc comment on why the two phases aren't a single
    // shared budget). `DeadlineIo` only needs `S: Read`, so this wraps generically
    // regardless of whether `stream` is the real `EnrollTlsStream` or (this module's own
    // tests) an in-memory duplex.
    let deadline = std::time::Instant::now() + crate::serve::HANDSHAKE_TIMEOUT;
    let request: EnrollRequest = {
        let mut guarded = crate::serve::DeadlineIo::new(&mut stream, deadline);
        indicatrix_net::messages::read_message_bounded(&mut guarded, MAX_ENROLL_REQUEST_LEN)
            .map_err(|e| e.to_string())?
    };

    match request {
        EnrollRequest::Issue { .. } => {
            tracing::warn!(
                "enrollment connection {peer:?}: refused an Issue without the operator secret"
            );
            let response = EnrollResponse::IssueRefused {
                reason: "this listener requires the operator secret to issue a token -- update \
                         `indicatrix-worker cert issue-token`"
                    .to_string(),
            };
            indicatrix_net::messages::write_message(&mut stream, &response)
                .map_err(|e| e.to_string())
        }
        EnrollRequest::IssueAuthorized {
            name,
            mut operator_secret,
        } => {
            let authorised = registry.operator_secret_matches(&operator_secret);
            operator_secret.zeroize_local();
            let response = issue_response(&name, authorised, registry, pki_dir, peer);
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

#[cfg(test)]
mod issue_authorisation_tests {
    use super::*;
    use indicatrix_net::messages::{read_message, write_message};
    use std::io::Cursor;
    use zeroize::Zeroizing;

    /// An in-memory connection: the request is pre-loaded, the reply collects in `output`.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Sends one loopback `Issue` carrying `secret` through `registry`'s handler and
    /// returns the reply.
    fn issue_from_loopback(registry: &EnrollRegistry, secret: [u8; 32]) -> EnrollResponse {
        let mut request = Vec::new();
        write_message(
            &mut request,
            &EnrollRequest::IssueAuthorized {
                name: "laptop".to_string(),
                operator_secret: secret,
            },
        )
        .unwrap();
        let mut duplex = Duplex {
            input: Cursor::new(request),
            output: Vec::new(),
        };
        let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        handle_enroll_connection(&mut duplex, registry, Path::new("."), None, Some(peer)).unwrap();
        read_message(&mut Cursor::new(duplex.output)).unwrap()
    }

    /// Loopback alone is not authority: without the operator secret the `Issue` is refused.
    #[test]
    fn an_issue_from_loopback_without_the_operator_secret_is_refused() {
        let registry = EnrollRegistry::new().with_operator_secret(Zeroizing::new([7u8; 32]));
        let reply = issue_from_loopback(&registry, [0u8; 32]);
        assert!(
            matches!(&reply, EnrollResponse::IssueRefused { reason } if reason.contains("operator secret")),
            "{reply:?}"
        );
    }

    /// A registry nobody configured a secret for issues nothing.
    #[test]
    fn an_issue_is_refused_when_no_operator_secret_is_configured() {
        let reply = issue_from_loopback(&EnrollRegistry::new(), [0u8; 32]);
        assert!(
            matches!(reply, EnrollResponse::IssueRefused { .. }),
            "{reply:?}"
        );
    }
}
