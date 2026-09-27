//! The v14 `HELLO` gate shared by both build modes and both listener ports.
//!
//! Reads the peer's `HELLO` version first (so an older or newer peer gets a refusal
//! naming both protocol versions instead of a decode error), then refuses any role this
//! PORT does not serve (viewers on the viewer port, joining workers on the worker
//! port), checking both the role the `HELLO` claims and the role of the
//! client certificate the peer authenticated with (`crate::pki::role`). Every mismatch
//! is refused with `error_codes::ROLE_REFUSED` and a message naming the mix-up.

use crate::pki::role::role_name;
use indicatrix_net::{
    handshake::{self, HelloReadError},
    messages::{ErrorMsg, Hello, NetError, PeerRole, error_codes},
};
use std::io::{Read, Write};

/// What [`read_and_check_hello`] decided about the peer's `HELLO`.
pub enum HelloCheck {
    /// A same-version `HELLO` of the port's role: continue with the build check and
    /// `WELCOME`.
    Accepted(Hello),
    /// Refuse the pairing: send this `ErrorMsg` in place of `WELCOME` (see
    /// [`send_refusal`]) and end the connection.
    Refused(ErrorMsg),
}

/// Reads the peer's `HELLO` and classifies it for a listener serving `port`: a different
/// protocol version, or a role (claimed or certified) this port does not serve, becomes
/// [`HelloCheck::Refused`] with a message naming what both sides have; everything else
/// is [`HelloCheck::Accepted`] for the caller's own build-compatibility gate.
///
/// `cert_role` is the role of the peer's client certificate (`None` only for
/// `--insecure-no-tls`, which has no certificate to check).
///
/// # Errors
///
/// [`NetError`] if the frame cannot be read, or a same-version `HELLO` does not decode.
pub fn read_and_check_hello<S: Read>(
    stream: &mut S,
    port: PeerRole,
    cert_role: Option<PeerRole>,
) -> Result<HelloCheck, NetError> {
    let hello = match handshake::read_hello(stream) {
        Ok(hello) => hello,
        Err(HelloReadError::Net(e)) => return Err(e),
        Err(HelloReadError::Incompatible(incompatible)) => {
            return Ok(HelloCheck::Refused(ErrorMsg {
                code: error_codes::BUILD_MISMATCH,
                message: format!("refusing to pair: {incompatible}"),
            }));
        }
    };
    Ok(check_role(hello, port, cert_role))
}

/// The role half of [`read_and_check_hello`]: only a consistent `HELLO` whose role --
/// and whose certificate's role -- is the port's own passes.
pub fn check_role(hello: Hello, port: PeerRole, cert_role: Option<PeerRole>) -> HelloCheck {
    let refuse = |message: String| {
        HelloCheck::Refused(ErrorMsg {
            code: error_codes::ROLE_REFUSED,
            message,
        })
    };
    if !hello.role_is_consistent() {
        return refuse(format!(
            "refusing to pair: HELLO role {:?} disagrees with its capability ({}) -- a worker must \
             report its render capability, a viewer must not",
            hello.role,
            if hello.capability.is_some() {
                "present"
            } else {
                "absent"
            }
        ));
    }
    if let Some(cert) = cert_role
        && cert != port
    {
        return refuse(format!(
            "refusing to pair: a {} certificate cannot be used on this {} port -- {}",
            role_name(cert),
            role_name(port),
            match port {
                PeerRole::Viewer => "viewers need a viewer certificate (`cert issue-client`)",
                PeerRole::Worker =>
                    "joining workers need a worker certificate (`cert issue-client --role worker` or \
                     `cert issue-token --role worker`)",
            }
        ));
    }
    match (port, hello.role) {
        (PeerRole::Viewer, PeerRole::Viewer) | (PeerRole::Worker, PeerRole::Worker) => {
            HelloCheck::Accepted(hello)
        }
        (PeerRole::Viewer, PeerRole::Worker) => refuse(
            "refusing to pair: this is a VIEWER port -- `indicatrix-worker join` must dial the \
             coordinator's worker port (default 7880; `serve --worker-bind`) of a build with the \
             `worker` feature"
                .to_string(),
        ),
        (PeerRole::Worker, PeerRole::Viewer) => refuse(
            "refusing to pair: this is a coordinator's WORKER port -- viewers connect to the viewer \
             port (default 7878)"
                .to_string(),
        ),
    }
}

/// Logs and writes `refusal` in place of `WELCOME` -- best-effort, like
/// `super::connection::refuse_for_capacity`: the peer may already have hung up, and the
/// warning logged here is the meaningful outcome either way.
pub fn send_refusal<S: Write>(stream: &mut S, refusal: &ErrorMsg) {
    tracing::warn!("{} (code {})", refusal.message, refusal.code);
    let _ = indicatrix_net::messages::write_message(stream, refusal);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_net::messages::{Backend, PROTOCOL_VERSION, RenderCapability};

    fn framed(hello: &Hello) -> std::io::Cursor<Vec<u8>> {
        let mut buf = Vec::new();
        indicatrix_net::messages::write_message(&mut buf, hello).unwrap();
        std::io::Cursor::new(buf)
    }

    fn worker_hello() -> Hello {
        Hello {
            role: PeerRole::Worker,
            capability: Some(RenderCapability {
                backend: Backend::Cpu { threads: 4 },
                max_pixels: 1,
                min_cadence_ms: 100,
                hdr: false,
            }),
            ..Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8])
        }
    }

    fn refused_code(check: &HelloCheck) -> Option<u32> {
        match check {
            HelloCheck::Accepted(_) => None,
            HelloCheck::Refused(e) => Some(e.code),
        }
    }

    /// A v13 peer's three-field `HELLO` (`protocol_version`, `build_hash`, `source_hash`;
    /// postcard encodes a tuple exactly like that struct) is refused with a message naming
    /// both versions.
    #[test]
    fn a_v13_hello_is_refused_naming_both_versions() {
        let mut buf = Vec::new();
        indicatrix_net::messages::write_message(&mut buf, &(13_u16, [1_u8; 8], [2_u8; 8])).unwrap();
        let check =
            read_and_check_hello(&mut std::io::Cursor::new(buf), PeerRole::Viewer, None).unwrap();
        let HelloCheck::Refused(refusal) = check else {
            panic!("a v13 HELLO must be refused");
        };
        assert_eq!(refusal.code, error_codes::BUILD_MISMATCH);
        assert!(
            refusal.message.contains("v13") && refusal.message.contains("v14"),
            "{}",
            refusal.message
        );
    }

    #[test]
    fn a_worker_hello_on_the_viewer_port_is_refused_pointing_at_the_worker_port() {
        let check =
            read_and_check_hello(&mut framed(&worker_hello()), PeerRole::Viewer, None).unwrap();
        let HelloCheck::Refused(refusal) = check else {
            panic!("a worker HELLO must be refused on the viewer port");
        };
        assert_eq!(refusal.code, error_codes::ROLE_REFUSED);
        assert!(refusal.message.contains("join"), "{}", refusal.message);
        assert!(refusal.message.contains("7880"), "{}", refusal.message);
    }

    /// Every combination of port, claimed role and certificate role: only the port's own
    /// role on both counts passes.
    #[test]
    fn the_role_matrix_accepts_only_matching_hello_and_certificate_roles() {
        let viewer = Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]);
        let roles = [PeerRole::Viewer, PeerRole::Worker];
        for port in roles {
            for claimed in roles {
                for cert in [None, Some(PeerRole::Viewer), Some(PeerRole::Worker)] {
                    let hello = match claimed {
                        PeerRole::Viewer => viewer.clone(),
                        PeerRole::Worker => worker_hello(),
                    };
                    let check = check_role(hello, port, cert);
                    let ok = claimed == port && cert.is_none_or(|c| c == port);
                    let expected = (!ok).then_some(error_codes::ROLE_REFUSED);
                    assert_eq!(
                        refused_code(&check),
                        expected,
                        "port={port:?} claimed={claimed:?} cert={cert:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_certificate_mix_up_names_both_roles() {
        let HelloCheck::Refused(refusal) = check_role(
            Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]),
            PeerRole::Viewer,
            Some(PeerRole::Worker),
        ) else {
            panic!("a worker certificate must be refused on the viewer port");
        };
        assert!(
            refusal.message.contains("worker certificate")
                && refusal.message.contains("viewer port"),
            "{}",
            refusal.message
        );
    }

    #[test]
    fn an_inconsistent_role_is_refused_and_a_viewer_is_accepted() {
        let viewer = Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]);
        let mut odd = viewer.clone();
        odd.capability = Some(RenderCapability {
            backend: Backend::Cpu { threads: 4 },
            max_pixels: 1,
            min_cadence_ms: 100,
            hdr: false,
        });
        assert_eq!(
            refused_code(&check_role(odd, PeerRole::Viewer, None)),
            Some(error_codes::ROLE_REFUSED)
        );
        assert!(matches!(
            check_role(viewer, PeerRole::Viewer, Some(PeerRole::Viewer)),
            HelloCheck::Accepted(_)
        ));
        assert!(matches!(
            check_role(worker_hello(), PeerRole::Worker, Some(PeerRole::Worker)),
            HelloCheck::Accepted(_)
        ));
    }
}
