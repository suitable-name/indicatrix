//! The tier table's and the Tier form's few callbacks that need Rust beyond the design edits
//! in `EditorModel` (`ui/models/tier_table.slint`, `TierTableModel`).
//!
//! Three of them read a number a cutter typed the way every other number field does: a plain
//! number or a small calculation (`0.5+0.25`, `96/4`), with a plain-English toast when it cannot
//! be read (`indicatrix_editor::loading::eval_number`). Slint's own `to-float` reads neither
//! calculations nor a decimal comma, and silently turns anything else into 0.
//!
//! - `offset_selected` -- the multi-select bar's Offset box: moves every selected tier.
//! - `add_facet` -- the Tier form's "+ Add" box.
//! - `rotate_tier` -- the Tier form's Rotate buttons.
//! - `any_detached` -- whether any row is detached, for the Simple interface's note.
//!
//! The first three hand the number to the `EditorModel` callback that already makes the edit
//! (`nudge_angle`, `facet_add`, `tier_rotate_indices`), so the undo step, the refresh, the
//! preview replan and the refusals are those callbacks' own, unchanged.

use crate::{EditorModel, MainWindow, TierTableModel, gui::show_toast};
use indicatrix_editor::loading::eval_number;
use slint::ComponentHandle;

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

/// Registers `TierTableModel`'s callbacks. Called once from `setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_tier_table_model(ui: &MainWindow) {
    let model = ui.global::<TierTableModel>();

    model.on_offset_selected({
        let ui_weak = ui.as_weak();
        move |anchor, text| {
            let Some(ui) = ui_weak.upgrade() else {
                return false;
            };
            // The batch acts on the group that holds the tier the cursor is on.
            if anchor < 0 {
                show_toast(
                    &ui,
                    "Click one of the selected tiers first, then press Offset.",
                    "info",
                );
                return false;
            }
            match read_number("Offset", &text) {
                Ok(degrees) => {
                    ui.global::<EditorModel>()
                        .invoke_nudge_angle(anchor, degrees as f32);
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
                        .invoke_facet_add(tier, position as f32);
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
                        .invoke_tier_rotate_indices(tier, signed_teeth(teeth, direction) as f32);
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
