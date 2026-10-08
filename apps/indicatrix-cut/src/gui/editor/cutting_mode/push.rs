//! Writes one page of cutting mode into `CuttingModeModel`.
//!
//! All the wording comes from `indicatrix_editor::cutting_mode::display`; this only maps it onto
//! the model's properties and rows. Setting a property to the value it holds changes nothing, so
//! pushing a page again after a tick redraws only what moved.

use crate::{CmChip, CmScheduleRow, CmWheelMark, CmWheelScale, CuttingModeModel, MainWindow};
use indicatrix_editor::cutting_mode::{
    CuttingPlan,
    dial::IndexWheel,
    display::{StepPage, progress_fraction, progress_line, stone_caption, wheel_caption},
    progress::Progress,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// Pushes the page of the step at `position` of `plan`, with the marks `progress` holds, and
/// the progress line under it. `design_name` is the window title's design name.
///
/// A `position` past the last step clears the page.
pub(super) fn push_page(
    ui: &MainWindow,
    plan: &CuttingPlan,
    progress: &Progress,
    position: usize,
    design_name: &str,
) {
    let Some(page) = StepPage::new(&plan.steps, position, progress) else {
        clear_page(ui);
        return;
    };
    let step = &plan.steps[position];
    let wheel = IndexWheel::new(plan.gear_teeth, step, progress);
    let model = ui.global::<CuttingModeModel>();

    model.set_design_name(design_name.into());
    model.set_step_count(i32::try_from(plan.steps.len()).unwrap_or(i32::MAX));
    model.set_step_index(i32::try_from(position).unwrap_or(i32::MAX));
    model.set_heading(page.heading.into());
    model.set_tier_name(page.tier_name.into());
    model.set_side_label(page.side.into());
    model.set_angle_text(page.angle.into());
    model.set_chips(ModelRc::new(VecModel::from(
        page.chips
            .into_iter()
            .map(|chip| CmChip {
                text: chip.text.into(),
                ticked: chip.ticked,
            })
            .collect::<Vec<_>>(),
    )));
    model.set_meet_text(page.meet.into());
    model.set_depth_text(page.depth.into());
    model.set_cheater_text(page.cheater.into());
    model.set_notes_text(page.notes.into());
    model.set_tool_text(page.tool.into());
    model.set_step_state(page.state.code());
    model.set_state_text(page.state_text.into());
    model.set_has_previous(page.has_previous);
    model.set_has_next(page.has_next);

    model.set_done_count(i32::try_from(progress.done_count(&plan.steps)).unwrap_or(i32::MAX));
    model.set_progress_text(progress_line(&plan.steps, progress).into());
    model.set_progress_fraction(progress_fraction(&plan.steps, progress));
    model.set_stone_caption(stone_caption(step, plan.steps.len()).into());
    model.set_wheel_caption(wheel_caption(plan.gear_teeth, step).into());

    model.set_schedule(ModelRc::new(VecModel::from(
        (0..plan.steps.len())
            .filter_map(|i| StepPage::new(&plan.steps, i, progress))
            .map(|row| CmScheduleRow {
                code: row.tier_name.into(),
                angle: row.angle.into(),
                indices: row
                    .chips
                    .iter()
                    .map(|chip| chip.text.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                state: row.state.code(),
            })
            .collect::<Vec<_>>(),
    )));

    model.set_wheel_minor(wheel.minor_ticks.into());
    model.set_wheel_major(wheel.major_ticks.into());
    model.set_wheel_spokes(wheel.spokes.into());
    model.set_wheel_marks(ModelRc::new(VecModel::from(
        wheel
            .marks
            .into_iter()
            .map(|mark| CmWheelMark {
                x: mark.x,
                y: mark.y,
                label_x: mark.label_x,
                label_y: mark.label_y,
                label: SharedString::from(mark.label),
                ticked: mark.ticked,
            })
            .collect::<Vec<_>>(),
    )));
    model.set_wheel_scale(ModelRc::new(VecModel::from(
        wheel
            .scale
            .into_iter()
            .map(|number| CmWheelScale {
                x: number.x,
                y: number.y,
                label: SharedString::from(number.label),
            })
            .collect::<Vec<_>>(),
    )));
}

/// Empties the page: nothing to show (the screen is closed, or no step could be built).
pub(super) fn clear_page(ui: &MainWindow) {
    let model = ui.global::<CuttingModeModel>();
    model.set_step_count(0);
    model.set_step_index(0);
    model.set_heading(SharedString::new());
    model.set_tier_name(SharedString::new());
    model.set_side_label(SharedString::new());
    model.set_angle_text(SharedString::new());
    model.set_chips(ModelRc::default());
    model.set_meet_text(SharedString::new());
    model.set_depth_text(SharedString::new());
    model.set_cheater_text(SharedString::new());
    model.set_notes_text(SharedString::new());
    model.set_tool_text(SharedString::new());
    model.set_step_state(0);
    model.set_state_text(SharedString::new());
    model.set_has_previous(false);
    model.set_has_next(false);
    model.set_done_count(0);
    model.set_progress_text(SharedString::new());
    model.set_progress_fraction(0.0);
    model.set_stone_caption(SharedString::new());
    model.set_wheel_caption(SharedString::new());
    model.set_schedule(ModelRc::default());
    model.set_wheel_minor(SharedString::new());
    model.set_wheel_major(SharedString::new());
    model.set_wheel_spokes(SharedString::new());
    model.set_wheel_marks(ModelRc::default());
    model.set_wheel_scale(ModelRc::default());
}
