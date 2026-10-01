//! How an edit renumbers the tier list, so the positional multi-selection can follow
//! its tiers instead of silently naming different ones afterwards.
//!
//! `EditorSession::multi_selected` holds row indices. Adding, removing or moving a tier
//! shifts the rows around it, and undoing or redoing such an edit shifts them back; a
//! selection left as it was would then point at unrelated tiers, and a second Delete
//! would remove those.

use indicatrix_cut_core::Edit;
use std::collections::BTreeSet;

/// One positional change an edit makes to the tier list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shift {
    /// A tier was inserted at this position, pushing later tiers down one row.
    Insert(usize),
    /// The tier at this position was removed, pulling later tiers up one row.
    Remove(usize),
    /// The tier at `from` was taken out and re-inserted at `to` (`Vec::remove` then
    /// `Vec::insert`, exactly as `Edit::MoveTier` applies it).
    Move { from: usize, to: usize },
}

impl Shift {
    /// Where the tier that sat at `index` before this change sits after it, or `None`
    /// when this change removed it.
    const fn apply(self, index: usize) -> Option<usize> {
        match self {
            Self::Insert(at) => Some(if index >= at { index + 1 } else { index }),
            Self::Remove(at) => {
                if index == at {
                    None
                } else if index > at {
                    Some(index - 1)
                } else {
                    Some(index)
                }
            }
            Self::Move { from, to } => {
                if index == from {
                    return Some(to);
                }
                let without_moved = if index > from { index - 1 } else { index };
                Some(if without_moved >= to {
                    without_moved + 1
                } else {
                    without_moved
                })
            }
        }
    }
}

/// The row renumbering one edit causes, in the order its parts are applied.
///
/// Built from the `Edit` BEFORE it is applied: `Edit::AddTier`, `Edit::RemoveTier` and
/// `Edit::MoveTier` (alone or inside an `Edit::Batch`, which `History` records for
/// undoing a removal) are the only edits that move rows; every other edit leaves every
/// tier where it was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierIndexMap {
    shifts: Vec<Shift>,
}

impl TierIndexMap {
    /// The renumbering `edit` causes when applied.
    #[must_use]
    pub fn of(edit: &Edit) -> Self {
        let mut shifts = Vec::new();
        collect_shifts(edit, &mut shifts);
        Self { shifts }
    }

    /// Where the tier that sat at `index` before the edit sits after it, or `None`
    /// when the edit removed that tier.
    #[must_use]
    pub fn map(&self, index: usize) -> Option<usize> {
        self.shifts
            .iter()
            .try_fold(index, |current, shift| shift.apply(current))
    }

    /// `selection` renumbered to follow its tiers through the edit; a removed tier
    /// drops out of the selection.
    #[must_use]
    pub fn remap(&self, selection: &BTreeSet<usize>) -> BTreeSet<usize> {
        selection
            .iter()
            .filter_map(|&index| self.map(index))
            .collect()
    }
}

/// Appends `edit`'s row shifts to `shifts`, sub-edits of a batch in application order.
fn collect_shifts(edit: &Edit, shifts: &mut Vec<Shift>) {
    match edit {
        Edit::AddTier { index, .. } => shifts.push(Shift::Insert(*index)),
        Edit::RemoveTier { index } => shifts.push(Shift::Remove(*index)),
        Edit::MoveTier { from, to } => shifts.push(Shift::Move {
            from: *from,
            to: *to,
        }),
        Edit::Batch(edits) => {
            for sub_edit in edits {
                collect_shifts(sub_edit, shifts);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::ConstraintTier;

    fn tier() -> ConstraintTier {
        ConstraintTier {
            angle_deg: -40.0,
            name: String::new(),
            indices: Vec::new(),
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    #[test]
    fn an_insertion_pushes_the_rows_at_and_after_it_down() {
        let map = TierIndexMap::of(&Edit::AddTier {
            index: 2,
            tier: tier(),
        });
        assert_eq!(
            [0, 1, 2, 3].map(|index| map.map(index)),
            [Some(0), Some(1), Some(3), Some(4)]
        );
    }

    #[test]
    fn a_removal_drops_its_row_and_pulls_later_rows_up() {
        let map = TierIndexMap::of(&Edit::RemoveTier { index: 1 });
        assert_eq!(
            [0, 1, 2, 3].map(|index| map.map(index)),
            [Some(0), None, Some(1), Some(2)]
        );
    }

    #[test]
    fn a_move_follows_remove_then_insert() {
        // [a, b, c, d, e] with `a` moved to position 2 is [b, c, a, d, e].
        let down = TierIndexMap::of(&Edit::MoveTier { from: 0, to: 2 });
        assert_eq!(
            [0, 1, 2, 3, 4].map(|index| down.map(index)),
            [Some(2), Some(0), Some(1), Some(3), Some(4)]
        );
        // [a, b, c, d, e] with `d` moved to position 1 is [a, d, b, c, e].
        let up = TierIndexMap::of(&Edit::MoveTier { from: 3, to: 1 });
        assert_eq!(
            [0, 1, 2, 3, 4].map(|index| up.map(index)),
            [Some(0), Some(2), Some(3), Some(1), Some(4)]
        );
    }

    #[test]
    fn a_batch_composes_its_parts_in_order() {
        // The undo of a removal: the tier comes back at its old row.
        let undo_of_removal = Edit::Batch(vec![
            Edit::AddTier {
                index: 1,
                tier: tier(),
            },
            Edit::RestoreTierId {
                index: 1,
                id: indicatrix_cut_core::TierId(9),
            },
        ]);
        let map = TierIndexMap::of(&undo_of_removal);
        assert_eq!(
            [0, 1, 2].map(|index| map.map(index)),
            [Some(0), Some(2), Some(3)]
        );
    }

    #[test]
    fn edits_that_do_not_move_rows_leave_the_selection_alone() {
        let map = TierIndexMap::of(&Edit::RetargetAngles {
            changes: vec![(1, -40.0, -41.0)],
        });
        let selection = BTreeSet::from([0, 2, 4]);
        assert_eq!(map.remap(&selection), selection);
    }

    #[test]
    fn remapping_a_selection_drops_removed_rows_and_renumbers_the_rest() {
        let map = TierIndexMap::of(&Edit::RemoveTier { index: 0 });
        assert_eq!(
            map.remap(&BTreeSet::from([0, 1])),
            BTreeSet::from([0]),
            "row 0 is gone and row 1 is now row 0"
        );
    }
}
