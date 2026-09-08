//! The native `.indicatrix.toml` sidecar format (legacy `.gemcut.toml` files still
//! load), paired with (never replacing) a real `.asc` export, carrying design state
//! `.asc` has no field for at all.
//!
//! The on-disk document shape itself -- the schema, TOML encode/decode, sidecar path
//! rules, and the paired-`.asc` fingerprint -- lives in [`indicatrix_formats::native`], a
//! dependency-free-of-editor-types format crate this module re-exports from (see that
//! module's own doc comment for the format, "the gap this closes", and the
//! fingerprint). What stays HERE is everything that needs this crate's own editor
//! types in scope: [`convert`]'s free functions between the on-disk mirror types and
//! [`crate::design::Design`]/[`crate::preform::PreformSpec`]/
//! [`crate::material::MaterialSelection`]/`indicatrix`'s own
//! [`indicatrix::geometry::meet_solver::MeetConstraint`] (see [`convert`]'s own doc
//! comment for why these are free functions, not `From`/`Into` impls), and the two
//! directions built on top of them: [`load`]'s [`load_paired`] and [`save`]'s
//! [`save_paired`].
//!
//! # Preserving the original `.asc` text
//!
//! [`indicatrix_formats::asc::to_asc_string`] is round-trip-stable but not byte-identical to
//! hand-authored `GemCAD` output. Saving a design that came from a catalogue `.asc`
//! and was never actually edited should not reformat that file just because this
//! native format now exists: [`save_paired`] takes the caller's already-loaded
//! original `.asc` text and preserves it byte for byte whenever the design's current
//! [`crate::design::Design::to_asc_schedule`] output is semantically equal to what
//! that text itself parses to -- i.e. nothing authored actually changed, even if only
//! `girdle_diameter_mm`/`material`/`preform` were touched (none of which round-trip
//! into `.asc`). Once a real tier/preform edit breaks that equality, this regenerates
//! a fresh `.asc` instead -- see the
//! `save_paired_preserves_untouched_text`/`save_paired_regenerates_after_a_real_edit`
//! tests.

mod convert;
mod load;
mod save;
#[cfg(test)]
mod tests;

pub use convert::to_native_file;
pub use load::{LoadPairedError, LoadPairedResult, TierOverlay, load_paired};
pub use save::{PairedSave, SaveError, save_paired};

/// Kept under its historic name -- `apps/indicatrix-cut` calls this as
/// `indicatrix_cut_core::native::parse_toml_string`.
pub use indicatrix_formats::native::from_toml_str as parse_toml_string;
pub use indicatrix_formats::native::{
    FORMAT_VERSION, FingerprintCheck, LEGACY_NATIVE_EXTENSION_SUFFIX, MaterialTable,
    NATIVE_EXTENSION_SUFFIX, NativeDesignFile, NativeFormatError, NativeMeetConstraint,
    NativePreformShape, PreformTable, TierTable, asc_path_for_native, check_fingerprint,
    native_path_for_asc, sha256_hex, to_toml_string,
};
