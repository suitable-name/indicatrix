//! Cutting Mode's model: the steps of the solved design, the current step, and what the
//! full-screen page shows for it.
//!
//! [`open`] builds the plan ([`indicatrix_editor::cutting_mode::build_plan`]) from the
//! current solve and starts at a given step, else at the first step not done yet.
//! [`next`], [`previous`], [`jump`] and [`toggle_done`] move and mark; the marks live in
//! the progress store of [`CuttingState`](super::CuttingState) (`state.cutting.progress`),
//! so they outlive the screen and the app. [`view`] gives the page's texts.
//!
//! The mode belongs to one design generation. When the design changed since
//! [`open`], [`restart_if_stale`] builds the plan again (keeping the mode open when the
//! design is solved again, else closing it) and says so, so the page can tell the user.
//!
//! - [`diagram`]: the highlighted three-panel diagram beside the numerals.
//!
//! No GUI types: the app maps [`ModeView`] onto its Slint properties.

pub mod diagram;

#[cfg(test)]
mod tests;

use indicatrix_cut_core::{design::TierRef, format_sheet_indices};
use indicatrix_editor::cutting_mode::{CuttingStep, StepSide};

use self::diagram::DiagramScene;
use super::NotSolved;
use crate::app_state::AppState;

/// What the page tells the user after the design changed under an open Cutting Mode.
pub const DESIGN_CHANGED_MESSAGE: &str = "The design changed; Cutting Mode restarted";

/// The open Cutting Mode: its steps and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeState {
    /// The current step, an index into [`Self::steps`].
    pub step: usize,
    /// The steps in cutting order, for the design as solved at [`Self::generation`].
    pub steps: Vec<CuttingStep>,
    /// The design generation the steps were built at.
    pub generation: u64,
    /// The index text of each step as the printed sheet writes it (`96-08-16` style),
    /// `-` for a tier that lists no index.
    pub index_texts: Vec<String>,
}

/// Everything the page shows about the current step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeView {
    /// The current step, counting from 0 (the page shows `step + 1`).
    pub step: usize,
    /// The number of steps.
    pub count: usize,
    /// The tier's code (`P1`, `G1`, `C1`, `T`).
    pub code: String,
    /// The angle with two decimals and a degree sign (`42.50°`).
    pub angle_text: String,
    /// The index positions, dash-joined and zero-padded (`96-08-16`).
    pub index_text: String,
    /// The tier's name.
    pub name: String,
    /// The cutter's note, then the cheater offset, depth and tool lines; may be empty.
    /// Lines are separated by newlines.
    pub notes: String,
    /// Whether the step is marked done.
    pub done: bool,
    /// `Pavilion`, `Girdle` or `Crown`.
    pub section: String,
    /// The same as [`Self::section`], as a value: picks the diagram panel.
    pub side: StepSide,
    /// The flat tier the step cuts (an index into `design.tiers`), `None` for a concave
    /// tier; what the diagram highlights.
    pub tier_index: Option<usize>,
    /// Whether a step comes before this one.
    pub has_previous: bool,
    /// Whether a step comes after this one.
    pub has_next: bool,
}

/// What [`restart_if_stale`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staleness {
    /// The mode is closed, or still matches the design; nothing changed.
    Fresh,
    /// The design changed and is solved again: the mode was rebuilt at the first step
    /// not done.
    Restarted,
    /// The design changed and has no current solve: the mode was closed.
    Closed,
}

/// Opens Cutting Mode on the current solve, at step `start` when it is a valid step,
/// else at the first step not done yet (the last step when everything is done).
///
/// # Errors
///
/// [`NotSolved`] when the design has no solve for its current generation, no tier to cut,
/// or a solve that does not fit its tiers.
pub fn open(state: &mut AppState, start: Option<usize>) -> Result<(), NotSolved> {
    let generation = state.session.current_generation();
    // The plan is built (and cached) by the Cut page's code; it answers `NotSolved` without
    // a current solve.
    let steps = super::plan(state)?.steps.clone();
    super::sync_progress(state);
    let design = &state.session.design;
    let index_texts = steps
        .iter()
        .map(|step| listed_indices(design, step))
        .collect();
    let step = match start {
        Some(wanted) if wanted < steps.len() => wanted,
        _ => steps
            .iter()
            .position(|step| !step_done(state, step))
            .unwrap_or_else(|| steps.len().saturating_sub(1)),
    };
    state.cutting.mode = Some(ModeState {
        step,
        steps,
        generation,
        index_texts,
    });
    Ok(())
}

/// Closes Cutting Mode. Returns whether it was open.
pub fn close(state: &mut AppState) -> bool {
    state.cutting.mode.take().is_some()
}

/// Whether Cutting Mode is open.
#[must_use]
pub const fn is_open(state: &AppState) -> bool {
    state.cutting.mode.is_some()
}

/// Whether Cutting Mode is open for a design that has changed since it was opened.
#[must_use]
pub fn is_stale(state: &AppState) -> bool {
    state
        .cutting
        .mode
        .as_ref()
        .is_some_and(|mode| mode.generation != state.session.current_generation())
}

/// If the open Cutting Mode no longer matches the design, rebuilds it (when the design is
/// solved again) or closes it. See [`Staleness`].
pub fn restart_if_stale(state: &mut AppState) -> Staleness {
    if !is_stale(state) {
        return Staleness::Fresh;
    }
    if open(state, None).is_ok() {
        Staleness::Restarted
    } else {
        state.cutting.mode = None;
        Staleness::Closed
    }
}

/// Moves to the next step. False at the last step or when the mode is closed.
pub const fn next(state: &mut AppState) -> bool {
    match &mut state.cutting.mode {
        Some(mode) if mode.step + 1 < mode.steps.len() => {
            mode.step += 1;
            true
        }
        _ => false,
    }
}

/// Moves to the previous step. False at the first step or when the mode is closed.
pub const fn previous(state: &mut AppState) -> bool {
    match &mut state.cutting.mode {
        Some(mode) if mode.step > 0 => {
            mode.step -= 1;
            true
        }
        _ => false,
    }
}

/// Moves to `step`. False when there is no such step (or it is the current one) or the
/// mode is closed.
pub const fn jump(state: &mut AppState, step: usize) -> bool {
    match &mut state.cutting.mode {
        Some(mode) if step < mode.steps.len() && step != mode.step => {
            mode.step = step;
            true
        }
        _ => false,
    }
}

/// Flips the done mark of the current step and returns the new state, `None` when the
/// mode is closed.
pub fn toggle_done(state: &mut AppState) -> Option<bool> {
    super::sync_progress(state);
    let mode = state.cutting.mode.as_ref()?;
    let step = mode.steps.get(mode.step)?;
    Some(state.cutting.progress.toggle_done(step))
}

/// The page's view of the current step, `None` when the mode is closed.
#[must_use]
pub fn view(state: &AppState) -> Option<ModeView> {
    let mode = state.cutting.mode.as_ref()?;
    let step = mode.steps.get(mode.step)?;
    super::sync_progress(state);
    Some(ModeView {
        step: mode.step,
        count: mode.steps.len(),
        code: step.code.clone(),
        angle_text: format!("{:.2}°", step.angle_deg),
        index_text: mode
            .index_texts
            .get(mode.step)
            .cloned()
            .unwrap_or_else(|| "-".to_owned()),
        name: step.name.clone(),
        notes: detail_text(step),
        done: state.cutting.progress.is_done(step),
        section: step.side.label().to_owned(),
        side: step.side,
        tier_index: match step.tier {
            TierRef::Flat(index) => Some(index),
            TierRef::Concave(_) => None,
        },
        has_previous: mode.step > 0,
        has_next: mode.step + 1 < mode.steps.len(),
    })
}

/// The diagram scene of the design as solved now, for the highlighted picture; `None`
/// without a current solve or when the stone does not close.
#[must_use]
pub fn diagram_scene(state: &AppState) -> Option<DiagramScene> {
    let generation = state.session.current_generation();
    let solved = state.solve.current_tiers(generation)?;
    DiagramScene::build(&state.session.design, solved)
}

/// Whether `step` is marked done in the progress store.
fn step_done(state: &AppState, step: &CuttingStep) -> bool {
    state.cutting.progress.is_done(step)
}

/// The index text of `step` as the printed sheet writes the tier's listed indices.
fn listed_indices(design: &indicatrix_cut_core::Design, step: &CuttingStep) -> String {
    let listed = match step.tier {
        TierRef::Flat(index) => design.tiers.get(index).map(|tier| tier.indices.as_slice()),
        TierRef::Concave(index) => design
            .concave_tiers
            .get(index)
            .map(|tier| tier.indices.as_slice()),
    };
    listed.map_or_else(
        || {
            let values: Vec<f64> = step.indices.iter().map(|index| index.value).collect();
            format_sheet_indices(&values)
        },
        format_sheet_indices,
    )
}

/// The lines under the facet name: the cutter's note, the cheater offset, the depth and
/// mast, and a concave tier's tool line.
fn detail_text(step: &CuttingStep) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !step.notes.is_empty() {
        lines.push(step.notes.clone());
    }
    if let Some(degrees) = step.cheater_offset_deg {
        lines.push(format!("Cheater offset {degrees:+.2}°"));
    }
    match (step.mast, step.depth_mm) {
        (Some(mast), Some(millimetres)) => {
            lines.push(format!("Depth {millimetres:.2} mm  ·  mast {mast:.4}"));
        }
        (Some(mast), None) => lines.push(format!("Mast {mast:.4}")),
        (None, _) => {}
    }
    if let Some(tool) = &step.tool_line {
        lines.push(tool.clone());
    }
    lines.join("\n")
}
