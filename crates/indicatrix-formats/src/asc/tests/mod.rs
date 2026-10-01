//! Tests for [`super`], split by topic: shared real-fixture data and the
//! round-trip checker, header/tier parsing, header-field validation errors,
//! tier-name/notes-marker quirks, the `to_asc_string` round-trip property,
//! `MeetInstruction` parsing, the 17 adversarial-input regression cases pinned
//! as regression tests, and `to_asc_string`'s own write-time safety
//! rejections.

mod fixtures;

mod adversarial;
mod culet_and_names;
mod encoding;
mod header_and_tier_parsing;
mod header_validation_errors;
mod meet_instruction;
mod round_trip;
mod tier_quirks;
mod write_safety;
