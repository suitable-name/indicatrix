//! Cutting mode's hold on the Solid viewport: the picture of the stone after each step.
//!
//! The page shows `SolidPreviewModel.image`, the picture the Cut slider already draws, so a step
//! needs no picture of its own: moving to step `k` puts the slider at "after step `k`" (the
//! rough cut by the steps before it, plus this tier) and selects the tier's row, which the
//! viewport highlights. Opening takes a note of where the slider, the selection and the view
//! mode were ([`Saved`]) and closing puts them back.
//!
//! The Diagram view (mode 3) draws the finished design only, with its own picture, so cutting
//! mode switches to the Solid view while it is open.

use crate::{
    EditorModel, MainWindow, SolidPreviewModel,
    gui::solid_preview::cut_slider::{CutPosition, MODEL_FINISHED},
};
use indicatrix_editor::cutting_mode::CuttingStep;
use slint::{ComponentHandle, Model};

/// `SolidPreviewModel.view_mode` of the Solid view.
const VIEW_SOLID: i32 = 0;
/// `SolidPreviewModel.view_mode` of the Diagram view.
const VIEW_DIAGRAM: i32 = 3;

/// What the viewport showed before cutting mode took it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Saved {
    /// `SolidPreviewModel.tier_cutoff`: the Cut slider.
    pub(super) tier_cutoff: i32,
    /// `SolidPreviewModel.view_mode`.
    pub(super) view_mode: i32,
    /// `EditorModel.selected_tier_index`.
    pub(super) selected_row: i32,
}

impl Saved {
    /// What a design that replaced the one on screen starts with: the finished stone, the Solid
    /// view as it was left, nothing selected.
    pub(super) const fn after_replacement(self) -> Self {
        Self {
            tier_cutoff: MODEL_FINISHED,
            view_mode: self.view_mode,
            selected_row: -1,
        }
    }
}

/// The view mode cutting mode draws in: the Solid view when the viewport is in the Diagram,
/// else the one it is in (the solid picture is drawn in the others too).
pub(super) const fn view_mode_while_open(view_mode: i32) -> i32 {
    if view_mode == VIEW_DIAGRAM {
        VIEW_SOLID
    } else {
        view_mode
    }
}

/// The `SolidPreviewModel.tier_cutoff` that draws the stone after step `position` (counting
/// from 0) of `step_count` steps: the cut steps `0..=position`. After the last step that is
/// the finished design.
#[must_use]
pub(super) fn cutoff_after(position: usize, step_count: usize) -> i32 {
    CutPosition::from_steps(position.saturating_add(1), step_count).to_model()
}

/// Takes a note of the viewport as it is now.
pub(super) fn capture(ui: &MainWindow) -> Saved {
    Saved {
        tier_cutoff: ui.global::<SolidPreviewModel>().get_tier_cutoff(),
        view_mode: ui.global::<SolidPreviewModel>().get_view_mode(),
        selected_row: ui.global::<EditorModel>().get_selected_tier_index(),
    }
}

/// Draws the stone after step `position` and highlights the step's tier. Does nothing for what
/// is already so, so stepping through the same picture twice redraws nothing.
pub(super) fn show_step(ui: &MainWindow, step: &CuttingStep, position: usize, step_count: usize) {
    let solid = ui.global::<SolidPreviewModel>();
    let view_mode = view_mode_while_open(solid.get_view_mode());
    if solid.get_view_mode() != view_mode {
        solid.set_view_mode(view_mode);
    }
    set_cutoff(ui, cutoff_after(position, step_count));
    select_row(ui, step.table_row);
}

/// Puts the viewport back the way [`capture`] found it.
pub(super) fn restore(ui: &MainWindow, saved: Saved) {
    let solid = ui.global::<SolidPreviewModel>();
    if solid.get_view_mode() != saved.view_mode {
        solid.set_view_mode(saved.view_mode);
    }
    set_cutoff(ui, saved.tier_cutoff);
    let editor = ui.global::<EditorModel>();
    if editor.get_selected_tier_index() != saved.selected_row {
        let in_range = usize::try_from(saved.selected_row)
            .is_ok_and(|row| row < editor.get_tiers().row_count());
        editor.set_selected_tier_index(if in_range { saved.selected_row } else { -1 });
    }
}

/// Moves the Cut slider to `cutoff` and asks for the redraw, unless it is there already.
fn set_cutoff(ui: &MainWindow, cutoff: i32) {
    let solid = ui.global::<SolidPreviewModel>();
    if solid.get_tier_cutoff() != cutoff {
        solid.set_tier_cutoff(cutoff);
        solid.invoke_tier_cutoff_changed();
    }
}

/// Selects row `row` of the tier table (flat tiers first, then concave ones), which makes the
/// viewport highlight its facets. A row the table does not have leaves the selection alone.
fn select_row(ui: &MainWindow, row: usize) {
    let editor = ui.global::<EditorModel>();
    let Ok(row) = i32::try_from(row) else {
        return;
    };
    if row < i32::try_from(editor.get_tiers().row_count()).unwrap_or(0)
        && editor.get_selected_tier_index() != row
    {
        editor.set_selected_tier_index(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stone_after_a_step_is_cut_by_the_steps_up_to_it() {
        // Step 0 of 8 is the first cut: the slider reads "after steps 0..=0", i.e. model 0.
        assert_eq!(cutoff_after(0, 8), 0);
        assert_eq!(cutoff_after(3, 8), 3);
        assert_eq!(cutoff_after(6, 8), 6);
        // After the last step the stone is the finished design.
        assert_eq!(cutoff_after(7, 8), MODEL_FINISHED);
        assert_eq!(cutoff_after(0, 1), MODEL_FINISHED);
        // A position past the end reads as finished too, and never panics.
        assert_eq!(cutoff_after(50, 8), MODEL_FINISHED);
        assert_eq!(cutoff_after(usize::MAX, 8), MODEL_FINISHED);
    }

    #[test]
    fn the_cutoffs_decode_back_to_the_step_they_show() {
        for count in [1_usize, 2, 7, 12] {
            for position in 0..count {
                let steps = CutPosition::from_model(cutoff_after(position, count), count).steps();
                if position + 1 == count {
                    assert_eq!(steps, None, "the last step is the finished stone");
                } else {
                    assert_eq!(steps, Some(position + 1), "step {position} of {count}");
                }
            }
        }
    }

    #[test]
    fn the_diagram_view_is_left_for_the_solid_view_and_the_others_stay() {
        assert_eq!(view_mode_while_open(3), 0);
        assert_eq!(view_mode_while_open(0), 0);
        assert_eq!(view_mode_while_open(1), 1);
        assert_eq!(view_mode_while_open(2), 2);
    }

    #[test]
    fn a_replaced_design_comes_back_finished_and_unselected() {
        let saved = Saved {
            tier_cutoff: 4,
            view_mode: 3,
            selected_row: 2,
        };
        assert_eq!(
            saved.after_replacement(),
            Saved {
                tier_cutoff: MODEL_FINISHED,
                view_mode: 3,
                selected_row: -1,
            }
        );
    }
}
