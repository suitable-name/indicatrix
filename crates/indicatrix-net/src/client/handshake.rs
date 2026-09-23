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

/// This client's own `HELLO`, using [`handshake::local_hello`] when this build has
/// render capacity (this crate's `render` feature) and
/// [`handshake::UNKNOWN_BUILD_HASH`] otherwise -- see the module doc comment.
#[cfg(feature = "render")]
fn local_hello_for_this_build() -> Hello {
    handshake::local_hello()
}

#[cfg(not(feature = "render"))]
const fn local_hello_for_this_build() -> Hello {
    Hello {
        protocol_version: messages::PROTOCOL_VERSION,
        build_hash: handshake::UNKNOWN_BUILD_HASH,
        source_hash: handshake::UNKNOWN_BUILD_HASH,
    }
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
pub fn handshake<S: Read + Write>(stream: &mut S) -> Result<Welcome, ClientError> {
    let local = local_hello_for_this_build();
    messages::write_message(stream, &local)?;

    let raw = crate::framing::read_frame(stream).map_err(crate::messages::NetError::Framing)?;

    if let Ok((welcome, remainder)) = postcard::take_from_bytes::<Welcome>(&raw)
        && remainder.is_empty()
    {
        #[cfg(feature = "render")]
        if welcome.render.is_some() {
            let remote_as_hello = Hello {
                protocol_version: welcome.protocol_version,
                build_hash: welcome.build_hash,
                source_hash: welcome.source_hash,
            };
            handshake::verify_compatible(&local, &remote_as_hello)
                .map_err(ClientError::Incompatible)?;
        }
        return Ok(welcome);
    }

    if let Ok((err, remainder)) = postcard::take_from_bytes::<ErrorMsg>(&raw)
        && remainder.is_empty()
    {
        return Err(ClientError::Refused(err));
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
    pub protocol_version: u16,
    pub build_hash: [u8; 8],
    pub render: Option<RenderCapability>,
    pub library: bool,
    pub tilt_curves: bool,
}

impl From<Welcome> for ConnectionInfo {
    fn from(w: Welcome) -> Self {
        Self {
            protocol_version: w.protocol_version,
            build_hash: w.build_hash,
            render: w.render,
            library: w.library,
            tilt_curves: w.tilt_curves,
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
            }),
            library: true,
            tilt_curves: true,
        }
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
        assert_eq!(sent_hello, local_hello_for_this_build());
    }

    #[test]
    fn handshake_reports_refusal_when_the_worker_sends_an_error_reply() {
        let mut input = Vec::new();
        write_message(
            &mut input,
            &ErrorMsg {
                code: 1,
                message: "refusing to pair: build hash mismatch".to_string(),
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
