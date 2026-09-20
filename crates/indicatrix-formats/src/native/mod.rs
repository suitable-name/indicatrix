//! The native design file -- a text sidecar that carries design state a paired
//! `.asc` cutting-schedule file has no field for, meant to sit next to (never
//! replace) a real `.asc` export.
//!
//! # The gap this closes
//!
//! `.asc`'s own schema is whatever `GemCAD` (and this crate's own [`asc`](crate::asc)
//! module) can express -- angles, mast distances, index-wheel positions, free-text
//! notes -- and nothing more. An editor built on top of `.asc` naturally wants to
//! author several things that have nowhere to live in that format at all:
//!
//! - A rough blank's own shape and size, independent of any cut facet.
//! - A tier's authored meet constraint as INTENT, not just a recorded number:
//!   `.asc`'s `G` field carries only free-text meet notes, and a constraint that pins
//!   a tier's mast to an exact scale reference has no field to round-trip through at
//!   all -- naive `.asc` reimport instead re-derives every tier's constraint fresh
//!   from geometry, silently discarding a deliberate "meet an existing facet" /
//!   "meet a named facet" choice.
//! - Which of a tier's symmetry-generated facets are exempt from group propagation.
//! - A real-world millimeter binding for the design's girdle diameter -- `.asc` is
//!   unitless.
//! - A material preset and specific-gravity/refractive-index override.
//!
//! # Format: TOML
//!
//! TOML over JSON for a file real people open, diff, and hand-edit: it supports
//! comments (`# this crown was copied from RBC-445`), and its `[[tiers]]`
//! array-of-tables keeps a one-tier edit to a one-line diff. Like JSON it is
//! self-describing (field names travel with the value, unlike `postcard`), so an
//! unrecognized key survives inspection rather than being silently misread by
//! position -- see the `unknown_fields_survive_a_round_trip` test.
//!
//! [`NativeDesignFile`], [`PreformTable`], [`MaterialTable`], and [`TierTable`] each
//! carry a `#[serde(flatten)]` `unknown: toml::Table` field: keys a newer build wrote
//! that this build doesn't claim land there and are written back unchanged rather than
//! dropped. `toml::Table` is an ordered map, never a `HashMap`, so re-serializing a
//! value this build only reads reproduces its exact key order -- what keeps a
//! save-without-editing round trip byte-identical (see the determinism test).
//!
//! # The fingerprint: catching drift between the two files
//!
//! `GemCAD` (or a hand edit) can rewrite the paired `.asc` without this native file
//! ever knowing. [`NativeDesignFile::asc_sha256`] is a SHA-256 over the paired `.asc`
//! file's raw bytes as of this file's last save. [`check_fingerprint`] recomputes
//! that hash at load time and compares -- a mismatch is reported
//! ([`FingerprintCheck::Mismatch`]), never silently trusted either way: a caller
//! pairing this file back up with its `.asc` is expected to keep loading the `.asc`'s
//! own geometry (canonical regardless of what this file says) while skipping any
//! per-tier overlay this file carries, since a changed `.asc` may no longer have the
//! same tiers at the same positions an overlay array was written against. A
//! legitimately re-touched `.asc` is expected, not corruption, so a mismatch is a
//! reported fact, never a hard refusal to load.
//!
//! # Extension and layout
//!
//! `<name>.indicatrix.toml`, sitting next to its paired `<name>.asc` in the same
//! directory; the double extension makes both "plain text" and "which app's schema"
//! visible from the file name alone. Older sidecars saved as `<name>.gemcut.toml`
//! (this format's former name) still load -- see
//! [`LEGACY_NATIVE_EXTENSION_SUFFIX`]. [`NativeDesignFile`] also stores its paired
//! `.asc`'s bare file name (see [`NativeDesignFile::asc_filename`]'s own doc comment),
//! so the two files can be moved together without this one going stale.
//!
//! # Module layout
//!
//! [`path`] is pure `.asc`<->native path arithmetic; [`fingerprint`] is the SHA-256
//! drift check; [`schema`] is the TOML document itself -- the schema types plus
//! [`to_toml_string`]/[`from_toml_str`], its serialize/parse pair. Building a
//! [`NativeDesignFile`] from -- or applying one back onto -- an actual editor design
//! is `indicatrix-cut-core`'s own `native` module's job, not this crate's: this
//! module only owns the on-disk document shape, never the in-memory editor types it
//! mirrors (see that crate's `native` module for the conversions, and for the
//! load/save functions that pair a document built here back up with a real `.asc`).

mod fingerprint;
mod path;
mod schema;
#[cfg(test)]
mod tests;

pub use fingerprint::{FingerprintCheck, check_fingerprint, sha256_hex};
pub use path::{
    LEGACY_NATIVE_EXTENSION_SUFFIX, NATIVE_EXTENSION_SUFFIX, asc_path_for_native,
    native_path_for_asc,
};
pub use schema::{
    FORMAT_VERSION, MaterialTable, NativeDesignFile, NativeFormatError, NativeMeetConstraint,
    NativePreformShape, PreformTable, SourceTable, TierTable, from_toml_str, to_toml_string,
};
