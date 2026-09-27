//! [`AscTier`]: one `a` record -- a single facet tier at a given angle and
//! mast (height) setting, occurring at one or more index-wheel positions.

use super::meet_instruction::{MeetInstruction, parse_meet_instruction};

/// One `a` record: a single facet tier at a given angle and mast (height) setting,
/// occurring at one or more index-wheel positions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AscTier {
    /// Signed angle from the girdle plane, in degrees. `GemCAD` convention: negative is
    /// pavilion, non-negative is crown (0 deg is a flat facet -- table on the crown
    /// side, culet on the pavilion side; distinguishing the two when the file doesn't
    /// bother signing the culet's zero is the caller's job, see `cuts.rs`).
    pub angle_deg: f64,
    /// The "mast" / height setting: how far the facet plane is cut from the stone's
    /// center, always stored as `GemCAD` wrote it (a small fraction of files use a
    /// negative mast for one special near-zero-angle facet; callers should take the
    /// magnitude when turning this into a plane offset).
    pub mast: f64,
    /// Facet name(s) as written in the file (e.g. "P1", "C7", "G1", "1", "U"). Empty
    /// when the file leaves the facet unnamed, which is common -- roughly two-thirds
    /// of real tier records never name a facet at all. When more than one distinct
    /// name shares a single tier (rare, but real), they are joined with `/`.
    pub name: String,
    /// Every index-wheel position at which this tier's facet occurs. Usually
    /// integers, but `GemCAD` allows fractional positions (about 0.2% of index tokens
    /// in the sampled corpus) for angles that don't land exactly on a gear tooth.
    pub indices: Vec<f64>,
    /// Free-text notes after the `G` marker, if any (e.g. "Cut to mast depth X.").
    pub notes: String,
}

impl AscTier {
    /// Every distinct name this tier is known by, split back out of the joined
    /// `name` field (see that field's doc comment for why more than one name folds
    /// into a single `/`-joined string). Empty if the tier is unnamed.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        if self.name.is_empty() {
            Vec::new()
        } else {
            self.name.split('/').collect()
        }
    }

    /// Parses `notes` (the raw text after this tier's `G` marker, if any) into a
    /// structured [`MeetInstruction`]. Returns `None` when there are no notes at all.
    ///
    /// This is computed on demand from the same text [`super::to_asc_string`] writes back
    /// out verbatim -- there is no separate stored field, so there is nothing that
    /// could drift out of sync with the raw text or put the round-trip property at
    /// risk.
    #[must_use]
    pub fn meet_instruction(&self) -> Option<MeetInstruction> {
        parse_meet_instruction(&self.notes)
    }
}
