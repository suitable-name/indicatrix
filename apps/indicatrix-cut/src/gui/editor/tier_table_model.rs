//! The tier table's and the Tier form's few callbacks that need Rust beyond the design edits
//! in `EditorModel` (`ui/models/tier_table.slint`, `TierTableModel`).
//!
//! Three of them read a number a cutter typed the way every other number field does: a plain
//! number or a small calculation (`0.5+0.25`, `96/4`), with a plain-English toast when it cannot
//! be read (`indicatrix_editor::loading::eval_number`). Slint's own `to-float` reads neither
//! calculations nor a decimal comma, and silently turns anything else into 0.
//!
//! - `offset_selected` -- the multi-select bar's Offset box: moves every ticked tier,
//!   whether or not the cursor is on one of them (`offset_anchor`).
//! - `add_facet` -- the Tier form's "+ Add" box.
//! - `rotate_tier` -- the Tier form's Rotate buttons.
//! - `any_detached` -- whether any row is detached, for the Simple interface's note.
//!
//! The first three hand the number to the `EditorModel` callback that already makes the edit
//! (`nudge_angle`, `facet_add`, `tier_rotate_indices`), so the undo step, the refresh, the
//! preview replan and the refusals are those callbacks' own, unchanged.

use crate::{
    EditorModel, MainWindow, TierTableModel,
    gui::{editor::state::EditorState, show_toast},
};
use indicatrix_editor::loading::eval_number;
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc};

/// Narrows a UI value to the `f32` the Slint models hold.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Slint model properties are f32; the values are angles, positions and color channels in the normal float range"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

/// Reads the number or calculation in a field called `label`. `Err` is the toast text: a blank
/// field asks for a number, text that is no number gets the usual "... is not a number.", a
/// calculation that cannot be done says why, and `inf` or `NaN` is refused.
fn read_number(label: &str, text: &str) -> Result<f64, String> {
    if text.trim().is_empty() {
        return Err(format!(
            "{label} is empty -- type a number or a calculation."
        ));
    }
    let value = eval_number(text, None).map_err(|error| error.message(label, text))?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{label} must be a finite number."))
    }
}

/// The signed tooth count a Rotate button asks for: `direction` 1 moves the facets to higher
/// index numbers, -1 to lower ones.
fn signed_teeth(teeth: f64, direction: i32) -> f64 {
    if direction < 0 { -teeth } else { teeth }
}

/// The row the Offset bar hands to `nudge_angle`, which moves the whole ticked group when its
/// anchor is one of the ticked rows. `cursor` is the keyboard cursor row (negative when there
/// is none), `ticked` the ticked rows.
///
/// With a group of two or more ticked: the cursor row when it is ticked, otherwise the first
/// ticked row, since ticking a box does not move the cursor and a click on a ticked row would
/// end the group. With fewer: the cursor row, else the one ticked row. `None` when there is
/// neither.
fn offset_anchor(cursor: i32, ticked: &[i32]) -> Option<i32> {
    let first = ticked.iter().copied().min();
    if ticked.len() > 1 {
        return if ticked.contains(&cursor) {
            Some(cursor)
        } else {
            first
        };
    }
    if cursor >= 0 { Some(cursor) } else { first }
}

/// Registers `TierTableModel`'s callbacks. Called once from `setup_editor_callbacks`.
///
/// `state` is the editor state the other callbacks hold: the Offset anchor is resolved from
/// `EditorState::multi_selected`, the very set `nudge_angle` moves, and not from the rows'
/// `multi_selected` flags, which a background solve's snapshot can briefly leave stale.
pub(in crate::gui::editor) fn setup_tier_table_model(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let model = ui.global::<TierTableModel>();

    model.on_offset_selected({
        let ui_weak = ui.as_weak();
        let state = Rc::clone(state);
        move |anchor, text| {
            let Some(ui) = ui_weak.upgrade() else {
                return false;
            };
            // The batch acts on the ticked group. `nudge_angle` takes the group that holds
            // its anchor row, so the anchor is a ticked row: the cursor row when it is one,
            // otherwise the first ticked row. Ticking a box does not move the cursor.
            let ticked: Vec<i32> = state
                .borrow()
                .multi_selected
                .iter()
                .filter_map(|&index| i32::try_from(index).ok())
                .collect();
            let Some(anchor) = offset_anchor(anchor, &ticked) else {
                show_toast(
                    &ui,
                    "Tick two or more tiers first, then press Offset.",
                    "info",
                );
                return false;
            };
            match read_number("Offset", &text) {
                Ok(degrees) => {
                    ui.global::<EditorModel>()
                        .invoke_nudge_angle(anchor, to_f32(degrees));
                    true
                }
                Err(message) => {
                    show_toast(&ui, &message, "error");
                    false
                }
            }
        }
    });

    model.on_add_facet({
        let ui_weak = ui.as_weak();
        move |tier, text| {
            let Some(ui) = ui_weak.upgrade() else {
                return false;
            };
            match read_number("Position", &text) {
                Ok(position) => {
                    ui.global::<EditorModel>()
                        .invoke_facet_add(tier, to_f32(position));
                    true
                }
                Err(message) => {
                    show_toast(&ui, &message, "error");
                    false
                }
            }
        }
    });

    model.on_rotate_tier({
        let ui_weak = ui.as_weak();
        move |tier, text, direction| {
            let Some(ui) = ui_weak.upgrade() else {
                return false;
            };
            match read_number("Teeth", &text) {
                Ok(teeth) => {
                    ui.global::<EditorModel>()
                        .invoke_tier_rotate_indices(tier, to_f32(signed_teeth(teeth, direction)));
                    true
                }
                Err(message) => {
                    show_toast(&ui, &message, "error");
                    false
                }
            }
        }
    });

    model.on_any_detached(|rows| {
        use slint::Model;
        rows.iter().any(|row| row.is_detached)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_number_is_read_as_it_is() {
        assert_eq!(read_number("Offset", "0.5"), Ok(0.5));
        assert_eq!(read_number("Offset", "  -2 "), Ok(-2.0));
    }

    #[test]
    fn a_calculation_is_worked_out() {
        assert_eq!(read_number("Offset", "0.5+0.25"), Ok(0.75));
        assert_eq!(read_number("Position", "96/4"), Ok(24.0));
        assert_eq!(read_number("Teeth", "(2 + 1) * 2"), Ok(6.0));
    }

    #[test]
    fn a_blank_field_asks_for_a_number() {
        let message = read_number("Position", "   ").unwrap_err();
        assert_eq!(
            message,
            "Position is empty -- type a number or a calculation."
        );
    }

    #[test]
    fn text_that_is_no_number_gets_the_usual_message() {
        assert_eq!(
            read_number("Offset", "abc"),
            Err("Offset 'abc' is not a number.".to_string())
        );
    }

    #[test]
    fn a_broken_calculation_says_why() {
        let message = read_number("Offset", "1/0").unwrap_err();
        assert!(
            message.starts_with("Offset '1/0' cannot be calculated: "),
            "{message}"
        );
        let message = read_number("Offset", "(1+2").unwrap_err();
        assert!(
            message.starts_with("Offset '(1+2' cannot be calculated: "),
            "{message}"
        );
    }

    #[test]
    fn infinity_and_nan_are_refused() {
        assert_eq!(
            read_number("Teeth", "inf"),
            Err("Teeth must be a finite number.".to_string())
        );
        assert_eq!(
            read_number("Teeth", "NaN"),
            Err("Teeth must be a finite number.".to_string())
        );
    }

    #[test]
    fn a_ticked_cursor_row_is_the_anchor() {
        assert_eq!(offset_anchor(2, &[0, 2, 3]), Some(2));
    }

    #[test]
    fn without_a_cursor_the_first_ticked_row_is_the_anchor() {
        assert_eq!(offset_anchor(-1, &[3, 1, 2]), Some(1));
    }

    #[test]
    fn a_cursor_on_an_unticked_row_falls_back_to_the_first_ticked_row() {
        // The cursor stays where it was when boxes are ticked; the bar still moves the group.
        assert_eq!(offset_anchor(5, &[1, 2]), Some(1));
    }

    #[test]
    fn with_no_group_the_cursor_row_is_kept() {
        assert_eq!(offset_anchor(4, &[]), Some(4));
        assert_eq!(offset_anchor(4, &[1]), Some(4));
    }

    #[test]
    fn with_no_cursor_and_no_group_there_is_no_anchor() {
        assert_eq!(offset_anchor(-1, &[]), None);
        assert_eq!(offset_anchor(-1, &[2]), Some(2));
    }

    #[test]
    fn the_rotate_direction_signs_the_teeth() {
        assert_eq!(signed_teeth(3.0, 1), 3.0);
        assert_eq!(signed_teeth(3.0, -1), -3.0);
        assert_eq!(
            signed_teeth(-3.0, -1),
            3.0,
            "the sign flips, it is not forced negative"
        );
    }
}
