//! A generation-stamp registry for analysis results with no other natural home to track
//! their own "computed against generation N" bookkeeping in.
//!
//! Today the Retarget proposal, whose generation is produced from two different call
//! sites (the Shift-mode rebuild and the async Optimize-mode completion).
//!
//! A UI keeps one [`StaleStamps`] (the desktop in a UI-thread `thread_local!`),
//! [`StaleStamps::stamp`]s a result the moment it lands, [`StaleStamps::clear`]s it
//! when the result is discarded, and asks [`StaleStamps::is_stale`] against the live
//! generation to drive its "Stale: design changed" badge.

use crate::session::result_is_stale;
use std::collections::BTreeMap;

/// Which analysis result a generation stamp names. `Ord` only for `BTreeMap`'s key
/// bound (no `HashMap` iteration in a decision path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultKind {
    /// "Retarget for material"'s held proposal.
    Retarget,
}

/// The generation each [`ResultKind`] was last stamped at; absent means "no result
/// to badge".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaleStamps {
    stamps: BTreeMap<ResultKind, u64>,
}

impl StaleStamps {
    /// An empty registry (usable in a `const` context, e.g. a `thread_local!`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            stamps: BTreeMap::new(),
        }
    }

    /// Records that `kind`'s current result was computed against `generation`.
    pub fn stamp(&mut self, kind: ResultKind, generation: u64) {
        self.stamps.insert(kind, generation);
    }

    /// Forgets `kind`'s stamp, so a badge can never outlive its result.
    pub fn clear(&mut self, kind: ResultKind) {
        self.stamps.remove(&kind);
    }

    /// Forgets every stamp.
    pub fn clear_all(&mut self) {
        self.stamps.clear();
    }

    /// `kind`'s stamped generation, `None` when nothing is stamped.
    #[must_use]
    pub fn get(&self, kind: ResultKind) -> Option<u64> {
        self.stamps.get(&kind).copied()
    }

    /// Whether `kind`'s result is stale against `live_generation` -- never for an
    /// unstamped kind (see [`result_is_stale`]).
    #[must_use]
    pub fn is_stale(&self, kind: ResultKind, live_generation: u64) -> bool {
        result_is_stale(self.get(kind), live_generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_with_no_stamp_is_never_stale() {
        let stamps = StaleStamps::default();
        assert_eq!(stamps.get(ResultKind::Retarget), None);
        assert!(!stamps.is_stale(ResultKind::Retarget, 9));
    }

    #[test]
    fn stamping_then_reading_back_the_same_generation_round_trips() {
        let mut stamps = StaleStamps::default();
        stamps.stamp(ResultKind::Retarget, 7);
        assert_eq!(stamps.get(ResultKind::Retarget), Some(7));
    }

    #[test]
    fn clearing_a_stamped_kind_forgets_it() {
        let mut stamps = StaleStamps::default();
        stamps.stamp(ResultKind::Retarget, 3);
        stamps.clear(ResultKind::Retarget);
        assert_eq!(stamps.get(ResultKind::Retarget), None);
    }

    #[test]
    fn restamping_overwrites_the_previous_generation() {
        let mut stamps = StaleStamps::default();
        stamps.stamp(ResultKind::Retarget, 1);
        stamps.stamp(ResultKind::Retarget, 2);
        assert_eq!(stamps.get(ResultKind::Retarget), Some(2));
    }

    #[test]
    fn a_stamped_result_goes_stale_once_the_generation_moves() {
        let mut stamps = StaleStamps::default();
        stamps.stamp(ResultKind::Retarget, 5);
        assert!(!stamps.is_stale(ResultKind::Retarget, 5));
        assert!(stamps.is_stale(ResultKind::Retarget, 6));
    }
}
