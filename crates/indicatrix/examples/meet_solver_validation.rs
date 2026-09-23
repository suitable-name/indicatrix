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

use indicatrix::geometry::{
    meet_solver::{
        MeetConstraint, SolveStrategy, apply_ratio_anchors, meet_tier_inputs_from_asc,
        solve_meet_points, solve_meet_points_verified,
    },
    stone_metrics::ExternalProportions,
};
use indicatrix_formats::asc;
use rusqlite::Connection;
use std::collections::HashSet;

/// Worker count for the parallel solve below. Fixed rather than queried from the
/// system (`std::thread::available_parallelism`) so a run's shape doesn't silently
/// change between machines -- this is a throwaway probe re-run often while iterating,
/// not a shipped tool, so a hardcoded figure matching the dev machine (16 cores) is
/// the simpler choice.
const THREADS: usize = 16;

struct AscRow {
    detail_id: i64,
    content: Vec<u8>,
    /// Printed crown-height/width and pavilion-depth/width proportions from
    /// `diagram_details` (`NULL` for a design the source never recorded them for).
    /// Only used by [`solve_one_ratio_anchored`]; [`solve_one`] (the original
    /// tier-0-bootstrap measurement) ignores these entirely.
    cw_ratio: Option<f64>,
    pw_ratio: Option<f64>,
    /// Printed `Vol/W^3`, `L/W` and `H/W`, used (together with the two ratios
    /// above) only by Report C's [`solve_one_verified`] as the external
    /// verification targets.
    volume: Option<f64>,
    lw_ratio: Option<f64>,
    hw_ratio: Option<f64>,
}

/// Which kind of information actually determined a tier's constraint, *before* the
/// bootstrap fallback (see `main`) might override tier 0. Used only for reporting --
/// separates "the schedule stated this" from "the solver had to infer it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstraintKind {
    ScaleReference,
    MeetNamed,
    MeetExisting,
}

#[derive(Clone)]
struct TierResult {
    strategy: SolveStrategy,
    rel_err: f64,
    kind: ConstraintKind,
    /// Only meaningful when `kind == ConstraintKind::MeetNamed`: `Some(true)` if
    /// every name this tier's stated `"Meet <names>"` instruction referenced
    /// resolved to a known tier (via the *exact same* `name_to_tier`/`girdle_tier`/
    /// `resolve_name` logic `solve_meet_points` itself uses internally -- see
    /// `classify_named_resolution` below), `Some(false)` if one or more didn't,
    /// `None` for any other `kind`. This is what splits `MeetNamed` into the
    /// `MeetNamed-resolved` / `MeetNamed-unresolved` buckets in the report: real
    /// `.asc` meet text is hand-typed free prose ("Meet the girdle", "Meet 2 and
    /// the culet", "Meet P1, P4, P4, Form, PCP") where a majority of stated names
    /// never resolve to anything at all, and the original evidence for the
    /// vertex-incidence model was measured only on the subset that does resolve --
    /// so the two populations must be reported separately, never blended.
    named_resolved: Option<bool>,
    /// Whether the solver's constructive pass actually settled this tier via its
    /// resolved named references (as opposed to falling back to the rank-1 prior
    /// because the references hadn't settled yet, or no incident level existed).
    /// Read from the solver's own `detail` string.
    used_named: bool,
    /// For a rank-1 fallback on a tier with resolved refs: why (from the solver's
    /// detail string). 'u' = refs not settled at release, 'n' = no incident
    /// feasible level, ' ' = not a named fallback.
    fallback_cause: char,
}

/// Everything one design's solve contributes to the aggregate report. Produced by
/// [`solve_one`], which is the unit of work parallelized across `THREADS` workers.
#[derive(Default)]
struct DesignResult {
    parse_ok: bool,
    /// True iff the design had no stated scale-reference tier at all, so the harness
    /// had to bootstrap one from the file's own tier-0 real mast (see `solve_one`).
    /// Only meaningful when `parse_ok`.
    no_scale_reference: bool,
    tier_results: Vec<TierResult>,
    /// Worst meet-derived tier's relative error, or `None` if the design had none to
    /// score. Only meaningful when `parse_ok`.
    worst_err: Option<f64>,
    /// Which `ConstraintKind`s appear anywhere in this design's tiers -- for the
    /// per-design bucket counts (a design can and often does mix kinds).
    has_meet_named: bool,
    has_meet_existing: bool,
    has_scale_reference: bool,
    /// Only meaningful for [`solve_one_ratio_anchored`]'s results: true iff every
    /// block that needed an anchor at all got it from a printed `C/W`/`P/W` ratio
    /// (or a schedule's own stated scale reference) -- i.e. this design's solve
    /// needed *no* real-recorded-mast fallback anywhere. False whenever at least
    /// one block's ratio was missing and had to fall back to its own tier-0 real
    /// mast (a harness-only crutch unavailable to the ~2,700 catalogued designs
    /// with no `.asc` file at all -- see the module doc comment).
    fully_ratio_anchored: bool,
    /// Reports C and D only ([`solve_one_verified`] /
    /// [`solve_one_ratio_anchored_verified`]): the externally-verified repair
    /// search's own accounting. `None` for the other reports.
    verify: Option<indicatrix::geometry::meet_solver::VerifiedSolveReport>,
}

/// Solves one design end-to-end (parse, bootstrap scale reference if needed, solve,
/// score against the file's own real masts) and returns everything the aggregate
/// report needs. No shared state with any other design -- safe to call from any
/// thread on a disjoint row.
fn solve_one(row: &AscRow) -> DesignResult {
    solve_one_impl(row, |gear, tiers| (solve_meet_points(gear, tiers), None))
}

/// Report C: like [`solve_one`] (identical anchoring and scoring conventions),
/// but solving via [`solve_meet_points_verified`] with the design's printed
/// proportions as the external repair/verification targets.
fn solve_one_verified(row: &AscRow) -> DesignResult {
    let targets = ExternalProportions {
        vol_w3: row.volume,
        lw: row.lw_ratio,
        cw: row.cw_ratio,
        pw: row.pw_ratio,
        hw: row.hw_ratio,
    };
    solve_one_impl(row, move |gear, tiers| {
        let (solved, report) = solve_meet_points_verified(gear, tiers, &targets, &[]);
        (solved, Some(report))
    })
}

fn solve_one_impl(
    row: &AscRow,
    solve: impl FnOnce(
        u32,
        &[indicatrix::geometry::meet_solver::MeetTierInput],
    ) -> (
        Vec<indicatrix::geometry::meet_solver::SolvedTier>,
        Option<indicatrix::geometry::meet_solver::VerifiedSolveReport>,
    ),
) -> DesignResult {
    let mut out = DesignResult::default();

    let text = String::from_utf8_lossy(&row.content);
    let Ok(schedule) = asc::parse_asc(&text) else {
        return out; // parse_ok stays false
    };
    out.parse_ok = true;
    if schedule.tiers.is_empty() {
        return out;
    }

    let mut tiers = meet_tier_inputs_from_asc(&schedule);
    let original_kinds: Vec<ConstraintKind> = tiers
        .iter()
        .map(|t| match &t.constraint {
            MeetConstraint::ScaleReference(_) => ConstraintKind::ScaleReference,
            MeetConstraint::MeetNamed(_) => ConstraintKind::MeetNamed,
            MeetConstraint::MeetExisting => ConstraintKind::MeetExisting,
        })
        .collect();
    // Captured *before* the tier-0 scale-reference bootstrap below can overwrite a
    // `MeetNamed` tier 0's `.constraint` (and with it, its name list) -- so a
    // bootstrapped design's original stated names are never lost.
    let original_named_names: Vec<Option<Vec<String>>> = tiers
        .iter()
        .map(|t| match &t.constraint {
            MeetConstraint::MeetNamed(names) => Some(names.clone()),
            _ => None,
        })
        .collect();
    out.has_meet_named = original_kinds.contains(&ConstraintKind::MeetNamed);
    out.has_meet_existing = original_kinds.contains(&ConstraintKind::MeetExisting);
    out.has_scale_reference = original_kinds.contains(&ConstraintKind::ScaleReference);

    // Per-block scale anchors. A design's arrangement has continuous degrees of
    // freedom that preserve every vertex incidence (the crown block translating
    // vertically along the girdle wall, likewise the pavilion, and the girdle's own
    // radial scale -- see meet_solver's module docs), so each block (crown /
    // pavilion / girdle, classified by facet-normal y-sign) needs one stated
    // dimension. Real schedules state some of these ("Set girdle thickness", "Set
    // stone size"); for each block with no stated anchor, bootstrap the block's
    // first tier with its own real mast -- standing in for exactly the dimensions a
    // printed GemCAD diagram states outright (C/W, P/W, girdle size). Bootstrapped
    // tiers are excluded from scoring below, same as stated scale references.
    let mut bootstrapped: Vec<bool> = vec![false; tiers.len()];
    {
        // Side per tier (crown = +1, pavilion = -1, girdle = 0), mirroring the
        // solver's normal-y classification (unsigned-zero angles inherit the
        // previous tier's side).
        let mut last_crown = true;
        let side: Vec<i8> = tiers
            .iter()
            .map(|t| {
                let crown = if t.angle_deg == 0.0 {
                    if t.angle_deg.is_sign_negative() {
                        false
                    } else {
                        last_crown
                    }
                } else {
                    t.angle_deg > 0.0
                };
                last_crown = crown;
                let y = if crown {
                    t.angle_deg.abs().to_radians().cos()
                } else {
                    -t.angle_deg.abs().to_radians().cos()
                };
                if y.abs() <= 1e-6 {
                    0
                } else if y > 0.0 {
                    1
                } else {
                    -1
                }
            })
            .collect();
        for block in [1i8, -1, 0] {
            let members: Vec<usize> = (0..tiers.len()).filter(|&i| side[i] == block).collect();
            let has_anchor = members
                .iter()
                .any(|&i| matches!(tiers[i].constraint, MeetConstraint::ScaleReference(_)));
            if let (false, Some(&first)) = (has_anchor, members.first()) {
                tiers[first].constraint =
                    MeetConstraint::ScaleReference(schedule.tiers[first].mast);
                bootstrapped[first] = true;
                out.no_scale_reference = true; // >=1 block needed a bootstrap
            }
        }
    }

    // Computed against `tiers` in exactly the state `solve_meet_points` itself will
    // receive them (i.e. *after* the scale-reference bootstrap above), since that's
    // the same `name_to_tier`/`girdle_tier` state the solver's own internal
    // resolution will use -- see `classify_named_resolution`'s doc comment.
    let named_resolution = classify_named_resolution(&tiers, &original_named_names);

    let (solved, verify_report) = solve(schedule.gear_teeth_abs(), &tiers);
    if solved.len() != schedule.tiers.len() {
        return out;
    }
    out.verify = verify_report;

    score_solved_tiers(
        &schedule,
        &solved,
        &original_kinds,
        &named_resolution,
        |i| original_kinds[i] == ConstraintKind::ScaleReference || bootstrapped[i],
        &mut out,
    );
    out
}

/// Scores every meet-derived tier of one solved design against the schedule's own
/// real recorded masts, filling `out.tier_results` and `out.worst_err`. Tiers for
/// which `is_given` returns true (stated scale references, bootstrap or ratio
/// anchors) are excluded -- an anchor is *given*, not *solved*, so counting it
/// would flatter the numbers -- as are near-zero-mast tiers (relative error is
/// undefined there).
fn score_solved_tiers(
    schedule: &asc::AscSchedule,
    solved: &[indicatrix::geometry::meet_solver::SolvedTier],
    original_kinds: &[ConstraintKind],
    named_resolution: &[Option<bool>],
    is_given: impl Fn(usize) -> bool,
    out: &mut DesignResult,
) {
    let mut worst: Option<f64> = None;
    for (i, sol) in solved.iter().enumerate() {
        let real = schedule.tiers[i].mast.abs();
        if is_given(i) || real < 1e-6 {
            continue;
        }
        let rel_err = (sol.mast - real).abs() / real;
        out.tier_results.push(TierResult {
            strategy: sol.strategy,
            rel_err,
            kind: original_kinds[i],
            named_resolved: named_resolution[i],
            used_named: sol.detail.contains("named reference"),
            fallback_cause: if sol.detail.contains("refs not yet settled") {
                'u'
            } else if sol.detail.contains("no feasible level incident") {
                'n'
            } else {
                ' '
            },
        });
        worst = Some(worst.map_or(rel_err, |w: f64| w.max(rel_err)));
    }
    out.worst_err = worst;
}

/// The production ratio-anchoring path: like [`solve_one`], but anchors each block
/// from the design's own printed `C/W`/`P/W` proportions
/// ([`apply_ratio_anchors`]) instead of bootstrapping from the file's real tier-0
/// mast. This produces the measurement that matters for "is the solver usable in
/// production": `solve_one`'s tier-0-real-mast bootstrap is a harness-only crutch
/// (real usage, especially the ~2,700 catalogued designs with no `.asc` file at all,
/// has no recorded masts to bootstrap from), so a bootstrap-free measurement is the
/// number that actually reflects production usability.
///
/// A block whose ratio is `None` (a partial diagram) or that `apply_ratio_anchors`
/// otherwise couldn't cover still falls back to its own tier-0 real mast, exactly
/// like `solve_one`, so every design remains solvable and comparable -- but that
/// fallback is recorded (`DesignResult::fully_ratio_anchored`) and every anchor
/// tier, whichever path supplied it, is excluded from scoring below: an anchor is
/// *given*, not *solved*, so counting it as a free zero-error tier would flatter the
/// numbers.
fn solve_one_ratio_anchored(row: &AscRow) -> DesignResult {
    solve_one_ratio_anchored_impl(row, false)
}

/// Report D: the production path end-to-end -- printed-ratio anchors like
/// [`solve_one_ratio_anchored`], but solved via [`solve_meet_points_verified`]
/// with the ratio-derived crown/pavilion anchors marked *adjustable* (the
/// search calibrates them against the printed figures) and the printed
/// proportions as the repair/verification targets. Anchoring and scoring
/// conventions are otherwise identical to Report B, so the two are directly
/// comparable.
fn solve_one_ratio_anchored_verified(row: &AscRow) -> DesignResult {
    solve_one_ratio_anchored_impl(row, true)
}

fn solve_one_ratio_anchored_impl(row: &AscRow, verified: bool) -> DesignResult {
    let mut out = DesignResult::default();

    let text = String::from_utf8_lossy(&row.content);
    let Ok(schedule) = asc::parse_asc(&text) else {
        return out; // parse_ok stays false
    };
    out.parse_ok = true;
    if schedule.tiers.is_empty() {
        return out;
    }

    let mut tiers = meet_tier_inputs_from_asc(&schedule);
    let original_kinds: Vec<ConstraintKind> = tiers
        .iter()
        .map(|t| match &t.constraint {
            MeetConstraint::ScaleReference(_) => ConstraintKind::ScaleReference,
            MeetConstraint::MeetNamed(_) => ConstraintKind::MeetNamed,
            MeetConstraint::MeetExisting => ConstraintKind::MeetExisting,
        })
        .collect();
    let original_named_names: Vec<Option<Vec<String>>> = tiers
        .iter()
        .map(|t| match &t.constraint {
            MeetConstraint::MeetNamed(names) => Some(names.clone()),
            _ => None,
        })
        .collect();
    out.has_meet_named = original_kinds.contains(&ConstraintKind::MeetNamed);
    out.has_meet_existing = original_kinds.contains(&ConstraintKind::MeetExisting);
    out.has_scale_reference = original_kinds.contains(&ConstraintKind::ScaleReference);

    apply_ratio_anchors(&mut tiers, row.cw_ratio, row.pw_ratio);

    // Anything the ratio path newly turned into a ScaleReference (i.e. wasn't
    // already one before the call) is a ratio-derived anchor -- given, not solved.
    let ratio_anchored: Vec<bool> = (0..tiers.len())
        .map(|i| {
            original_kinds[i] != ConstraintKind::ScaleReference
                && matches!(tiers[i].constraint, MeetConstraint::ScaleReference(_))
        })
        .collect();

    // Any block that still has no anchor at all (its ratio was `None`, or it had no
    // tiers for `apply_ratio_anchors` to pick from) falls back to its own tier-0
    // real mast, same convention as `solve_one`, so the design stays solvable.
    let mut fallback_anchored: Vec<bool> = vec![false; tiers.len()];
    let mut fully_ratio_anchored = true;
    {
        use indicatrix::geometry::meet_solver::classify_blocks;
        let blocks = classify_blocks(&tiers);
        for block in [
            indicatrix::geometry::meet_solver::Block::Crown,
            indicatrix::geometry::meet_solver::Block::Pavilion,
            indicatrix::geometry::meet_solver::Block::Girdle,
        ] {
            let members: Vec<usize> = (0..tiers.len()).filter(|&i| blocks[i] == block).collect();
            let has_anchor = members
                .iter()
                .any(|&i| matches!(tiers[i].constraint, MeetConstraint::ScaleReference(_)));
            if let (false, Some(&first)) = (has_anchor, members.first()) {
                tiers[first].constraint =
                    MeetConstraint::ScaleReference(schedule.tiers[first].mast);
                fallback_anchored[first] = true;
                fully_ratio_anchored = false;
            }
        }
    }
    out.fully_ratio_anchored = fully_ratio_anchored;

    let named_resolution = classify_named_resolution(&tiers, &original_named_names);

    let solved = if verified {
        let targets = ExternalProportions {
            vol_w3: row.volume,
            lw: row.lw_ratio,
            cw: row.cw_ratio,
            pw: row.pw_ratio,
            hw: row.hw_ratio,
        };
        // Ratio-derived crown/pavilion anchors are estimates the search may
        // calibrate; the girdle reference is pure unit choice (every target is
        // scale-invariant), and real-mast fallback anchors are exact.
        let blocks = indicatrix::geometry::meet_solver::classify_blocks(&tiers);
        let adjustable: Vec<usize> = (0..tiers.len())
            .filter(|&i| {
                ratio_anchored[i] && blocks[i] != indicatrix::geometry::meet_solver::Block::Girdle
            })
            .collect();
        let (solved, report) =
            solve_meet_points_verified(schedule.gear_teeth_abs(), &tiers, &targets, &adjustable);
        out.verify = Some(report);
        solved
    } else {
        solve_meet_points(schedule.gear_teeth_abs(), &tiers)
    };
    if solved.len() != schedule.tiers.len() {
        return out;
    }

    score_solved_tiers(
        &schedule,
        &solved,
        &original_kinds,
        &named_resolution,
        |i| {
            original_kinds[i] == ConstraintKind::ScaleReference
                || ratio_anchored[i]
                || fallback_anchored[i]
        },
        &mut out,
    );
    out
}

/// Classifies each tier's name resolution via the *same* [`MeetNameResolver`] the
/// solver itself uses at solve time (no mirrored logic to drift out of sync).
/// `tiers` must be in the same state (post scale-reference bootstrap) passed to
/// `solve_meet_points`; `original_names` supplies each tier's stated name list
/// from *before* that bootstrap could have overwritten a `MeetNamed` tier 0's
/// constraint.
///
/// Returns `None` for any tier that wasn't originally `MeetNamed`; `Some(true)`
/// iff the stated list fully resolved (every token resolved to a tier or was a
/// recognized non-facet word, and at least one token actually named a tier --
/// see [`indicatrix::geometry::meet_solver::ResolvedNames::fully`]).
fn classify_named_resolution(
    tiers: &[indicatrix::geometry::meet_solver::MeetTierInput],
    original_names: &[Option<Vec<String>>],
) -> Vec<Option<bool>> {
    let resolver = indicatrix::geometry::meet_solver::MeetNameResolver::new(tiers);
    original_names
        .iter()
        .map(|names| {
            names
                .as_ref()
                .map(|names| resolver.resolve_names(names).fully)
        })
        .collect()
}

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
fn run_and_report(
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

/// Reports C/D extra accounting: acceptance rate, search cost, score medians
/// (initial / after anchor calibration / final), and the quality split between
/// accepted and unaccepted designs (the verifier's precision claim, checked
/// end-to-end).
fn print_verified_extras(results: &[DesignResult]) {
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
fn count_ratio_coverage_for_designs_without_asc(conn: &Connection) -> (i64, i64, i64, i64) {
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

#[expect(
    clippy::too_many_lines,
    reason = "straight-line report printing in a temporary probe; splitting it up would \
              scatter the report's structure across helper functions for no clarity gain"
)]
fn print_report(
    parse_ok: usize,
    parse_err: usize,
    no_scale_reference_at_all: usize,
    tier_results: &[TierResult],
    design_worst_err: &[Option<f64>],
    (designs_with_meet_named, designs_with_meet_existing, designs_with_scale_reference): (
        usize,
        usize,
        usize,
    ),
) {
    println!("\n=== Corpus coverage ===");
    println!("  designs parsed OK:  {parse_ok}");
    println!("  designs parse error: {parse_err}");
    println!(
        "  designs needing >=1 block's real-mast fallback (see this report's header): {no_scale_reference_at_all}"
    );
    println!("  designs with >=1 MeetNamed tier:      {designs_with_meet_named}");
    println!("  designs with >=1 MeetExisting tier:   {designs_with_meet_existing}");
    println!("  designs with >=1 ScaleReference tier: {designs_with_scale_reference}");

    println!(
        "\n=== Strategy usage ({} meet-derived tiers, scale-reference tiers excluded) ===",
        tier_results.len()
    );
    for strategy in [
        SolveStrategy::DependencyOrder,
        SolveStrategy::JointGroup,
        SolveStrategy::LeastSquaresFallback,
        SolveStrategy::Failed,
    ] {
        let n = tier_results
            .iter()
            .filter(|t| t.strategy == strategy)
            .count();
        println!("  {strategy:?}: {n} ({:.1}%)", pct(n, tier_results.len()));
    }
    let exact = tier_results
        .iter()
        .filter(|t| {
            matches!(
                t.strategy,
                SolveStrategy::DependencyOrder | SolveStrategy::JointGroup
            )
        })
        .count();
    let fallback = tier_results
        .iter()
        .filter(|t| {
            matches!(
                t.strategy,
                SolveStrategy::LeastSquaresFallback | SolveStrategy::Failed
            )
        })
        .count();
    println!(
        "  -- exact (DependencyOrder+JointGroup): {exact} ({:.1}%)",
        pct(exact, tier_results.len())
    );
    println!(
        "  -- fallback (LeastSquares+Failed):     {fallback} ({:.1}%)",
        pct(fallback, tier_results.len())
    );

    println!("\n=== Overall (all meet-derived tiers, blended) ===");
    let mut all_errs: Vec<f64> = tier_results.iter().map(|t| t.rel_err).collect();
    all_errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "  exact-match rate (DependencyOrder+JointGroup): {:.1}%",
        pct(exact, tier_results.len())
    );
    println!("  median relative error: {:.4}", median(&all_errs));

    println!("\n=== Split by ConstraintKind (do not blend) ===");
    for kind in [
        ConstraintKind::ScaleReference,
        ConstraintKind::MeetNamed,
        ConstraintKind::MeetExisting,
    ] {
        let subset: Vec<&TierResult> = tier_results.iter().filter(|t| t.kind == kind).collect();
        if subset.is_empty() {
            println!("  {kind:?}: 0 tiers (excluded from scoring, or none present)");
            continue;
        }
        let exact = subset
            .iter()
            .filter(|t| {
                matches!(
                    t.strategy,
                    SolveStrategy::DependencyOrder | SolveStrategy::JointGroup
                )
            })
            .count();
        let joint = subset
            .iter()
            .filter(|t| t.strategy == SolveStrategy::JointGroup)
            .count();
        println!(
            "  {kind:?}: {} tiers, {:.1}% exact ({:.1}% via JointGroup specifically), median rel. err {:.4}",
            subset.len(),
            pct(exact, subset.len()),
            pct(joint, subset.len()),
            median(&subset.iter().map(|t| t.rel_err).collect::<Vec<_>>())
        );
    }

    println!("\n=== MeetNamed split by name resolution (the real gate is on -resolved) ===");
    println!(
        "  (resolution measured against the solver's own name_to_tier/girdle_tier logic \
         -- see classify_named_resolution -- not a post-hoc filter)"
    );
    for (label, want_resolved) in [
        ("MeetNamed-resolved", true),
        ("MeetNamed-unresolved", false),
    ] {
        let subset: Vec<&TierResult> = tier_results
            .iter()
            .filter(|t| {
                t.kind == ConstraintKind::MeetNamed && t.named_resolved == Some(want_resolved)
            })
            .collect();
        if subset.is_empty() {
            println!("  {label}: 0 tiers");
            continue;
        }
        let exact = subset
            .iter()
            .filter(|t| {
                matches!(
                    t.strategy,
                    SolveStrategy::DependencyOrder | SolveStrategy::JointGroup
                )
            })
            .count();
        let used: Vec<&&TierResult> = subset.iter().filter(|t| t.used_named).collect();
        let unused: Vec<&&TierResult> = subset.iter().filter(|t| !t.used_named).collect();
        println!(
            "  {label}: {} tiers, {:.1}% exact (DependencyOrder+JointGroup), median rel. err {:.4}",
            subset.len(),
            pct(exact, subset.len()),
            median(&subset.iter().map(|t| t.rel_err).collect::<Vec<_>>())
        );
        println!(
            "    of which constructively used named refs: {} (median rel. err {:.4}); \
             fell back to rank-1: {} (median rel. err {:.4})",
            used.len(),
            median(&used.iter().map(|t| t.rel_err).collect::<Vec<_>>()),
            unused.len(),
            median(&unused.iter().map(|t| t.rel_err).collect::<Vec<_>>())
        );
        for (cause, tag) in [
            ("refs not settled at release", 'u'),
            ("no incident feasible level", 'n'),
        ] {
            let sub: Vec<f64> = unused
                .iter()
                .filter(|t| t.fallback_cause == tag)
                .map(|t| t.rel_err)
                .collect();
            println!(
                "      fallback cause `{cause}`: {} tiers (median rel. err {:.4})",
                sub.len(),
                median(&sub)
            );
        }
    }

    println!("\n=== Per-tier relative error distribution (meet-derived tiers only) ===");
    let mut errs: Vec<f64> = tier_results.iter().map(|t| t.rel_err).collect();
    errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    print_percentiles(&errs);

    println!(
        "\n=== Per-tier relative error, exact strategies only (DependencyOrder+JointGroup) ==="
    );
    let mut exact_errs: Vec<f64> = tier_results
        .iter()
        .filter(|t| {
            matches!(
                t.strategy,
                SolveStrategy::DependencyOrder | SolveStrategy::JointGroup
            )
        })
        .map(|t| t.rel_err)
        .collect();
    exact_errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    print_percentiles(&exact_errs);

    println!("\n=== Per-design success (worst meet-derived tier's relative error) ===");
    let scored: Vec<f64> = design_worst_err.iter().filter_map(|w| *w).collect();
    let no_meet_tiers = design_worst_err.iter().filter(|w| w.is_none()).count();
    println!("  designs with >=1 meet-derived tier: {}", scored.len());
    println!("  designs with zero meet-derived tiers (all scale-reference): {no_meet_tiers}");
    for threshold in [0.01, 0.10] {
        let within = scored.iter().filter(|&&e| e <= threshold).count();
        println!(
            "  every meet-derived tier within {:.0}%: {within} ({:.1}%)",
            threshold * 100.0,
            pct(within, scored.len())
        );
    }
    let mut sorted_scored = scored;
    sorted_scored.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("  worst-tier-per-design distribution:");
    print_percentiles(&sorted_scored);
}

fn print_percentiles(sorted: &[f64]) {
    if sorted.is_empty() {
        println!("  (no data)");
        return;
    }
    let n = sorted.len();
    let at = |p: f64| sorted[((n as f64 * p) as usize).min(n - 1)];
    println!(
        "  p10={:.4} p25={:.4} median={:.4} p75={:.4} p90={:.4} p99={:.4} max={:.4}",
        at(0.10),
        at(0.25),
        at(0.50),
        at(0.75),
        at(0.90),
        at(0.99),
        sorted[n - 1]
    );
}

fn median(vals: &[f64]) -> f64 {
    let mut v = vals.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() { 0.0 } else { v[v.len() / 2] }
}

fn pct(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * n as f64 / total as f64
    }
}

fn load_asc_rows(conn: &Connection) -> Vec<AscRow> {
    let mut stmt = conn
        .prepare(
            "SELECT af.detail_id, af.content, dd.cw_ratio, dd.pw_ratio, \
                    dd.volume, dd.lw_ratio, dd.hw_ratio \
             FROM attached_files af \
             LEFT JOIN diagram_details dd ON af.detail_id = dd.id \
             WHERE af.name LIKE '%.asc' \
             ORDER BY af.detail_id, af.id",
        )
        .expect("prepare attached_files query");
    let rows = stmt
        .query_map([], |row| {
            Ok(AscRow {
                detail_id: row.get(0)?,
                content: row.get(1)?,
                cw_ratio: row.get(2)?,
                pw_ratio: row.get(3)?,
                volume: row.get(4)?,
                lw_ratio: row.get(5)?,
                hw_ratio: row.get(6)?,
            })
        })
        .expect("query attached_files");
    rows.filter_map(Result::ok).collect()
}

fn find_db_path() -> String {
    for candidate in [
        "facet_diagrams.sqlite",
        "../../facet_diagrams.sqlite",
        "../facet_diagrams.sqlite",
    ] {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "facet_diagrams.sqlite".to_string()
}
