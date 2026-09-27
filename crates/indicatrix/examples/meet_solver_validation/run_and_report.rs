//! Parallel dispatch: runs a solving strategy across every design (split across
//! `THREADS` scoped worker threads, results re-slotted by original row index) and
//! prints the aggregate report under a given header.

use crate::{
    print_report::print_report,
    types::{AscRow, DesignResult, THREADS, TierResult},
};

/// Runs `solve_fn` across every design in `rows` (split across [`THREADS`] scoped
/// worker threads; results are re-slotted by original row index so the reported
/// medians/percentiles never depend on thread scheduling) and prints the full
/// report under the given header.
///
/// Workers take rows interleaved (worker `w` solves rows `w`, `w + THREADS`,
/// ...) rather than in contiguous chunks: per-design cost varies by orders of
/// magnitude (candidate-vertex enumeration is cubic in plane count, and Report
/// C's repair search multiplies that by up to ~120 pipeline runs), so a
/// contiguous chunk of heavy designs would strand one thread while the rest
/// idle.
pub fn run_and_report(
    header: &str,
    rows: &[AscRow],
    solve_fn: fn(&AscRow) -> DesignResult,
    needed_fallback: fn(&DesignResult) -> bool,
) -> Vec<DesignResult> {
    println!("\n\n########## {header} ##########");
    let start = std::time::Instant::now();
    let total = rows.len();
    let mut slots: Vec<Option<DesignResult>> = Vec::with_capacity(total);
    slots.resize_with(total, || None);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|w| {
                s.spawn(move || {
                    let mut mine: Vec<(usize, DesignResult)> = Vec::new();
                    let mut idx = w;
                    while idx < total {
                        mine.push((idx, solve_fn(&rows[idx])));
                        idx += THREADS;
                    }
                    mine
                })
            })
            .collect();
        for h in handles {
            for (idx, result) in h.join().expect("solver worker thread panicked") {
                slots[idx] = Some(result);
            }
        }
    });
    let design_results: Vec<DesignResult> = slots
        .into_iter()
        .map(|slot| slot.expect("every design slot filled"))
        .collect();
    println!(
        "Solved {} designs across {THREADS} threads in {:.2?}.",
        design_results.len(),
        start.elapsed()
    );

    let parse_ok = design_results.iter().filter(|d| d.parse_ok).count();
    let parse_err = design_results.len() - parse_ok;
    let no_scale_reference_at_all = design_results.iter().filter(|d| needed_fallback(d)).count();
    let bucket_counts = (
        design_results
            .iter()
            .filter(|d| d.parse_ok && d.has_meet_named)
            .count(),
        design_results
            .iter()
            .filter(|d| d.parse_ok && d.has_meet_existing)
            .count(),
        design_results
            .iter()
            .filter(|d| d.parse_ok && d.has_scale_reference)
            .count(),
    );
    let design_worst_err: Vec<Option<f64>> = design_results
        .iter()
        .filter(|d| d.parse_ok)
        .map(|d| d.worst_err)
        .collect();
    let tier_results: Vec<TierResult> = design_results
        .iter()
        .flat_map(|d| d.tier_results.iter().cloned())
        .collect();

    print_report(
        parse_ok,
        parse_err,
        no_scale_reference_at_all,
        &tier_results,
        &design_worst_err,
        bucket_counts,
    );
    design_results
}
