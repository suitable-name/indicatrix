//! Build-compatibility verification: the part of the handshake that refuses to pair a
//! viewer and a worker running different `indicatrix` physics.
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
//! # Where the build identity comes from
//!
//! [`indicatrix::BUILD_ID`] is a deterministic content hash of `indicatrix`'s own
//! `src/**/*.rs` tree, computed in `indicatrix`'s `build.rs`. This module only parses
//! that hex string into the `[u8; 8]` wire representation and compares it.
//!
//! # Refusal is the only outcome
//!
//! [`verify_compatible`] returns `Ok(())` for an exact match and an [`Incompatible`]
//! error for everything else, including two builds whose hash could not be established
//! at all (see [`UNKNOWN_BUILD_HASH`]) -- deliberately no "close enough" tier.
//!
//! # Library-only builds
//!
//! [`local_hello`]/[`local_build_hash`] need `indicatrix` and are only compiled under
//! this crate's `render` feature. A library-only `indicatrix-worker` never renders, so
//! a physics mismatch can't corrupt anything it does -- [`verify_compatible`] stays
//! available unconditionally (pure comparison logic), but a library-only server simply
//! never calls it.

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

/// Builds this process's `HELLO` message: the current protocol version paired with its
/// own [`local_build_hash`]. Only compiled under this crate's `render` feature.
#[cfg(feature = "render")]
#[must_use]
pub fn local_hello() -> Hello {
    Hello {
        protocol_version: crate::messages::PROTOCOL_VERSION,
        build_hash: local_build_hash(),
    }
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
}

impl std::fmt::Display for Incompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProtocolVersionMismatch { local, remote } => {
                write!(
                    f,
                    "protocol version mismatch: local={local}, remote={remote}"
                )
            }
            Self::UnknownBuild => write!(
                f,
                "at least one side's indicatrix build identity could not be established"
            ),
            Self::BuildHashMismatch { local, remote } => {
                write!(
                    f,
                    "indicatrix build hash mismatch: local={local:02x?}, remote={remote:02x?}"
                )
            }
        }
    }
}

impl std::error::Error for Incompatible {}

/// Verifies that `local` and `remote` describe compatible `indicatrix` builds.
///
/// Typically one side's own [`local_hello`] and the `HELLO`/`WELCOME` just received
/// from the other side. `Ok(())` only for an exact protocol-version and build-hash
/// match; refusal (an [`Incompatible`] variant) is the only other outcome, deliberately
/// with no "close enough" tier -- see the module docs.
///
/// # Errors
///
/// Returns [`Incompatible::ProtocolVersionMismatch`], [`Incompatible::UnknownBuild`],
/// or [`Incompatible::BuildHashMismatch`] for the respective disagreement -- see each
/// variant's doc comment.
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

    #[test]
    fn mismatched_build_hash_is_refused() {
        let local = Hello {
            protocol_version: 1,
            build_hash: [1; 8],
        };
        let remote = Hello {
            protocol_version: 1,
            build_hash: [2; 8],
        };
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
        let local = Hello {
            protocol_version: 1,
            build_hash: [1; 8],
        };
        let remote = Hello {
            protocol_version: 2,
            build_hash: [1; 8],
        };
        assert_eq!(
            verify_compatible(&local, &remote),
            Err(Incompatible::ProtocolVersionMismatch {
                local: 1,
                remote: 2
            })
        );
    }

    #[test]
    fn a_single_byte_difference_is_still_refused_no_close_enough_path() {
        let mut hash = [0xAB; 8];
        let local = Hello {
            protocol_version: 1,
            build_hash: hash,
        };
        hash[7] ^= 1; // flip one bit deep in the last byte
        let remote = Hello {
            protocol_version: 1,
            build_hash: hash,
        };
        assert!(verify_compatible(&local, &remote).is_err());
    }

    #[test]
    fn two_unknown_builds_are_never_compatible_with_each_other() {
        let a = Hello {
            protocol_version: 1,
            build_hash: UNKNOWN_BUILD_HASH,
        };
        let b = Hello {
            protocol_version: 1,
            build_hash: UNKNOWN_BUILD_HASH,
        };
        assert_eq!(verify_compatible(&a, &b), Err(Incompatible::UnknownBuild));
    }

    #[test]
    fn one_unknown_build_is_refused_even_if_the_other_is_known() {
        let known = Hello {
            protocol_version: 1,
            build_hash: [3; 8],
        };
        let unknown = Hello {
            protocol_version: 1,
            build_hash: UNKNOWN_BUILD_HASH,
        };
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
}
