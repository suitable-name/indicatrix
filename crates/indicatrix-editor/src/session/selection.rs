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
    /// The whole tier list was replaced (`Edit::ReplaceSchedule`): no row can be
    /// followed through it, so every row drops out of the selection.
    Clear,
}

impl Shift {
    /// Where the tier that sat at `index` before this change sits after it, or `None`
    /// when this change removed it.
    const fn apply(self, index: usize) -> Option<usize> {
        match self {
            Self::Clear => None,
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
/// undoing a removal), and their concave twins for the concave list, are the only
/// edits that move rows; every other edit leaves every tier where it was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierIndexMap {
    shifts: Vec<Shift>,
    /// The same for `design.concave_tiers`, which numbers independently of the flat
    /// list: a concave edit never shifts a flat row and vice versa.
    concave_shifts: Vec<Shift>,
}

impl TierIndexMap {
    /// The renumbering `edit` causes when applied.
    #[must_use]
    pub fn of(edit: &Edit) -> Self {
        let mut map = Self::default();
        collect_shifts(edit, &mut map);
        map
    }

    /// [`Self::map`] for a position in `design.concave_tiers`.
    #[must_use]
    pub fn map_concave(&self, index: usize) -> Option<usize> {
        self.concave_shifts
            .iter()
            .try_fold(index, |current, shift| shift.apply(current))
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

    /// Appends the renumbering of a change that happened after this one, so the map
    /// then describes both together: a jump along the history is several undos or redos,
    /// and one row has to be followed through all of them.
    pub fn extend(&mut self, later: &Self) {
        self.shifts.extend_from_slice(&later.shifts);
        self.concave_shifts.extend_from_slice(&later.concave_shifts);
    }

    /// Where the row `row` of a tier table sits after the change this map describes, or
    /// `None` when the change removed its tier.
    ///
    /// A tier table lists the flat tiers first and the concave tiers after them, so a row
    /// at or past `flat_before` (the flat tier count before the change) names a concave
    /// tier. `flat_after` and `concave_after` are the counts after it: a row that lands
    /// outside them names nothing and is dropped too. This is the desktop's single
    /// selected row, which `EditorSession::multi_selected` cannot hold because it
    /// numbers the flat tiers alone.
    #[must_use]
    pub fn map_table_row(
        &self,
        row: usize,
        flat_before: usize,
        flat_after: usize,
        concave_after: usize,
    ) -> Option<usize> {
        if row < flat_before {
            return self.map(row).filter(|&mapped| mapped < flat_after);
        }
        self.map_concave(row - flat_before)
            .filter(|&mapped| mapped < concave_after)
            .map(|mapped| flat_after + mapped)
    }
}

/// Appends `edit`'s row shifts to `map`, sub-edits of a batch in application order.
fn collect_shifts(edit: &Edit, map: &mut TierIndexMap) {
    match edit {
        Edit::AddTier { index, .. } => map.shifts.push(Shift::Insert(*index)),
        Edit::RemoveTier { index } => map.shifts.push(Shift::Remove(*index)),
        Edit::MoveTier { from, to } => map.shifts.push(Shift::Move {
            from: *from,
            to: *to,
        }),
        Edit::AddConcaveTier { index, .. } => map.concave_shifts.push(Shift::Insert(*index)),
        Edit::RemoveConcaveTier { index } => map.concave_shifts.push(Shift::Remove(*index)),
        Edit::MoveConcaveTier { from, to } => map.concave_shifts.push(Shift::Move {
            from: *from,
            to: *to,
        }),
        Edit::ReplaceSchedule(_) => map.shifts.push(Shift::Clear),
        Edit::Batch(edits) => {
            for sub_edit in edits {
                collect_shifts(sub_edit, map);
            }
        }
        // Every other edit moves no row of either list.
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
    fn tier_index_map_is_identity_for_concave_edits() {
        use indicatrix_cut_core::{
            TierId,
            design::{ConcaveTier, ConcaveTool, ToolMotion},
        };
        let concave = ConcaveTier {
            name: "Groove".to_owned(),
            angle_deg: -40.0,
            indices: vec![0.0],
            instructions: String::new(),
            tool: ConcaveTool::Cylinder,
            tool_azimuth_deg: 0.0,
            displacement: [0.0; 3],
            diameter_ratio: 0.5,
            tool_angle_deg: None,
            motion: ToolMotion::Reciprocating,
        };
        let edits = [
            Edit::AddConcaveTier {
                index: 0,
                tier: concave.clone(),
            },
            Edit::RemoveConcaveTier { index: 0 },
            Edit::ModifyConcaveTier {
                index: 0,
                tier: concave,
            },
            Edit::MoveConcaveTier { from: 0, to: 1 },
            Edit::RestoreConcaveTierId {
                index: 0,
                id: TierId(3),
            },
            Edit::RestoreConcaveIndices {
                tiers: vec![(0, vec![1.0])],
            },
        ];
        for edit in &edits {
            let map = TierIndexMap::of(edit);
            assert_eq!(
                [0, 1, 2, 3].map(|index| map.map(index)),
                [Some(0), Some(1), Some(2), Some(3)],
                "{edit:?} must not move flat rows"
            );
        }
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
    fn relation_edits_move_no_rows_and_leave_a_batch_to_its_other_parts() {
        use indicatrix_cut_core::{
            TierId,
            design::{RelationExpr, TierRelation},
        };
        let relation = TierRelation::new(RelationExpr::offset_from(TierId(0), -2.0));
        for edit in [
            Edit::SetTierRelation {
                index: 1,
                relation: Some(relation),
            },
            Edit::SetTierRelation {
                index: 1,
                relation: None,
            },
        ] {
            let map = TierIndexMap::of(&edit);
            assert_eq!(
                [0, 1, 2, 3].map(|index| map.map(index)),
                [Some(0), Some(1), Some(2), Some(3)],
                "{edit:?} must not move rows"
            );
        }
        // A removal that also frees a relation and moves a follower: only the removal
        // renumbers rows.
        let batch = Edit::Batch(vec![
            Edit::RemoveTier { index: 0 },
            Edit::SetTierRelation {
                index: 0,
                relation: None,
            },
            Edit::RetargetAngles {
                changes: vec![(0, -40.0, -41.0)],
            },
        ]);
        let map = TierIndexMap::of(&batch);
        assert_eq!(
            [0, 1, 2].map(|index| map.map(index)),
            [None, Some(0), Some(1)]
        );
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

    #[test]
    fn replacing_the_whole_schedule_drops_every_selected_row() {
        let design = indicatrix_cut_core::Design::fresh(
            indicatrix_cut_core::PreformSpec::block(1.0, 1.0, 2.0),
            96,
            4,
            1.62,
        );
        let replace =
            Edit::ReplaceSchedule(Box::new(indicatrix_cut_core::ScheduleState::of(&design)));
        let map = TierIndexMap::of(&replace);
        assert_eq!(
            [0, 1, 2].map(|index| map.map(index)),
            [None, None, None],
            "no row can be followed through a whole-list replacement"
        );
        assert!(map.remap(&BTreeSet::from([0, 2])).is_empty());
        // The concave list is not part of the replacement.
        assert_eq!(map.map_concave(1), Some(1));
    }

    #[test]
    fn concave_edits_renumber_the_concave_list_only() {
        let concave = indicatrix_cut_core::Design::concave_fixture()
            .concave_tiers
            .remove(0);
        let insert = TierIndexMap::of(&Edit::AddConcaveTier {
            index: 1,
            tier: concave,
        });
        assert_eq!(
            [0, 1, 2].map(|index| insert.map_concave(index)),
            [Some(0), Some(2), Some(3)]
        );
        let remove = TierIndexMap::of(&Edit::RemoveConcaveTier { index: 0 });
        assert_eq!(
            [0, 1].map(|index| remove.map_concave(index)),
            [None, Some(0)]
        );
        let mv = TierIndexMap::of(&Edit::MoveConcaveTier { from: 0, to: 1 });
        assert_eq!(
            [0, 1, 2].map(|index| mv.map_concave(index)),
            [Some(1), Some(0), Some(2)]
        );
        // A flat edit leaves the concave list alone.
        let flat = TierIndexMap::of(&Edit::RemoveTier { index: 0 });
        assert_eq!(flat.map_concave(0), Some(0));
    }

    #[test]
    fn two_changes_extended_into_one_map_move_a_row_like_the_changes_in_turn() {
        // [a, b, c, d] with `d` moved to the front is [d, a, b, c]; then `b` (now row 2)
        // is removed: [d, a, c].
        let mut both = TierIndexMap::of(&Edit::MoveTier { from: 3, to: 0 });
        both.extend(&TierIndexMap::of(&Edit::RemoveTier { index: 2 }));
        assert_eq!(
            [0, 1, 2, 3].map(|index| both.map(index)),
            [Some(1), None, Some(2), Some(0)]
        );
        // Following a tier through the second change after the first gives the same.
        let first = TierIndexMap::of(&Edit::MoveTier { from: 3, to: 0 });
        let second = TierIndexMap::of(&Edit::RemoveTier { index: 2 });
        for index in 0..4 {
            assert_eq!(
                both.map(index),
                first.map(index).and_then(|moved| second.map(moved))
            );
        }
        // The concave list is carried the same way.
        let mut concave = TierIndexMap::of(&Edit::RemoveConcaveTier { index: 0 });
        concave.extend(&TierIndexMap::of(&Edit::RemoveConcaveTier { index: 0 }));
        assert_eq!(
            [0, 1, 2].map(|index| concave.map_concave(index)),
            [None, None, Some(0)]
        );
    }

    /// A table lists the flat tiers first, then the concave ones.
    #[test]
    fn a_table_row_follows_its_tier_whether_it_is_flat_or_concave() {
        // Four flat tiers and two concave tiers (rows 0-3 and 4-5). Tier 1 is removed and
        // concave tier 0 is inserted at the front: three flat tiers, three concave tiers.
        let mut map = TierIndexMap::of(&Edit::RemoveTier { index: 1 });
        let concave = indicatrix_cut_core::Design::concave_fixture()
            .concave_tiers
            .remove(0);
        map.extend(&TierIndexMap::of(&Edit::AddConcaveTier {
            index: 0,
            tier: concave,
        }));
        let after = |row| map.map_table_row(row, 4, 3, 3);
        // Flat rows: tier 0 stays, tier 1 is gone, tiers 2 and 3 move up.
        assert_eq!([0, 1, 2, 3].map(after), [Some(0), None, Some(1), Some(2)]);
        // Concave rows: they start at row 3 now, and the inserted tier pushed both down.
        assert_eq!([4, 5].map(after), [Some(4), Some(5)]);
        // A row past the old table names nothing.
        assert_eq!(after(6), None);
    }

    #[test]
    fn a_table_row_that_lands_outside_the_new_table_is_dropped() {
        // An insertion pushes the last flat tier past the end the caller reports.
        let map = TierIndexMap::of(&Edit::AddTier {
            index: 0,
            tier: tier(),
        });
        assert_eq!(map.map_table_row(2, 3, 4, 0), Some(3));
        assert_eq!(
            map.map_table_row(2, 3, 3, 0),
            None,
            "row 3 does not exist in a table of three"
        );
    }
}
