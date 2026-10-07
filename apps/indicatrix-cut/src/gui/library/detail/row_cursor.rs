//! The keyboard cursor of the library's cutting-instructions table.
//!
//! `cutting_table.slint` hides the rows a Pavilion or Crown filter leaves out, and Slint has no
//! loop to find the next row that is still shown. So Up, Down, Home and End ask this module:
//! `CuttingTableModel.step_row(angles, current, step, mode)` returns the `order_idx` the cursor
//! moves to. The work is a pure function over the visible `order_idx` values
//! ([`step_row_cursor`]), so a test can check every edge without a window.

use super::shared::side_shown_in_mode;
use crate::{AngleItem, CuttingTableModel, MainWindow};
use slint::{ComponentHandle, Model, ModelRc};

/// The `order_idx` of every row that the filter `mode` (0 = All, 1 = Pavilion, 2 = Crown)
/// shows, in the order the table draws them.
fn visible_order_indices(angles: &ModelRc<AngleItem>, mode: i32) -> Vec<i32> {
    angles
        .iter()
        .filter(|angle| side_shown_in_mode(angle.side, mode))
        .map(|angle| angle.order_idx)
        .collect()
}

/// Where the cursor goes when it moves `step` rows from `current` over the rows `visible` (their
/// `order_idx` values, top to bottom). The result is always one of `visible`, or `-1` when the
/// list is empty.
///
/// - `current` is one of the rows: move by `step` and stop at the first or last row (no wrap).
///   A step far larger than the list (Home and End pass one) lands on the end.
/// - `current` is not one of the rows (the cursor is unset at `-1`, or the filter just hid its
///   row): a single step down lands on the first row and a single step up on the last; a long
///   step is Home or End and lands on the end it points to.
#[must_use]
pub(super) fn step_row_cursor(visible: &[i32], current: i32, step: i32) -> i32 {
    let Some(last) = visible.len().checked_sub(1) else {
        return -1;
    };
    let target = match visible.iter().position(|&index| index == current) {
        Some(position) => {
            let wanted = i64::try_from(position)
                .unwrap_or(0)
                .saturating_add(i64::from(step));
            usize::try_from(wanted.max(0)).map_or(0, |wanted| wanted.min(last))
        }
        None if step.unsigned_abs() > 1 => {
            if step > 0 {
                last
            } else {
                0
            }
        }
        None => {
            if step > 0 {
                0
            } else {
                last
            }
        }
    };
    visible.get(target).copied().unwrap_or(-1)
}

/// Registers `CuttingTableModel::step_row`. Called by
/// [`super::setup_filtered_row_count_callback`], which `gui::main_window::callbacks` already runs
/// for the cutting table's row-count pills.
pub(super) fn setup_row_cursor_callback(ui: &MainWindow) {
    ui.global::<CuttingTableModel>()
        .on_step_row(|angles, current, step, mode| {
            step_row_cursor(&visible_order_indices(&angles, mode), current, step)
        });
}

#[cfg(test)]
mod tests {
    use super::{step_row_cursor, visible_order_indices};
    use crate::AngleItem;
    use slint::{ModelRc, VecModel};

    fn angle(order_idx: i32, side: i32) -> AngleItem {
        AngleItem {
            order_idx,
            side,
            facet: "P1".into(),
            code: "P1".into(),
            angle: "0.0".into(),
            index_val: "".into(),
            notes: "".into(),
            second_line: "".into(),
        }
    }

    #[test]
    fn an_empty_table_has_no_cursor_row() {
        assert_eq!(step_row_cursor(&[], -1, 1), -1);
        assert_eq!(step_row_cursor(&[], 3, -1), -1);
    }

    #[test]
    fn down_from_the_unset_cursor_lands_on_the_first_row_and_up_on_the_last() {
        let rows = [4, 5, 6];
        assert_eq!(step_row_cursor(&rows, -1, 1), 4);
        assert_eq!(step_row_cursor(&rows, -1, -1), 6);
    }

    #[test]
    fn a_step_moves_one_row_and_stops_at_both_ends() {
        let rows = [0, 1, 2];
        assert_eq!(step_row_cursor(&rows, 0, 1), 1);
        assert_eq!(step_row_cursor(&rows, 1, 1), 2);
        assert_eq!(step_row_cursor(&rows, 2, 1), 2);
        assert_eq!(step_row_cursor(&rows, 2, -1), 1);
        assert_eq!(step_row_cursor(&rows, 0, -1), 0);
    }

    #[test]
    fn home_and_end_reach_the_ends_from_anywhere() {
        let rows = [3, 5, 8, 9];
        assert_eq!(step_row_cursor(&rows, 5, 1_000_000), 9);
        assert_eq!(step_row_cursor(&rows, 8, -1_000_000), 3);
        // From the unset cursor too: Home is the first row, End the last.
        assert_eq!(step_row_cursor(&rows, -1, -1_000_000), 3);
        assert_eq!(step_row_cursor(&rows, -1, 1_000_000), 9);
        // The largest steps cannot overflow.
        assert_eq!(step_row_cursor(&rows, 3, i32::MAX), 9);
        assert_eq!(step_row_cursor(&rows, 9, i32::MIN), 3);
    }

    /// The cursor row was hidden by a filter change: the next press starts from the edge.
    #[test]
    fn a_cursor_on_a_hidden_row_restarts_from_the_edge() {
        let rows = [1, 2];
        assert_eq!(step_row_cursor(&rows, 7, 1), 1);
        assert_eq!(step_row_cursor(&rows, 7, -1), 2);
    }

    /// Steps count visible rows, not `order_idx` values: a Crown filter skips the pavilion rows.
    #[test]
    fn steps_skip_the_rows_a_filter_hides() {
        // order_idx 0..=4 with sides pavilion, crown, pavilion, crown, crown.
        let angles = ModelRc::new(VecModel::from(vec![
            angle(0, -1),
            angle(1, 1),
            angle(2, -1),
            angle(3, 1),
            angle(4, 1),
        ]));
        let crown = visible_order_indices(&angles, 2);
        assert_eq!(crown, vec![1, 3, 4]);
        assert_eq!(step_row_cursor(&crown, 1, 1), 3);
        assert_eq!(step_row_cursor(&crown, 3, -1), 1);
        let pavilion = visible_order_indices(&angles, 1);
        assert_eq!(pavilion, vec![0, 2]);
        assert_eq!(step_row_cursor(&pavilion, 0, 1), 2);
        assert_eq!(visible_order_indices(&angles, 0), vec![0, 1, 2, 3, 4]);
    }
}
