//! Reading the undo history as a list and moving through it by position.
//!
//! The History panel shows every step the session has taken (the done ones and the undone
//! ones), lets the cutter jump to any of them, and draws a small picture of the design at
//! each. This module is the plain-data half of that:
//!
//! - [`EditorSession::history_entries`] and [`EditorSession::history_position`] read the
//!   list;
//! - [`EditorSession::jump_to`] moves the design to a position in one call. It is only
//!   walking along the history (the same undos and redos), so it is not itself a step: the
//!   steps it walks over stay where they are and a jump back is another jump. Making an
//!   edit after a jump back drops the undone steps, exactly as after an ordinary undo;
//! - [`EditorSession::design_at`] and [`HistorySnapshot`] work out a copy of the design at
//!   another position without touching the session, so a worker thread can draw it.
//!
//! # Generation
//!
//! A jump bumps the generation ONCE at the end, however many steps it walked, so a
//! background job sees one change, and the saved mark compares the same way it would after
//! one edit. The selections are renumbered after every single step, as repeated undos
//! would, so they keep following their tiers.

use super::{EditChange, EditorSession, TierIndexMap};
use indicatrix_cut_core::{Design, EditError, History, HistoryEntry, JumpError};
use std::{fmt, sync::atomic::Ordering};

/// What [`EditorSession::jump_to`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumpOutcome {
    /// What changed, for refreshing the display; `None` when the design already stood at
    /// the position and nothing moved.
    pub change: Option<EditChange>,
    /// The step the design stands at now.
    pub position: usize,
}

/// Why [`EditorSession::jump_to`] did not reach the position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpFailure {
    /// The reason. For [`JumpError::Replay`], `at` is the step the design stands at now.
    pub error: JumpError,
    /// What changed before the failure, if some steps were walked: the display must be
    /// refreshed for it. `None` when nothing moved.
    pub change: Option<EditChange>,
}

impl fmt::Display for JumpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}

impl std::error::Error for JumpFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// A detached copy of a design and its history, from which the design at any step can be
/// worked out on another thread while the session goes on being edited.
#[derive(Debug, Clone)]
pub struct HistorySnapshot {
    design: Design,
    history: History,
}

impl HistorySnapshot {
    /// The steps the snapshot's history holds, oldest first -- see [`History::entries`].
    #[must_use]
    pub fn entries(&self) -> Vec<HistoryEntry> {
        self.history.entries()
    }

    /// The step the snapshot's design stands at.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.history.len_undo()
    }

    /// A copy of the design at `position` -- see [`EditorSession::design_at`].
    ///
    /// # Errors
    ///
    /// [`History::design_at`]'s error.
    pub fn design_at(&self, position: usize) -> Result<Design, JumpError> {
        self.history.design_at(&self.design, position)
    }
}

impl EditorSession {
    /// Every step of the history, oldest first, the undone ones last -- see
    /// [`History::entries`]. Entry `i` is at position `i + 1`.
    #[must_use]
    pub fn history_entries(&self) -> Vec<HistoryEntry> {
        self.history.entries()
    }

    /// The words for the Undo hint: what the next undo takes back. A step that was given its
    /// own words ([`History::apply_labeled`], like opening a saved variant) keeps them;
    /// otherwise the words are those of the edit the undo replays
    /// ([`indicatrix_cut_core::Edit::describe`]). Empty when there is nothing to undo.
    #[must_use]
    pub fn undo_hint(&self) -> String {
        self.history.undo_label().map_or_else(
            || {
                self.history
                    .peek_undo()
                    .map_or_else(String::new, |edit| edit.describe(&self.design))
            },
            str::to_owned,
        )
    }

    /// [`Self::undo_hint`] for the next redo.
    #[must_use]
    pub fn redo_hint(&self) -> String {
        self.history.redo_label().map_or_else(
            || {
                self.history
                    .peek_redo()
                    .map_or_else(String::new, |edit| edit.describe(&self.design))
            },
            str::to_owned,
        )
    }

    /// The step the design stands at: 0 before any step, up to the number of steps.
    #[must_use]
    pub const fn history_position(&self) -> usize {
        self.history.len_undo()
    }

    /// A copy of the design as it stands at `position` (0 is the design before any step),
    /// worked out by replaying the steps between here and there on the copy. The session is
    /// not touched: this is what a "save this step as a variant" or a thumbnail uses.
    ///
    /// # Errors
    ///
    /// [`JumpError::OutOfRange`] when `position` is past the last step;
    /// [`JumpError::Replay`] when a step cannot be replayed.
    pub fn design_at(&self, position: usize) -> Result<Design, JumpError> {
        self.history.design_at(&self.design, position)
    }

    /// A detached copy of the design and its history -- see [`HistorySnapshot`].
    #[must_use]
    pub fn history_snapshot(&self) -> HistorySnapshot {
        HistorySnapshot {
            design: self.design.clone(),
            history: self.history.clone(),
        }
    }

    /// Moves the design to `position` -- as many undos or redos as it takes -- and bumps the
    /// generation once if anything moved. See this module's documentation: a jump is not
    /// itself a step, so you can jump back.
    ///
    /// # Errors
    ///
    /// [`JumpFailure`] with [`JumpError::OutOfRange`] before anything moves when `position`
    /// is past the last step. With [`JumpError::Replay`] when a step cannot be replayed: the
    /// design is left at the last step that worked, the failure's `change` says what moved
    /// before it, and the history agrees with the design.
    pub fn jump_to(&mut self, position: usize) -> Result<JumpOutcome, JumpFailure> {
        self.jump_to_mapped(position).0
    }

    /// [`Self::jump_to`], also returning how the steps it replayed renumbered the tier
    /// rows (all of them composed into one map, also when the jump stopped on a failed
    /// step), for a caller that keeps a row selection of its own and must make it follow
    /// its tier the way [`Self::multi_selected`] does. See [`TierIndexMap::map_table_row`]
    /// and, for the counts it needs, [`JumpOutcome::change`].
    pub fn jump_to_mapped(
        &mut self,
        position: usize,
    ) -> (Result<JumpOutcome, JumpFailure>, TierIndexMap) {
        let mut composed = TierIndexMap::default();
        let steps = self.history.len_total();
        if position > steps {
            let failure = JumpFailure {
                error: JumpError::OutOfRange { position, steps },
                change: None,
            };
            return (Err(failure), composed);
        }
        self.expire_relation_notice();
        let before = self.tier_counts();
        let mut moved = false;
        let failure = loop {
            let current = self.history.len_undo();
            if current == position {
                break None;
            }
            match self.step_history(position > current, &mut composed) {
                Ok(true) => moved = true,
                // Nothing left to replay in that direction, which the range check above
                // rules out; stop rather than loop.
                Ok(false) => break None,
                Err(error) => break Some(error),
            }
        };
        let change = moved.then(|| self.finish_jump(before));
        let result = match failure {
            None => Ok(JumpOutcome {
                change,
                position: self.history.len_undo(),
            }),
            Some(error) => Err(JumpFailure {
                error: JumpError::Replay {
                    at: self.history.len_undo(),
                    error,
                },
                change,
            }),
        };
        (result, composed)
    }

    /// One undo (`forward == false`) or redo (`forward == true`) with the selections
    /// renumbered after it and the generation left alone. The step's row renumbering is
    /// appended to `composed`.
    fn step_history(
        &mut self,
        forward: bool,
        composed: &mut TierIndexMap,
    ) -> Result<bool, EditError> {
        let next = if forward {
            self.history.peek_redo()
        } else {
            self.history.peek_undo()
        };
        let renumbering = next.map(TierIndexMap::of);
        let moved = if forward {
            self.history.redo(&mut self.design)?
        } else {
            self.history.undo(&mut self.design)?
        };
        if moved {
            self.renumber_selection(renumbering.as_ref());
            if let Some(step) = &renumbering {
                composed.extend(step);
            }
        }
        Ok(moved)
    }

    /// The single generation bump of a jump, and the description of what it changed.
    fn finish_jump(&self, (tier_count_before, concave_count_before): (usize, usize)) -> EditChange {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let (tier_count_after, concave_count_after) = self.tier_counts();
        EditChange {
            generation,
            tier_count_before,
            tier_count_after,
            concave_count_before,
            concave_count_after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{ConstraintTier, Edit};
    use std::time::Duration;

    fn tier(name: &str, angle_deg: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: Vec::new(),
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// A session with three tiers added (steps 1 to 3) and the design after each step
    /// (`snapshots[0]` is the design before any).
    fn three_adds() -> (EditorSession, Vec<Design>) {
        let mut session = EditorSession::fresh();
        let mut snapshots = vec![session.design.clone()];
        for (index, (name, angle)) in [("P1", -40.0), ("P2", -35.0), ("C1", 30.0)]
            .into_iter()
            .enumerate()
        {
            session
                .apply(Edit::AddTier {
                    index,
                    tier: tier(name, angle),
                })
                .expect("add must apply");
            snapshots.push(session.design.clone());
        }
        (session, snapshots)
    }

    #[test]
    fn jump_to_reaches_every_step_and_bumps_the_generation_once() {
        let (mut session, snapshots) = three_adds();
        for target in [0, 2, 3, 3, 1, 3, 0] {
            let before = session.current_generation();
            let moved = session.history_position() != target;
            let outcome = session.jump_to(target).expect("jump must succeed");
            assert_eq!(outcome.position, target);
            assert_eq!(session.history_position(), target);
            assert_eq!(session.design, snapshots[target], "the design at {target}");
            assert_eq!(
                session.current_generation(),
                before + u64::from(moved),
                "one bump per jump that moved, however many steps it walked"
            );
            assert_eq!(outcome.change.is_some(), moved);
            if let Some(change) = outcome.change {
                assert_eq!(change.generation, session.current_generation());
                assert_eq!(change.tier_count_after, snapshots[target].tiers.len());
                assert!(change.tier_count_changed());
            }
        }
        assert_eq!(
            session.history_entries().len(),
            3,
            "a jump never loses a step"
        );
    }

    #[test]
    fn a_jump_is_not_a_step_so_it_can_be_jumped_back() {
        let (mut session, snapshots) = three_adds();
        session.jump_to(1).unwrap();
        assert!(session.history.can_redo());
        assert_eq!(session.history.len_redo(), 2);
        session.jump_to(3).unwrap();
        assert_eq!(session.design, snapshots[3]);
    }

    #[test]
    fn jumping_to_where_the_design_stands_changes_nothing() {
        let (mut session, _) = three_adds();
        let generation = session.current_generation();
        let outcome = session.jump_to(3).unwrap();
        assert_eq!(outcome.change, None);
        assert_eq!(outcome.position, 3);
        assert_eq!(session.current_generation(), generation);
    }

    #[test]
    fn jumping_past_the_last_step_is_refused_and_changes_nothing() {
        let (mut session, snapshots) = three_adds();
        let generation = session.current_generation();
        let failure = session.jump_to(4).unwrap_err();
        assert_eq!(
            failure.error,
            JumpError::OutOfRange {
                position: 4,
                steps: 3
            }
        );
        assert_eq!(failure.change, None);
        assert_eq!(session.design, snapshots[3]);
        assert_eq!(session.current_generation(), generation);
        assert!(failure.to_string().contains("no step 4"), "{failure}");
    }

    #[test]
    fn a_failed_replay_leaves_the_design_at_the_last_good_step() {
        let (mut session, _) = three_adds();
        // Behind the history's back the design loses its newest tier, so undoing step 3
        // (which removes tier 2) cannot work.
        session.design.tiers.truncate(2);
        let generation = session.current_generation();
        let failure = session.jump_to(0).unwrap_err();
        assert!(matches!(failure.error, JumpError::Replay { at: 3, .. }));
        assert_eq!(failure.change, None, "nothing moved before the failure");
        assert_eq!(session.history_position(), 3);
        assert_eq!(session.current_generation(), generation);
    }

    #[test]
    fn a_jump_that_fails_after_walking_steps_reports_the_change_once() {
        let mut session = EditorSession::fresh();
        for (index, (name, angle)) in [("P1", -40.0), ("P2", -35.0)].into_iter().enumerate() {
            session
                .apply(Edit::AddTier {
                    index,
                    tier: tier(name, angle),
                })
                .unwrap();
        }
        // Step 3 re-angles tier 0; its undo only needs tier 0. Step 2 added tier 1; its undo
        // needs tier 1, which the corruption below removes.
        session
            .apply(Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -41.0),
            })
            .unwrap();
        session.design.tiers.truncate(1);
        let generation = session.current_generation();
        let failure = session.jump_to(0).unwrap_err();
        assert!(matches!(failure.error, JumpError::Replay { at: 2, .. }));
        let change = failure.change.expect("one step was walked");
        assert_eq!(change.generation, generation + 1);
        assert_eq!(session.current_generation(), generation + 1);
        assert_eq!(session.history_position(), 2);
        assert!((session.design.tiers[0].angle_deg + 40.0).abs() < 1e-9);
    }

    #[test]
    fn the_selection_follows_its_tiers_across_a_jump() {
        let (mut session, _) = three_adds();
        // P2 (row 1) is selected; removing P1 above it shifts it to row 0.
        session.multi_selected = std::iter::once(1).collect();
        session.apply(Edit::RemoveTier { index: 0 }).unwrap();
        assert_eq!(session.multi_selected, std::iter::once(0).collect());
        // Jumping back over the removal puts P1 back above it, so P2 is row 1 again.
        session.jump_to(3).unwrap();
        assert_eq!(session.multi_selected, std::iter::once(1).collect());
        // Jumping all the way back removes P2 and the selection goes with it.
        session.jump_to(0).unwrap();
        assert_eq!(session.multi_selected, std::collections::BTreeSet::new());
    }

    #[test]
    fn design_at_equals_the_design_real_undos_reach_and_touches_nothing() {
        let (mut session, snapshots) = three_adds();
        session.undo().unwrap();
        let generation = session.current_generation();
        let history = session.history.clone();
        let design = session.design.clone();
        for (position, snapshot) in snapshots.iter().enumerate() {
            assert_eq!(&session.design_at(position).unwrap(), snapshot);
        }
        assert_eq!(session.design, design);
        assert_eq!(session.history, history);
        assert_eq!(session.current_generation(), generation);
        assert_eq!(
            session.design_at(9),
            Err(JumpError::OutOfRange {
                position: 9,
                steps: 3
            })
        );
    }

    #[test]
    fn a_snapshot_keeps_answering_for_the_moment_it_was_taken() {
        let (mut session, snapshots) = three_adds();
        let snapshot = session.history_snapshot();
        session.jump_to(0).unwrap();
        session
            .apply(Edit::AddTier {
                index: 0,
                tier: tier("X", -20.0),
            })
            .unwrap();
        assert_eq!(snapshot.position(), 3);
        assert_eq!(snapshot.entries().len(), 3);
        for (position, expected) in snapshots.iter().enumerate() {
            assert_eq!(&snapshot.design_at(position).unwrap(), expected);
        }
    }

    #[test]
    fn merged_nudges_are_one_entry_with_a_new_revision_each_time() {
        let mut session = EditorSession::fresh();
        session
            .apply(Edit::AddTier {
                index: 0,
                tier: tier("P1", -40.0),
            })
            .unwrap();
        let mut seen = Vec::new();
        for step in 0..3 {
            session
                .nudge_angles(&[0], -0.25, Duration::from_millis(100 * step))
                .unwrap();
            seen.push(session.history_entries().last().unwrap().revision);
        }
        assert_eq!(
            session.history_entries().len(),
            2,
            "the add and one nudge run"
        );
        assert!(seen[0] != seen[1] && seen[1] != seen[2], "{seen:?}");
        assert_ne!(session.history_entries()[1].label, "");
    }
}
