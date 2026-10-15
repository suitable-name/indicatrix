//! Tests for [`super`], split by topic: shared fixtures, per-tier field
//! round-trips, refractive index, history/notes, imported-meet/draft handling,
//! catalogue `.asc` preserve-vs-regenerate, untouched `.asc` notes, printed
//! proportions, custom materials, and the preform offset / whole-design round trip.

mod fixtures;

mod custom_material;
mod design_file;
mod design_file_meta;
mod dispersion_model;
mod field_round_trips;
mod history_and_notes;
mod imported_meet_and_drafts;
mod preform_offset_and_full_design;
mod printed_proportions_and_version;
mod refractive_index;
mod relations;
mod save_preserve_regenerate;
mod untouched_notes;
