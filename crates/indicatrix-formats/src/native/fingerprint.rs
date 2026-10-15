//! SHA-256 content fingerprinting: [`sha256_hex`], used for the integrity hash of a
//! design file's attachments.

use sha2::{Digest, Sha256};
use std::fmt;

/// Lowercase hex SHA-256 of `bytes`.
///
/// SHA-256 (not a faster non-cryptographic hash) only because `sha2` is already this
/// workspace's pinned choice for content fingerprints elsewhere -- there is no
/// adversarial input here, only accidental damage to detect.
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
