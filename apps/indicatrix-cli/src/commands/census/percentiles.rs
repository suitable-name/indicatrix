//! The percentile tables at the end of a census (follow-ups plan 2026-10-19, section 4.4) and
//! the branch summary behind decision D4.
//!
//! A percentile is the nearest-rank one: the value at rank `ceil(p n / 100)` of the sorted
//! figures, so the 100th is the maximum and no value is interpolated. Only rows whose
//! `census_status` is `valid` count: the others have no measured shape.

use super::rows::Row;
use indicatrix_cut_core::meet_preservation::PreservationPolicy;
use std::collections::{BTreeMap, BTreeSet};

/// A branch mismatch whose ideal masts disagree by more than this (in units of the stone's
/// extent, one preservation tolerance) is listed apart: the diagnosis is not clean there.
pub(super) const SPREAD_TOLERANCE: f64 = 2e-6;

/// The modes, in the order their tables are printed, with the policy each is read against.
const MODES: [(&str, PreservationPolicy); 2] = [
    ("shift", PreservationPolicy::RETARGET),
    ("optimize", PreservationPolicy::OPTIMIZE),
];

/// The figures: name, the column they read and whether the absolute value is read (the shape
/// measures without a turn grant) or the signed excess over the turn's travel.
const FIGURES: [(&str, &str, bool); 5] = [
    ("depth", "excess_depth", false),
    ("girdle", "excess_girdle", false),
    ("junction", "excess_junction", false),
    ("aspect", "d_aspect", true),
    ("table", "d_table_pts", true),
];

/// The constant of `policy` a figure is bounded by.
fn limit_of(figure: &str, policy: &PreservationPolicy) -> f64 {
    match figure {
        "depth" => policy.max_depth_change,
        "girdle" => policy.max_girdle_change,
        "junction" => policy.max_junction_displacement,
        "aspect" => policy.max_aspect_change,
        _ => policy.max_table_percent_change,
    }
}

/// The nearest-rank percentile `percent` of `sorted` (ascending); `None` when it is empty.
pub(super) fn percentile(sorted: &[f64], percent: usize) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (percent * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied()
}

/// One figure of one mode against the constant that bounds it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Table {
    /// `shift` or `optimize`.
    pub mode: &'static str,
    /// `depth`, `girdle`, `junction`, `aspect` or `table`.
    pub figure: &'static str,
    /// Rows with a measured figure.
    pub n: usize,
    /// The 50th percentile.
    pub p50: Option<f64>,
    /// The 90th percentile.
    pub p90: Option<f64>,
    /// The 95th percentile.
    pub p95: Option<f64>,
    /// The 100th percentile (the maximum).
    pub p100: Option<f64>,
    /// The current constant (`RETARGET` for shift, `OPTIMIZE` for optimize).
    pub limit: f64,
    /// Rows whose figure alone exceeds the constant.
    pub over_limit: usize,
}

/// How many refusals are branch mismatches (decision D4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct BranchSummary {
    /// Shift rows that reached the projection (status valid or invalid).
    pub reaching: usize,
    /// Of those, by `branch` value: none, infeasible, `branch_mismatch`, unresolved.
    pub counts: BTreeMap<String, usize>,
    /// Branch mismatches whose ideal masts agree within [`SPREAD_TOLERANCE`].
    pub mismatch_clean: usize,
    /// Branch mismatches whose ideal masts disagree by more.
    pub mismatch_wide: usize,
    /// Designs with at least one shift row that reached the projection.
    pub designs_reaching: usize,
    /// Designs with at least one branch mismatch.
    pub designs_mismatch: usize,
    /// The same two counts for the designs of the library alone.
    pub library_reaching: usize,
    /// Library designs with at least one branch mismatch.
    pub library_mismatch: usize,
}

/// Everything the end of a census prints.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Summary {
    /// Rows in all.
    pub rows: usize,
    /// Distinct designs.
    pub designs: usize,
    /// Rows by `status` (empty status counted as `none`).
    pub statuses: BTreeMap<String, usize>,
    /// The percentile tables.
    pub tables: Vec<Table>,
    /// The branch summary.
    pub branch: BranchSummary,
}

fn design_id(row: &Row) -> (String, String) {
    (row.text("source").to_string(), row.text("key").to_string())
}

fn tables(rows: &[Row]) -> Vec<Table> {
    let mut tables = Vec::new();
    for (mode, policy) in MODES {
        if !rows.iter().any(|row| row.text("mode") == mode) {
            continue;
        }
        for (figure, column, absolute) in FIGURES {
            let mut values: Vec<f64> = rows
                .iter()
                .filter(|row| row.text("mode") == mode && row.text("census_status") == "valid")
                .filter_map(|row| row.num(column))
                .map(|value| if absolute { value.abs() } else { value })
                .collect();
            values.sort_by(f64::total_cmp);
            let limit = limit_of(figure, &policy);
            tables.push(Table {
                mode,
                figure,
                n: values.len(),
                p50: percentile(&values, 50),
                p90: percentile(&values, 90),
                p95: percentile(&values, 95),
                p100: percentile(&values, 100),
                limit,
                over_limit: values.iter().filter(|value| **value > limit).count(),
            });
        }
    }
    tables
}

fn branch_summary(rows: &[Row]) -> BranchSummary {
    let mut summary = BranchSummary::default();
    let mut reaching = BTreeSet::new();
    let mut mismatch = BTreeSet::new();
    for row in rows {
        let reached =
            row.text("mode") == "shift" && matches!(row.text("status"), "valid" | "invalid");
        if !reached {
            continue;
        }
        summary.reaching += 1;
        let branch = row.text("branch");
        *summary
            .counts
            .entry(if branch.is_empty() { "none" } else { branch }.to_string())
            .or_default() += 1;
        reaching.insert(design_id(row));
        if branch == "branch_mismatch" {
            mismatch.insert(design_id(row));
            if row.num("ideal_spread").unwrap_or(0.0) > SPREAD_TOLERANCE {
                summary.mismatch_wide += 1;
            } else {
                summary.mismatch_clean += 1;
            }
        }
    }
    summary.designs_reaching = reaching.len();
    summary.designs_mismatch = mismatch.len();
    summary.library_reaching = reaching.iter().filter(|(s, _)| s == "library").count();
    summary.library_mismatch = mismatch.iter().filter(|(s, _)| s == "library").count();
    summary
}

/// The summary of `rows`.
pub(super) fn summarize(rows: &[Row]) -> Summary {
    let mut statuses = BTreeMap::new();
    for row in rows {
        let status = row.text("status");
        *statuses
            .entry(if status.is_empty() { "none" } else { status }.to_string())
            .or_default() += 1;
    }
    Summary {
        rows: rows.len(),
        designs: rows.iter().map(design_id).collect::<BTreeSet<_>>().len(),
        statuses,
        tables: tables(rows),
        branch: branch_summary(rows),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::census::rows::Cell;

    #[test]
    fn nearest_rank_percentiles_pick_actual_values() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(percentile(&values, 50), Some(5.0));
        assert_eq!(percentile(&values, 90), Some(9.0));
        assert_eq!(percentile(&values, 95), Some(10.0));
        assert_eq!(percentile(&values, 100), Some(10.0));
        assert_eq!(percentile(&[7.0], 50), Some(7.0));
        assert_eq!(percentile(&[], 50), None);
    }

    fn shift_row(key: &str, excess: f64, branch: &str, spread: f64) -> Row {
        let mut row = Row::new();
        row.set("source", Cell::text("library"));
        row.set("key", Cell::text(key));
        row.set("mode", Cell::text("shift"));
        row.set("status", Cell::text("invalid"));
        row.set("census_status", Cell::text("valid"));
        row.set("excess_depth", Cell::num(excess));
        row.set("branch", Cell::text(branch));
        row.set("ideal_spread", Cell::num(spread));
        row
    }

    #[test]
    fn the_tables_count_what_the_constant_alone_would_refuse() {
        let rows = vec![
            shift_row("1", 0.01, "none", 0.0),
            shift_row("2", 0.09, "none", 0.0),
            shift_row("3", 0.5, "branch_mismatch", 0.0),
        ];
        let summary = summarize(&rows);
        let depth = &summary.tables[0];
        assert_eq!((depth.mode, depth.figure, depth.n), ("shift", "depth", 3));
        assert_eq!(depth.p100, Some(0.5));
        assert_eq!(depth.over_limit, 2, "0.09 and 0.5 exceed 0.08");
        assert_eq!(
            summary.tables.len(),
            5,
            "no optimize rows, no optimize tables"
        );
    }

    #[test]
    fn the_branch_summary_separates_clean_and_wide_mismatches() {
        let rows = vec![
            shift_row("1", 0.0, "branch_mismatch", 0.0),
            shift_row("2", 0.0, "branch_mismatch", 0.5),
            shift_row("2", 0.0, "infeasible", 0.0),
        ];
        let branch = summarize(&rows).branch;
        assert_eq!(branch.reaching, 3);
        assert_eq!((branch.mismatch_clean, branch.mismatch_wide), (1, 1));
        assert_eq!((branch.designs_reaching, branch.designs_mismatch), (2, 2));
        assert_eq!((branch.library_reaching, branch.library_mismatch), (2, 2));
        assert_eq!(branch.counts.get("infeasible"), Some(&1));
    }
}
