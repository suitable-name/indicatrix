//! [`crate::design::Design::cutting_sheet`]: a design's printable cutting sequence.
//!
//! This is the library half of what today only exists as raw `.asc` text or a
//! catalogue TSV, neither meant for a cutter to read at the machine.
//! [`crate::design::Design::cutting_sheet`] takes a design and its already-solved masts and
//! returns a [`CuttingSheet`]: every tier, in cutting order, with its angle,
//! index list, solved mast (cutting depth), and a printable meet
//! instruction. The editor's only remaining job is to show [`CuttingSheet::rows`]
//! in a table and hand [`CuttingSheet::to_text`]'s string to Save/Print --
//! see that method's doc comment for the exact layout.
//!
//! [`crate::design::Design::facet_meets`] is the companion read: which other tiers a given
//! tier's own `MeetConstraint::MeetNamed` resolves to, using the exact same
//! `MeetNameResolver` the solver itself uses (girdle/culet/table synonyms,
//! side-prefix and plural stripping, compound vertex specs all included) --
//! not a naive name match.
//!
//! [`diff_tiers`] lives here too: a positional before/after tier comparison
//! (e.g. a saved snapshot against a design's current state) for a "compare
//! two designs" view, sharing this module's `ConstraintTier`/`SolvedTier`
//! imports.

mod build;
mod diff;
mod sheet;

#[cfg(test)]
mod tests;

pub use diff::{ConcaveTierDelta, TierDelta, diff_concave_tiers, diff_tiers};
pub(crate) use sheet::format_index;
pub use sheet::{ConcaveRowInfo, CutSheetRow, CuttingSheet, SHEET_NAME_WIDTH};
