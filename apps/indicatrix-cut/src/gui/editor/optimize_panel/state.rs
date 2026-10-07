//! What the Optimize tab remembers between clicks: whether the cutter has taken the
//! "Vary anchored tiers" default into their own hands, the angle ranges they typed, the
//! last finished run (so a candidate can be previewed, compared and applied) and the
//! measured speed of the last run (so the next time estimate is better).
//!
//! Slint-free, so the rules are tested without a window. The state is thread-local: every
//! reader and writer runs on the Slint event-loop thread, the same reasoning as
//! `callbacks::retarget_actions::RETARGET_ASYNC`.

use indicatrix_cut_core::{Design, OptimizeCandidate, OptimizeOutcome, OptimizeResult};
use indicatrix_editor::optimize_view::{RangeInput, candidate_outcome, range_rows};
use std::cell::RefCell;

/// One finished run, kept until the design moves on or a new run replaces it.
pub(in crate::gui::editor) struct StoredRun {
    /// What the run found.
    pub result: OptimizeResult,
    /// The design the run started from (names the tiers in the change table).
    pub design: Design,
    /// The design generation the run started at; a candidate applies only while the design
    /// is still at it.
    pub generation: u64,
    /// The candidate the cutter picked, an index into `result.candidates`.
    pub selected: Option<usize>,
}

impl StoredRun {
    /// The picked candidate.
    pub fn selected_candidate(&self) -> Option<&OptimizeCandidate> {
        self.selected
            .and_then(|index| self.result.candidates.get(index))
    }

    /// The outcome the picked candidate stands for (see
    /// [`indicatrix_editor::optimize_view::candidate_outcome`]).
    pub fn selected_outcome(&self) -> Option<OptimizeOutcome> {
        self.selected_candidate()
            .map(|candidate| candidate_outcome(&self.result.outcome, candidate))
    }

    /// The candidate that `outcome` was built from: the one with the same angle changes.
    /// Candidates differ in at least one angle, so this is unambiguous.
    pub fn candidate_for(&self, outcome: &OptimizeOutcome) -> Option<&OptimizeCandidate> {
        self.result
            .candidates
            .iter()
            .find(|candidate| candidate.changes == outcome.changes)
    }
}

/// The ranges table as the cutter left it.
pub(super) struct RangeTable {
    /// [`range_signature`] of the design the table was built for.
    pub signature: Vec<u64>,
    /// The ranges the cutter typed; a tier without an entry uses its default.
    pub inputs: Vec<RangeInput>,
}

impl RangeTable {
    pub(super) const fn new(signature: Vec<u64>) -> Self {
        Self {
            signature,
            inputs: Vec::new(),
        }
    }

    /// Records what the cutter typed for `tier`, replacing an earlier entry.
    pub(super) fn set(&mut self, tier: usize, min_text: String, max_text: String) {
        let entry = RangeInput {
            tier,
            min_text,
            max_text,
        };
        match self.inputs.iter_mut().find(|input| input.tier == tier) {
            Some(existing) => *existing = entry,
            None => self.inputs.push(entry),
        }
    }
}

/// The measured cost of an evaluation on the last run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MeasuredRate {
    /// How many tiers the design had.
    pub tier_count: usize,
    /// Milliseconds per evaluation.
    pub ms_per_evaluation: f64,
}

/// Everything the tab remembers.
pub(in crate::gui::editor) struct PanelState {
    /// The cutter ticked or unticked "Vary anchored tiers" themselves, so the default
    /// rule (on when nothing else is free) stops overriding it.
    pub(super) vary_touched: bool,
    /// The design epoch this state belongs to; a new design resets it.
    pub(super) epoch: Option<u64>,
    /// The ranges table, once it has been opened.
    pub(super) ranges: Option<RangeTable>,
    /// The last finished run.
    pub(in crate::gui::editor) run: Option<StoredRun>,
    /// The speed of the last run.
    pub(super) measured: Option<MeasuredRate>,
}

impl PanelState {
    const fn new() -> Self {
        Self {
            vary_touched: false,
            epoch: None,
            ranges: None,
            run: None,
            measured: None,
        }
    }

    /// Forgets everything that belonged to the previous design.
    pub(super) fn reset_for_new_design(&mut self, epoch: u64) {
        self.vary_touched = false;
        self.epoch = Some(epoch);
        self.ranges = None;
        self.run = None;
        self.measured = None;
    }
}

thread_local! {
    static PANEL: RefCell<PanelState> = const { RefCell::new(PanelState::new()) };
}

/// Runs `f` on the panel state. Keep `f` short and free of UI calls: the state is borrowed
/// for its whole length.
pub(in crate::gui::editor) fn with_panel<R>(f: impl FnOnce(&mut PanelState) -> R) -> R {
    PANEL.with(|cell| f(&mut cell.borrow_mut()))
}

/// The candidate that `outcome` stands for, copied out of the last run, if the outcome came
/// from it. The preview and the compare window use this to show the candidate's masts too.
pub(in crate::gui::editor) fn candidate_for_outcome(
    outcome: &OptimizeOutcome,
) -> Option<OptimizeCandidate> {
    with_panel(|panel| {
        panel
            .run
            .as_ref()
            .and_then(|run| run.candidate_for(outcome).cloned())
    })
}

/// A fingerprint of the tiers the ranges table lists: their positions, angles and whether
/// they follow a relation. The table is rebuilt only when it changes, so typing in it is
/// never overwritten by an unrelated refresh.
pub(super) fn range_signature(design: &Design, vary_anchored: bool) -> Vec<u64> {
    range_rows(design, vary_anchored)
        .iter()
        .flat_map(|row| {
            [
                row.tier as u64,
                row.angle_deg.to_bits(),
                u64::from(row.driven),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{AngleChange, ConstraintTier, ObjectiveComponents};

    fn tier(angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: String::new(),
            indices: vec![0.0],
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    fn design() -> Design {
        let mut design = indicatrix_editor::EditorSession::fresh().design;
        design
            .tiers
            .push(tier(40.0, MeetConstraint::ScaleReference(0.6)));
        design.tiers.push(tier(-41.0, MeetConstraint::MeetExisting));
        design
    }

    fn components() -> ObjectiveComponents {
        ObjectiveComponents {
            windowing_pct: 10.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        }
    }

    fn candidate(to_deg: f64, score: f32) -> OptimizeCandidate {
        OptimizeCandidate {
            changes: vec![AngleChange {
                index: 1,
                from_deg: -41.0,
                to_deg,
            }],
            mast_changes: Vec::new(),
            after: components(),
            score,
            yield_loss_pct: 5.0,
            tone: None,
        }
    }

    fn stored(selected: Option<usize>) -> StoredRun {
        let best = candidate(-42.0, 9.0);
        let second = candidate(-43.0, 9.5);
        StoredRun {
            result: OptimizeResult {
                outcome: OptimizeOutcome {
                    before: components(),
                    before_score: 10.0,
                    before_yield_loss_pct: 6.0,
                    after: best.after,
                    after_score: best.score,
                    after_yield_loss_pct: best.yield_loss_pct,
                    evaluations: 30,
                    changes: best.changes.clone(),
                    cancelled: false,
                    polish_evaluations: 0,
                    polish_improvement: 0.0,
                },
                mast_changes: Vec::new(),
                candidates: vec![best, second],
                tone_before: None,
                tone_goal: None,
                lighting: indicatrix_cut_core::CANONICAL_LIGHTING_PRESET,
                starts_run: 1,
                best_start: 0,
            },
            design: design(),
            generation: 7,
            selected,
        }
    }

    #[test]
    fn the_picked_candidate_and_its_outcome_come_from_the_selection() {
        let run = stored(Some(1));
        assert_eq!(run.selected_candidate().map(|c| c.score), Some(9.5));
        let outcome = run.selected_outcome().expect("a candidate is picked");
        assert_eq!(outcome.changes[0].to_deg, -43.0);
        assert_eq!(outcome.after_score, 9.5);
        assert_eq!(outcome.before_score, 10.0);

        let none = stored(None);
        assert!(none.selected_candidate().is_none());
        assert!(none.selected_outcome().is_none());
        assert!(stored(Some(9)).selected_candidate().is_none());
    }

    #[test]
    fn an_outcome_finds_the_candidate_it_was_built_from() {
        let run = stored(Some(1));
        let outcome = run.selected_outcome().expect("picked");
        let found = run.candidate_for(&outcome).expect("found");
        assert_eq!(found.score, 9.5);
        let mut other = outcome;
        other.changes[0].to_deg = -50.0;
        assert!(run.candidate_for(&other).is_none());
    }

    #[test]
    fn the_panel_state_looks_up_the_candidate_of_an_outcome_and_forgets_on_a_new_design() {
        let run = stored(Some(0));
        let outcome = run.selected_outcome().expect("picked");
        with_panel(|panel| panel.run = Some(run));
        assert_eq!(candidate_for_outcome(&outcome).map(|c| c.score), Some(9.0));

        with_panel(|panel| panel.reset_for_new_design(3));
        assert!(candidate_for_outcome(&outcome).is_none());
        assert_eq!(with_panel(|panel| panel.epoch), Some(3));
    }

    #[test]
    fn typing_a_range_twice_keeps_the_last_entry() {
        let mut table = RangeTable::new(Vec::new());
        table.set(1, "35".to_string(), "45".to_string());
        table.set(2, "10".to_string(), "20".to_string());
        table.set(1, "36".to_string(), "44".to_string());
        assert_eq!(table.inputs.len(), 2);
        assert_eq!(table.inputs[0].min_text, "36");
        assert_eq!(table.inputs[1].tier, 2);
    }

    #[test]
    fn the_signature_changes_with_an_angle_and_with_the_switch() {
        let design = design();
        let off = range_signature(&design, false);
        let on = range_signature(&design, true);
        assert_ne!(off, on, "the pinned crown tier joins");
        assert_eq!(off, range_signature(&design, false));

        let mut moved = design;
        moved.tiers[1].angle_deg = -41.5;
        assert_ne!(off, range_signature(&moved, false));
    }

    #[test]
    fn the_signature_of_a_design_with_nothing_movable_is_empty() {
        let design = indicatrix_editor::EditorSession::fresh().design;
        assert_eq!(range_signature(&design, true).len(), 0);
    }
}
