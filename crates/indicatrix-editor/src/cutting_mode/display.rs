//! The texts one page of cutting mode shows, and the progress line under it.
//!
//! Plain strings and flags, so the desktop maps them straight onto its Slint rows and the
//! wording is pinned by tests rather than by eye.

use super::{
    CuttingStep,
    progress::{Progress, StepState, next_position, previous_position},
};

/// One index chip: the position and whether it is ticked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipFace {
    /// The position as the cutting sheet prints it.
    pub text: String,
    /// Whether it is ticked, or the whole step is done.
    pub ticked: bool,
}

/// Everything one page shows about its step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepPage {
    /// `Step 3 of 12`.
    pub heading: String,
    /// The page heading: the tier's code (`P1`, `T`), as the cutting sheet's label column
    /// shows it. The tier's own name is at the front of [`Self::meet`].
    pub tier_name: String,
    /// `Crown`, `Pavilion` or `Girdle`.
    pub side: String,
    /// The angle, huge on the page: `42.50°`.
    pub angle: String,
    /// The index chips.
    pub chips: Vec<ChipFace>,
    /// The instruction: the tier's name, then what it meets (`Crown Main: Meet P1, P2`).
    pub meet: String,
    /// Depth and mast; empty for a concave tier.
    pub depth: String,
    /// `Cheater offset +1.50°`; empty without one.
    pub cheater: String,
    /// The cutter's note; empty without one.
    pub notes: String,
    /// A concave tier's tool line; empty for a flat tier.
    pub tool: String,
    /// Whether the step is done, not done or changed since it was marked.
    pub state: StepState,
    /// `Done`, `Changed since you marked it`, or empty.
    pub state_text: String,
    /// Whether a step comes before this one.
    pub has_previous: bool,
    /// Whether a step comes after this one.
    pub has_next: bool,
}

impl StepPage {
    /// The page of the step at `position` among `steps`, or `None` past the end.
    #[must_use]
    pub fn new(steps: &[CuttingStep], position: usize, progress: &Progress) -> Option<Self> {
        let step = steps.get(position)?;
        let state = progress.state(step);
        Some(Self {
            heading: format!("Step {} of {}", step.number, steps.len()),
            tier_name: step.code.clone(),
            side: step.side.label().to_owned(),
            angle: format!("{:.2}°", step.angle_deg),
            chips: step
                .indices
                .iter()
                .enumerate()
                .map(|(chip, index)| ChipFace {
                    text: index.text.clone(),
                    ticked: progress.chip_ticked(step, chip),
                })
                .collect(),
            meet: step.meet.clone(),
            depth: depth_text(step),
            cheater: step
                .cheater_offset_deg
                .map_or_else(String::new, |deg| format!("Cheater offset {deg:+.2}°")),
            notes: step.notes.clone(),
            tool: step.tool_line.clone().unwrap_or_default(),
            state,
            state_text: state_text(state).to_owned(),
            has_previous: previous_position(position).is_some(),
            has_next: next_position(position, steps.len()).is_some(),
        })
    }
}

/// The depth line of a flat step: millimetres and mast, or the mast alone without a girdle
/// diameter. Empty for a concave step, whose cut is bounded by its tool.
fn depth_text(step: &CuttingStep) -> String {
    match (step.mast, step.depth_mm) {
        (Some(mast), Some(mm)) => format!("Depth {mm:.2} mm  ·  mast {mast:.4}"),
        (Some(mast), None) => format!("Mast {mast:.4}"),
        (None, _) => String::new(),
    }
}

/// The badge text of a step state.
#[must_use]
pub const fn state_text(state: StepState) -> &'static str {
    match state {
        StepState::NotDone => "",
        StepState::Done => "Done",
        StepState::Changed => "Changed since you marked it",
    }
}

/// The progress line: `4 of 12 steps done`, with the number of steps that changed since they
/// were marked when there are any.
#[must_use]
pub fn progress_line(steps: &[CuttingStep], progress: &Progress) -> String {
    let done = progress.done_count(steps);
    let changed = progress.changed_count(steps);
    let noun = if steps.len() == 1 { "step" } else { "steps" };
    let changed_note = match changed {
        0 => String::new(),
        1 => "  ·  1 step changed since you marked it".to_owned(),
        many => format!("  ·  {many} steps changed since you marked them"),
    };
    format!("{done} of {} {noun} done{changed_note}", steps.len())
}

/// How much of the progress bar is full, from 0 to 1.
#[must_use]
pub fn progress_fraction(steps: &[CuttingStep], progress: &Progress) -> f32 {
    if steps.is_empty() {
        return 0.0;
    }
    progress.done_count(steps) as f32 / steps.len() as f32
}

/// The caption under the picture of the stone after a step.
#[must_use]
pub fn stone_caption(step: &CuttingStep, step_count: usize) -> String {
    format!(
        "The stone after step {} of {step_count}, {} highlighted",
        step.number, step.code
    )
}

/// The caption under the index wheel.
#[must_use]
pub fn wheel_caption(gear_teeth: u32, step: &CuttingStep) -> String {
    let count = step.indices.len();
    let noun = if count == 1 { "index" } else { "indices" };
    format!("{gear_teeth}-tooth index wheel, {count} {noun} marked")
}
