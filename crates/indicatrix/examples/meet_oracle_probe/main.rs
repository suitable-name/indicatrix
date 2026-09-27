//! TEMPORARY oracle probe for the vertex-incidence meet model.
//!
//! For every tier of every corpus design, this pins every *other* tier at its real
//! recorded mast and asks: is this tier's own recorded mast realized as `n . v` for a
//! vertex `v` of the arrangement of the other tiers' planes? (The "E3b" measurement
//! from the Python analysis, reproduced in Rust, in f64, deterministically -- no
//! convex-hull crate anywhere.) It also gathers the statistics the incremental solver
//! rebuild needs:
//!
//! - rank of the true vertex among candidate levels ordered by `n . v` descending
//!   (rank 0 == support-function tangency, the current solver's model);
//! - how much intersecting candidate values across a tier's symmetric index
//!   instances prunes the candidate list;
//! - whether "deepest cut that leaves every other facet alive" identifies the true
//!   level (a candidate local selection rule);
//! - E4: for stated `"Meet <names>"` tiers whose names resolve, does restricting
//!   candidates to vertices incident to the named tiers pin the true value;
//! - an incremental-reachability ceiling: starting from the design's scale anchor,
//!   can every tier's true vertex be pinned by already-reachable tiers plus the
//!   tier's own sibling instances;
//! - a global-solve ceiling: with the TRUE incidence structure known, is the whole
//!   mast vector uniquely determined by the anchors alone (one big linear system).
//!
//! Run from the workspace root:
//! ```text
//! cargo run --profile probe -p indicatrix --example meet_oracle_probe
//! ```

// Temporary measurement tooling: readability-of-the-measurement wins over the
// production lint bar here (the lints below are all style/perf-shape, not
// correctness).
#![allow(
    clippy::doc_markdown,
    clippy::suboptimal_flops,
    clippy::many_single_char_names,
    clippy::needless_range_loop,
    clippy::option_if_let_else,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::too_long_first_doc_paragraph,
    clippy::useless_let_if_seq
)]

use rusqlite::Connection;
use std::collections::HashSet;

mod analyze;
mod candidates;
mod db;
mod linalg;
mod report;
mod types;

use analyze::analyze_one;
use db::{find_db_path, load_asc_rows};
use report::{frac_below, pct, print_dist, print_rank_hist};
use types::{AscRow, DesignResult, MATCH_REL, THREADS, TierStats};

fn main() {
    let db_path = find_db_path();
    println!("Using database: {db_path}");
    let conn = Connection::open(&db_path).expect("open facet_diagrams.sqlite");
    let rows = load_asc_rows(&conn);
    let mut seen: HashSet<i64> = HashSet::new();
    let unique_rows: Vec<AscRow> = rows
        .into_iter()
        .filter(|r| seen.insert(r.detail_id))
        .collect();
    println!("{} unique designs.", unique_rows.len());

    let start = std::time::Instant::now();
    let chunk_len = unique_rows.len().div_ceil(THREADS).max(1);
    let chunks: Vec<&[AscRow]> = unique_rows.chunks(chunk_len).collect();
    let mut results: Vec<DesignResult> = Vec::with_capacity(unique_rows.len());
    std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| s.spawn(move || chunk.iter().map(analyze_one).collect::<Vec<_>>()))
            .collect();
        for h in handles {
            results.extend(h.join().expect("worker thread panicked"));
        }
    });
    println!("Analyzed in {:.2?}.", start.elapsed());

    let parsed = results.iter().filter(|d| d.parsed).count();
    let skipped = results.iter().filter(|d| d.skipped_too_big).count();
    println!("parsed={parsed} skipped_too_big={skipped}");

    let all_tiers: Vec<&TierStats> = results
        .iter()
        .flat_map(|d| d.tiers.iter())
        .filter(|t| t.scored)
        .collect();
    println!("scored meet-derived tiers: {}", all_tiers.len());

    // E3b
    let err0: Vec<f64> = all_tiers.iter().map(|t| t.err0).collect();
    let errw: Vec<f64> = all_tiers.iter().map(|t| t.err_worst).collect();
    print_dist("E3b err (instance 0)", &err0);
    print_dist("E3b err (worst instance)", &errw);
    println!(
        "E3b frac < 0.5% (inst 0): {:.1}%   (worst inst): {:.1}%",
        frac_below(&err0, MATCH_REL) * 100.0,
        frac_below(&errw, MATCH_REL) * 100.0
    );

    // Tangency
    let tang: Vec<f64> = all_tiers
        .iter()
        .filter(|t| t.tangency_ratio.is_finite())
        .map(|t| t.tangency_ratio)
        .collect();
    print_dist("tangency ratio (max cand / true)", &tang);

    // Ranks
    let matched: Vec<&TierStats> = all_tiers
        .iter()
        .copied()
        .filter(|t| t.rank.is_some())
        .collect();
    println!(
        "\ntiers with a matching level (rank defined): {}",
        matched.len()
    );
    print_rank_hist(
        "rank (inst 0)",
        &matched.iter().map(|t| t.rank.unwrap()).collect::<Vec<_>>(),
    );
    let sym_matched: Vec<usize> = all_tiers.iter().filter_map(|t| t.rank_sym).collect();
    print_rank_hist("rank_sym (symmetry-intersected)", &sym_matched);

    // Deepest-safe rule
    let with_safe: Vec<&TierStats> = matched
        .iter()
        .copied()
        .filter(|t| t.deepest_safe_rank.is_some())
        .collect();
    let safe_hits = with_safe
        .iter()
        .filter(|t| t.deepest_safe_rank == t.rank)
        .count();
    let safe_off1 = with_safe
        .iter()
        .filter(|t| {
            let (r, s) = (t.rank.unwrap(), t.deepest_safe_rank.unwrap());
            r.abs_diff(s) <= 1
        })
        .count();
    println!(
        "\ndeepest-safe rule: true==deepest_safe {}/{} ({:.1}%), within 1 level {:.1}%",
        safe_hits,
        with_safe.len(),
        pct(safe_hits, with_safe.len()),
        pct(safe_off1, with_safe.len())
    );

    // E4
    let e4_all: Vec<f64> = all_tiers.iter().filter_map(|t| t.e4_err).collect();
    let e4_full: Vec<f64> = all_tiers
        .iter()
        .filter(|t| {
            t.named_resolved
                .is_some_and(|(res, tot)| res == tot && tot > 0)
        })
        .filter_map(|t| t.e4_err)
        .collect();
    print_dist("E4 err (>=1 resolved named ref)", &e4_all);
    println!(
        "E4 frac < 0.5%: {:.1}% (n={})",
        frac_below(&e4_all, MATCH_REL) * 100.0,
        e4_all.len()
    );
    print_dist("E4 err (all names resolved)", &e4_full);
    println!(
        "E4-fully-resolved frac < 0.5%: {:.1}% (n={})",
        frac_below(&e4_full, MATCH_REL) * 100.0,
        e4_full.len()
    );
    let named_tiers = all_tiers
        .iter()
        .filter(|t| t.named_resolved.is_some())
        .count();
    let named_some = all_tiers
        .iter()
        .filter(|t| t.named_resolved.is_some_and(|(r, _)| r > 0))
        .count();
    println!("MeetNamed tiers: {named_tiers}, with >=1 exact-resolved ref: {named_some}");

    // Reachability
    let reachable = all_tiers.iter().filter(|t| t.reachable).count();
    println!(
        "\nincremental reachability ceiling: {}/{} tiers ({:.1}%)",
        reachable,
        all_tiers.len(),
        pct(reachable, all_tiers.len())
    );
    let designs_scored = results.iter().filter(|d| d.any_scored).count();
    let designs_all = results.iter().filter(|d| d.all_reachable).count();
    println!(
        "designs with every scored tier reachable: {}/{} ({:.1}%)",
        designs_all,
        designs_scored,
        pct(designs_all, designs_scored)
    );

    // Selection-rule experiment
    let n_scored = all_tiers.len();
    let hits = |f: &dyn Fn(&TierStats) -> bool| all_tiers.iter().filter(|t| f(t)).count();
    println!("\nselection rules (others at truth), hit = pred within 0.5% of true:");
    for (name, f) in [
        (
            "rank1          ",
            &(|t: &TierStats| t.hit_rank1) as &dyn Fn(&TierStats) -> bool,
        ),
        ("named->rank1   ", &|t: &TierStats| t.hit_named_rank1),
        ("degree         ", &|t: &TierStats| t.hit_degree),
        ("named->degree  ", &|t: &TierStats| t.hit_named_degree),
    ] {
        let h = hits(f);
        println!("  {name}: {h}/{n_scored} ({:.1}%)", pct(h, n_scored));
    }
    let nr_err: Vec<f64> = all_tiers.iter().filter_map(|t| t.named_rank1_err).collect();
    print_dist("  named->rank1 pred rel err", &nr_err);
    println!(
        "  named->rank1 err <=10%: {:.1}%",
        frac_below(&nr_err, 0.10) * 100.0
    );
    // Per-design: every scored tier within 10% / hit, under named->rank1.
    let mut d_hit = 0usize;
    let mut d_10 = 0usize;
    let mut d_tot = 0usize;
    for d in &results {
        let scored: Vec<&TierStats> = d.tiers.iter().filter(|t| t.scored).collect();
        if scored.is_empty() {
            continue;
        }
        d_tot += 1;
        if scored.iter().all(|t| t.hit_named_rank1) {
            d_hit += 1;
        }
        if scored
            .iter()
            .all(|t| t.named_rank1_err.is_some_and(|e| e <= 0.10))
        {
            d_10 += 1;
        }
    }
    println!(
        "  designs all-hit under named->rank1: {d_hit}/{d_tot} ({:.1}%); all within 10%: {d_10} ({:.1}%)",
        pct(d_hit, d_tot),
        pct(d_10, d_tot)
    );

    // Global-solve ceiling
    let gerr: Vec<f64> = all_tiers.iter().filter_map(|t| t.global_err).collect();
    print_dist("\nglobal true-incidence solve err", &gerr);
    println!(
        "global-solve frac < 0.5%: {:.1}%  < 5%: {:.1}%  (n={}, of {} scored)",
        frac_below(&gerr, MATCH_REL) * 100.0,
        frac_below(&gerr, 0.05) * 100.0,
        gerr.len(),
        all_tiers.len()
    );
    // Per-design: every scored tier under 10% via the global solve.
    let mut designs_global_ok = 0usize;
    let mut designs_global_tot = 0usize;
    for d in &results {
        let scored: Vec<&TierStats> = d.tiers.iter().filter(|t| t.scored).collect();
        if scored.is_empty() {
            continue;
        }
        designs_global_tot += 1;
        if scored
            .iter()
            .all(|t| t.global_err.is_some_and(|e| e <= 0.10))
        {
            designs_global_ok += 1;
        }
    }
    println!(
        "designs with every scored tier within 10% via global solve: {}/{} ({:.1}%)",
        designs_global_ok,
        designs_global_tot,
        pct(designs_global_ok, designs_global_tot)
    );

    // Degeneracy signal
    let mut ratios: Vec<f64> = Vec::new();
    let mut sep_ok = 0usize;
    let mut sep_tot = 0usize;
    for d in &results {
        let (Some(dt), Some(ds), Some(err)) =
            (d.degeneracy_truth, d.degeneracy_solved, d.solver_median_err)
        else {
            continue;
        };
        if !dt.is_finite() || !ds.is_finite() {
            continue;
        }
        // Only designs where the solver is meaningfully wrong are informative.
        if err > 0.05 {
            sep_tot += 1;
            if dt > ds {
                sep_ok += 1;
            }
            ratios.push(if dt > 0.0 { ds / dt } else { f64::NAN });
        }
    }
    print_dist(
        "\nD(solved)/D(truth) on designs where solver median err > 5%",
        &ratios,
    );
    println!(
        "degeneracy separates (D(truth) > D(solved)) on {}/{} such designs ({:.1}%)",
        sep_ok,
        sep_tot,
        pct(sep_ok, sep_tot)
    );
}
