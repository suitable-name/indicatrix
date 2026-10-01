//! [`TierId`]: a stable, never-reused identifier for one [`super::ConstraintTier`]
//! slot in a [`super::Design`], independent of that tier's current position.
//!
//! # Why not a field on `ConstraintTier` itself
//!
//! The natural home for a tier's own identity is a field on [`super::ConstraintTier`]
//! itself. That field would be the right design, but widening that struct would be a
//! breaking change to roughly 83 struct-literal construction sites across 24 files
//! (see [`super::Design::cheater_offsets_deg`]'s own doc comment, which already
//! declined exactly this for the same reason). Instead, [`TierId`]
//! lives in [`super::Design::tier_ids`], a `Vec<TierId>` kept parallel to
//! [`super::Design::tiers`] (same length, same order, moved/inserted/removed in
//! lockstep by every [`crate::edit::Edit`] arm that touches the tier list) --
//! additive, so it costs every existing `ConstraintTier` construction site nothing.
//!
//! # What "stable" means here
//!
//! Allocated once, from the monotonic counter [`super::Design::next_tier_id`], the
//! first time a tier occupies a slot (construction, or [`crate::edit::Edit::AddTier`]);
//! never reused, even after the tier that held it is removed -- so a
//! [`super::Design::tier_targets`] or manufacturability-warning consumer that cached a
//! [`TierId`] across an edit can tell "this is a different tier now" from "the tier I
//! knew moved" without re-deriving anything from position. Survives add/remove/move/
//! undo/redo exactly like every other piece of a tier's own state, because
//! [`super::Design::tier_ids`] is mutated by the very same [`crate::edit::Design::apply_edit`]
//! arms that mutate `tiers`, never independently. The integration tests
//! `undoing_a_tier_removal_restores_the_exact_tier_id` and
//! `history_undo_and_redo_of_a_removal_keep_the_tier_ids` (`tests/edit_roundtrip.rs`)
//! pin this, comparing identities through [`super::Design::tier_ids_eq`].
/// Stable identifier of one tier slot; never reused within a design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TierId(pub u64);

impl TierId {
    /// The raw counter value -- for display/debugging and for the native sidecar
    /// mirror (`indicatrix_formats::native`), which stores this as a plain `u64`.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TierId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}
