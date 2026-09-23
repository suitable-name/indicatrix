//! [`solve_meet_points_verified`]: [`super::solve_meet_points`] plus an
//! externally-verified greedy repair search, and the report type describing
//! what it did.
//!
//! See the crate module docs for why external (printed-proportion) scoring
//! can discriminate a correct solve when nothing internal can.
//! [`VerifiedSearch`] repeatedly re-runs
//! [`super::solve::SolveContext::run_pipeline`] with different phase-1
//! vertex-level overrides (and, when the caller allows it, adjusted anchor
//! values), scored by [`ExternalProportions::combined_deviation`], and keeps
//! the best-scoring configuration found.

use super::{
    MAX_PLANES, MeetConstraint, MeetTierInput, SolveControl, SolveError, SolvedTier,
    solve::{PipelineResult, SolveContext},
};
use crate::geometry::stone_metrics::ExternalProportions;
use std::collections::BTreeMap;

/// Combined-deviation score at or below which a solve counts as externally
/// verified.
///
/// Measured on the full 2,881-design corpus (temporary calibration probe,
/// deterministic run): applied to the *plain* solver's output, the accept test
/// at 0.01 had **100% precision** (every accepted design's worst meet-derived
/// tier was within 10% of truth, n=185) and 59.5% recall of the
/// correctly-solved designs; the printed figures themselves reproduce the true
/// solid's measurements to a ~0.1% median, so this threshold sits an order of
/// magnitude above the data's noise floor and an order of magnitude below a
/// wrong solve's typical deviation (median 0.32). Applied end-to-end *after*
/// [`solve_meet_points_verified`]'s repair search -- which can occasionally
/// construct a compensating-error configuration that matches the printed
/// figures, a multiple-comparisons effect the plain measurement doesn't have --
/// precision is 90.4% by the strict every-tier-within-10% test, with the
/// accepted designs' pooled per-tier median error at 0.0001
/// (`examples/meet_solver_validation.rs` Report C, full corpus).
pub const VERIFY_ACCEPT_TOL: f64 = 0.01;

/// Repair-search budget: greedy rounds (each committing at most one decision
/// override) and the alternative vertex levels tried per decision. Level 1 is
/// the default pick, so the alternatives cover levels 0, 2 and 3 -- the
/// oracle-measured true level is within the first three levels for 94.4% of
/// tiers, and within these four for slightly more.
const VERIFY_MAX_ROUNDS: usize = 4;
const VERIFY_ALT_LEVELS: [usize; 3] = [0, 2, 3];

/// Minimum combined-score improvement a round's best override must deliver to
/// be committed. Guards against chasing sub-noise fluctuations.
const VERIFY_MIN_GAIN: f64 = 0.002;

/// Pipeline-run budget scale for one design's repair search. A single pipeline
/// run costs roughly `tiers * planes^3` (the constructive pass re-enumerates
/// candidate vertices per settled tier, and tier count grows with plane count),
/// i.e. ~`planes^4` -- so a flat run cap would let the corpus's heavy tail
/// (~190 designs above 100 planes) each burn many core-minutes. Instead a
/// design's budget is `VERIFY_RUN_BUDGET / total_planes^4`, clamped to
/// `[1, VERIFY_MAX_RUNS]` (`VERIFY_MAX_RUNS_CALIBRATED` when anchors are
/// adjustable -- the anchor moves need extra room): a typical 60-plane design
/// gets the full budget, a 150-plane one ~20 runs, a 300-plane one essentially
/// the plain solve only.
const VERIFY_RUN_BUDGET: f64 = 1.0e10;
const VERIFY_MAX_RUNS: usize = 120;
const VERIFY_MAX_RUNS_CALIBRATED: usize = 170;

/// Anchor-calibration move sets, both multiplicative on the anchor's current
/// value. The coarse grid runs once per adjustable anchor before the greedy
/// loop (an estimated anchor's error is up to ~25-40%, so the grid spans that
/// range); the fine steps then compete with the level overrides inside the
/// greedy loop itself, letting anchor refinement and cascade repair interleave
/// -- each committed anchor move re-solves every meet-derived tier against the
/// moved anchor.
const ANCHOR_GRID: [f64; 4] = [0.7, 0.85, 1.2, 1.4];
const ANCHOR_FINE_STEPS: [f64; 4] = [0.93, 0.965, 1.035, 1.075];

/// Greedy rounds when anchors are adjustable: anchor convergence takes several
/// committed moves (7%-scale steps) on top of the level repairs, so the
/// calibrated path gets twice the rounds. The run budget above still caps
/// total cost.
const VERIFY_MAX_ROUNDS_CALIBRATED: usize = 8;

/// One candidate move of the verified repair search's greedy scan.
///
/// NOTE -- rejected: a joint multiplier over all adjustable anchors (vs. independent
/// per-anchor moves), on the hypothesis that the Report-D acceptance-precision collapse
/// (90.4% fixed anchors -> 24.2% adjustable) came from wandering along the
/// observability-blind crown-vs-pavilion translation direction, which a joint multiplier
/// cannot enter. Measured, it recovered nothing (24.0% vs. 24.2% precision, 199 vs. 214
/// accepted) -- with *any* continuous anchor freedom, wrong configurations reproducing
/// all five printed figures stay reachable; external certification requires trusted
/// (real-mast) anchors, and in calibrated mode the accept flag is a quality signal, not
/// a correctness certificate. Independent per-anchor moves, marginally better, ship.
enum RepairMove {
    /// Force this tier's phase-1 pick to this vertex-level index.
    Level(usize, usize),
    /// Set this adjustable anchor tier's mast to this value.
    Anchor(usize, f64),
}

/// What [`solve_meet_points_verified`]'s repair search did, for reporting.
#[derive(Debug, Clone, Copy)]
pub struct VerifiedSolveReport {
    /// Combined external deviation of the plain (unrepaired) solve.
    /// `INFINITY` when it could not be measured.
    pub initial_score: f64,
    /// Combined external deviation after the coarse anchor-calibration grid
    /// (equal to `initial_score` when no anchors were adjustable or the grid
    /// found nothing better).
    pub score_after_calibration: f64,
    /// Combined external deviation of the returned configuration.
    pub final_score: f64,
    /// `final_score <= VERIFY_ACCEPT_TOL`: the returned configuration
    /// reproduces the printed proportions to verification accuracy.
    pub accepted: bool,
    /// Vertex-level decision overrides the search committed.
    pub overrides_applied: usize,
    /// Anchor-value moves the search committed (coarse grid and fine steps).
    pub anchor_moves_applied: usize,
    /// Total pipeline runs spent (1 = the plain solve was already accepted or
    /// the search found nothing to try).
    pub pipeline_runs: usize,
}

/// [`solve_meet_points`](super::solve_meet_points) plus an externally-verified
/// repair search.
///
/// The corpus problem this addresses (measured; see the module docs): wrong
/// solves are *self-consistent* -- every tier still lands on a real meet
/// vertex -- so no internal score can tell them from the truth. The
/// proportions printed on a design (`targets`: `Vol/W^3`, `L/W`, `C/W`,
/// `P/W`, `H/W`) are external, and the true configuration reproduces them to
/// ~0.1% (median) while a wrong solve is off by ~30%: a discrimination
/// measured at 100% precision at the [`VERIFY_ACCEPT_TOL`] threshold (see its
/// doc comment, including the post-search precision caveat).
///
/// The search is greedy in decision space: run the plain pipeline; while the
/// combined deviation stays above [`VERIFY_ACCEPT_TOL`], try overriding each
/// meet-derived tier's phase-1 vertex-level pick with the nearby alternatives
/// ([`VERIFY_ALT_LEVELS`]), re-running the pipeline (so the change cascades
/// through everything built on that tier), and commit the single best-scoring
/// move per round. Deterministic: fixed iteration order, `BTreeMap` state, and
/// a deterministic scorer ([`measure_solid`](crate::geometry::stone_metrics::measure_solid)).
///
/// `adjustable_anchors` lists tiers whose [`MeetConstraint::ScaleReference`]
/// values are *estimates* the search may adjust (Report B's printed-ratio
/// anchors, which carry 10-25% isolated error -- never pass anchors holding
/// real recorded masts). For those, a coarse per-anchor calibration grid
/// ([`ANCHOR_GRID`]) runs first, and fine anchor moves
/// ([`ANCHOR_FINE_STEPS`]) then compete with the level overrides inside the
/// greedy loop, all scored by the same printed figures. Pass `&[]` for the
/// plain repair search.
///
/// Measured end-to-end on the full 2,881-design corpus
/// (`examples/meet_solver_validation.rs` Report C, Report-A anchoring,
/// `adjustable_anchors` empty, deterministic run): overall median relative
/// error 0.2110 -> **0.1278**, designs with every meet-derived tier within 10%
/// of truth 10.8% -> **23.7%**, 409 designs (14.2%) verified-accepted with a
/// pooled per-tier median error of 0.0001, at a mean cost of 68.4 pipeline
/// runs per design (~29 minutes for the corpus at 16 threads). Unaccepted
/// designs still improve on average (pooled per-tier median 0.1485 vs. the
/// plain solve's 0.2110 blended), since the search keeps any committed move
/// that brought the measured solid closer to the printed figures.
///
/// With adjustable anchors (Report D in the same probe: the production path,
/// printed-ratio anchors, full corpus, deterministic run): overall median
/// relative error 0.2858 -> **0.2420**, within-10% designs 5.1% -> 5.7%, at a
/// mean cost of 97.3 pipeline runs per design. **The accept flag is weaker in
/// this mode**: 214 designs accepted, but only 24.2% of them pass the strict
/// every-tier-within-10% test (vs. 90.4% with fixed anchors) -- with
/// continuous anchor freedom, wrong configurations that reproduce all five
/// printed figures are reachable (see the NOTE on [`RepairMove`] for the
/// second variant that was measured trying to close this). Treat calibrated
/// acceptance as a quality *signal* (accepted designs' pooled tier median
/// 0.1142 vs. unaccepted 0.2497), never as the correctness certificate the
/// fixed-anchor mode provides.
///
/// Latency: each repair run costs one full pipeline solve, `O(P^3)` per
/// refinement sweep in the plane count `P`; see the module docs' "Cost
/// envelope" section and `VERIFY_RUN_BUDGET` for the measured figures. Never
/// cancels and reports no progress -- a thin wrapper over
/// [`solve_meet_points_verified_with`] passing a no-op [`SolveControl`], and
/// above [`MAX_PLANES`] it keeps the legacy behavior of returning an all-
/// [`super::SolveStrategy::Failed`] result instead of an error.
#[must_use]
pub fn solve_meet_points_verified(
    gear_teeth_abs: u32,
    tiers: &[MeetTierInput],
    targets: &ExternalProportions,
    adjustable_anchors: &[usize],
) -> (Vec<SolvedTier>, VerifiedSolveReport) {
    match solve_meet_points_verified_with(
        gear_teeth_abs,
        tiers,
        targets,
        adjustable_anchors,
        &SolveControl::default(),
    ) {
        Ok(result) => result,
        Err(SolveError::TooManyPlanes { .. }) => {
            let ctx = SolveContext::new(gear_teeth_abs, tiers);
            let report = VerifiedSolveReport {
                initial_score: f64::INFINITY,
                score_after_calibration: f64::INFINITY,
                final_score: f64::INFINITY,
                accepted: false,
                overrides_applied: 0,
                anchor_moves_applied: 0,
                pipeline_runs: 0,
            };
            (ctx.failed_solved(), report)
        }
        Err(SolveError::Cancelled) => {
            unreachable!("SolveControl::default() never sets cancel")
        }
    }
}

/// Cancellable, progress-reporting sibling of [`solve_meet_points_verified`].
///
/// Same search and same determinism guarantee: an unused `control` reproduces
/// [`solve_meet_points_verified`] exactly. Every pipeline run the repair
/// search performs -- the coarse anchor grid, and every greedy-round trial --
/// is itself a natural cancel point and progress source: each goes through
/// [`super::solve::SolveContext::run_pipeline`] with `control` threaded
/// through it, so a cancel or [`super::SolveProgress`] report from deep inside
/// one repair-search run surfaces exactly the way a plain
/// [`super::solve_meet_points_with`] solve's does. No separate
/// verified-search-level progress unit.
///
/// # Errors
///
/// [`SolveError::TooManyPlanes`] when the design has more than [`MAX_PLANES`]
/// facet-plane instances (checked before any solving starts);
/// [`SolveError::Cancelled`] the first time `control`'s cancel flag is
/// observed set, from inside whichever pipeline run was in flight.
pub fn solve_meet_points_verified_with(
    gear_teeth_abs: u32,
    tiers: &[MeetTierInput],
    targets: &ExternalProportions,
    adjustable_anchors: &[usize],
    control: &SolveControl<'_>,
) -> Result<(Vec<SolvedTier>, VerifiedSolveReport), SolveError> {
    let ctx = SolveContext::new(gear_teeth_abs, tiers);
    if ctx.total_planes > MAX_PLANES {
        return Err(SolveError::TooManyPlanes {
            planes: ctx.total_planes,
            max: MAX_PLANES,
        });
    }

    let adjustable: Vec<usize> = {
        let mut v: Vec<usize> = adjustable_anchors
            .iter()
            .copied()
            .filter(|&i| i < tiers.len() && ctx.is_anchor[i])
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };

    let planes = ctx.total_planes as f64;
    let run_cap = if adjustable.is_empty() {
        VERIFY_MAX_RUNS
    } else {
        VERIFY_MAX_RUNS_CALIBRATED
    };
    let max_runs =
        ((VERIFY_RUN_BUDGET / (planes * planes * planes * planes)) as usize).clamp(1, run_cap);
    let max_rounds = if adjustable.is_empty() {
        VERIFY_MAX_ROUNDS
    } else {
        VERIFY_MAX_ROUNDS_CALIBRATED
    };

    let anchor_values: BTreeMap<usize, f64> = adjustable
        .iter()
        .map(|&i| {
            let v = match &tiers[i].constraint {
                MeetConstraint::ScaleReference(v) => v.abs(),
                _ => 0.0,
            };
            (i, v)
        })
        .collect();

    let level_overrides: BTreeMap<usize, usize> = BTreeMap::new();
    let best = ctx.run_pipeline(&level_overrides, &anchor_values, control)?;
    let best_score = ctx.config_score(&best.mast, targets);
    let initial_score = best_score;

    let mut search = VerifiedSearch {
        ctx: &ctx,
        targets,
        adjustable: &adjustable,
        max_runs,
        level_overrides,
        anchor_values,
        best,
        best_score,
        runs: 1,
        anchor_moves: 0,
    };
    search.coarse_anchor_grid(control)?;
    let score_after_calibration = search.best_score;
    for _ in 0..max_rounds {
        if !search.greedy_round(control)? {
            break;
        }
    }

    let report = VerifiedSolveReport {
        initial_score,
        score_after_calibration,
        final_score: search.best_score,
        accepted: search.best_score <= VERIFY_ACCEPT_TOL,
        overrides_applied: search.level_overrides.len(),
        anchor_moves_applied: search.anchor_moves,
        pipeline_runs: search.runs,
    };
    Ok((ctx.to_solved(&search.best), report))
}

/// Mutable state of one verified repair search, shared by its phases (the
/// coarse anchor grid and the greedy rounds).
struct VerifiedSearch<'a, 'c> {
    ctx: &'c SolveContext<'a>,
    targets: &'c ExternalProportions,
    adjustable: &'c [usize],
    max_runs: usize,
    level_overrides: BTreeMap<usize, usize>,
    anchor_values: BTreeMap<usize, f64>,
    best: PipelineResult,
    best_score: f64,
    runs: usize,
    anchor_moves: usize,
}

impl VerifiedSearch<'_, '_> {
    /// True once no further trial should run: the configuration is accepted,
    /// unmeasurable (no printed-figure overlap -- nothing to optimize), or the
    /// run budget is spent.
    fn done(&self) -> bool {
        !self.best_score.is_finite()
            || self.best_score <= VERIFY_ACCEPT_TOL
            || self.runs >= self.max_runs
    }

    /// Coarse anchor-calibration grid: per adjustable anchor (ascending), try
    /// [`ANCHOR_GRID`] multiples of its current value against the fixed level
    /// state, committing every improvement as it is found.
    ///
    /// # Errors
    ///
    /// [`SolveError::Cancelled`] the first time `control`'s cancel flag is
    /// observed set inside a trial pipeline run.
    fn coarse_anchor_grid(&mut self, control: &SolveControl<'_>) -> Result<(), SolveError> {
        for ai in self.adjustable {
            let base = self.anchor_values[ai];
            if base <= 1e-9 {
                continue;
            }
            for &mult in &ANCHOR_GRID {
                if self.done() {
                    return Ok(());
                }
                let mut trial_anchors = self.anchor_values.clone();
                trial_anchors.insert(*ai, base * mult);
                let trial =
                    self.ctx
                        .run_pipeline(&self.level_overrides, &trial_anchors, control)?;
                self.runs += 1;
                let score = self.ctx.config_score(&trial.mast, self.targets);
                if score < self.best_score {
                    self.anchor_values = trial_anchors;
                    self.best = trial;
                    self.best_score = score;
                    self.anchor_moves += 1;
                }
            }
        }
        Ok(())
    }

    /// Every candidate move of one greedy round, in deterministic order: level
    /// overrides for each not-yet-overridden meet-derived tier, then fine
    /// multiplicative steps for each adjustable anchor.
    fn candidate_moves(&self) -> Vec<RepairMove> {
        let mut moves: Vec<RepairMove> = Vec::new();
        for i in 0..self.ctx.tiers.len() {
            if self.ctx.is_anchor[i] || self.level_overrides.contains_key(&i) {
                continue;
            }
            let Some((current_li, n_levels)) = self.best.last_pick[i] else {
                continue;
            };
            for &alt in &VERIFY_ALT_LEVELS {
                if alt != current_li && alt < n_levels {
                    moves.push(RepairMove::Level(i, alt));
                }
            }
        }
        for &ai in self.adjustable {
            let base = self.anchor_values[&ai];
            for &step in &ANCHOR_FINE_STEPS {
                moves.push(RepairMove::Anchor(ai, base * step));
            }
        }
        moves
    }

    /// Applies one move to the committed search state.
    fn commit(&mut self, mv: &RepairMove) {
        match *mv {
            RepairMove::Level(i, alt) => {
                self.level_overrides.insert(i, alt);
            }
            RepairMove::Anchor(ai, val) => {
                self.anchor_values.insert(ai, val);
                self.anchor_moves += 1;
            }
        }
    }

    /// One greedy round: score every candidate move, commit the single best
    /// improvement (or exit early on an accepted trial). Returns whether the
    /// search should continue with another round.
    ///
    /// # Errors
    ///
    /// [`SolveError::Cancelled`] the first time `control`'s cancel flag is
    /// observed set inside a trial pipeline run.
    fn greedy_round(&mut self, control: &SolveControl<'_>) -> Result<bool, SolveError> {
        if self.done() {
            return Ok(false);
        }
        let moves = self.candidate_moves();
        let mut round_best: Option<(usize, PipelineResult, f64)> = None;
        for (mi, mv) in moves.iter().enumerate() {
            if self.runs >= self.max_runs {
                break;
            }
            let (trial_levels, trial_anchors) = match *mv {
                RepairMove::Level(i, alt) => {
                    let mut l = self.level_overrides.clone();
                    l.insert(i, alt);
                    (l, self.anchor_values.clone())
                }
                RepairMove::Anchor(ai, val) => {
                    let mut a = self.anchor_values.clone();
                    a.insert(ai, val);
                    (self.level_overrides.clone(), a)
                }
            };
            let trial = self
                .ctx
                .run_pipeline(&trial_levels, &trial_anchors, control)?;
            self.runs += 1;
            let score = self.ctx.config_score(&trial.mast, self.targets);
            if score <= VERIFY_ACCEPT_TOL {
                self.commit(mv);
                self.best = trial;
                self.best_score = score;
                return Ok(false);
            }
            if score < self.best_score && round_best.as_ref().is_none_or(|r| score < r.2) {
                round_best = Some((mi, trial, score));
            }
        }
        Ok(match round_best {
            Some((mi, result, score)) if score < self.best_score - VERIFY_MIN_GAIN => {
                self.commit(&moves[mi]);
                self.best = result;
                self.best_score = score;
                self.runs < self.max_runs
            }
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The calibrated search must recover a corrupted *estimated* anchor from
    /// printed figures alone: a hip-roofed stone whose crown-roof anchor starts
    /// 29% low gets pulled back to ~its true mast by matching the measured
    /// `H/W` against the printed one. (True roof mast `1/sqrt(2)`: apex at
    /// `y = 1`, floor at `-0.5`, width 2, so printed `H/W = 0.75`.)
    #[test]
    fn verified_search_calibrates_an_estimated_anchor() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let tiers = vec![
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(1.0),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: -0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.5),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                // Estimated anchor, deliberately 29% below the true mast `s`.
                constraint: MeetConstraint::ScaleReference(0.5),
                names: vec![],
            },
        ];
        let targets = ExternalProportions {
            hw: Some(0.75),
            ..Default::default()
        };
        let (solved, report) = solve_meet_points_verified(4, &tiers, &targets, &[2]);
        assert!(report.accepted, "report: {report:?}");
        assert!(report.anchor_moves_applied >= 1, "report: {report:?}");
        assert!(
            report.score_after_calibration < report.initial_score,
            "calibration must have improved the score: {report:?}"
        );
        // Fixed anchors must be untouched; the adjustable one must land within
        // the grid's resolution of the true mast.
        assert!((solved[0].mast - 1.0).abs() < 1e-12);
        assert!((solved[1].mast - 0.5).abs() < 1e-12);
        let rel = (solved[2].mast - s).abs() / s;
        assert!(
            rel < 0.02,
            "calibrated roof mast {} vs true {s} (rel {rel})",
            solved[2].mast
        );
    }

    /// An empty `adjustable_anchors` list must leave `solve_meet_points_verified`
    /// byte-identical to the plain solve when the targets already match (no
    /// spurious moves committed on an already-verified configuration).
    #[test]
    fn verified_search_accepts_a_matching_config_without_searching() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let tiers = vec![
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(1.0),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: -0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.5),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(s),
                names: vec![],
            },
        ];
        let targets = ExternalProportions {
            hw: Some(0.75),
            ..Default::default()
        };
        let (solved, report) = solve_meet_points_verified(4, &tiers, &targets, &[]);
        assert!(report.accepted);
        assert_eq!(report.pipeline_runs, 1);
        assert_eq!(report.overrides_applied, 0);
        assert_eq!(report.anchor_moves_applied, 0);
        let plain = super::super::solve_meet_points(4, &tiers);
        for (a, b) in solved.iter().zip(&plain) {
            assert_eq!(a.mast.to_bits(), b.mast.to_bits());
        }
    }
}
