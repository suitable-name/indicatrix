//! Done marks and index ticks of a design's cutting steps.
//!
//! [`Progress`] holds what the library remembers for one design: for each mark a key and the
//! fingerprint of the step's cutting values at the moment it was set. A mark counts only while
//! that fingerprint still equals the step's current one; otherwise the step reads
//! [`StepState::Changed`] ("Changed since you marked it") and is not done.
//!
//! Every action (mark, undo, tick) is a pure function of the progress and the step that returns
//! the [`MarkChange`]s to store; the caller stores them and hands the same list to
//! [`Progress::apply`], so the screen and the library cannot disagree.

use super::CuttingStep;
use std::collections::BTreeMap;

/// Whether a step is done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// Not marked.
    NotDone,
    /// Marked, and its cutting values are what they were then.
    Done,
    /// Marked once, but its cutting values changed since. It does not count as done.
    Changed,
}

impl StepState {
    /// The number the Slint model carries: 0 not done, 1 done, 2 changed since marked.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::NotDone => 0,
            Self::Done => 1,
            Self::Changed => 2,
        }
    }
}

/// One change to the stored marks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkChange {
    /// Marks `key` with the step fingerprint `signature` (replacing an older mark).
    Mark {
        /// The step key, or a step's index key.
        key: String,
        /// The step's fingerprint now.
        signature: String,
    },
    /// Removes the mark of `key`.
    Unmark {
        /// The step key, or a step's index key.
        key: String,
    },
}

/// The marks of one design, by key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    marks: BTreeMap<String, String>,
}

impl Progress {
    /// Builds the progress from stored `(key, signature)` pairs.
    #[must_use]
    pub fn from_marks<I>(marks: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        Self {
            marks: marks.into_iter().collect(),
        }
    }

    /// Whether `step` is done, not done, or changed since it was marked.
    #[must_use]
    pub fn state(&self, step: &CuttingStep) -> StepState {
        match self.marks.get(&step.key) {
            None => StepState::NotDone,
            Some(signature) if *signature == step.signature => StepState::Done,
            Some(_) => StepState::Changed,
        }
    }

    /// Whether the chip for index `chip` of `step` shows ticked: the whole step is done, or the
    /// index has a tick made while the step still had its present cutting values.
    #[must_use]
    pub fn chip_ticked(&self, step: &CuttingStep, chip: usize) -> bool {
        if self.state(step) == StepState::Done {
            return true;
        }
        step.indices
            .get(chip)
            .is_some_and(|index| self.marks.get(&index.key) == Some(&step.signature))
    }

    /// How many of `steps` are done. A step marked before its values changed does not count.
    #[must_use]
    pub fn done_count(&self, steps: &[CuttingStep]) -> usize {
        steps
            .iter()
            .filter(|step| self.state(step) == StepState::Done)
            .count()
    }

    /// How many of `steps` were marked once but have changed since.
    #[must_use]
    pub fn changed_count(&self, steps: &[CuttingStep]) -> usize {
        steps
            .iter()
            .filter(|step| self.state(step) == StepState::Changed)
            .count()
    }

    /// The first step that is not done (a changed step is not done), if there is one.
    #[must_use]
    pub fn first_open(&self, steps: &[CuttingStep]) -> Option<usize> {
        steps
            .iter()
            .position(|step| self.state(step) != StepState::Done)
    }

    /// Where cutting mode opens: the first step not done, or the last step when every step is
    /// done (so the screen says so), or the first when there are no steps.
    #[must_use]
    pub fn resume_position(&self, steps: &[CuttingStep]) -> usize {
        self.first_open(steps)
            .unwrap_or_else(|| steps.len().saturating_sub(1))
    }

    /// The changes for the "Mark step done" button: marks a step that is not done, and takes
    /// the mark of one that is done back, together with all its index ticks.
    #[must_use]
    pub fn toggle_done(&self, step: &CuttingStep) -> Vec<MarkChange> {
        if self.state(step) == StepState::Done {
            let mut changes = vec![MarkChange::Unmark {
                key: step.key.clone(),
            }];
            changes.extend(
                step.indices
                    .iter()
                    .filter(|index| self.marks.contains_key(&index.key))
                    .map(|index| MarkChange::Unmark {
                        key: index.key.clone(),
                    }),
            );
            changes
        } else {
            vec![mark(&step.key, step)]
        }
    }

    /// The changes for Space or D: marks the step done. A step that is already done needs
    /// nothing, so the key can be pressed again to move on without taking anything back.
    #[must_use]
    pub fn mark_done(&self, step: &CuttingStep) -> Vec<MarkChange> {
        if self.state(step) == StepState::Done {
            Vec::new()
        } else {
            vec![mark(&step.key, step)]
        }
    }

    /// The changes for a click on index chip `chip` of `step`.
    ///
    /// - Ticking the last index that was not ticked also marks the step done.
    /// - Un-ticking an index of a step that is done un-marks the step and keeps the other
    ///   indices ticked, so no work is lost.
    /// - Otherwise it flips the one tick.
    ///
    /// A chip number past the step's indices changes nothing.
    #[must_use]
    pub fn toggle_chip(&self, step: &CuttingStep, chip: usize) -> Vec<MarkChange> {
        let Some(index) = step.indices.get(chip) else {
            return Vec::new();
        };
        if !self.chip_ticked(step, chip) {
            let mut changes = vec![mark(&index.key, step)];
            let others_ticked =
                (0..step.indices.len()).all(|other| other == chip || self.chip_ticked(step, other));
            if others_ticked {
                changes.push(mark(&step.key, step));
            }
            return changes;
        }
        if self.state(step) == StepState::Done {
            let mut changes = vec![MarkChange::Unmark {
                key: step.key.clone(),
            }];
            for (other, other_index) in step.indices.iter().enumerate() {
                if other == chip {
                    changes.push(MarkChange::Unmark {
                        key: other_index.key.clone(),
                    });
                } else if self.marks.get(&other_index.key) != Some(&step.signature) {
                    changes.push(mark(&other_index.key, step));
                }
            }
            return changes;
        }
        vec![MarkChange::Unmark {
            key: index.key.clone(),
        }]
    }

    /// Applies `changes` to the marks held here, the same way storing them changes the library.
    pub fn apply(&mut self, changes: &[MarkChange]) {
        for change in changes {
            match change {
                MarkChange::Mark { key, signature } => {
                    self.marks.insert(key.clone(), signature.clone());
                }
                MarkChange::Unmark { key } => {
                    self.marks.remove(key);
                }
            }
        }
    }

    /// Removes every mark.
    pub fn clear(&mut self) {
        self.marks.clear();
    }
}

fn mark(key: &str, step: &CuttingStep) -> MarkChange {
    MarkChange::Mark {
        key: key.to_owned(),
        signature: step.signature.clone(),
    }
}

/// The step after `position` among `count` steps, or `None` at the last one.
#[must_use]
pub const fn next_position(position: usize, count: usize) -> Option<usize> {
    if position + 1 < count {
        Some(position + 1)
    } else {
        None
    }
}

/// The step before `position`, or `None` at the first one.
#[must_use]
pub const fn previous_position(position: usize) -> Option<usize> {
    position.checked_sub(1)
}
