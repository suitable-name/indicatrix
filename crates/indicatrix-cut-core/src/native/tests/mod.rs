//! Tests for [`super`], split by topic: shared fixtures, per-tier field
//! round-trips, refractive index, history/notes, fingerprint mismatches,
//! imported-meet/draft handling, save preserve-vs-regenerate, untouched
//! `.asc` notes, printed proportions/format version, custom materials, and
//! the preform offset/native-only autosave path.

mod fixtures;

mod custom_material;
mod design_file;
mod design_file_meta;
mod field_round_trips;
mod fingerprint_mismatch;
mod history_and_notes;
mod imported_meet_and_drafts;
mod preform_offset_and_native_only;
mod printed_proportions_and_version;
mod refractive_index;
mod save_preserve_regenerate;
mod untouched_notes;
