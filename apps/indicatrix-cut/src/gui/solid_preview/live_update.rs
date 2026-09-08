//! Which geometry to hand the solid preview after an edit, chosen automatically among
//! three tiers of freshness.
//!
//! Pure and independent of `preview_state`'s worker
//! thread/`RedrawGate` machinery -- [`plan_preview`] is a plain function callers run
//! on the UI thread before ever touching [`super::preview_state::SolidPreviewState`].
//!
//! # The three tiers
//!
//! 1. **Pinned.** Every tier is
//!    [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`] -- each
//!    mast is read directly off its own constraint, no solver call, so the design
//!    previews instantly regardless of tier count. Checked first, unconditionally.
//! 2. **Fresh.** At least one tier is free, `last_solved` is aligned with `design`'s
//!    current tier count (the precondition `resolve_dirty` panics on otherwise), and
//!    [`DirtySolver::resolve_dirty`] finishes within `budget`.
//! 3. **Stale.** Same call as tier 2, but wall time exceeds `budget`. The solve can't
//!    be interrupted mid-flight, so "over budget" is only known once the result is in
//!    hand. That fresh result is still chained forward as [`PreviewPlan::solved`] (so
//!    the NEXT call doesn't repeat the same slow resolve), but this frame draws the
//!    OLD (`previous`) planes so the user sees the last solid actually finished.
//!    `pending` names every dirty tier, for the caller's overlay.
//!
//! `last_solved` missing or misaligned (an `AddTier`/`RemoveTier` edit, which can
//! never resolve as a subgraph) falls back to a full [`indicatrix_cut_core::Design::solve`];
//! a successful full solve is authoritative regardless of timing, reported
//! [`Freshness::Fresh`] too. Either path failing with [`indicatrix_cut_core::MissingAnchor`]
//! reports [`Freshness::Unsolvable`], falling back to whatever `last_solved` planes
//! are available -- never an empty `PreviewPlan::planes` when a prior frame exists.

use glam::{DVec3, Vec3};
use indicatrix::geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier};
use indicatrix_cut_core::{Design, MissingAnchor};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

/// Default over-budget threshold for [`plan_preview`]'s tier-2/3 decision.
///
/// `50` ms, a UI-responsiveness budget rather than a guarantee every design resolves
/// within it. Measured against the worst fixture available (CrackOtto-Step, 103
/// tiers, nearly all non-anchor): that resolve took **2.13 s**, confirming this is
/// exactly the shape tier 3 (`Freshness::Stale`) exists for -- a budget loose enough
/// to tolerate it would make ordinary ScaleReference-heavy edits feel just as
/// sluggish. See `tests::timing_resolve_dirty_on_crackotto_step_103_tier`.
pub const DEFAULT_PREVIEW_BUDGET: Duration = Duration::from_millis(50);

/// Abstracts [`indicatrix_cut_core::Design::resolve_dirty`] so [`plan_preview`] is testable:
/// a fake implementation can return canned results or sleep a controlled amount.
pub trait DirtySolver {
    /// # Errors
    ///
    /// See [`indicatrix_cut_core::Design::resolve_dirty`]'s own `# Errors` section.
    fn resolve_dirty(
        &self,
        design: &Design,
        previous: &[SolvedTier],
        dirty: &BTreeSet<usize>,
    ) -> Result<Vec<SolvedTier>, MissingAnchor>;
}

/// The real [`DirtySolver`], delegating straight to [`indicatrix_cut_core::Design::resolve_dirty`].
/// Every other implementation in this crate exists solely for this module's tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealSolver;

impl DirtySolver for RealSolver {
    fn resolve_dirty(
        &self,
        design: &Design,
        previous: &[SolvedTier],
        dirty: &BTreeSet<usize>,
    ) -> Result<Vec<SolvedTier>, MissingAnchor> {
        design.resolve_dirty(previous, dirty)
    }
}

/// Which of the module doc comment's three tiers [`plan_preview`] chose, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// No solver call was made.
    Pinned,
    /// Solved within budget (subgraph or full).
    Fresh,
    /// `pending` is the `dirty` set, for the caller's overlay/banner.
    Stale { pending: BTreeSet<usize> },
    /// See [`indicatrix_cut_core::MissingAnchor`].
    Unsolvable(MissingAnchor),
}

/// [`plan_preview`]'s result: what to draw, what to remember as `last_solved` next
/// call, and why.
#[derive(Debug, Clone)]
pub struct PreviewPlan {
    /// The plane arrangement to draw THIS frame, in the same `(normal, offset)` `f32`
    /// convention `mesh_cache::MeshCache::get_or_build` takes.
    pub planes: Vec<(Vec3, f32)>,
    /// The mast list to pass as `last_solved` next call -- can be newer than `planes`
    /// (see [`Freshness::Stale`]).
    pub solved: Option<Vec<SolvedTier>>,
    pub freshness: Freshness,
}

/// Narrows a [`indicatrix_cut_core::Design::planes_from_solved`] result to the `f32`
/// convention every other `solid_preview` module uses.
fn narrow_planes(planes: Vec<(DVec3, f64)>) -> Vec<(Vec3, f32)> {
    planes
        .into_iter()
        .map(|(n, m)| (Vec3::new(n.x as f32, n.y as f32, n.z as f32), m as f32))
        .collect()
}

/// Every tier's mast read directly off its own [`MeetConstraint::ScaleReference`] --
/// tier 1 of the module doc comment. Panics if any tier is not `ScaleReference`.
fn masts_from_pinned_tiers(design: &Design) -> Vec<SolvedTier> {
    design
        .tiers
        .iter()
        .map(|tier| {
            let MeetConstraint::ScaleReference(mast) = &tier.constraint else {
                unreachable!(
                    "masts_from_pinned_tiers: caller must confirm every tier is \
                     ScaleReference first"
                );
            };
            SolvedTier {
                mast: *mast,
                strategy: SolveStrategy::ScaleReference,
                detail: "pinned (ScaleReference)".to_string(),
            }
        })
        .collect()
}

/// Chooses which geometry to draw after an edit -- see the module doc comment for the
/// three tiers this implements.
///
/// `dirty` is the set of tier indices the triggering edit touched (see
/// `indicatrix_cut_core::resolve::affected_tiers`); `last_solved` is the previous call's
/// [`PreviewPlan::solved`], or `None` after an `AddTier`/`RemoveTier` edit.
#[must_use]
pub fn plan_preview(
    design: &Design,
    last_solved: Option<&[SolvedTier]>,
    dirty: &BTreeSet<usize>,
    budget: Duration,
    solver: &dyn DirtySolver,
) -> PreviewPlan {
    if design
        .tiers
        .iter()
        .all(|tier| matches!(tier.constraint, MeetConstraint::ScaleReference(_)))
    {
        let solved = masts_from_pinned_tiers(design);
        let planes = narrow_planes(design.planes_from_solved(&solved));
        return PreviewPlan {
            planes,
            solved: Some(solved),
            freshness: Freshness::Pinned,
        };
    }

    // Mismatch (or no previous solve) means an `AddTier`/`RemoveTier` edit happened;
    // a full solve is the only valid path.
    let aligned_previous = last_solved.filter(|previous| previous.len() == design.tiers.len());

    let Some(previous) = aligned_previous else {
        return match design.solve() {
            Ok(solved) => {
                let planes = narrow_planes(design.planes_from_solved(&solved));
                PreviewPlan {
                    planes,
                    solved: Some(solved),
                    freshness: Freshness::Fresh,
                }
            }
            Err(err) => {
                let planes = last_solved.map_or_else(Vec::new, |previous| {
                    narrow_planes(design.planes_from_solved(previous))
                });
                PreviewPlan {
                    planes,
                    solved: None,
                    freshness: Freshness::Unsolvable(err),
                }
            }
        };
    };

    let start = Instant::now();
    let result = solver.resolve_dirty(design, previous, dirty);
    let elapsed = start.elapsed();

    match result {
        Ok(new_solved) if elapsed <= budget => {
            let planes = narrow_planes(design.planes_from_solved(&new_solved));
            PreviewPlan {
                planes,
                solved: Some(new_solved),
                freshness: Freshness::Fresh,
            }
        }
        Ok(new_solved) => {
            // Over budget: show the OLD planes, but chain the fresh (late) result
            // forward as the next call's `last_solved`.
            let planes = narrow_planes(design.planes_from_solved(previous));
            PreviewPlan {
                planes,
                solved: Some(new_solved),
                freshness: Freshness::Stale {
                    pending: dirty.clone(),
                },
            }
        }
        Err(err) => {
            let planes = narrow_planes(design.planes_from_solved(previous));
            PreviewPlan {
                planes,
                solved: None,
                freshness: Freshness::Unsolvable(err),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::{Block, classify_blocks, meet_tier_inputs_from_asc};
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    /// Proves a branch of [`plan_preview`] never falls through to `resolve_dirty`.
    struct PanicSolver;

    impl DirtySolver for PanicSolver {
        fn resolve_dirty(
            &self,
            _design: &Design,
            _previous: &[SolvedTier],
            _dirty: &BTreeSet<usize>,
        ) -> Result<Vec<SolvedTier>, MissingAnchor> {
            panic!("DirtySolver::resolve_dirty must not be called on this branch");
        }
    }

    /// Returns `result` immediately -- the "resolve finished within budget" case.
    struct InstantSolver {
        result: Vec<SolvedTier>,
    }

    impl DirtySolver for InstantSolver {
        fn resolve_dirty(
            &self,
            _design: &Design,
            _previous: &[SolvedTier],
            _dirty: &BTreeSet<usize>,
        ) -> Result<Vec<SolvedTier>, MissingAnchor> {
            Ok(self.result.clone())
        }
    }

    /// Sleeps `sleep` before returning `result` -- simulates an over-budget resolve.
    struct SlowSolver {
        result: Vec<SolvedTier>,
        sleep: Duration,
    }

    impl DirtySolver for SlowSolver {
        fn resolve_dirty(
            &self,
            _design: &Design,
            _previous: &[SolvedTier],
            _dirty: &BTreeSet<usize>,
        ) -> Result<Vec<SolvedTier>, MissingAnchor> {
            std::thread::sleep(self.sleep);
            Ok(self.result.clone())
        }
    }

    fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: vec![0.0],
            constraint,
            imported_meet: None,
            detached: Vec::new(),
        }
    }

    fn pinned_design() -> Design {
        Design::new(
            PreformSpec::block(1.0, 1.0, 1.0),
            ScheduleMeta {
                gear_teeth: 96,
                ..ScheduleMeta::default()
            },
            vec![tier("Table", 0.0, MeetConstraint::ScaleReference(0.5))],
        )
    }

    /// One anchored (`ScaleReference`) tier plus one free (`MeetExisting`) tier in the
    /// same block, just enough real structure for `Design::solve()` to succeed.
    fn free_design() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta {
                gear_teeth: 96,
                ..ScheduleMeta::default()
            },
            vec![
                tier("C1", 30.0, MeetConstraint::ScaleReference(0.6)),
                tier("C2", 40.0, MeetConstraint::MeetExisting),
            ],
        )
    }

    #[test]
    fn pinned_only_design_never_calls_the_solver() {
        let design = pinned_design();
        let plan = plan_preview(
            &design,
            None,
            &BTreeSet::new(),
            DEFAULT_PREVIEW_BUDGET,
            &PanicSolver,
        );
        assert_eq!(plan.freshness, Freshness::Pinned);
        assert_ne!(plan.planes, [] as [(Vec3, f32); 0]);
        assert_eq!(plan.solved.map(|s| s.len()), Some(1));
    }

    #[test]
    fn cheap_free_design_resolves_fresh_within_budget() {
        let design = free_design();
        let previous = design.solve().expect("fixture must solve");
        let solver = InstantSolver {
            result: previous.clone(),
        };
        let dirty = BTreeSet::from([1]);

        let plan = plan_preview(
            &design,
            Some(&previous),
            &dirty,
            Duration::from_millis(50),
            &solver,
        );
        assert_eq!(plan.freshness, Freshness::Fresh);
        assert!(plan.solved.is_some());
        assert_ne!(plan.planes, [] as [(Vec3, f32); 0]);
    }

    #[test]
    fn over_budget_solver_reports_stale_with_the_edited_tier_pending() {
        let design = free_design();
        let previous = design.solve().expect("fixture must solve");
        let solver = SlowSolver {
            result: previous.clone(),
            sleep: Duration::from_millis(40),
        };
        let dirty = BTreeSet::from([1]);
        let budget = Duration::from_millis(5);

        let plan = plan_preview(&design, Some(&previous), &dirty, budget, &solver);
        match plan.freshness {
            Freshness::Stale { pending } => assert_eq!(pending, dirty),
            other => panic!("expected Stale, got {other:?}"),
        }
        assert!(
            plan.solved.is_some(),
            "the fresh (late) result must still be chained forward as the next \
             call's last_solved, even though this frame reports Stale"
        );
    }

    #[test]
    fn misaligned_previous_falls_back_to_a_full_solve() {
        let design = free_design();
        // Wrong length: stands in for a `last_solved` left over from before an
        // `AddTier`/`RemoveTier` edit.
        let stale_previous: Vec<SolvedTier> = Vec::new();
        let dirty = BTreeSet::from([1]);

        let plan = plan_preview(
            &design,
            Some(&stale_previous),
            &dirty,
            DEFAULT_PREVIEW_BUDGET,
            &PanicSolver,
        );
        assert_eq!(plan.freshness, Freshness::Fresh);
        assert_eq!(plan.solved.map(|s| s.len()), Some(design.tiers.len()));
    }

    #[test]
    fn no_previous_at_all_also_falls_back_to_a_full_solve() {
        let design = free_design();
        let plan = plan_preview(
            &design,
            None,
            &BTreeSet::new(),
            DEFAULT_PREVIEW_BUDGET,
            &PanicSolver,
        );
        assert_eq!(plan.freshness, Freshness::Fresh);
        assert!(plan.solved.is_some());
    }

    /// `indicatrix-cut-core`'s "CrackOtto-Step" fixture (PC 05.115, 103 tiers), re-authored
    /// as a full [`Design`] rather than raw planes: every tier is implicit
    /// `MeetExisting`, so one `ScaleReference` anchor is bootstrapped per
    /// crown/pavilion/girdle block from that tier's real recorded mast, leaving the
    /// rest genuinely free.
    const CRACKOTTO_STEP_ASC: &str = include_str!(
        "../../../../../crates/indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc"
    );

    fn crackotto_step_design() -> Design {
        let schedule =
            indicatrix_formats::asc::parse_asc(CRACKOTTO_STEP_ASC).expect("fixture must parse");
        let mut inputs = meet_tier_inputs_from_asc(&schedule);
        let blocks = classify_blocks(&inputs);
        for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
            let anchored = inputs.iter().zip(&blocks).any(|(t, &b)| {
                b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_))
            });
            if anchored {
                continue;
            }
            if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
                inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
            }
        }
        let tiers = inputs
            .into_iter()
            .zip(&schedule.tiers)
            .map(|(input, original)| ConstraintTier {
                angle_deg: input.angle_deg,
                name: original.name.clone(),
                indices: input.indices,
                constraint: input.constraint,
                imported_meet: None,
                detached: Vec::new(),
            })
            .collect();
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta {
                gemcad_version: schedule.gemcad_version.clone(),
                gear_teeth: schedule.gear_teeth,
                gear_reference_angle: schedule.gear_reference_angle,
                symmetry_order: schedule.symmetry_order,
                mirror: schedule.mirror,
                refractive_index: schedule.refractive_index,
                headers: schedule.headers.clone(),
                footnotes: schedule.footnotes,
            },
            tiers,
        )
    }

    #[test]
    #[ignore = "timing measurement, not a correctness check -- run with \
                --release --ignored --nocapture"]
    fn timing_resolve_dirty_on_crackotto_step_103_tier() {
        let design = crackotto_step_design();
        assert_eq!(
            design.tiers.len(),
            103,
            "fixture must have its real tier count"
        );
        let baseline = design.solve().expect("must solve to get a starting point");

        let mut edited = design;
        edited.tiers[20].angle_deg += 0.5;
        let dirty = BTreeSet::from([20]);

        let start = Instant::now();
        let result = edited
            .resolve_dirty(&baseline, &dirty)
            .expect("subgraph resolve");
        let elapsed = start.elapsed();
        assert_eq!(result.len(), 103);

        println!(
            "CrackOtto-Step (103 tiers): resolve_dirty for a single-tier edit: {elapsed:?} \
             (DEFAULT_PREVIEW_BUDGET = {DEFAULT_PREVIEW_BUDGET:?})"
        );
    }
}
