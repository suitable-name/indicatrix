//! Collecting one design's two view results, which arrive separately (possibly from two
//! different lanes), together with the version of the design's record they were rendered
//! from -- see the parent module's `finish_item_at_revision`, the one caller.

use crate::bridge::preview_render::PreviewView;
use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

/// Which version of a design's record a render was made from, as the catalogue's
/// `diagram_entries.updated_at` stamp read together with that record.
///
/// A design's two views are separate items that resolve the record independently,
/// possibly on different lanes and seconds apart. [`Self::merge`] folds their revisions
/// together: if they disagree, the design was edited between the two resolves and the
/// pair describes two different designs, so it must not be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui::batch::preview) enum RecordRevision {
    /// No item of the design reported the revision it resolved from.
    Unknown,
    /// Every reporting item resolved the record at this `updated_at` (`None` being a
    /// row that was never stamped).
    Stamp(Option<i64>),
    /// Two items resolved the record at different revisions.
    Conflicting,
}

impl RecordRevision {
    /// Combines the revisions two items of the same design resolved from: an unknown
    /// side defers to the other, equal stamps stay, unequal stamps conflict.
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, known) | (known, Self::Unknown) => known,
            (Self::Stamp(a), Self::Stamp(b)) if a == b => Self::Stamp(a),
            _ => Self::Conflicting,
        }
    }
}

/// One design's front/top views as they trickle in from (possibly) two different
/// lanes, plus how many of its (always 2) items are still outstanding. Lives in
/// `super::spawn_preview_batch`'s shared `design_state` map for exactly as long as at
/// least one of a design's two items hasn't finished yet -- [`record_item_result`]
/// removes the entry the moment `remaining` reaches `0` and hands the finished pair to
/// its caller for saving.
pub(in crate::gui::batch::preview) struct DesignAccum {
    front: Option<Vec<u8>>,
    top: Option<Vec<u8>>,
    remaining: u8,
    /// The record version the items so far resolved from, merged.
    revision: RecordRevision,
}

/// A finished design's front/top pair and the record version it was rendered from,
/// ready to save -- what [`record_item_result`] hands back once both items are in.
pub(super) struct FinishedViews {
    pub(super) front: Option<Vec<u8>>,
    pub(super) top: Option<Vec<u8>>,
    pub(super) revision: RecordRevision,
}

/// Records one item's result (`bytes`, `None` on any failure) and the record `revision`
/// it was rendered from against `entry_id`'s accumulator, creating it on first touch.
/// Returns the finished pair -- ready to save -- the moment this was the design's LAST
/// outstanding item, regardless of which lane produced either result; `None` while the
/// design still has an item in flight elsewhere.
pub(super) fn record_item_result(
    design_state: &Mutex<HashMap<i64, DesignAccum>>,
    entry_id: i64,
    view: PreviewView,
    bytes: Option<Vec<u8>>,
    revision: RecordRevision,
) -> Option<FinishedViews> {
    let mut map = design_state.lock().unwrap_or_else(PoisonError::into_inner);
    let accum = map.entry(entry_id).or_insert_with(|| DesignAccum {
        front: None,
        top: None,
        remaining: 2,
        revision: RecordRevision::Unknown,
    });
    match view {
        PreviewView::Front => accum.front = bytes,
        PreviewView::Top => accum.top = bytes,
    }
    accum.revision = accum.revision.merge(revision);
    accum.remaining = accum.remaining.saturating_sub(1);
    if accum.remaining > 0 {
        return None;
    }
    map.remove(&entry_id).map(|done| FinishedViews {
        front: done.front,
        top: done.top,
        revision: done.revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_defers_to_the_known_side_and_conflicts_on_unequal_stamps() {
        let seen = RecordRevision::Stamp(Some(5));
        assert_eq!(RecordRevision::Unknown.merge(seen), seen);
        assert_eq!(seen.merge(RecordRevision::Unknown), seen);
        assert_eq!(
            RecordRevision::Unknown.merge(RecordRevision::Unknown),
            RecordRevision::Unknown
        );
        assert_eq!(seen.merge(RecordRevision::Stamp(Some(5))), seen);
        assert_eq!(
            seen.merge(RecordRevision::Stamp(Some(6))),
            RecordRevision::Conflicting
        );
        assert_eq!(
            seen.merge(RecordRevision::Stamp(None)),
            RecordRevision::Conflicting,
            "an unstamped row is a different version from a stamped one"
        );
        assert_eq!(
            RecordRevision::Conflicting.merge(seen),
            RecordRevision::Conflicting
        );
        assert_eq!(
            seen.merge(RecordRevision::Conflicting),
            RecordRevision::Conflicting
        );
    }

    #[test]
    fn record_item_result_hands_back_the_pair_once_with_the_merged_revision() {
        let state = Mutex::new(HashMap::new());
        let revision = RecordRevision::Stamp(Some(5));
        assert!(
            record_item_result(&state, 7, PreviewView::Top, Some(vec![2]), revision).is_none(),
            "the front view is still outstanding"
        );
        let done = record_item_result(&state, 7, PreviewView::Front, Some(vec![1]), revision)
            .expect("both items are in");
        assert_eq!(done.front, Some(vec![1]));
        assert_eq!(done.top, Some(vec![2]));
        assert_eq!(done.revision, revision);
        assert!(
            state.lock().unwrap().is_empty(),
            "a finished design leaves no accumulator behind"
        );
    }

    #[test]
    fn items_resolved_at_different_revisions_finish_as_conflicting() {
        let state = Mutex::new(HashMap::new());
        let _ = record_item_result(
            &state,
            7,
            PreviewView::Front,
            Some(vec![1]),
            RecordRevision::Stamp(Some(5)),
        );
        let done = record_item_result(
            &state,
            7,
            PreviewView::Top,
            Some(vec![2]),
            RecordRevision::Stamp(Some(6)),
        )
        .expect("both items are in");
        assert_eq!(done.revision, RecordRevision::Conflicting);
    }

    #[test]
    fn a_failed_item_without_a_revision_does_not_weaken_the_other_items_stamp() {
        let state = Mutex::new(HashMap::new());
        let _ = record_item_result(&state, 7, PreviewView::Front, None, RecordRevision::Unknown);
        let done = record_item_result(
            &state,
            7,
            PreviewView::Top,
            Some(vec![2]),
            RecordRevision::Stamp(Some(5)),
        )
        .expect("both items are in");
        assert_eq!(done.front, None);
        assert_eq!(done.revision, RecordRevision::Stamp(Some(5)));
    }
}
