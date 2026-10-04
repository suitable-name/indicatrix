//! Tests for [`super`], split by topic: shared fixtures, basic per-field
//! edits, per-tier annotations (cheater offset/note) and their
//! renumbering/relocation, undo/redo mechanics and validation, index
//! remapping and `SetSchedule`, angle retargeting, coalesced edits, a
//! generated-tier-list property test, `MoveTier`, and `Batch`/`describe`.

mod fixtures;

mod annotations;
mod basic_edits;
mod batch_and_describe;
mod coalescing;
mod concave_flat_interplay;
mod move_tier;
mod property_test;
mod remap_and_schedule;
mod retarget;
mod undo_redo_and_validation;
