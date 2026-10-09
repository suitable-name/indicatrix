//! The solve policy every editor front end shares.
//!
//! When a solve runs synchronously versus in the background, when an edit schedules a
//! debounced auto-solve, the "Solving..." and "auto-solve off" banner texts, the
//! cancellable solve with the solver's over-plane-cap fallback, the plane-cap diagnosis,
//! and the conversion of solved masts into viewport planes.
//!
//! Only the decisions live here -- no threads, timers or clock. The desktop runs
//! the solve on a worker thread and debounces with a `slint::Timer`; the web app
//! uses a Worker and `setTimeout`. Both ask these functions what to do.

use indicatrix::geometry::{
    GpuFacetPlane,
    meet_solver::{
        MeetConstraint, SolveControl, SolveError, SolveStrategy, SolvedTier, solve_meet_points,
    },
};
use indicatrix_cut_core::{Design, DesignSolveError};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Solve cost is driven by plane count, not tier count.
///
/// A wide-orbit round-brilliant tier emits many planes at once (the corpus's largest
/// measured design is 103 tiers but 210 planes), so a small tier count can still be an
/// expensive, UI-blocking solve.
///
/// A rough estimate, not a re-measurement of the corpus: roughly double a 16-tier
/// reference point at the corpus's own ~2 planes-per-tier ratio, kept comfortably
/// under the 210-plane/5.9 s worst case.
pub const SYNC_SOLVE_PLANE_LIMIT: usize = 32;

/// Once a design has a real measured solve time, that measurement is a far better signal
/// than any plane-count estimate.
///
/// A design that solved in under this long most recently is still fine to solve again
/// synchronously, however many planes it has.
pub const SYNC_SOLVE_TIME_LIMIT: Duration = Duration::from_millis(500);

/// The most meet-derived tiers (every tier whose mast the solver has to find, as
/// opposed to an authored `ScaleReference` anchor) a design may have and still be
/// solved inline.
///
/// Anchored tiers are constants to the solver; meet-derived ones are what its
/// refinement sweeps iterate over, so this bounds the work a plane count alone
/// does not (many single-index meet tiers fit under
/// [`SYNC_SOLVE_PLANE_LIMIT`]).
pub const SYNC_SOLVE_MEET_TIER_LIMIT: usize = 8;

/// Debounce delay between an edit landing and an eligible auto-solve actually
/// dispatching.
///
/// Short enough that a burst of keystrokes (typing an angle, say) only ever triggers the
/// LAST one, long enough that it reads as "just happened," not "laggy."
pub const AUTO_SOLVE_DEBOUNCE: Duration = Duration::from_millis(150);

/// How long a "one edit behind" (partial) preview frame waits for a FOLLOW-UP edit
/// before concluding the edit stream went idle and asking for a full replan of its
/// own.
///
/// Longer than [`AUTO_SOLVE_DEBOUNCE`]: this waits for the edit stream itself
/// to quiet down, not just one keystroke's burst.
pub const IDLE_REPLAN_DEBOUNCE: Duration = Duration::from_millis(400);

/// How often a running background solve's banner updates its elapsed time.
pub const SOLVING_TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Whether a solve for a design with `plane_count` planes should still run
/// synchronously (the New/Load/explicit-Solve fast path) rather than in the
/// background.
///
/// Prefers this design's own last REAL measured solve time when one
/// exists, since a real measurement beats any estimate; falls back to
/// `plane_count` only for a design that has never solved yet.
///
/// A measurement describes the design as it was when it was taken, so a fast one
/// can outlive the edits that made the design expensive. The desktop editor's
/// inline solve on its UI thread therefore asks [`should_solve_synchronously_for`]
/// instead, where a measurement can only veto an inline solve, never grant one.
#[must_use]
pub fn should_solve_synchronously(plane_count: usize, last_solve: Option<Duration>) -> bool {
    last_solve.map_or(plane_count <= SYNC_SOLVE_PLANE_LIMIT, |last| {
        last <= SYNC_SOLVE_TIME_LIMIT
    })
}

/// What a design will cost to solve, read off its tiers without solving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolveCostEstimate {
    /// The facet planes the schedule emits: one per index of every tier. Solve
    /// cost is driven by this, not by the tier count -- a wide-orbit tier emits
    /// many planes at once.
    pub planes: usize,
    /// Tiers whose mast the solver derives (meet an unspecified vertex, or meet
    /// named facets) rather than one anchored by an authored scale value.
    pub meet_derived_tiers: usize,
    /// Whether any tier carries an authoring-level target. Resolving one runs
    /// extra solves (a bootstrap plus a bisection) on top of the design's own.
    pub has_tier_targets: bool,
}

impl SolveCostEstimate {
    /// The estimate for `design`. Linear in the tier count; never solves.
    #[must_use]
    pub fn of(design: &Design) -> Self {
        Self {
            planes: design.tiers.iter().map(|t| t.indices.len()).sum(),
            meet_derived_tiers: design
                .tiers
                .iter()
                .filter(|t| !matches!(t.constraint, MeetConstraint::ScaleReference(_)))
                .count(),
            has_tier_targets: !design.tier_targets.is_empty(),
        }
    }
}

/// Whether a design of this `cost` may be solved inline on a UI thread.
///
/// Conservative on purpose, because an inline solve of a slow design freezes the
/// window (the corpus's worst case is 5.9 s). It is inline only when ALL hold:
/// at most [`SYNC_SOLVE_PLANE_LIMIT`] planes, at most [`SYNC_SOLVE_MEET_TIER_LIMIT`]
/// meet-derived tiers, no tier targets, and the design's last measured solve (if
/// it has one) took no longer than [`SYNC_SOLVE_TIME_LIMIT`]. Everything else runs
/// on the background worker, which can be cancelled.
///
/// The measurement is a veto, not a grant: it was taken on an earlier state of the
/// design, and the solver's cost is not monotonic in the edits between, so a fast
/// old measurement must not admit a design the estimate rules out.
#[must_use]
pub fn should_solve_synchronously_for(
    cost: SolveCostEstimate,
    last_solve: Option<Duration>,
) -> bool {
    cost.planes <= SYNC_SOLVE_PLANE_LIMIT
        && cost.meet_derived_tiers <= SYNC_SOLVE_MEET_TIER_LIMIT
        && !cost.has_tier_targets
        && last_solve.is_none_or(|last| last <= SYNC_SOLVE_TIME_LIMIT)
}

/// Whether an edit that just landed should schedule a debounced auto-solve.
///
/// `budget` `0` disables auto-solve outright (stale-marker-only behaviour); otherwise, a
/// design with no measurement YET (`last_solve: None` -- nothing has solved since the
/// last New/Load) is scheduled optimistically: a fresh design's schedule starts empty and
/// solves near-instantly.
///
/// Once a real measurement exists,
/// it alone decides.
#[must_use]
pub const fn should_schedule_auto_solve(last_solve: Option<Duration>, budget: Duration) -> bool {
    if budget.is_zero() {
        return false;
    }
    match last_solve {
        Some(last) => last.as_millis() < budget.as_millis(),
        None => true,
    }
}

/// The banner text shown while auto-solve is disabled FOR THIS DESIGN specifically
/// (as opposed to a `0` budget) -- this design's own last measured solve already
/// exceeds the configured budget.
#[must_use]
pub fn auto_solve_off_note(last_solve: Duration) -> String {
    format!(
        "Auto-solve off for this design: last solve took {:.1}s.",
        last_solve.as_secs_f32()
    )
}

/// The "Solving..." banner text a running background solve shows, ticked forward
/// every [`SOLVING_TICK_INTERVAL`].
#[must_use]
pub fn solving_banner(tier_count: usize, elapsed: Duration) -> String {
    format!(
        "Solving... ({tier_count} tier{}) -- {:.1}s elapsed",
        if tier_count == 1 { "" } else { "s" },
        elapsed.as_secs_f32()
    )
}

/// [`Design::solve`]'s cancellable counterpart for a background solve.
///
/// Same legacy [`SolveError::TooManyPlanes`] fallback [`Design::solve`] documents on
/// itself (reproduced here, since that method's own `SolveControl::default()` can never
/// observe a real cancel).
///
/// [`SolveError::Cancelled`] is returned as a real `Err`, not swallowed: the caller
/// treats it exactly like any other solve error.
///
/// # Errors
///
/// Whatever [`Design::solve_with`] returns, except `TooManyPlanes` (replaced by the
/// solver's own all-`Failed` fallback list); `Cancelled` when `cancel` is set.
pub fn solve_cancellably(
    design: &Design,
    cancel: &AtomicBool,
) -> Result<Vec<SolvedTier>, DesignSolveError> {
    match design.solve_with(&SolveControl::with_cancel(cancel)) {
        Err(DesignSolveError::Solve(SolveError::TooManyPlanes { .. })) => {
            // Threads `cancel` into this fallback too: a cancel observed right as the
            // design turns out to be over `MAX_PLANES` must still return `Cancelled`
            // rather than silently finishing the (legacy, always-`Ok`) fallback build.
            if cancel.load(Ordering::Relaxed) {
                return Err(DesignSolveError::Solve(SolveError::Cancelled));
            }
            Ok(solve_meet_points(
                design.meta.gear_teeth_abs(),
                &design.meet_tier_inputs(),
            ))
        }
        other => other,
    }
}

/// A CHEAP (no solve) pre-filter for [`too_many_planes_message`].
///
/// `true` iff `solved` carries the exact tell the solver stamps into every non-anchor
/// tier's own [`SolvedTier::detail`] for its all-`SolveStrategy::Failed`
/// over-`MAX_PLANES` fallback ("...
///
/// above the N-plane cap for candidate-vertex enumeration").
/// [`too_many_planes_message`]'s own `solve_with` re-check is the only
/// AUTHORITATIVE source of the numbers; this only lets a caller skip that re-check
/// for the overwhelming majority of `Degenerate`/`Unbounded` results that have
/// nothing to do with the plane cap. A false negative only means an ordinary
/// (unhelpful but not wrong) message shows instead of the plane-cap one.
#[must_use]
pub fn likely_hit_plane_cap(solved: &[SolvedTier]) -> bool {
    solved
        .iter()
        .any(|t| matches!(t.strategy, SolveStrategy::Failed) && t.detail.contains("plane cap"))
}

/// An over-cap design silently renders as an ordinary (misleading)
/// "Degenerate"/"Unbounded" status.
///
/// Since `Design::solve()`/[`solve_cancellably`] both reproduce the solver's own
/// all-`Failed` fallback rather than an error.
///
/// Runs `Design::solve_with` (which surfaces the real
/// [`SolveError::TooManyPlanes`]) purely to check for this one condition: `Some`
/// with a cutter-actionable sentence iff it applies, `None` otherwise.
#[must_use]
pub fn too_many_planes_message(design: &Design) -> Option<String> {
    match design.solve_with(&SolveControl::default()) {
        Err(DesignSolveError::Solve(SolveError::TooManyPlanes { planes, max })) => Some(format!(
            "This design has {planes} facet planes; the solver supports up to {max} -- reduce \
             symmetry or split the design."
        )),
        _ => None,
    }
}

/// Converts an already-solved mast list into the `GpuFacetPlane`s a viewport draws.
///
/// It goes via [`Design::planes_from_solved`] -- the `n . x <= m` half-space convention
/// flipped to `GpuFacetPlane`'s `n . x + d = 0` (`d = -m`).
///
/// See
/// [`crate::view_model::solid_status::design_to_gpu_planes`] for the
/// solve-internally counterpart.
#[must_use]
pub fn design_to_gpu_planes_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> Vec<GpuFacetPlane> {
    design
        .planes_from_solved(solved)
        .into_iter()
        .map(|(normal, offset)| GpuFacetPlane::new(normal.as_vec3(), -offset as f32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    #[test]
    fn a_zero_budget_never_schedules_and_no_measurement_schedules_optimistically() {
        assert!(!should_schedule_auto_solve(None, Duration::ZERO));
        assert!(should_schedule_auto_solve(None, Duration::from_millis(1)));
        assert!(should_schedule_auto_solve(
            Some(Duration::from_millis(100)),
            Duration::from_millis(200)
        ));
        assert!(!should_schedule_auto_solve(
            Some(Duration::from_millis(200)),
            Duration::from_millis(200)
        ));
    }

    #[test]
    fn a_measurement_beats_the_plane_count_estimate() {
        assert!(should_solve_synchronously(SYNC_SOLVE_PLANE_LIMIT, None));
        assert!(!should_solve_synchronously(
            SYNC_SOLVE_PLANE_LIMIT + 1,
            None
        ));
        assert!(should_solve_synchronously(
            10_000,
            Some(SYNC_SOLVE_TIME_LIMIT)
        ));
        assert!(!should_solve_synchronously(
            1,
            Some(SYNC_SOLVE_TIME_LIMIT + Duration::from_millis(1))
        ));
    }

    const SMALL: SolveCostEstimate = SolveCostEstimate {
        planes: 4,
        meet_derived_tiers: 2,
        has_tier_targets: false,
    };

    #[test]
    fn a_small_design_solves_inline_unless_its_last_solve_was_slow() {
        assert!(should_solve_synchronously_for(SMALL, None));
        assert!(should_solve_synchronously_for(
            SMALL,
            Some(SYNC_SOLVE_TIME_LIMIT)
        ));
        assert!(!should_solve_synchronously_for(
            SMALL,
            Some(SYNC_SOLVE_TIME_LIMIT + Duration::from_millis(1))
        ));
    }

    #[test]
    fn a_fast_measurement_never_admits_a_design_the_estimate_rules_out() {
        let fast = Some(Duration::from_millis(1));
        let too_many_planes = SolveCostEstimate {
            planes: SYNC_SOLVE_PLANE_LIMIT + 1,
            ..SMALL
        };
        let too_many_meet_tiers = SolveCostEstimate {
            meet_derived_tiers: SYNC_SOLVE_MEET_TIER_LIMIT + 1,
            ..SMALL
        };
        let targeted = SolveCostEstimate {
            has_tier_targets: true,
            ..SMALL
        };
        for cost in [too_many_planes, too_many_meet_tiers, targeted] {
            assert!(!should_solve_synchronously_for(cost, None), "{cost:?}");
            assert!(!should_solve_synchronously_for(cost, fast), "{cost:?}");
        }
    }

    #[test]
    fn the_limits_themselves_are_still_inline() {
        let at_limits = SolveCostEstimate {
            planes: SYNC_SOLVE_PLANE_LIMIT,
            meet_derived_tiers: SYNC_SOLVE_MEET_TIER_LIMIT,
            has_tier_targets: false,
        };
        assert!(should_solve_synchronously_for(at_limits, None));
    }

    #[test]
    fn the_estimate_counts_indices_and_only_the_non_anchored_tiers() {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let expected_planes: usize = design.tiers.iter().map(|t| t.indices.len()).sum();
        let anchored = SolveCostEstimate::of(&design);
        assert_eq!(anchored.planes, expected_planes);
        assert!(anchored.planes > 0);
        assert_eq!(
            anchored.meet_derived_tiers, 0,
            "every template tier is anchored"
        );
        assert!(!anchored.has_tier_targets);

        design.tiers[0].constraint = MeetConstraint::MeetExisting;
        design.tiers[1].constraint = MeetConstraint::MeetNamed(vec!["P1".to_string()]);
        let mixed = SolveCostEstimate::of(&design);
        assert_eq!(mixed.meet_derived_tiers, 2);
        assert_eq!(
            mixed.planes, expected_planes,
            "a constraint change moves no plane"
        );
    }

    #[test]
    fn banners_read_as_sentences() {
        assert_eq!(
            solving_banner(1, Duration::from_millis(1300)),
            "Solving... (1 tier) -- 1.3s elapsed"
        );
        assert_eq!(
            solving_banner(3, Duration::ZERO),
            "Solving... (3 tiers) -- 0.0s elapsed"
        );
        assert_eq!(
            auto_solve_off_note(Duration::from_millis(2500)),
            "Auto-solve off for this design: last solve took 2.5s."
        );
    }

    #[test]
    fn a_set_cancel_flag_cancels_before_any_work() {
        let design = crate::EditorSession::fresh().design;
        let cancel = AtomicBool::new(false);
        assert!(solve_cancellably(&design, &cancel).is_ok());
        assert!(!likely_hit_plane_cap(&[]));
        assert_eq!(too_many_planes_message(&design), None);
    }
}
