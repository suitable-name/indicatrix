//! Client-side `HELLO`/`WELCOME`, and the "test connection" operation built on it.
//!
//! # Why the reply is decoded two ways
//!
//! A worker replies to `HELLO` with EITHER `WELCOME` (compatible) or `ERROR`
//! (incompatible), written with the same untagged [`crate::messages::write_message`]
//! either way -- and `postcard` is not self-describing, so a real client can't tell
//! which is coming from a tag on the wire. [`handshake`] reads the one reply frame,
//! tries to decode it as [`Welcome`], and falls back to [`ErrorMsg`] if that fails.
//! Decoding uses `postcard::take_from_bytes` and explicitly requires the returned
//! remainder to be empty -- `postcard::from_bytes` alone does NOT enforce that (postcard
//! 1.1.3 happily ignores trailing bytes after a structurally valid decode), so without
//! this check an accidental cross-decode would only need byte length to be *at least*
//! enough. Requiring the whole frame consumed narrows that back down to needing every
//! field to also land on a valid discriminant/length -- vanishingly unlikely for two
//! structurally different types.
//!
//! # Why the client re-verifies compatibility itself
//!
//! The worker already refuses an incompatible `HELLO`. [`handshake`] does not simply
//! trust that: when BOTH sides have render capacity, it also runs
//! [`crate::handshake::verify_compatible`] against the returned `WELCOME`, refusing
//! with [`ClientError::Incompatible`] even if the worker's own check somehow passed
//! something it shouldn't have. Two independent checks, neither trusting the other --
//! there is no runtime signal distinguishing "two different physics builds summed
//! together" from "a converged render", so refusal has to be the default on either
//! side noticing a mismatch.
//!
//! # Why the indicatrix build-hash check is skipped for a library-only peer
//!
//! [`crate::handshake::local_hello`] needs `indicatrix` (only compiled under this
//! crate's `render` feature) -- a library-only client has no indicatrix build to report,
//! and a physics mismatch can never corrupt a connection that never renders. So this
//! module sends [`crate::handshake::UNKNOWN_BUILD_HASH`] on a non-`render` build, and
//! only runs [`crate::handshake::verify_compatible`] when this build has render
//! capacity AND the worker's `WELCOME` says it does too -- otherwise a fine
//! library-only pairing would always be refused by `verify_compatible`'s "an unknown
//! build is never compatible with anything" rule, correct for two render-capable peers
//! but wrong for two that neither render.

use super::ClientError;
use crate::{
    handshake,
    messages::{self, ErrorMsg, Hello, RenderCapability, Welcome},
};
use std::io::{Read, Write};

/// This client's own `HELLO`.
///
/// Uses [`handshake::local_hello`] when this build has render capacity (this crate's `render`
/// feature) and [`handshake::UNKNOWN_BUILD_HASH`] otherwise -- see the module doc comment.
#[cfg(all(feature = "render", not(feature = "zoning")))]
fn local_hello_for_this_build() -> Hello {
    handshake::local_hello()
}

/// A zoning build always advertises the zoning capability (see `crate::messages::zoning`).
#[cfg(all(feature = "render", feature = "zoning"))]
fn local_hello_for_this_build() -> Hello {
    handshake::local_hello_zoning()
}

#[cfg(not(feature = "render"))]
fn local_hello_for_this_build() -> Hello {
    Hello::viewer(
        messages::PROTOCOL_VERSION,
        handshake::UNKNOWN_BUILD_HASH,
        handshake::UNKNOWN_BUILD_HASH,
    )
}

/// Performs `HELLO`/`WELCOME` over `stream` and, when both sides have render capacity,
/// verifies indicatrix build/protocol compatibility.
///
/// Sends this process's own [`local_hello_for_this_build`], reads the one reply frame,
/// and tries to decode it as [`Welcome`] first, falling back to [`ErrorMsg`] (see the
/// module doc comment). On a successful `WELCOME` whose [`Welcome::render`] is `Some`
/// -- and only when this build itself has render capacity too -- additionally runs
/// [`handshake::verify_compatible`] against it before returning.
///
/// # Errors
///
/// [`ClientError::Net`] for a transport-level failure writing `HELLO` or reading the
/// reply frame. [`ClientError::Refused`] if the worker replied `ERROR`.
/// [`ClientError::Incompatible`] if the worker replied `WELCOME` with render capacity
/// but this client's own compatibility check still refuses it.
/// [`ClientError::MalformedHandshakeReply`] if the reply decoded as neither.
///
/// A reply that is neither but starts with a DIFFERENT protocol version (an older or
/// newer server's `WELCOME`, whose layout differs -- see `messages::hello`'s "version-probe
/// prefix") is reported as [`ClientError::Incompatible`] with
/// [`handshake::Incompatible::ProtocolVersionMismatch`], naming both versions.
pub fn handshake<S: Read + Write>(stream: &mut S) -> Result<Welcome, ClientError> {
    handshake_with_hello(stream, &local_hello_for_this_build())
}

/// [`handshake`] with a caller-built `HELLO` -- e.g. a narrower `accept_encodings` list,
/// or a worker's own `handshake::local_worker_hello` when joining a coordinator.
///
/// # Errors
///
/// See [`handshake`].
pub fn handshake_with_hello<S: Read + Write>(
    stream: &mut S,
    local: &Hello,
) -> Result<Welcome, ClientError> {
    #[cfg(not(feature = "zoning"))]
    messages::write_message(stream, local)?;
    // A zoning build appends the capability marker when `local.zoning` is set; otherwise the
    // frame is byte-identical to `write_message`'s.
    #[cfg(feature = "zoning")]
    messages::write_hello_message(stream, local)?;

    let raw = crate::framing::read_frame_bounded(stream, crate::framing::MAX_CONTROL_FRAME_LEN)
        .map_err(crate::messages::NetError::Framing)?;

    // A zoning build also accepts the capability marker after `WELCOME` (stripped here, so the
    // checks below run unchanged); every other trailing byte is still refused.
    #[cfg(feature = "zoning")]
    let (raw, zoning_tail) = messages::split_welcome_tail(raw);

    if let Ok((welcome, remainder)) = postcard::take_from_bytes::<Welcome>(&raw)
        && remainder.is_empty()
    {
        #[cfg(feature = "zoning")]
        let welcome = Welcome {
            zoning: zoning_tail,
            ..welcome
        };
        #[cfg(feature = "render")]
        if welcome.render.is_some() {
            let remote_as_hello = Hello::viewer(
                welcome.protocol_version,
                welcome.build_hash,
                welcome.source_hash,
            );
            handshake::verify_compatible(local, &remote_as_hello)
                .map_err(ClientError::Incompatible)?;
        }
        return Ok(welcome);
    }

    if let Ok((err, remainder)) = postcard::take_from_bytes::<ErrorMsg>(&raw)
        && remainder.is_empty()
    {
        return Err(ClientError::Refused(err));
    }

    if let Ok((remote, _)) = postcard::take_from_bytes::<u16>(&raw)
        && remote != messages::PROTOCOL_VERSION
    {
        return Err(ClientError::Incompatible(
            handshake::Incompatible::ProtocolVersionMismatch {
                local: messages::PROTOCOL_VERSION,
                remote,
            },
        ));
    }

    Err(ClientError::MalformedHandshakeReply)
}

/// What a "test connection" operation reports back to the settings UI -- everything a
/// user picking/configuring a worker would want to see, without committing to a render.
///
/// Mirrors [`Welcome`] directly: [`Self::render`] is `None` for a library-only worker
/// (check this before ever attempting to send a `RenderRequest`), [`Self::library`]
/// reports the read-only design-library protocol's availability, and
/// [`Self::tilt_curves`] reports `TILT_CURVES` availability (check before ever sending a
/// `TiltCurvesRequest`) -- see [`Welcome`]'s own doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// Wire protocol version the peer speaks.
    pub protocol_version: u16,
    /// Short hash identifying the peer build.
    pub build_hash: [u8; 8],
    /// Render capability the peer offers, if any.
    pub render: Option<RenderCapability>,
    /// Whether the peer serves the design library.
    pub library: bool,
    /// Whether the peer computes tilt curves.
    pub tilt_curves: bool,
    /// The payload encoding the server negotiated (v14) -- see
    /// [`Welcome::payload_encoding`].
    pub payload_encoding: crate::messages::PayloadEncoding,
    /// `zoning` builds only: whether the peer accepts `ZoningPayload` messages
    /// ([`Welcome::zoning`]).
    #[cfg(feature = "zoning")]
    pub zoning: bool,
}

impl From<Welcome> for ConnectionInfo {
    fn from(w: Welcome) -> Self {
        Self {
            protocol_version: w.protocol_version,
            build_hash: w.build_hash,
            render: w.render,
            library: w.library,
            tilt_curves: w.tilt_curves,
            payload_encoding: w.payload_encoding,
            #[cfg(feature = "zoning")]
            zoning: w.zoning,
        }
    }
}

/// The settings UI's "Test connection" button.
///
/// Connect (the caller has already established `stream`, e.g. a fresh mutual-TLS
/// `TcpStream`), handshake, report worker identity/backend/build compatibility -- then
/// the caller disconnects (simply by dropping `stream`) once this returns. No
/// `RenderRequest` is ever sent.
///
/// # Errors
///
/// Whatever [`handshake`] returns.
pub fn test_connection<S: Read + Write>(stream: &mut S) -> Result<ConnectionInfo, ClientError> {
    handshake(stream).map(ConnectionInfo::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{PROTOCOL_VERSION, write_message};
    use std::io::Cursor;

    /// A `Read + Write` over two independent in-memory buffers, standing in for one
    /// end of a duplex connection.
    struct DuplexHalf {
        in_: Cursor<Vec<u8>>,
        out: Vec<u8>,
    }

    impl DuplexHalf {
        fn new(input: Vec<u8>) -> Self {
            Self {
                in_: Cursor::new(input),
                out: Vec::new(),
            }
        }
    }

    impl Read for DuplexHalf {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.in_.read(buf)
        }
    }

    impl Write for DuplexHalf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.out.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A `WELCOME` this client's own `local_hello_for_this_build()` will always find
    /// compatible with itself, regardless of whether this crate's `render` feature is
    /// on.
    fn scripted_welcome() -> Welcome {
        Welcome {
            protocol_version: PROTOCOL_VERSION,
            build_hash: local_hello_for_this_build().build_hash,
            source_hash: local_hello_for_this_build().source_hash,
            render: Some(RenderCapability {
                backend: crate::messages::Backend::Cpu { threads: 8 },
                max_pixels: 8_294_400,
                min_cadence_ms: 100,
                hdr: false,
            }),
            library: true,
            tilt_curves: true,
            registration: None,
            payload_encoding: crate::messages::PayloadEncoding::Raw,
            #[cfg(feature = "zoning")]
            zoning: false,
        }
    }

    /// A v13 server's `WELCOME` (no `registration`/`payload_encoding`) is reported as a
    /// protocol-version mismatch naming both versions, not as a malformed reply.
    #[test]
    fn a_v13_welcome_is_reported_as_a_version_mismatch() {
        #[derive(serde::Serialize)]
        struct WelcomeV13 {
            protocol_version: u16,
            build_hash: [u8; 8],
            source_hash: [u8; 8],
            render: Option<RenderCapability>,
            library: bool,
            tilt_curves: bool,
        }
        let mut input = Vec::new();
        write_message(
            &mut input,
            &WelcomeV13 {
                protocol_version: 13,
                build_hash: [1; 8],
                source_hash: [2; 8],
                render: None,
                library: true,
                tilt_curves: false,
            },
        )
        .unwrap();
        let mut duplex = DuplexHalf::new(input);

        let err = handshake(&mut duplex).unwrap_err();
        let ClientError::Incompatible(incompatible) = &err else {
            panic!("expected ClientError::Incompatible, got {err:?}");
        };
        assert_eq!(
            *incompatible,
            handshake::Incompatible::ProtocolVersionMismatch {
                local: PROTOCOL_VERSION,
                remote: 13
            }
        );
        let message = err.to_string();
        let local = format!("v{PROTOCOL_VERSION}");
        assert!(
            message.contains("v13") && message.contains(&local),
            "{message}"
        );
    }

    #[test]
    fn handshake_succeeds_on_a_matching_welcome_and_sends_hello_first() {
        let mut input = Vec::new();
        write_message(&mut input, &scripted_welcome()).unwrap();
        let mut duplex = DuplexHalf::new(input);

        let welcome = handshake(&mut duplex).unwrap();
        assert_eq!(welcome, scripted_welcome());

        // The HELLO this client sent is exactly `local_hello_for_this_build()`.
        let mut out_cursor = Cursor::new(duplex.out);
        let sent_hello: Hello = messages::read_message(&mut out_cursor).unwrap();
        // The capability marker (zoning build) is not part of the decoded struct.
        #[cfg(feature = "zoning")]
        let expected_hello = Hello {
            zoning: false,
            ..local_hello_for_this_build()
        };
        #[cfg(not(feature = "zoning"))]
        let expected_hello = local_hello_for_this_build();
        assert_eq!(sent_hello, expected_hello);
    }

    /// A zoning client appends the capability marker to its `HELLO`; a worker's reply carrying
    /// the marker sets `Welcome::zoning`, a plain reply leaves it clear.
    #[cfg(feature = "zoning")]
    #[test]
    fn a_zoning_handshake_sends_the_marker_and_reads_the_workers_marker() {
        for worker_zoning in [false, true] {
            let mut input = Vec::new();
            messages::write_welcome_message(
                &mut input,
                &Welcome {
                    zoning: worker_zoning,
                    ..scripted_welcome()
                },
            )
            .unwrap();
            let mut duplex = DuplexHalf::new(input);
            let welcome = handshake(&mut duplex).unwrap();
            assert_eq!(welcome.zoning, worker_zoning);
            assert!(
                duplex.out.ends_with(&messages::ZONING_TAIL),
                "the HELLO ends with the marker"
            );
        }
    }

    /// A reply with a marker-shaped tail is the only extra thing accepted; the plain default
    /// worker reply (no tail) decodes exactly as before.
    #[cfg(feature = "zoning")]
    #[test]
    fn a_zoning_handshake_still_refuses_other_trailing_bytes() {
        let mut payload = postcard::to_allocvec(&scripted_welcome()).unwrap();
        payload.extend_from_slice(&[0x5A, 0x4E, 0x02]); // wrong marker version
        let mut input = Vec::new();
        crate::framing::write_frame(&mut input, &payload).unwrap();
        let err = handshake(&mut DuplexHalf::new(input)).unwrap_err();
        assert!(matches!(err, ClientError::MalformedHandshakeReply));
    }

    #[test]
    fn handshake_reports_refusal_when_the_worker_sends_an_error_reply() {
        let mut input = Vec::new();
        write_message(
            &mut input,
            &ErrorMsg {
                code: 1,
                message: "refusing to pair: build hash mismatch".to_string(),
                request_id: None,
            },
        )
        .unwrap();
        let mut duplex = DuplexHalf::new(input);

        let err = handshake(&mut duplex).unwrap_err();
        match err {
            ClientError::Refused(e) => {
                assert_eq!(e.code, 1);
                assert!(e.message.contains("mismatch"));
            }
            other => panic!("expected ClientError::Refused, got {other:?}"),
        }
    }

    /// Even when the worker replies with a well-formed `WELCOME`, this client's own
    /// [`handshake::verify_compatible`] check must still refuse a build-hash mismatch --
    /// the defense-in-depth check, distinct from the worker refusing on its own side.
    /// Only meaningful when this build itself has render capacity.
    #[cfg(feature = "render")]
    #[test]
    fn handshake_refuses_locally_even_when_the_worker_claims_compatibility() {
        let mut mismatched = scripted_welcome();
        mismatched.build_hash = [0xAB; 8];
        assert_ne!(mismatched.build_hash, handshake::local_build_hash());

        let mut input = Vec::new();
        write_message(&mut input, &mismatched).unwrap();
        let mut duplex = DuplexHalf::new(input);

        let err = handshake(&mut duplex).unwrap_err();
        assert!(
            matches!(err, ClientError::Incompatible(_)),
            "expected ClientError::Incompatible, got {err:?}"
        );
    }

    #[test]
    fn handshake_reports_a_malformed_reply_as_neither_welcome_nor_error() {
        let mut input = Vec::new();
        // Neither a valid Welcome nor a valid ErrorMsg encoding.
        crate::framing::write_frame(&mut input, &[0xFF, 0xFF, 0xFF]).unwrap();
        let mut duplex = DuplexHalf::new(input);

        let err = handshake(&mut duplex).unwrap_err();
        assert!(matches!(err, ClientError::MalformedHandshakeReply));
    }

    /// A reply frame that decodes as a valid `Welcome` PLUS trailing garbage
    /// must not be accepted -- `postcard::from_bytes` alone would silently ignore the
    /// extra bytes; `take_from_bytes` with an empty-remainder check must not.
    #[test]
    fn handshake_rejects_a_welcome_with_trailing_bytes_after_it() {
        let mut input = Vec::new();
        let mut payload = postcard::to_allocvec(&scripted_welcome()).unwrap();
        payload.push(0xEE); // trailing garbage past the structurally valid Welcome
        crate::framing::write_frame(&mut input, &payload).unwrap();
        let mut duplex = DuplexHalf::new(input);

        let err = handshake(&mut duplex).unwrap_err();
        assert!(
            matches!(err, ClientError::MalformedHandshakeReply),
            "expected ClientError::MalformedHandshakeReply, got {err:?}"
        );
    }

    #[test]
    fn test_connection_reports_worker_identity_without_sending_a_render_request() {
        let mut input = Vec::new();
        write_message(&mut input, &scripted_welcome()).unwrap();
        let mut duplex = DuplexHalf::new(input);

        let info = test_connection(&mut duplex).unwrap();
        let render = info
            .render
            .expect("scripted_welcome always advertises render capacity");
        assert_eq!(render.backend, crate::messages::Backend::Cpu { threads: 8 });
        assert_eq!(render.max_pixels, 8_294_400);
        assert_eq!(render.min_cadence_ms, 100);
        assert!(info.library);

        // Only the HELLO frame went out -- nothing that could decode as a
        // ClientMessage/RenderRequest.
        let mut out_cursor = Cursor::new(duplex.out.clone());
        let _hello: Hello = messages::read_message(&mut out_cursor).unwrap();
        assert_eq!(
            out_cursor.position(),
            out_cursor.get_ref().len() as u64,
            "test_connection must send nothing beyond HELLO"
        );
    }
}
