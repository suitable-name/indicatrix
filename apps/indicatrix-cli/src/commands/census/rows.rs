//! One census row: the columns of section 4.1 of the follow-ups plan, in order, and their
//! CSV and JSON forms.
//!
//! A row is a list of [`Cell`]s in the order of [`COLUMNS`] followed by [`TIMING_COLUMNS`]. The
//! numbers are rounded to six decimals when they are stored ([`quantize`]), so the CSV, the JSON
//! and the percentile summary all see the same figures.

use crate::format::{csv_line, json_number};
use serde_json::{Map, Value};

/// The `census_version` of the file format. Bump it when a column is added, removed or renamed.
pub(super) const CENSUS_VERSION: u32 = 2;

/// The deterministic columns, in file order.
///
/// `reach_fallbacks_upper_bound` counts every tier whose inward reach could not be read at
/// capture, turned or not, so it is an upper bound on the half-length fallbacks a retarget uses.
/// The four concave columns are filled by wave 2 of the plan and are empty until then.
///
/// Version 2 (2026-10-20) added the meet-identification columns: `gemcad_version` and
/// `mast_decimals` of the schedule, the captured network's `faces`, `valence` (`2E/V`: 3 when
/// every meet is three facets, 4 when every one is four) and `split_ratio` (`V/(F-2)`: 1 when
/// every junction is four-valent, 2 when every meet is split into three-valent ones), and
/// `max_slack_ppm`, how far the design misses its own meets in millionths of the stone.
pub(super) const COLUMNS: &[&str] = &[
    "source",
    "key",
    "title",
    "tiers",
    "planes",
    "pinned",
    "meet_derived",
    "named",
    "relations",
    "targets",
    "concave_tiers",
    "placements",
    "gear",
    "symmetry",
    "ri_from",
    "target",
    "ri_to",
    "mode",
    "crown_mode",
    "capture",
    "junctions",
    "edges",
    "live_facets",
    "reach_fallbacks_upper_bound",
    "gemcad_version",
    "mast_decimals",
    "faces",
    "valence",
    "split_ratio",
    "max_slack_ppm",
    "status",
    "strategy",
    "girdle_step",
    "reasons",
    "preserved_meets",
    "added_meets",
    "max_residual_norm",
    "census_status",
    "d_aspect",
    "d_table_pts",
    "d_crown",
    "d_pavilion",
    "d_girdle",
    "junction_disp",
    "travel_width",
    "travel_scale",
    "excess_depth",
    "excess_girdle",
    "excess_junction",
    "branch",
    "branch_tiers",
    "ideal_spread",
    "contacts_kept",
    "faces_cut_before",
    "faces_cut_after",
    "new_tool_warnings",
    "score_before",
    "score_unchanged_in_target",
    "score_after",
];

/// The columns that differ from run to run; printed only with `--timings`.
pub(super) const TIMING_COLUMNS: &[&str] = &[
    "t_capture_ms",
    "t_projection_ms",
    "t_solve_ms",
    "t_validate_ms",
    "t_fast_ms",
    "t_full_ms",
    "iterations",
    "rejected",
];

/// `value` rounded to six decimals, with a negative zero turned into zero.
pub(super) fn quantize(value: f64) -> f64 {
    let rounded = format!("{value:.6}").parse::<f64>().unwrap_or(value);
    if rounded.abs() <= 0.0 { 0.0 } else { rounded }
}

/// One value of a row.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) enum Cell {
    /// Not measured, not applicable.
    #[default]
    Empty,
    /// Words.
    Text(String),
    /// A count.
    Int(u64),
    /// A measurement, already rounded by [`quantize`].
    Num(f64),
}

impl Cell {
    /// A text cell.
    pub(super) fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// A count cell.
    pub(super) fn int(count: usize) -> Self {
        Self::Int(u64::try_from(count).unwrap_or(u64::MAX))
    }

    /// A number cell; empty when `value` is not finite.
    pub(super) fn num(value: f64) -> Self {
        if value.is_finite() {
            Self::Num(quantize(value))
        } else {
            Self::Empty
        }
    }

    /// A number cell, empty when `value` is absent.
    pub(super) fn opt(value: Option<f64>) -> Self {
        value.map_or(Self::Empty, Self::num)
    }

    fn render(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(text) => text.clone(),
            Self::Int(count) => count.to_string(),
            Self::Num(value) => format!("{value:.6}"),
        }
    }

    fn json(&self) -> Value {
        match self {
            Self::Empty => Value::Null,
            Self::Text(text) => Value::from(text.as_str()),
            Self::Int(count) => Value::from(*count),
            Self::Num(value) => json_number(*value, 6),
        }
    }
}

fn column_index(name: &str) -> usize {
    COLUMNS
        .iter()
        .chain(TIMING_COLUMNS)
        .position(|column| *column == name)
        .unwrap_or_else(|| panic!("unknown census column {name:?}"))
}

/// The header line of the CSV.
pub(super) fn csv_header(timings: bool) -> String {
    csv_line(&visible_columns(timings))
}

fn visible_columns(timings: bool) -> Vec<&'static str> {
    let mut names = COLUMNS.to_vec();
    if timings {
        names.extend_from_slice(TIMING_COLUMNS);
    }
    names
}

/// One row of the census.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Row {
    cells: Vec<Cell>,
}

impl Row {
    /// A row with every cell empty.
    pub(super) fn new() -> Self {
        Self {
            cells: vec![Cell::Empty; COLUMNS.len() + TIMING_COLUMNS.len()],
        }
    }

    /// Sets the cell of the column `name`.
    ///
    /// # Panics
    ///
    /// When `name` is not a census column: a programming error.
    pub(super) fn set(&mut self, name: &str, cell: Cell) {
        self.cells[column_index(name)] = cell;
    }

    fn cell(&self, name: &str) -> &Cell {
        &self.cells[column_index(name)]
    }

    /// The text of a text column; empty when there is none.
    pub(super) fn text(&self, name: &str) -> &str {
        match self.cell(name) {
            Cell::Text(text) => text,
            _ => "",
        }
    }

    /// The value of a number or count column.
    pub(super) fn num(&self, name: &str) -> Option<f64> {
        match self.cell(name) {
            Cell::Num(value) => Some(*value),
            Cell::Int(count) => u32::try_from(*count).ok().map(f64::from),
            _ => None,
        }
    }

    /// The CSV line of the row.
    pub(super) fn csv_line(&self, timings: bool) -> String {
        let shown = if timings {
            self.cells.len()
        } else {
            COLUMNS.len()
        };
        let fields: Vec<String> = self.cells[..shown].iter().map(Cell::render).collect();
        csv_line(&fields)
    }

    /// The JSON object of the row (keys come out sorted).
    pub(super) fn json(&self, timings: bool) -> Value {
        let mut object = Map::new();
        for (name, cell) in visible_columns(timings).into_iter().zip(&self.cells) {
            object.insert(name.to_string(), cell.json());
        }
        Value::Object(object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_rounds_to_six_decimals_and_drops_negative_zero() {
        assert_eq!(quantize(0.123_456_78), 0.123_457);
        assert_eq!(quantize(-1e-9).to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn a_row_prints_its_columns_in_order_and_hides_the_timings() {
        let mut row = Row::new();
        row.set("key", Cell::text("a,b"));
        row.set("tiers", Cell::int(7));
        row.set("d_aspect", Cell::num(0.5));
        row.set("iterations", Cell::int(3));
        let plain = row.csv_line(false);
        assert_eq!(plain.matches('\n').count(), 1);
        assert!(plain.starts_with(",\"a,b\",,7,"), "{plain}");
        assert!(plain.contains("0.500000"), "{plain}");
        assert_eq!(
            csv_header(false).split(',').count(),
            COLUMNS.len(),
            "one header name for every column"
        );
        let timed = row.csv_line(true);
        assert!(timed.trim_end().ends_with(",3,"), "{timed}");
        assert_eq!(
            csv_header(true).split(',').count(),
            COLUMNS.len() + TIMING_COLUMNS.len()
        );
    }

    #[test]
    fn the_columns_are_unique() {
        let mut names: Vec<&str> = COLUMNS.iter().chain(TIMING_COLUMNS).copied().collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), COLUMNS.len() + TIMING_COLUMNS.len());
    }

    #[test]
    fn json_and_text_accessors_see_the_same_cells() {
        let mut row = Row::new();
        row.set("status", Cell::text("valid"));
        row.set("junction_disp", Cell::num(0.25));
        row.set("junctions", Cell::int(12));
        assert_eq!(row.text("status"), "valid");
        assert_eq!(row.num("junction_disp"), Some(0.25));
        assert_eq!(row.num("junctions"), Some(12.0));
        let json = row.json(false);
        assert_eq!(json["status"], "valid");
        assert_eq!(json["junctions"], 12);
        assert!(json["capture"].is_null());
        assert!(json.get("iterations").is_none());
    }
}
