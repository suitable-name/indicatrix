//! Build-compatibility verification.
//!
//! The part of the handshake that refuses to pair a viewer and a worker running
//! different `indicatrix` physics.
//!
//! # Why this check exists, and why it can't be skipped
//!
//! Samples are additive: the remote-offload design rests on `sample_sum +=
//! trace_spectral_ray(..)` being valid regardless of which node computed which term,
//! which is only true if every node runs the SAME `trace_spectral_ray`. Two builds that
//! differ by, say, a spectral-MIS weighting fix produce numbers that both look like
//! plausible radiance -- no NaN, no panic, no obviously wrong magnitude distinguishes
//! "two different physics implementations summed together" from "a converged render".
//!
//! # Two independent identities, checked at two different strengths
//!
//! [`indicatrix::BUILD_ID`] is a hash of `indicatrix`'s crate VERSION, computed in its
//! `build.rs`, so builds of the same release pair across platforms and checkouts -- but
//! that guarantee rests entirely on a release-process promise ("bump the version
//! whenever anything under `src/` that affects a traced sample changes") that nothing
//! enforces. [`indicatrix::SOURCE_HASH`] is a content hash of the actual `indicatrix`
//! source tree (`.rs` and `.wgsl`), so it disagrees the moment physics-affecting source
//! changes even if the version didn't get bumped -- see `indicatrix`'s own `build.rs`
//! doc comment for the full rationale.
//!
//! [`verify_compatible`] therefore runs two checks of different strength: `build_hash`
//! disagreeing (or being [`UNKNOWN_BUILD_HASH`] on either side) is always a hard refusal
//! -- see [`Incompatible::BuildHashMismatch`]/[`Incompatible::UnknownBuild`]. `source_hash`
//! disagreeing is ALSO a hard refusal ([`Incompatible::SourceHash`]), but only when BOTH
//! sides could establish their own source hash; when either side's is unknown, that's
//! logged at `warn!` and pairing proceeds on `build_hash` alone, since an unknown source
//! hash is not itself evidence of a physics mismatch.
//!
//! # This is an accident detector, not a security control
//!
//! `build_hash`/`source_hash` are self-reported values a peer states in its own
//! `HELLO`/`WELCOME` -- nothing here signs or independently verifies them. A hostile
//! peer can trivially claim whatever hash it likes and pass this check while running
//! entirely different physics; [`verify_compatible`] only catches an HONEST peer running
//! a different, unintentionally-mismatched build (the release-process-promise gap the
//! module doc comment above describes). Mutual TLS plus the fingerprint allowlist (see
//! `apps/indicatrix-worker`'s `serve::tls` module) is the actual trust boundary; this
//! check runs strictly AFTER that, over an already-authenticated connection.
//!
//! # Refusal is the only outcome for a KNOWN disagreement
//!
//! [`verify_compatible`] returns `Ok(())` for an exact `build_hash` match (and either an
//! exact `source_hash` match or an unknown `source_hash` on some side) and an
//! [`Incompatible`] error for everything else -- deliberately no "close enough" tier for
//! anything it CAN compare.
//!
//! # Library-only builds
//!
//! [`local_hello`]/[`local_build_hash`]/[`local_source_hash`] need `indicatrix` and are
//! only compiled under this crate's `render` feature. A library-only `indicatrix-worker`
//! never renders, so a physics mismatch can't corrupt anything it does --
//! [`verify_compatible`] stays available unconditionally (pure comparison logic), but a
//! library-only server simply never calls it.

use crate::messages::Hello;

/// Sentinel `build_hash` for a [`indicatrix::BUILD_ID`] that could not be parsed.
///
/// Covers a string that isn't a 16-hex-character content hash. Two unidentifiable
/// builds are exactly the pairing nobody can vouch for, so this value is never treated
/// as compatible with anything, including itself. See [`verify_compatible`].
pub const UNKNOWN_BUILD_HASH: [u8; 8] = [0xFF; 8];

/// Parses a `indicatrix::BUILD_ID`-shaped string into its `[u8; 8]` wire representation.
///
/// Expects exactly 16 lowercase hex characters. Any string that isn't that shape --
/// including `indicatrix`'s own `"unknown"` fallback -- becomes [`UNKNOWN_BUILD_HASH`]
/// rather than panicking, since a malformed build id is itself a sign this build's
/// identity can't be trusted.
#[must_use]
pub fn parse_build_id(id: &str) -> [u8; 8] {
    if id.len() != 16 {
        return UNKNOWN_BUILD_HASH;
    }
    let mut out = [0u8; 8];
    for (i, byte_slot) in out.iter_mut().enumerate() {
        match u8::from_str_radix(&id[i * 2..i * 2 + 2], 16) {
            Ok(b) => *byte_slot = b,
            Err(_) => return UNKNOWN_BUILD_HASH,
        }
    }
    out
}

/// This process's own `indicatrix` build hash, parsed from [`indicatrix::BUILD_ID`].
///
/// Only compiled under this crate's `render` feature -- `indicatrix::BUILD_ID` doesn't
/// exist on a library-only build. Such a build has no indicatrix build to report and
/// skips this check entirely rather than calling it with a placeholder.
#[cfg(feature = "render")]
#[must_use]
pub fn local_build_hash() -> [u8; 8] {
    parse_build_id(indicatrix::BUILD_ID)
}

/// This process's own `indicatrix` source hash, parsed from [`indicatrix::SOURCE_HASH`].
///
/// Parsed exactly the way [`local_build_hash`] parses [`indicatrix::BUILD_ID`] -- see
/// [`parse_build_id`]. Unlike [`local_build_hash`] (a hash of the crate VERSION, a
/// release-process promise), this fingerprints the actual source tree, so it catches a
/// physics-affecting edit that didn't bump the version -- see the module doc comment.
#[cfg(feature = "render")]
#[must_use]
pub fn local_source_hash() -> [u8; 8] {
    parse_build_id(indicatrix::SOURCE_HASH)
}

/// Builds this process's viewer `HELLO` message.
///
/// The current protocol version paired with its own [`local_build_hash`] and
/// [`local_source_hash`], role [`crate::messages::PeerRole::Viewer`], accepting every
/// payload encoding this build supports. Only compiled under this crate's `render`
/// feature.
#[cfg(feature = "render")]
#[must_use]
pub fn local_hello() -> Hello {
    Hello::viewer(
        crate::messages::PROTOCOL_VERSION,
        local_build_hash(),
        local_source_hash(),
    )
}

/// Builds this process's `HELLO` as a render worker joining a coordinator (v14).
///
/// Like [`local_hello`] but with role [`crate::messages::PeerRole::Worker`] and the
/// worker's own `capability`. Only compiled under this crate's `render` feature.
#[cfg(feature = "render")]
#[must_use]
pub fn local_worker_hello(capability: crate::messages::RenderCapability) -> Hello {
    Hello {
        role: crate::messages::PeerRole::Worker,
        capability: Some(capability),
        ..local_hello()
    }
}

/// Why [`read_hello`]/[`decode_hello`] could not produce a [`Hello`].
#[derive(Debug)]
pub enum HelloReadError {
    /// Reading the frame failed, or a same-version `HELLO` did not decode.
    Net(crate::messages::NetError),
    /// The peer speaks another protocol version -- see the variant's `Display`, which
    /// names both versions. Answer with an `ErrorMsg` (code
    /// `error_codes::BUILD_MISMATCH`) built from it.
    Incompatible(Incompatible),
}

impl std::fmt::Display for HelloReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Net(e) => write!(f, "{e}"),
            Self::Incompatible(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HelloReadError {}

impl From<crate::messages::NetError> for HelloReadError {
    fn from(e: crate::messages::NetError) -> Self {
        Self::Net(e)
    }
}

/// Decodes a `HELLO` frame's bytes, version first.
///
/// Every `HELLO` since v12 starts with `protocol_version` (see `messages::hello`'s module
/// doc comment), so this reads that prefix alone first and refuses a different version
/// with [`Incompatible::ProtocolVersionMismatch`] -- a clear, two-version message -- instead
/// of the postcard error an older or newer `HELLO` layout would otherwise produce.
///
/// # Errors
///
/// [`HelloReadError::Incompatible`] for another protocol version,
/// [`HelloReadError::Net`] if the bytes do not decode at all.
pub fn decode_hello(bytes: &[u8]) -> Result<Hello, HelloReadError> {
    let (remote_version, _) =
        postcard::take_from_bytes::<u16>(bytes).map_err(crate::messages::NetError::from)?;
    if remote_version != crate::messages::PROTOCOL_VERSION {
        return Err(HelloReadError::Incompatible(
            Incompatible::ProtocolVersionMismatch {
                local: crate::messages::PROTOCOL_VERSION,
                remote: remote_version,
            },
        ));
    }
    Ok(postcard::from_bytes::<Hello>(bytes).map_err(crate::messages::NetError::from)?)
}

/// The cap [`read_hello`] enforces on the `HELLO` frame's length prefix.
///
/// Comfortably larger than any real `Hello` (a `RenderCapability` and a handful of
/// encoding flags, nowhere near 64 KiB) but far short of
/// [`crate::framing::MAX_FRAME_LEN`] (512 MiB).
///
/// `HELLO` is the first thing read off ANY connection, including one that hasn't
/// authenticated yet (the enrollment listener has no client-certificate check at all;
/// even the render listener's `check_auth` lets a certificate of the wrong role through
/// to this read -- see `apps/indicatrix-worker`'s `serve::tls` module doc comment).
/// A hostile or merely buggy peer's 4-byte length prefix must never be able to
/// commit this process to a `vec![0u8; 512 * 1024 * 1024]` allocation before a single
/// content byte has arrived.
pub const MAX_HELLO_LEN: u32 = 64 * 1024;

/// Reads one frame and decodes it with [`decode_hello`] -- what a server calls instead of
/// a plain `read_message::<Hello>`.
///
/// Uses [`crate::framing::read_frame_bounded`] with [`MAX_HELLO_LEN`], not the 512 MiB
/// [`crate::framing::read_frame`] default: the caller may not have authenticated
/// the peer at all yet.
///
/// # Errors
///
/// See [`decode_hello`]; framing errors (including an oversized length prefix) surface as
/// [`HelloReadError::Net`].
pub fn read_hello<R: std::io::Read>(reader: &mut R) -> Result<Hello, HelloReadError> {
    let bytes = crate::framing::read_frame_bounded(reader, MAX_HELLO_LEN)
        .map_err(crate::messages::NetError::from)?;
    decode_hello(&bytes)
}

/// Why [`verify_compatible`] refused to pair two builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incompatible {
    ProtocolVersionMismatch {
        local: u16,
        remote: u16,
    },
    /// At least one side's build hash is [`UNKNOWN_BUILD_HASH`] -- an unidentifiable
    /// build can never be vouched for, even against another unidentifiable build.
    UnknownBuild,
    BuildHashMismatch {
        local: [u8; 8],
        remote: [u8; 8],
    },
    /// Both sides' `build_hash` agreed, but their `source_hash`es -- established on
    /// BOTH sides -- disagreed: the actual `indicatrix` source tree differs even though
    /// the crate version did not (see the module doc comment). Never raised when either
    /// side's `source_hash` is [`UNKNOWN_BUILD_HASH`] -- that case is logged at `warn!`
    /// instead, see [`verify_compatible`].
    SourceHash {
        local: [u8; 8],
        remote: [u8; 8],
    },
}

impl std::fmt::Display for Incompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProtocolVersionMismatch { local, remote } => {
                write!(
                    f,
                    "protocol version mismatch: this side speaks protocol v{local}, the peer speaks \
                     v{remote} -- the viewer, coordinator and every worker must run the same build"
                )
            }
            Self::UnknownBuild => write!(
                f,
                "at least one side's indicatrix build identity could not be established"
            ),
            Self::BuildHashMismatch { local, remote } => {
                write!(
                    f,
                    "indicatrix version mismatch (build id local={local:02x?}, remote={remote:02x?}): viewer and worker must run the same indicatrix release"
                )
            }
            Self::SourceHash { local, remote } => write!(
                f,
                "indicatrix source mismatch despite matching build id (source hash local={local:02x?}, \
                 remote={remote:02x?}): viewer and worker must run byte-identical indicatrix source"
            ),
        }
    }
}

impl std::error::Error for Incompatible {}

/// Verifies that `local` and `remote` describe compatible `indicatrix` builds.
///
/// Typically one side's own [`local_hello`] and the `HELLO`/`WELCOME` just received
/// from the other side. Runs the two-level check the module doc comment describes:
/// `build_hash` must match exactly (and be known on both sides) or this refuses
/// outright; `source_hash` must ALSO match when both sides could establish their own
/// (a mismatch there refuses too, [`Incompatible::SourceHash`]), but an unknown
/// `source_hash` on either side is only logged at `warn!` -- not a refusal, since an
/// unknown source hash isn't itself evidence of a physics mismatch. `Ok(())` covers
/// every case that isn't a KNOWN disagreement -- deliberately no "close enough" tier
/// for anything this function CAN compare.
///
/// # Errors
///
/// Returns [`Incompatible::ProtocolVersionMismatch`], [`Incompatible::UnknownBuild`],
/// [`Incompatible::BuildHashMismatch`], or [`Incompatible::SourceHash`] for the
/// respective disagreement -- see each variant's doc comment.
pub fn verify_compatible(local: &Hello, remote: &Hello) -> Result<(), Incompatible> {
    if local.protocol_version != remote.protocol_version {
        return Err(Incompatible::ProtocolVersionMismatch {
            local: local.protocol_version,
            remote: remote.protocol_version,
        });
    }
    if local.build_hash == UNKNOWN_BUILD_HASH || remote.build_hash == UNKNOWN_BUILD_HASH {
        return Err(Incompatible::UnknownBuild);
    }
    if local.build_hash != remote.build_hash {
        return Err(Incompatible::BuildHashMismatch {
            local: local.build_hash,
            remote: remote.build_hash,
        });
    }

    if local.source_hash == UNKNOWN_BUILD_HASH || remote.source_hash == UNKNOWN_BUILD_HASH {
        // Not a refusal: an unknown source hash (e.g. `indicatrix`'s `src/` directory
        // wasn't found at build time -- see `build.rs`'s fallback) is not evidence the
        // physics differs, just that this finer check can't be run. `build_hash`
        // already matched above, so pairing proceeds on that alone.
        tracing::warn!(
            "indicatrix source-hash identity could not be established on at least one side \
             (local={:02x?}, remote={:02x?}) -- build_hash matched, so pairing anyway, but this \
             pairing's physics cannot be verified byte-for-byte against its peer",
            local.source_hash,
            remote.source_hash
        );
    } else if local.source_hash != remote.source_hash {
        return Err(Incompatible::SourceHash {
            local: local.source_hash,
            remote: remote.source_hash,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "render")]
    #[test]
    fn identical_hellos_are_compatible() {
        let hello = local_hello();
        assert!(verify_compatible(&hello, &hello).is_ok());
    }

    /// A [`Hello`] with an unknown `source_hash` -- the shape every pre-Finding-15 test
    /// below expects, so a mismatched/matched `build_hash` alone still drives the
    /// outcome (the `source_hash` check only ever warns, never refuses, when unknown).
    fn hello(protocol_version: u16, build_hash: [u8; 8]) -> Hello {
        Hello::viewer(protocol_version, build_hash, UNKNOWN_BUILD_HASH)
    }

    #[test]
    fn mismatched_build_hash_is_refused() {
        let local = hello(1, [1; 8]);
        let remote = hello(1, [2; 8]);
        assert_eq!(
            verify_compatible(&local, &remote),
            Err(Incompatible::BuildHashMismatch {
                local: [1; 8],
                remote: [2; 8]
            })
        );
    }

    #[test]
    fn mismatched_protocol_version_is_refused() {
        let local = hello(1, [1; 8]);
        let remote = hello(2, [1; 8]);
        assert_eq!(
            verify_compatible(&local, &remote),
            Err(Incompatible::ProtocolVersionMismatch {
                local: 1,
                remote: 2
            })
        );
    }

    /// The real current/previous pairing, not the generic `1` vs `2` the test above
    /// uses: a peer still advertising the previous [`crate::messages::PROTOCOL_VERSION`]
    /// is refused against this build's, exercising [`verify_compatible`] -- the same
    /// function [`crate::client::handshake::handshake`] calls -- with the exact values a
    /// real mismatched deploy would produce.
    #[test]
    fn a_peer_advertising_the_previous_protocol_version_is_refused() {
        let current = crate::messages::PROTOCOL_VERSION;
        assert_eq!(
            current, 23,
            "update this pinned value if PROTOCOL_VERSION moves again"
        );
        let local = hello(current, [1; 8]);
        let remote = hello(current - 1, [1; 8]);
        assert_eq!(
            verify_compatible(&local, &remote),
            Err(Incompatible::ProtocolVersionMismatch {
                local: current,
                remote: current - 1,
            })
        );
    }

    #[test]
    fn a_single_byte_difference_is_still_refused_no_close_enough_path() {
        let mut hash = [0xAB; 8];
        let local = hello(1, hash);
        hash[7] ^= 1; // flip one bit deep in the last byte
        let remote = hello(1, hash);
        assert!(verify_compatible(&local, &remote).is_err());
    }

    #[test]
    fn two_unknown_builds_are_never_compatible_with_each_other() {
        let a = hello(1, UNKNOWN_BUILD_HASH);
        let b = hello(1, UNKNOWN_BUILD_HASH);
        assert_eq!(verify_compatible(&a, &b), Err(Incompatible::UnknownBuild));
    }

    #[test]
    fn one_unknown_build_is_refused_even_if_the_other_is_known() {
        let known = hello(1, [3; 8]);
        let unknown = hello(1, UNKNOWN_BUILD_HASH);
        assert_eq!(
            verify_compatible(&known, &unknown),
            Err(Incompatible::UnknownBuild)
        );
        assert_eq!(
            verify_compatible(&unknown, &known),
            Err(Incompatible::UnknownBuild)
        );
    }

    #[test]
    fn matching_build_hash_but_mismatched_known_source_hash_is_refused() {
        let local = Hello::viewer(1, [4; 8], [5; 8]);
        let remote = Hello::viewer(1, [4; 8], [6; 8]);
        assert_eq!(
            verify_compatible(&local, &remote),
            Err(Incompatible::SourceHash {
                local: [5; 8],
                remote: [6; 8],
            })
        );
    }

    #[test]
    fn matching_build_hash_and_source_hash_is_compatible() {
        let local = Hello::viewer(1, [4; 8], [5; 8]);
        assert!(verify_compatible(&local, &local).is_ok());
    }

    #[test]
    fn an_unknown_source_hash_on_either_side_is_a_warning_not_a_refusal() {
        // build_hash matches on both; source_hash unknown on one side (or both) must
        // not refuse the pairing -- see verify_compatible's doc comment.
        let known_source = Hello::viewer(1, [7; 8], [8; 8]);
        let unknown_source = hello(1, [7; 8]); // source_hash: UNKNOWN_BUILD_HASH
        assert!(verify_compatible(&known_source, &unknown_source).is_ok());
        assert!(verify_compatible(&unknown_source, &known_source).is_ok());
        assert!(verify_compatible(&unknown_source, &unknown_source).is_ok());
    }

    /// The exact v13 `HELLO` layout (three fields), for the version-probe tests.
    #[derive(serde::Serialize)]
    struct HelloV13 {
        protocol_version: u16,
        build_hash: [u8; 8],
        source_hash: [u8; 8],
    }

    /// A v13 peer's `HELLO` does not decode as a v14 `Hello` at all; `decode_hello` still
    /// reports a version mismatch naming both versions, not a postcard error.
    #[test]
    fn decode_hello_reports_a_v13_hello_as_a_clear_version_mismatch() {
        let v13 = postcard::to_allocvec(&HelloV13 {
            protocol_version: 13,
            build_hash: [1; 8],
            source_hash: [2; 8],
        })
        .unwrap();
        assert!(postcard::from_bytes::<Hello>(&v13).is_err());
        let err = decode_hello(&v13).unwrap_err();
        let HelloReadError::Incompatible(incompatible) = err else {
            panic!("expected a version mismatch, got {err:?}");
        };
        assert_eq!(
            incompatible,
            Incompatible::ProtocolVersionMismatch {
                local: crate::messages::PROTOCOL_VERSION,
                remote: 13
            }
        );
        let message = incompatible.to_string();
        let local = format!("v{}", crate::messages::PROTOCOL_VERSION);
        assert!(
            message.contains(&local) && message.contains("v13"),
            "{message}"
        );
    }

    #[test]
    fn decode_hello_accepts_a_current_hello_and_rejects_garbage() {
        let current = hello(crate::messages::PROTOCOL_VERSION, [3; 8]);
        let bytes = postcard::to_allocvec(&current).unwrap();
        assert_eq!(decode_hello(&bytes).unwrap(), current);

        let mut framed = Vec::new();
        crate::framing::write_frame(&mut framed, &bytes).unwrap();
        assert_eq!(
            read_hello(&mut std::io::Cursor::new(framed)).unwrap(),
            current
        );

        // The right version prefix but a truncated body: a decode error, not a mismatch.
        let truncated = &bytes[..bytes.len() - 1];
        assert!(matches!(
            decode_hello(truncated),
            Err(HelloReadError::Net(_))
        ));
        assert!(matches!(decode_hello(&[]), Err(HelloReadError::Net(_))));
    }

    /// A hostile length prefix declaring 512 MiB must be refused from the 4-byte
    /// prefix alone, before `read_hello` ever attempts to allocate that much -- proven
    /// here by a reader with no actual payload bytes behind the prefix at all (a
    /// pre-allocation would try to read them and this test would hang/error on the
    /// reader instead of `read_hello` reporting the frame as too large immediately).
    #[test]
    fn read_hello_rejects_a_512_mib_prefix_without_allocating() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(512u32 * 1024 * 1024).to_le_bytes());
        let mut cursor = std::io::Cursor::new(buf);
        let err = read_hello(&mut cursor).unwrap_err();
        let HelloReadError::Net(crate::messages::NetError::Framing(
            crate::framing::FramingError::FrameTooLarge { len, max },
        )) = err
        else {
            panic!("expected a bounded FrameTooLarge refusal, got {err:?}");
        };
        assert_eq!(len, 512 * 1024 * 1024);
        assert_eq!(max, MAX_HELLO_LEN);
    }

    #[test]
    fn parse_build_id_rejects_malformed_strings() {
        assert_eq!(parse_build_id("unknown"), UNKNOWN_BUILD_HASH);
        assert_eq!(parse_build_id(""), UNKNOWN_BUILD_HASH);
        assert_eq!(parse_build_id("zzzzzzzzzzzzzzzz"), UNKNOWN_BUILD_HASH); // 16 chars, not hex
        assert_eq!(parse_build_id("00112233445566778899"), UNKNOWN_BUILD_HASH); // too long
    }

    #[cfg(feature = "render")]
    #[test]
    fn parse_build_id_round_trips_a_real_build_id() {
        let hash = local_build_hash();
        assert_ne!(
            hash, UNKNOWN_BUILD_HASH,
            "indicatrix::BUILD_ID should be a real 16-hex-char content hash in this workspace"
        );
    }

    #[cfg(feature = "render")]
    #[test]
    fn local_source_hash_round_trips_a_real_source_hash() {
        let hash = local_source_hash();
        assert_ne!(
            hash, UNKNOWN_BUILD_HASH,
            "indicatrix::SOURCE_HASH should be a real 16-hex-char content hash in this workspace"
        );
    }
}
