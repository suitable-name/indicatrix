//! A sweep as CSV text, for a spreadsheet.
//!
//! One header row and one row per finished angle. The angle column is the magnitude, like
//! the table the person saw: the tier's side of the girdle is not in the number. The
//! decimal separator is always a
//! point, numbers carry a fixed four decimals, a missing figure is an empty field, and a
//! field holding a comma, a quote or a line break is quoted with its quotes doubled.
//! Lines end with CRLF as RFC 4180 has it; spreadsheets read that and LF alike.

use super::{SweepMetric, SweepOutcome, SweepRow};

/// The line ending of the file.
const LINE_END: &str = "\r\n";

/// `field` as one CSV field: quoted only when it has to be.
fn escape(field: &str) -> String {
    if field.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// A figure with four decimals, or an empty field.
fn number(value: Option<f64>) -> String {
    value.map_or_else(String::new, |value| format!("{value:.4}"))
}

/// `yes` or `no`.
fn yes_no(flag: bool) -> String {
    (if flag { "yes" } else { "no" }).to_owned()
}

/// The fields of one row, in the order of the header.
fn row_fields(outcome: &SweepOutcome, row: &SweepRow, metrics: &[SweepMetric]) -> Vec<String> {
    let mut fields = vec![
        escape(&outcome.tier_name),
        format!("{:.4}", row.angle_deg.abs()),
        yes_no(row.is_current),
        yes_no(row.is_valid()),
    ];
    fields.extend(metrics.iter().map(|metric| number(metric.value(row))));
    fields.push(
        row.metrics
            .map_or_else(String::new, |figures| figures.warning_count.to_string()),
    );
    fields.push(escape(&row.notes_text()));
    fields
}

/// The rows of `outcome` as CSV text: a header, then one line per finished angle in
/// ascending angle order. The tilt-average columns are there only when the sweep asked
/// for them.
#[must_use]
pub fn sweep_csv(outcome: &SweepOutcome) -> String {
    let metrics: Vec<SweepMetric> = SweepMetric::ALL
        .into_iter()
        .filter(|metric| outcome.tilt_average || !metric.is_tilt())
        .collect();
    let mut header: Vec<&str> = vec!["tier", "angle_deg", "current", "valid"];
    header.extend(metrics.iter().map(|metric| metric.csv_name()));
    header.push("facet_warnings");
    header.push("notes");
    let mut text = header.join(",");
    text.push_str(LINE_END);
    for row in &outcome.rows {
        text.push_str(&row_fields(outcome, row, &metrics).join(","));
        text.push_str(LINE_END);
    }
    text
}
