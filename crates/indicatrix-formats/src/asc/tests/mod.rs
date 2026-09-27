//! Tests for [`super`], split by topic: shared real-fixture data and the
//! round-trip checker, header/tier parsing, header-field validation errors,
//! tier-name/notes-marker quirks, the `to_asc_string` round-trip property,
//! and `MeetInstruction` parsing.

mod fixtures;

mod header_and_tier_parsing;
mod header_validation_errors;
mod meet_instruction;
mod round_trip;
mod tier_quirks;
