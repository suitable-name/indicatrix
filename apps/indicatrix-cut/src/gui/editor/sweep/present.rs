//! What the Angle Sweep dialog shows: the table rows, the chart lines, the readout and the
//! status lines, built from the engine's rows, and the pushes into `SweepModel`.
//!
//! The builders take plain data and return Slint's plain structs (no window needed), so
//! they are tested directly; the `push_*` functions only copy their results into the model.

use super::state::{Dialog, columns_of};
use crate::{
    MainWindow, SweepCellItem, SweepColumnItem, SweepLegendItem, SweepModel, SweepPathItem,
    SweepRowItem,
};
use indicatrix_editor::sweep::{
    SweepMetric, SweepOutcome, SweepRow, best_flags, chart_path, chart_x_percent,
    current_x_percent, hover_text, series_range_text,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::fmt::Write as _;

/// The readout before the pointer or a row has picked an angle.
const READOUT_HINT: &str = "Move the pointer over the chart, or pick a row in the table.";

/// The label of "Use this angle" when no angle is picked.
const USE_LABEL: &str = "Use this angle";

/// A position that is not on the chart (the model's "not shown").
const NOT_SHOWN: f32 = -1.0;

fn model_of<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// The status column of a row.
pub(super) const fn status_label(row: &SweepRow) -> &'static str {
    match (row.is_current, row.is_valid()) {
        (true, true) => "Current",
        (true, false) => "Current (not valid)",
        (false, false) => "Not valid",
        (false, true) => "",
    }
}

/// The table: a row per angle, a cell per column, the best figure of each column marked.
pub(super) fn table_rows(outcome: &SweepOutcome, columns: &[SweepMetric]) -> Vec<SweepRowItem> {
    let flags = best_flags(&outcome.rows);
    outcome
        .rows
        .iter()
        .zip(flags)
        .map(|(row, flags)| SweepRowItem {
            angle: format!("{:.2}", row.angle_deg.abs()).into(),
            status: status_label(row).into(),
            is_current: row.is_current,
            is_valid: row.is_valid(),
            cells: model_of(
                columns
                    .iter()
                    .map(|&metric| SweepCellItem {
                        text: metric.cell_text(row).into(),
                        best: flags.get(metric),
                    })
                    .collect(),
            ),
            notes: row.notes_text().into(),
        })
        .collect()
}

/// The column titles, each telling whether the figure's line is on the chart.
pub(super) fn column_items(columns: &[SweepMetric], shown: &[SweepMetric]) -> Vec<SweepColumnItem> {
    columns
        .iter()
        .map(|&metric| SweepColumnItem {
            title: metric.column_title().into(),
            series: series_of(metric),
            shown: shown.contains(&metric),
        })
        .collect()
}

/// The number the model (and `SweepStyle`) knows a figure by.
fn series_of(metric: SweepMetric) -> i32 {
    i32::try_from(metric.index()).unwrap_or(0)
}

/// The chart lines for `lines`; a figure with no value anywhere draws nothing.
pub(super) fn path_items(rows: &[SweepRow], lines: &[SweepMetric]) -> Vec<SweepPathItem> {
    lines
        .iter()
        .filter_map(|&metric| {
            let commands = chart_path(rows, metric);
            (!commands.is_empty()).then(|| SweepPathItem {
                commands: commands.into(),
                series: series_of(metric),
            })
        })
        .collect()
}

/// What each drawn line's height means (its lowest and highest value).
pub(super) fn legend_items(rows: &[SweepRow], lines: &[SweepMetric]) -> Vec<SweepLegendItem> {
    lines
        .iter()
        .map(|&metric| SweepLegendItem {
            text: series_range_text(rows, metric).into(),
            series: series_of(metric),
        })
        .collect()
}

/// The ends of the angle axis (flattest on the left, magnitudes), or empty texts for no
/// rows.
pub(super) fn axis_labels(rows: &[SweepRow]) -> (String, String) {
    let low = rows.iter().map(|row| row.angle_deg.abs()).reduce(f64::min);
    let high = rows.iter().map(|row| row.angle_deg.abs()).reduce(f64::max);
    match (low, high) {
        (Some(low), Some(high)) => (format!("{low:.2}\u{b0}"), format!("{high:.2}\u{b0}")),
        _ => (String::new(), String::new()),
    }
}

/// A row's x position on the chart in percent, or the model's "not shown".
fn x_of(rows: &[SweepRow], row: Option<usize>) -> f32 {
    row.filter(|&index| index < rows.len())
        .map_or(NOT_SHOWN, |index| chart_x_percent(rows, index) as f32)
}

/// `1 angle`, `21 angles`.
fn angle_count(count: usize) -> String {
    format!("{count} angle{}", if count == 1 { "" } else { "s" })
}

/// The title over the results: the tier and how many angles came back.
pub(super) fn result_title(outcome: &SweepOutcome) -> String {
    let mut title = format!(
        "{}: {}, {} valid",
        outcome.tier_name,
        angle_count(outcome.rows.len()),
        outcome.valid_count()
    );
    if outcome.cancelled {
        let _ = write!(title, " (stopped, {} were planned)", outcome.requested);
    }
    title
}

/// The status line when a sweep is over.
pub(super) fn finished_status(outcome: &SweepOutcome) -> String {
    let done = outcome.rows.len();
    if outcome.cancelled {
        format!(
            "Stopped: {done} of {} done.",
            angle_count(outcome.requested)
        )
    } else if outcome.valid_count() == 0 {
        format!(
            "Done: {}, but none gives a stone. See the notes column.",
            angle_count(done)
        )
    } else {
        format!(
            "Done: {}, {} valid.",
            angle_count(done),
            outcome.valid_count()
        )
    }
}

/// The status line while a sweep runs.
pub(super) fn running_status(done: usize, total: usize) -> String {
    format!("{done} of {} done", angle_count(total))
}

/// The sentence under the chart: the angle under the pointer, else the selected angle, else
/// a hint.
pub(super) fn readout(dialog: &Dialog) -> String {
    let rows = dialog.rows();
    let (index, prefix) = match (dialog.hover, dialog.selected) {
        (Some(index), _) => (index, ""),
        (None, Some(index)) => (index, "Selected: "),
        (None, None) => return READOUT_HINT.to_owned(),
    };
    let Some(row) = rows.get(index) else {
        return READOUT_HINT.to_owned();
    };
    let mut text = format!("{prefix}{}", hover_text(rows, index, &dialog.lines()));
    if row.is_valid() && !row.notes.is_empty() {
        let _ = write!(text, " ({})", row.notes_text());
    }
    text
}

/// The label of "Use this angle": it names the angle once one is picked.
pub(super) fn use_label(dialog: &Dialog) -> String {
    dialog.pick().map_or_else(
        || USE_LABEL.to_owned(),
        |pick| format!("Use {:.2}\u{b0}", pick.angle_deg.abs()),
    )
}

/// The file name the CSV is offered under: `<design> <tier> angle sweep.csv`.
pub(super) fn csv_file_name(design_title: &str, tier_name: &str) -> String {
    let stem = format!("{design_title} {tier_name} angle sweep");
    format!(
        "{}.csv",
        crate::gui::library::local::sanitize_filename(&stem)
    )
}

/// Copies everything the results show into the model: the table, the chart and the pointer.
pub(super) fn push_results(ui: &MainWindow, dialog: &Dialog) {
    let model = ui.global::<SweepModel>();
    let Some(outcome) = &dialog.outcome else {
        model.set_has_results(false);
        model.set_rows(model_of(Vec::new()));
        model.set_columns(model_of(Vec::new()));
        model.set_paths(model_of(Vec::new()));
        model.set_legend(model_of(Vec::new()));
        push_pointer(ui, dialog);
        return;
    };
    let columns = columns_of(outcome);
    model.set_has_results(true);
    model.set_result_title(result_title(outcome).into());
    model.set_rows(model_of(table_rows(outcome, &columns)));
    let (low, high) = axis_labels(&outcome.rows);
    model.set_axis_low(low.into());
    model.set_axis_high(high.into());
    model.set_current_x(current_x_percent(&outcome.rows).map_or(NOT_SHOWN, |x| x as f32));
    push_lines(ui, dialog);
}

/// Copies the chart lines, their legend and the column toggles (after a pill was clicked).
pub(super) fn push_lines(ui: &MainWindow, dialog: &Dialog) {
    let model = ui.global::<SweepModel>();
    let lines = dialog.lines();
    model.set_columns(model_of(column_items(&dialog.columns(), &dialog.shown)));
    model.set_paths(model_of(path_items(dialog.rows(), &lines)));
    model.set_legend(model_of(legend_items(dialog.rows(), &lines)));
    model.set_readout_text(readout(dialog).into());
}

/// Copies the selected and hovered rows: their guides on the chart, the readout and the
/// state of "Use this angle".
pub(super) fn push_pointer(ui: &MainWindow, dialog: &Dialog) {
    let model = ui.global::<SweepModel>();
    let rows = dialog.rows();
    model.set_selected_row(row_number(dialog.selected));
    model.set_hover_row(row_number(dialog.hover));
    model.set_selected_x(x_of(rows, dialog.selected));
    model.set_hover_x(x_of(rows, dialog.hover));
    model.set_readout_text(readout(dialog).into());
    model.set_selected_usable(dialog.pick().is_some());
    model.set_use_label(use_label(dialog).into());
}

/// A row index as the model's `int`, `-1` for none.
fn row_number(row: Option<usize>) -> i32 {
    row.and_then(|index| i32::try_from(index).ok())
        .unwrap_or(-1)
}

/// The tier combo's entries.
pub(super) fn tier_names(labels: Vec<String>) -> ModelRc<SharedString> {
    model_of(labels.into_iter().map(SharedString::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_editor::sweep::{RowMetrics, TiltAverages};
    use slint::Model;

    fn figures(brilliance_pct: f32, windowing_pct: f32) -> RowMetrics {
        RowMetrics {
            brilliance_pct,
            windowing_pct,
            extinction_pct: 5.0,
            fire_index: 1.0,
            scintillation_pct: 20.0,
            yield_pct: Some(30.0),
            warning_count: 0,
            has_girdle: true,
            tilt: None::<TiltAverages>,
        }
    }

    fn row(angle_deg: f64, is_current: bool, metrics: Option<RowMetrics>) -> SweepRow {
        SweepRow {
            angle_deg,
            is_current,
            metrics,
            notes: Vec::new(),
        }
    }

    fn outcome_of(rows: Vec<SweepRow>, cancelled: bool) -> SweepOutcome {
        SweepOutcome {
            tier: 5,
            tier_name: "Pavilion Main".to_owned(),
            current_deg: -41.0,
            requested: 4,
            cancelled,
            tilt_average: false,
            rows,
        }
    }

    /// A pavilion sweep as the engine returns it: the flattest angle first (40, 41, 42
    /// degrees), every angle stored negative.
    fn sample() -> SweepOutcome {
        let mut invalid = row(-40.0, false, None);
        invalid.notes.push("The facets do not close.".to_owned());
        outcome_of(
            vec![
                invalid,
                row(-41.0, true, Some(figures(80.0, 10.0))),
                row(-42.0, false, Some(figures(78.0, 12.0))),
            ],
            false,
        )
    }

    fn dialog_of(outcome: SweepOutcome) -> Dialog {
        let mut dialog = Dialog::new();
        dialog.outcome = Some(outcome);
        dialog
    }

    #[test]
    fn a_row_says_whether_it_is_the_current_angle_or_not_valid() {
        let outcome = sample();
        let labels: Vec<&str> = outcome.rows.iter().map(status_label).collect();
        assert_eq!(labels, vec!["Not valid", "Current", ""]);
        let mut broken_current = row(-41.0, true, None);
        broken_current.notes.clear();
        assert_eq!(status_label(&broken_current), "Current (not valid)");
    }

    #[test]
    fn the_table_has_a_cell_per_column_and_marks_the_best_ones() {
        let outcome = sample();
        let columns = columns_of(&outcome);
        let rows = table_rows(&outcome, &columns);
        assert_eq!(rows.len(), 3);
        // The angles are stored negative and read as positive numbers.
        let angles: Vec<&str> = rows.iter().map(|row| row.angle.as_str()).collect();
        assert_eq!(angles, vec!["40.00", "41.00", "42.00"]);
        assert_eq!(rows[1].cells.row_count(), columns.len());
        // Brilliance is the first column: 80 beats 78; windowing 10 beats 12.
        let cell = |row: usize, column: usize| rows[row].cells.row_data(column).expect("a cell");
        assert_eq!(cell(1, 0).text.as_str(), "80.00");
        assert!(cell(1, 0).best && !cell(2, 0).best);
        assert!(cell(1, 1).best && !cell(2, 1).best);
        // An invalid row has dashes and no mark.
        assert_eq!(cell(0, 0).text.as_str(), "-");
        assert!(!cell(0, 0).best);
        assert_eq!(rows[0].notes.as_str(), "The facets do not close.");
        assert!(!rows[0].is_valid && rows[1].is_current);
    }

    #[test]
    fn the_columns_tell_the_pills_which_lines_are_drawn() {
        let outcome = sample();
        let columns = columns_of(&outcome);
        let items = column_items(&columns, &[SweepMetric::Brilliance, SweepMetric::Fire]);
        assert_eq!(items[0].title.as_str(), "Brilliance %");
        assert!(items[0].shown && !items[1].shown);
        let fire = items.iter().find(|item| item.series == 3).expect("fire");
        assert!(fire.shown);
    }

    #[test]
    fn a_line_is_drawn_per_shown_figure_and_a_figure_without_values_is_not() {
        let outcome = sample();
        let paths = path_items(
            &outcome.rows,
            &[SweepMetric::Brilliance, SweepMetric::TiltBrilliance],
        );
        assert_eq!(paths.len(), 1, "the sweep has no tilt figures");
        assert_eq!(paths[0].series, 0);
        assert!(paths[0].commands.as_str().starts_with("M "));
        let legend = legend_items(&outcome.rows, &[SweepMetric::Brilliance]);
        assert_eq!(legend[0].text.as_str(), "Brilliance 78.00 to 80.00 %");
    }

    #[test]
    fn the_axis_runs_over_the_swept_angles() {
        let outcome = sample();
        // Flattest on the left, magnitudes: the stored -40 and -42 read 40 and 42.
        assert_eq!(
            axis_labels(&outcome.rows),
            ("40.00\u{b0}".to_owned(), "42.00\u{b0}".to_owned())
        );
        assert_eq!(axis_labels(&[]), (String::new(), String::new()));
    }

    #[test]
    fn the_title_and_status_count_the_angles_and_say_when_it_was_stopped() {
        let outcome = sample();
        assert_eq!(result_title(&outcome), "Pavilion Main: 3 angles, 2 valid");
        assert_eq!(finished_status(&outcome), "Done: 3 angles, 2 valid.");
        let stopped = outcome_of(vec![row(-41.0, true, Some(figures(80.0, 10.0)))], true);
        assert_eq!(
            result_title(&stopped),
            "Pavilion Main: 1 angle, 1 valid (stopped, 4 were planned)"
        );
        assert_eq!(finished_status(&stopped), "Stopped: 1 of 4 angles done.");
        let hopeless = outcome_of(vec![row(-41.0, true, None)], false);
        assert!(finished_status(&hopeless).contains("none gives a stone"));
        assert_eq!(running_status(3, 21), "3 of 21 angles done");
    }

    #[test]
    fn the_readout_follows_the_pointer_then_the_selection_then_hints() {
        let mut dialog = dialog_of(sample());
        assert_eq!(readout(&dialog), READOUT_HINT);
        dialog.select(2);
        let selected = readout(&dialog);
        assert!(
            selected.starts_with("Selected: 42.00\u{b0}: Brilliance 78.00 %"),
            "{selected}"
        );
        dialog.hover_at(Some(0.0));
        let hovered = readout(&dialog);
        assert!(hovered.starts_with("40.00\u{b0}: not valid"), "{hovered}");
        assert!(hovered.contains("The facets do not close."), "{hovered}");
        assert_eq!(readout(&Dialog::new()), READOUT_HINT);
    }

    #[test]
    fn use_this_angle_names_the_angle_once_one_is_picked() {
        let mut dialog = dialog_of(sample());
        assert_eq!(use_label(&dialog), "Use this angle");
        dialog.select(2);
        assert_eq!(use_label(&dialog), "Use 42.00\u{b0}");
        dialog.select(1);
        assert_eq!(
            use_label(&dialog),
            "Use this angle",
            "the current angle needs no use"
        );
    }

    #[test]
    fn the_csv_is_offered_under_a_safe_name() {
        assert_eq!(
            csv_file_name("Round: Test", "Pavilion Main"),
            "Round_ Test Pavilion Main angle sweep.csv"
        );
        assert_eq!(csv_file_name("", "Crown"), "Crown angle sweep.csv");
    }

    #[test]
    fn row_numbers_and_positions_use_minus_one_for_none() {
        assert_eq!(row_number(None), -1);
        assert_eq!(row_number(Some(4)), 4);
        let outcome = sample();
        assert!(x_of(&outcome.rows, None) < 0.0);
        assert!(x_of(&outcome.rows, Some(9)) < 0.0);
        assert!(x_of(&outcome.rows, Some(1)) > 0.0);
    }
}
