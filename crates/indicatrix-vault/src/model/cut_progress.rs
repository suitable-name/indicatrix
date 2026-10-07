//! Cutting progress: the steps of a design the cutter has marked done.
//!
//! Kept in the local library keyed by the design's UUID (see [`super::design_key`]).
//!
//! See `Database::mark_step_done`, `unmark_step`, `cut_progress` and `clear_cut_progress`
//! for the storage side.

/// One step marked done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutProgressMark {
    /// Names the step: the id of the tier (or concave tier) it cuts.
    pub step_key: String,
    /// A fingerprint of the step's cutting values (angle, indices, depth) at the moment
    /// it was marked. When the design's values for that step no longer give this
    /// fingerprint, the mark is out of date and a screen can say so.
    pub step_signature: String,
    /// When the step was marked, in Unix seconds.
    pub done_at: i64,
}
