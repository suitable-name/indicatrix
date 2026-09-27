//! Corpus-wide validation harness for `indicatrix::geometry::meet_solver`.
//!
//! Reads every real `.asc` file directly out of `facet_diagrams.sqlite` (one per
//! design, deduplicated by `detail_id` the same way `asc_corpus_report.rs` does),
//! blanks out nothing but the *masts* (angles/indices/gear/meet-text all stay), asks
//! [`meet_solver::solve_meet_points`] to re-derive them from angles and meets alone,
//! and compares the result against the file's own real recorded masts -- perfect
//! ground truth, since every `.asc` file already carries the answer.
//!
//! # Why this lives in `examples/` and not in the test suite
//!
//! It needs a real catalogue of thousands of designs to say anything meaningful, and
//! no such catalogue is shipped with this repository -- `facet_diagrams.sqlite` is the
//! user's own library. A `#[test]` cannot depend on data that may not exist, so this
//! stays an example you run deliberately against a catalogue you already have.
//!
//! This harness is kept because what it measures is not reproducible any other way:
//! several constants in the shipped solver cite this harness's full-corpus runs as
//! their provenance (see `geometry::meet_solver::anchors`'s scale-anchor doc comments
//! and `geometry::meet_solver::verify`'s Report C references). Deleting it would leave
//! those numbers with no way to re-derive them.
//!
//! `geometry::meet_solver`'s own 23 unit tests cover the solver's logic on synthetic
//! cases and run in the normal suite; this covers the thing they structurally cannot,
//! which is whether the solver reproduces thousands of real recorded masts.
//!
//! Run from the workspace root:
//! ```text
//! cargo run --profile probe -p indicatrix --example meet_solver_validation
//! ```
//!
//! Reports A (real-mast anchors) and B (printed-ratio anchors) always run
//! (~45 s each on the full corpus). Append `verified` to also run Report C,
//! the externally-verified repair search (`solve_meet_points_verified`, scored
//! against each design's printed `Vol/W^3`/`L/W`/`C/W`/`P/W`/`H/W` figures) --
//! it re-runs the pipeline up to ~120 times per design, so budget ~25-35
//! minutes for the corpus:
//! ```text
//! cargo run --profile probe -p indicatrix --example meet_solver_validation -- verified
//! ```
//!
//! # Parallelism
//!
//! Each design's solve is fully independent (no shared state, no I/O once the BLOBs
//! are loaded), so this reads every row on the main thread first -- the sqlite read
//! is not the bottleneck, the geometry (repeated candidate-vertex enumerations per
//! tier) is -- then hands rows to `THREADS` `std::thread::scope` workers in
//! interleaved order (worker `w` solves rows `w`, `w + THREADS`, ...), which keeps
//! the load balanced when per-design cost varies by orders of magnitude (see
//! `run_and_report`). Deliberately `std::thread` rather than pulling in `rayon`:
//! `indicatrix` is kept near-zero-dependency on purpose (see its `Cargo.toml`), and this
//! probe is temporary. Results are re-slotted by original row index -- so which
//! thread a design happened to run on never affects the reported
//! medians/percentiles run to run.

use rusqlite::Connection;
use std::collections::HashSet;

mod db;
mod print_report;
mod run_and_report;
mod scoring;
mod solve_ratio_anchored;
mod solve_real_mast;
mod types;
mod verified_extras;

use db::{find_db_path, load_asc_rows};
use run_and_report::run_and_report;
use solve_ratio_anchored::{solve_one_ratio_anchored, solve_one_ratio_anchored_verified};
use solve_real_mast::{solve_one, solve_one_verified};
use types::AscRow;
use verified_extras::{count_ratio_coverage_for_designs_without_asc, print_verified_extras};

fn main() {
    let db_path = find_db_path();
    println!("Using database: {db_path}");
    let conn = Connection::open(&db_path).expect("open facet_diagrams.sqlite");
    let rows = load_asc_rows(&conn);
    println!(
        "Loaded {} attached .asc rows from the database.",
        rows.len()
    );

    // Dedup by detail_id, keeping the first row seen per design (matches
    // `asc_corpus_report.rs`'s convention) -- sequential, since it's a single cheap
    // pass and establishes the fixed processing order every run reports against.
    let mut seen_designs: HashSet<i64> = HashSet::new();
    let unique_rows: Vec<AscRow> = rows
        .into_iter()
        .filter(|row| seen_designs.insert(row.detail_id))
        .collect();
    println!(
        "{} unique designs after dedup by detail_id.",
        unique_rows.len()
    );

    let with_cw = unique_rows.iter().filter(|r| r.cw_ratio.is_some()).count();
    let with_pw = unique_rows.iter().filter(|r| r.pw_ratio.is_some()).count();
    let with_both = unique_rows
        .iter()
        .filter(|r| r.cw_ratio.is_some() && r.pw_ratio.is_some())
        .count();
    println!(
        "  of those, {with_cw} have a printed cw_ratio, {with_pw} a printed pw_ratio, {with_both} both."
    );
    let no_asc_ratio_coverage = count_ratio_coverage_for_designs_without_asc(&conn);
    println!(
        "  designs in diagram_details with NO attached .asc at all: {} total, {} with cw_ratio, \
         {} with pw_ratio, {} with both (the population Item 1 exists to serve).",
        no_asc_ratio_coverage.0,
        no_asc_ratio_coverage.1,
        no_asc_ratio_coverage.2,
        no_asc_ratio_coverage.3
    );

    run_and_report(
        "REPORT A -- baseline: bootstrapped from each file's own tier-0 REAL mast \
         (harness-only crutch; not available without an .asc file)",
        &unique_rows,
        solve_one,
        |d| d.no_scale_reference,
    );
    run_and_report(
        "REPORT B -- production path: anchored from printed C/W, P/W proportions \
         (falls back to tier-0 real mast only when a block's ratio is missing)",
        &unique_rows,
        solve_one_ratio_anchored,
        |d| d.parse_ok && !d.fully_ratio_anchored,
    );

    // Report C is opt-in (`-- verified` on the command line): the repair search
    // runs up to ~120 pipeline configurations per design, so a full-corpus pass
    // costs ~25-35 minutes instead of Report A's ~45 seconds.
    if std::env::args().any(|a| a == "verified") {
        let results = run_and_report(
            "REPORT C -- externally verified: Report A anchoring plus the printed-proportion \
             repair search (solve_meet_points_verified; pass `verified` to run this)",
            &unique_rows,
            solve_one_verified,
            |d| d.no_scale_reference,
        );
        print_verified_extras(&results);

        let results_d = run_and_report(
            "REPORT D -- production path, calibrated + verified: printed-ratio anchors marked \
             adjustable and calibrated against the printed figures, then the repair search \
             (solve_meet_points_verified with adjustable anchors; pass `verified` to run this)",
            &unique_rows,
            solve_one_ratio_anchored_verified,
            |d| d.parse_ok && !d.fully_ratio_anchored,
        );
        print_verified_extras(&results_d);
    }
}
