//! The History tab's list as plain data: which rows there are, in which order, and the
//! words on each. No Slint types, so the tests cover it directly.

use indicatrix_cut_core::HistoryEntry;

/// What the "Start" row is called.
pub(super) const START_TITLE: &str = "Start";

/// What the "Start" row says about itself.
const START_DETAIL: &str = "The design as it was opened or created";

/// Stands in for a step whose description is empty (never expected).
const UNNAMED_STEP: &str = "Change";

/// One row of the list: the Slint-free twin of `HistoryRowData`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RowSpec {
    /// The step the row goes to: 0 for "Start".
    pub(super) position: usize,
    /// What the step did.
    pub(super) title: String,
    /// The quieter second line.
    pub(super) detail: String,
    /// The design stands at this step.
    pub(super) current: bool,
    /// The step was undone (it is ahead of the current one).
    pub(super) undone: bool,
    /// This is the "Start" row.
    pub(super) start: bool,
}

/// The rows for `entries` (oldest first, as `History::entries` returns them) with the design
/// at step `position`: newest step first, "Start" last.
#[must_use]
pub(super) fn build_rows(entries: &[HistoryEntry], position: usize) -> Vec<RowSpec> {
    let mut rows: Vec<RowSpec> = entries
        .iter()
        .rev()
        .map(|entry| step_row(entry, position))
        .collect();
    rows.push(RowSpec {
        position: 0,
        title: START_TITLE.to_string(),
        detail: START_DETAIL.to_string(),
        current: position == 0,
        undone: false,
        start: true,
    });
    rows
}

/// One step's row.
fn step_row(entry: &HistoryEntry, position: usize) -> RowSpec {
    let title = if entry.label.trim().is_empty() {
        UNNAMED_STEP.to_string()
    } else {
        entry.label.clone()
    };
    let detail = if entry.undone {
        format!("Step {}, undone", entry.position)
    } else {
        format!("Step {}", entry.position)
    };
    RowSpec {
        position: entry.position,
        title,
        detail,
        current: !entry.undone && entry.position == position,
        undone: entry.undone,
        start: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(position: usize, label: &str, undone: bool) -> HistoryEntry {
        HistoryEntry {
            label: label.to_string(),
            position,
            revision: position as u64,
            undone,
        }
    }

    fn positions(rows: &[RowSpec]) -> Vec<usize> {
        rows.iter().map(|row| row.position).collect()
    }

    #[test]
    fn an_untouched_design_lists_only_the_start_row_and_it_is_current() {
        let rows = build_rows(&[], 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Start");
        assert!(rows[0].start && rows[0].current && !rows[0].undone);
        assert_eq!(rows[0].detail, "The design as it was opened or created");
    }

    #[test]
    fn rows_run_newest_first_and_end_with_start() {
        let entries = [
            entry(1, "Add tier T", false),
            entry(2, "Add tier P1", false),
            entry(3, "Set P1 angle", false),
        ];
        let rows = build_rows(&entries, 3);
        assert_eq!(positions(&rows), vec![3, 2, 1, 0]);
        assert_eq!(rows[0].title, "Set P1 angle");
        assert!(rows.last().unwrap().start);
    }

    #[test]
    fn exactly_one_row_is_current_and_undone_steps_are_marked() {
        let entries = [
            entry(1, "Add tier T", false),
            entry(2, "Add tier P1", true),
            entry(3, "Set P1 angle", true),
        ];
        let rows = build_rows(&entries, 1);
        assert_eq!(rows.iter().filter(|row| row.current).count(), 1);
        let current = rows.iter().find(|row| row.current).unwrap();
        assert_eq!(current.position, 1);
        assert_eq!(
            rows.iter()
                .filter(|row| row.undone)
                .map(|row| row.position)
                .collect::<Vec<_>>(),
            vec![3, 2]
        );
        assert!(
            !rows.last().unwrap().current,
            "Start is not current at step 1"
        );
    }

    #[test]
    fn the_details_say_the_step_number_and_whether_it_was_undone() {
        let entries = [entry(1, "Add tier T", false), entry(2, "Add tier P1", true)];
        let rows = build_rows(&entries, 1);
        assert_eq!(rows[0].detail, "Step 2, undone");
        assert_eq!(rows[1].detail, "Step 1");
    }

    #[test]
    fn going_all_the_way_back_makes_start_current_and_every_step_undone() {
        let entries = [entry(1, "Add tier T", true), entry(2, "Add tier P1", true)];
        let rows = build_rows(&entries, 0);
        assert!(rows.last().unwrap().current);
        assert!(rows[..2].iter().all(|row| row.undone && !row.current));
    }

    #[test]
    fn an_empty_description_gets_a_plain_name() {
        let rows = build_rows(&[entry(1, "  ", false)], 1);
        assert_eq!(rows[0].title, "Change");
    }
}
