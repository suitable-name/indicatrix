//! SHA-256 content fingerprinting: [`sha256_hex`] itself, and
//! [`FingerprintCheck`]/[`check_fingerprint`], the drift detector between a
//! native file and its paired `.asc` -- see the parent module's doc comment
//! ("The fingerprint: catching drift between the two files") for why this
//! exists and how a mismatch is handled (reported, never a hard refusal).

use sha2::{Digest, Sha256};
use std::fmt;

/// Lowercase hex SHA-256 of `bytes`.
///
/// This is the fingerprint [`crate::native::NativeDesignFile::asc_sha256`] stores and
/// [`check_fingerprint`] recomputes. SHA-256 (not a faster non-cryptographic hash)
/// only because `sha2` is already this workspace's pinned choice for content
/// fingerprints elsewhere -- there is no adversarial input here, only accidental
/// drift to detect.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // `write!` to a `String` never fails; `Result` here is only because the
        // trait is shared with I/O-backed writers.
        use fmt::Write as _;
        write!(out, "{byte:02x}").expect("writing hex digits to a String cannot fail");
    }
    out
}

/// Whether a loaded [`crate::native::NativeDesignFile`]'s recorded fingerprint still
/// matches its paired `.asc`'s actual current bytes -- see the module doc comment's
/// "The fingerprint" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FingerprintCheck {
    /// The paired `.asc` has not changed since this native file was last saved.
    Match,
    /// The paired `.asc`'s current content hashes differently than what this native
    /// file recorded -- it was edited (by `GemCAD`, by hand, or by anything else)
    /// without this native file being re-saved to match. A real, expected scenario
    /// (see the module doc comment), not a corrupted file.
    Mismatch {
        expected_sha256: String,
        found_sha256: String,
    },
}

impl fmt::Display for FingerprintCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Match => write!(f, "fingerprint matches the paired .asc file"),
            Self::Mismatch {
                expected_sha256,
                found_sha256,
            } => write!(
                f,
                "the paired .asc file has changed since this native file was last saved \
                 (recorded sha256 {expected_sha256}, current sha256 {found_sha256}) -- the \
                 .asc's own geometry was loaded as-is; this file's per-tier meet-intent \
                 overlay was NOT reapplied, since it may no longer line up with the changed \
                 tiers"
            ),
        }
    }
}

/// Recomputes `asc_bytes`'s SHA-256 and compares it against `native.asc_sha256`.
#[must_use]
pub fn check_fingerprint(
    native: &crate::native::NativeDesignFile,
    asc_bytes: &[u8],
) -> FingerprintCheck {
    let found_sha256 = sha256_hex(asc_bytes);
    if found_sha256.eq_ignore_ascii_case(&native.asc_sha256) {
        FingerprintCheck::Match
    } else {
        FingerprintCheck::Mismatch {
            expected_sha256: native.asc_sha256.clone(),
            found_sha256,
        }
    }
}
