//! The Cut page's rows and its progress summary, built from the cutting plan and the
//! done marks.

use indicatrix_cut_core::{design::TierRef, format_sheet_indices};
use indicatrix_editor::cutting_mode::{CuttingStep, progress::StepState};

use super::{NotSolved, plan, sync_progress};
use crate::app_state::AppState;

/// One row of the instruction list: a tier to cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutRow {
    /// The step's position in the cutting order, from 0 (what Cutting Mode opens at).
    pub step: usize,
    /// The tier's code in cutting order (`P1`, `G1`, `C1`, `T`).
    pub code: String,
    /// The angle, positive, two decimals: `41.00°`.
    pub angle_text: String,
    /// The index positions in the cutting sheet's style: `96-08-16`.
    pub index_text: String,
    /// The tier's name.
    pub name: String,
    /// The cutter's note; a concave tier's tool line when it has no note; else empty.
    pub notes: String,
    /// Whether the step is done.
    pub done: bool,
    /// Whether the step was marked once but its cutting values changed since.
    pub changed: bool,
    /// `Pavilion`, `Girdle` or `Crown`.
    pub section: &'static str,
    /// Whether this row opens a section (the list prints a header above it).
    pub section_start: bool,
    /// The flat tier's position in `design.tiers`, `None` for a concave tier.
    pub tier_index: Option<usize>,
}

/// How far the cutting has got.
#[derive(Debug, Clone, PartialEq)]
pub struct CutSummary {
    /// The number of steps.
    pub total: usize,
    /// The number of steps done.
    pub done: usize,
    /// The number of steps marked once whose values changed since.
    pub changed: usize,
    /// `done / total`, from 0 to 1.
    pub fraction: f32,
    /// `12 of 57 Facets Done`, with the changed count appended when there are changed steps.
    pub text: String,
    /// Where Cutting Mode resumes: the first step not done, else the last step.
    pub next_step: usize,
    /// Whether every step is done.
    pub all_done: bool,
}

/// The angle of `step` as the instructions print it: positive, two decimals.
#[must_use]
pub fn angle_text(step: &CuttingStep) -> String {
    format!("{:.2}\u{b0}", step.angle_deg)
}

/// The index positions of `step` in the cutting sheet's style: `96-08-16`.
#[must_use]
pub fn index_text(step: &CuttingStep) -> String {
    let positions: Vec<f64> = step.indices.iter().map(|index| index.value).collect();
    format_sheet_indices(&positions)
}

/// The instruction rows of the open design, in cutting order, with their done flags.
///
/// # Errors
/// [`NotSolved`] when the design has no solve of its current generation (or no tiers).
pub fn instruction_rows(state: &AppState) -> Result<Vec<CutRow>, NotSolved> {
    let plan = plan(state)?;
    sync_progress(state);
    let progress = &state.cutting.progress;
    let mut previous = None;
    let mut rows = Vec::with_capacity(plan.steps.len());
    for (position, step) in plan.steps.iter().enumerate() {
        let section = step.side.label();
        let step_state = progress.state(step);
        rows.push(CutRow {
            step: position,
            code: step.code.clone(),
            angle_text: angle_text(step),
            index_text: index_text(step),
            name: step.name.clone(),
            notes: if step.notes.is_empty() {
                step.tool_line.clone().unwrap_or_default()
            } else {
                step.notes.clone()
            },
            done: step_state == StepState::Done,
            changed: step_state == StepState::Changed,
            section,
            section_start: previous != Some(section),
            tier_index: match step.tier {
                TierRef::Flat(index) => Some(index),
                TierRef::Concave(_) => None,
            },
        });
        previous = Some(section);
    }
    Ok(rows)
}

/// The progress of the open design's cutting.
///
/// # Errors
/// [`NotSolved`] when the design has no solve of its current generation (or no tiers).
pub fn summary(state: &AppState) -> Result<CutSummary, NotSolved> {
    let plan = plan(state)?;
    sync_progress(state);
    let progress = &state.cutting.progress;
    let total = plan.steps.len();
    let done = progress.done_count(&plan.steps);
    let changed = progress.changed_count(&plan.steps);
    let text = if changed > 0 {
        format!("{done} of {total} Facets Done, {changed} Changed Since Marked")
    } else {
        format!("{done} of {total} Facets Done")
    };
    Ok(CutSummary {
        total,
        done,
        changed,
        fraction: progress.progress_fraction(&plan.steps),
        text,
        next_step: progress.resume_position(&plan.steps),
        all_done: total > 0 && done == total,
    })
}
