//! Reports C/D extras: the externally-verified repair search's own acceptance/score
//! accounting, and printed-ratio coverage among catalogued designs that have no
//! attached `.asc` file at all.

use rusqlite::Connection;

use crate::{
    print_report::{median, pct},
    types::DesignResult,
};

/// Reports C/D extra accounting: acceptance rate, search cost, score medians
/// (initial / after anchor calibration / final), and the quality split between
/// accepted and unaccepted designs (the verifier's precision claim, checked
/// end-to-end).
pub fn print_verified_extras(results: &[DesignResult]) {
    const fn report_of(
        d: &DesignResult,
    ) -> &indicatrix::geometry::meet_solver::VerifiedSolveReport {
        match &d.verify {
            Some(r) => r,
            None => panic!("filtered to Some above"),
        }
    }
    let with_report: Vec<&DesignResult> = results.iter().filter(|d| d.verify.is_some()).collect();
    let accepted = with_report.iter().filter(|d| report_of(d).accepted).count();
    let total_runs: usize = with_report.iter().map(|d| report_of(d).pipeline_runs).sum();
    let total_overrides: usize = with_report
        .iter()
        .map(|d| report_of(d).overrides_applied)
        .sum();
    let total_anchor_moves: usize = with_report
        .iter()
        .map(|d| report_of(d).anchor_moves_applied)
        .sum();
    println!("\n=== Verified-search accounting ===");
    println!(
        "  accepted (combined printed-figure deviation <= tol): {accepted}/{} ({:.1}%)",
        with_report.len(),
        pct(accepted, with_report.len()),
    );
    println!(
        "  pipeline runs: {total_runs} total, mean {:.1}/design | level overrides committed: \
         {total_overrides} | anchor moves committed: {total_anchor_moves}",
        total_runs as f64 / with_report.len().max(1) as f64,
    );
    let finite_scores = |f: fn(&indicatrix::geometry::meet_solver::VerifiedSolveReport) -> f64| {
        with_report
            .iter()
            .map(|d| f(report_of(d)))
            .filter(|v| v.is_finite())
            .collect::<Vec<f64>>()
    };
    println!(
        "  combined-score medians: initial {:.4} | after anchor calibration {:.4} | final {:.4}",
        median(&finite_scores(|r| r.initial_score)),
        median(&finite_scores(|r| r.score_after_calibration)),
        median(&finite_scores(|r| r.final_score)),
    );
    for (label, want) in [("accepted", true), ("unaccepted", false)] {
        let subset: Vec<&&DesignResult> = with_report
            .iter()
            .filter(|d| report_of(d).accepted == want && d.worst_err.is_some())
            .collect();
        let ok = subset
            .iter()
            .filter(|d| d.worst_err.is_some_and(|w| w <= 0.10))
            .count();
        let errs: Vec<f64> = subset
            .iter()
            .flat_map(|d| d.tier_results.iter().map(|t| t.rel_err))
            .collect();
        println!(
            "  {label} designs: {} | every meet-derived tier within 10%: {:.1}% | pooled tier median rel err: {:.4}",
            subset.len(),
            pct(ok, subset.len()),
            median(&errs),
        );
    }
}

/// Counts, among `diagram_details` rows that have no `.asc` attachment at all (the
/// ~2,700-design population that ratio-anchoring exists to serve), how many have
/// `cw_ratio`, `pw_ratio`, and both -- a direct coverage number independent of
/// anything measurable via real masts (there are none for this population).
pub fn count_ratio_coverage_for_designs_without_asc(conn: &Connection) -> (i64, i64, i64, i64) {
    conn.query_row(
        "SELECT COUNT(*), \
                SUM(CASE WHEN cw_ratio IS NOT NULL THEN 1 ELSE 0 END), \
                SUM(CASE WHEN pw_ratio IS NOT NULL THEN 1 ELSE 0 END), \
                SUM(CASE WHEN cw_ratio IS NOT NULL AND pw_ratio IS NOT NULL THEN 1 ELSE 0 END) \
         FROM diagram_details dd \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM attached_files af WHERE af.detail_id = dd.id AND af.name LIKE '%.asc' \
         )",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            ))
        },
    )
    .expect("count ratio coverage for no-asc designs")
}
