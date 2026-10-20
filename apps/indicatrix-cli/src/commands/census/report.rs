//! The census as text: the CSV or JSON body and the percentile summary.
//!
//! The CSV is one comment line (`# census_version = 2`), the header, the rows and, after them,
//! the summary as `#` comment lines, so a reader that skips `#` lines sees a plain table.

use super::{
    percentiles::{SPREAD_TOLERANCE, Summary, Table},
    rows::{CENSUS_VERSION, COLUMNS, Row, TIMING_COLUMNS, csv_header},
};
use crate::format::{Align, fixed, fixed_or_na, json_optional, json_text, table};
use serde_json::{Value, json};
use std::fmt::Write as _;

/// Decimals of a percentile in the printed tables.
const TABLE_DECIMALS: usize = 6;

/// The summary as plain lines (no `#`).
pub(super) fn summary_text(summary: &Summary) -> String {
    let mut text = format!(
        "Census Summary (census_version {CENSUS_VERSION})\nrows: {}\ndesigns: {}\n",
        summary.rows, summary.designs
    );
    let statuses: Vec<String> = summary
        .statuses
        .iter()
        .map(|(status, count)| format!("{status}={count}"))
        .collect();
    let _ = writeln!(text, "status: {}", statuses.join(" "));
    for mode in ["shift", "optimize"] {
        let tables: Vec<&Table> = summary.tables.iter().filter(|t| t.mode == mode).collect();
        if tables.is_empty() {
            continue;
        }
        let against = if mode == "shift" {
            "RETARGET"
        } else {
            "OPTIMIZE"
        };
        let _ = writeln!(
            text,
            "\nMode {mode} against {against} (excess over the turn travel for depth, girdle and \
             junction; absolute change for aspect and table; nearest-rank percentiles of the \
             rows whose census_status is valid):"
        );
        let rows: Vec<Vec<String>> = tables
            .iter()
            .map(|t| {
                vec![
                    t.figure.to_string(),
                    t.n.to_string(),
                    fixed_or_na(t.p50, TABLE_DECIMALS),
                    fixed_or_na(t.p90, TABLE_DECIMALS),
                    fixed_or_na(t.p95, TABLE_DECIMALS),
                    fixed_or_na(t.p100, TABLE_DECIMALS),
                    fixed(t.limit, TABLE_DECIMALS),
                    t.over_limit.to_string(),
                ]
            })
            .collect();
        text.push_str(&table(
            &[
                "figure",
                "n",
                "p50",
                "p90",
                "p95",
                "p100",
                "constant",
                "over_constant",
            ],
            &[
                Align::Left,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
            ],
            &rows,
        ));
    }
    let branch = &summary.branch;
    if branch.reaching > 0 {
        let counts: Vec<String> = branch
            .counts
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect();
        let _ = writeln!(
            text,
            "\nBranch diagnosis of the shift rows that reached the projection: {} rows ({})\n\
             branch_mismatch with the ideal masts in agreement (spread at most {SPREAD_TOLERANCE:e}): {}\n\
             branch_mismatch with a wider spread (read these by hand): {}\n\
             designs reaching the projection: {}; with a branch mismatch: {}\n\
             library designs reaching the projection: {}; with a branch mismatch: {}",
            branch.reaching,
            counts.join(" "),
            branch.mismatch_clean,
            branch.mismatch_wide,
            branch.designs_reaching,
            branch.designs_mismatch,
            branch.library_reaching,
            branch.library_mismatch,
        );
    }
    text
}

/// The CSV: header comment, header, rows, summary comments.
pub(super) fn csv(rows: &[Row], timings: bool, summary: &Summary) -> String {
    let mut text = format!("# census_version = {CENSUS_VERSION}\n");
    text.push_str(&csv_header(timings));
    for row in rows {
        text.push_str(&row.csv_line(timings));
    }
    for line in summary_text(summary).lines() {
        text.push_str("# ");
        text.push_str(line);
        text.push('\n');
    }
    text
}

fn table_json(t: &Table) -> Value {
    json!({
        "mode": t.mode,
        "figure": t.figure,
        "n": t.n,
        "p50": json_optional(t.p50, 6),
        "p90": json_optional(t.p90, 6),
        "p95": json_optional(t.p95, 6),
        "p100": json_optional(t.p100, 6),
        "constant": t.limit,
        "over_constant": t.over_limit,
    })
}

/// The JSON: version, columns, rows and the summary.
pub(super) fn json(rows: &[Row], timings: bool, summary: &Summary) -> String {
    let mut columns: Vec<&str> = COLUMNS.to_vec();
    if timings {
        columns.extend_from_slice(TIMING_COLUMNS);
    }
    let branch = &summary.branch;
    let document = json!({
        "census_version": CENSUS_VERSION,
        "columns": columns,
        "rows": rows.iter().map(|row| row.json(timings)).collect::<Vec<_>>(),
        "summary": {
            "rows": summary.rows,
            "designs": summary.designs,
            "statuses": summary.statuses,
            "tables": summary.tables.iter().map(table_json).collect::<Vec<_>>(),
            "branch": {
                "reaching": branch.reaching,
                "counts": branch.counts,
                "mismatch_clean": branch.mismatch_clean,
                "mismatch_wide": branch.mismatch_wide,
                "designs_reaching": branch.designs_reaching,
                "designs_mismatch": branch.designs_mismatch,
                "library_reaching": branch.library_reaching,
                "library_mismatch": branch.library_mismatch,
            },
        },
    });
    json_text(&document)
}
