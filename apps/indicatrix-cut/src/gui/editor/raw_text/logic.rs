//! The pure half of "Edit as Text": what the live check says about the text in the box, and
//! the rows of the line comparison. No toolkit, no state, no threads -- the glue in
//! [`super`] hands these functions the design and the texts and puts the answers on screen.

use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::Design;
use indicatrix_editor::raw_text::{ApplyPlan, DiffKind, TextDiff, TextProblem, plan_apply};

/// Unchanged lines kept on each side of a change in the comparison; longer runs are folded.
const DIFF_CONTEXT: usize = 3;

/// `RawTextModel.status_kind` of a sentence that is only information.
pub(super) const KIND_NEUTRAL: i32 = 0;

/// `RawTextModel.status_kind` of a text that is ready to apply.
pub(super) const KIND_READY: i32 = 1;

/// `RawTextModel.status_kind` of a problem with the text (shown in red).
pub(super) const KIND_PROBLEM: i32 = 2;

/// What the text in the box amounts to, measured against the text the design wrote.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Outcome {
    /// The text has the same lines as the text the design wrote: nothing to apply.
    Unchanged,
    /// The text cannot be applied, with the line of the problem when there is one.
    Problem(TextProblem),
    /// The text can be applied (or, when [`ApplyPlan::is_noop`], would change nothing).
    Plan(Box<ApplyPlan>),
}

/// Whether two texts have the same lines (a trailing line break and the kind of line break
/// do not count).
pub(super) fn same_lines(a: &str, b: &str) -> bool {
    a.lines().eq(b.lines())
}

/// Checks `edited` against the design: `base` is the text the design wrote (see
/// `indicatrix_editor::raw_text::generate_text`) and `custom` the custom materials it was
/// written with.
pub(super) fn evaluate(
    design: &Design,
    base: &str,
    edited: &str,
    custom: &[GemMaterial],
) -> Outcome {
    if same_lines(base, edited) {
        return Outcome::Unchanged;
    }
    match plan_apply(design, base, edited, custom) {
        Ok(plan) => Outcome::Plan(Box::new(plan)),
        Err(problem) => Outcome::Problem(problem),
    }
}

/// What the dialog shows for an [`Outcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PanelView {
    /// The one-sentence live check.
    pub(super) status: String,
    /// How to colour it: [`KIND_NEUTRAL`], [`KIND_READY`] or [`KIND_PROBLEM`].
    pub(super) kind: i32,
    /// What applying the text would change.
    pub(super) changed: Vec<String>,
    /// What applying the text would lose or ignore.
    pub(super) lost: Vec<String>,
    /// Whether the Apply button is available.
    pub(super) can_apply: bool,
}

impl PanelView {
    /// A view with a sentence and nothing else.
    fn sentence(status: impl Into<String>, kind: i32) -> Self {
        Self {
            status: status.into(),
            kind,
            changed: Vec::new(),
            lost: Vec::new(),
            can_apply: false,
        }
    }

    /// The view of a dialog that has no text to check yet.
    pub(super) fn empty() -> Self {
        Self::sentence("", KIND_NEUTRAL)
    }

    /// The view of a problem with `message`.
    pub(super) fn problem(message: impl Into<String>) -> Self {
        Self::sentence(message, KIND_PROBLEM)
    }
}

/// The sentence for a text that has not been edited yet.
pub(super) const UNCHANGED_SENTENCE: &str = "No changes yet. Edit the text, then choose Apply.";

/// The sentence for a text that differs from the design's but means the same design.
const NOTHING_SENTENCE: &str =
    "These edits change nothing in the design (spacing and blank lines are not kept).";

/// The sentence for a text that can be applied.
const READY_SENTENCE: &str =
    "The text is valid. Apply changes the design in one step that Undo takes back.";

/// The dialog's live check for `outcome`.
pub(super) fn panel_view(outcome: &Outcome) -> PanelView {
    match outcome {
        Outcome::Unchanged => PanelView::sentence(UNCHANGED_SENTENCE, KIND_NEUTRAL),
        Outcome::Problem(problem) => PanelView::sentence(problem.to_string(), KIND_PROBLEM),
        Outcome::Plan(plan) if plan.is_noop() => PanelView {
            lost: plan.report.lost.clone(),
            ..PanelView::sentence(NOTHING_SENTENCE, KIND_NEUTRAL)
        },
        Outcome::Plan(plan) => PanelView {
            changed: plan.report.changed.clone(),
            lost: plan.report.lost.clone(),
            can_apply: true,
            ..PanelView::sentence(READY_SENTENCE, KIND_READY)
        },
    }
}

/// The problem shown when the design moved on while the dialog was open.
pub(super) fn design_moved_problem() -> TextProblem {
    TextProblem {
        line: None,
        message: "The design changed while this window was open. Use Revert to write the text \
                  again."
            .to_owned(),
    }
}

/// One row of the comparison as the dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RowView {
    /// `RawTextDiffRow.kind`: 0 in both texts, 1 added, 2 removed, 3 folded unchanged lines.
    pub(super) kind: i32,
    /// The line number in the other text; empty for an added line.
    pub(super) old_line: String,
    /// The line number in the text in the box; empty for a removed line.
    pub(super) new_line: String,
    /// The line, or for a folded run how many lines it holds.
    pub(super) text: String,
}

/// The comparison as the dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompareView {
    /// What is being compared with what.
    pub(super) title: String,
    /// The counts, and what added and removed mean.
    pub(super) summary: String,
    /// The rows, runs of unchanged lines folded.
    pub(super) rows: Vec<RowView>,
}

/// `RawTextDiffRow.kind` of a row.
const fn kind_code(kind: DiffKind) -> i32 {
    match kind {
        DiffKind::Same => 0,
        DiffKind::Added => 1,
        DiffKind::Removed => 2,
        DiffKind::Skipped => 3,
    }
}

/// A line number as the dialog shows it; empty where the line is not in that text.
fn number(line: Option<usize>) -> String {
    line.map_or_else(String::new, |line| line.to_string())
}

/// The heading of a comparison with the other text called `label`.
pub(super) fn compare_title(label: &str) -> String {
    format!("Comparing {label} with the text in the box")
}

/// The comparison of the other text (`label` names it, for example "the snapshot") with the
/// text in the box, as `diff` (the other text is the old side, the box the new side).
pub(super) fn compare_view(label: &str, diff: &TextDiff) -> CompareView {
    let rows = if diff.is_identical() {
        diff.rows.clone()
    } else {
        diff.collapsed(DIFF_CONTEXT)
    };
    let summary = if diff.is_identical() {
        diff.summary()
    } else {
        format!(
            "{} Added lines are only in the text box, removed lines only in {label}.",
            diff.summary()
        )
    };
    CompareView {
        title: compare_title(label),
        summary,
        rows: rows
            .into_iter()
            .map(|row| RowView {
                kind: kind_code(row.kind),
                old_line: number(row.old_line),
                new_line: number(row.new_line),
                text: row.text,
            })
            .collect(),
    }
}

/// The view shown instead of a comparison that could not be made.
pub(super) fn compare_failure_view(label: &str, reason: &str) -> CompareView {
    CompareView {
        title: format!("Cannot compare with {label}"),
        summary: reason.to_owned(),
        rows: Vec::new(),
    }
}
