//! Which tiers the policy may move, and how an action becomes a candidate design.
//!
//! [`MoveSpace`] measures the movable set of a closed design (and the hinges anchored tiers
//! turn about); [`apply_action`] applies a per-tier action to a copy of a design, clamped to
//! the step limit and checked for safety. Both environments reach them through
//! [`super::common`].

#![allow(
    clippy::redundant_pub_crate,
    reason = "crate-private helper module: `pub(crate)` documents the intended reach of each item"
)]

use super::{action::ContinuousTierAction, error::EnvError, traits::StepRejection};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    Design, OptimizeOptions,
    design::{FacetSide, TierHinge, mast_through, tier_hinges},
    free_tier_indices_with,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    optimize::MAX_SAFE_CANDIDATE_ANGLE_DEG as CORE_MAX_SAFE_CANDIDATE_ANGLE_DEG,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Smallest angle magnitude (degrees) a movable tier may be written at. Below one degree a
/// facet is practically horizontal: the meet solver loses the crown/pavilion side and the
/// plane approaches the degenerate table/culet plane.
pub const MIN_SAFE_CANDIDATE_ANGLE_DEG: f64 = 1.0;
/// Largest signed-angle magnitude accepted by cut-core's safe authoring range.
pub const MAX_SAFE_CANDIDATE_ANGLE_DEG: f64 = CORE_MAX_SAFE_CANDIDATE_ANGLE_DEG;

/// A hinge-anchored tier needs a mast above this to be a plane at all.
const MIN_HINGE_MAST: f64 = 1e-9;

/// Whether moving a tier from `old` to `new` degrees is safe: finite, magnitude within
/// `[MIN_SAFE_ANGLE_DEG, MAX_SAFE_CANDIDATE_ANGLE_DEG]`, and on the same side of the girdle
/// (the sign bit decides crown or pavilion in the meet solver).
fn angle_is_safe(old: f64, new: f64) -> bool {
    new.is_finite()
        && (MIN_SAFE_CANDIDATE_ANGLE_DEG..=MAX_SAFE_CANDIDATE_ANGLE_DEG).contains(&new.abs())
        && new.is_sign_negative() == old.is_sign_negative()
}

/// The tiers the policy may move, and the hinges anchored tiers turn about.
#[derive(Debug, Clone)]
pub(crate) struct MoveSpace {
    free: Vec<usize>,
    anchors: BTreeMap<usize, TierHinge>,
    baseline: Arc<MeetPreservationBaseline>,
    original: Design,
    policy: PreservationPolicy,
    requested_bounds: BTreeMap<usize, (f64, f64)>,
    vary_anchored: bool,
    only_tiers: Option<BTreeSet<usize>>,
}

impl MoveSpace {
    /// Measures the movable set of the (closed) `design`.
    ///
    /// Every imported `.asc` design pins every tier to a `ScaleReference` mast, which only
    /// moves when it is varied as an anchored tier: with `vary_anchored` the tier turns about
    /// its measured hinge (its mast is recomputed so the plane keeps passing through it), the
    /// way the editor's Optimize does.
    #[cfg(test)]
    pub(crate) fn measure(design: &Design, vary_anchored: bool) -> Result<Self, EnvError> {
        Self::measure_with(
            design,
            vary_anchored,
            None,
            PreservationPolicy::OPTIMIZE,
            &BTreeMap::new(),
        )
    }

    pub(crate) fn measure_with(
        design: &Design,
        vary_anchored: bool,
        only_tiers: Option<&BTreeSet<usize>>,
        policy: PreservationPolicy,
        requested_bounds: &BTreeMap<usize, (f64, f64)>,
    ) -> Result<Self, EnvError> {
        let baseline = Arc::new(
            MeetPreservationBaseline::capture(design)
                .map_err(|error| EnvError::InvalidBaseline(error.to_string()))?,
        );
        let solved = design.solve().map_err(EnvError::Unsolvable)?;
        let hinges = if vary_anchored {
            tier_hinges(design, &solved)
        } else {
            BTreeMap::new()
        };
        let options = OptimizeOptions {
            vary_anchored,
            only_tiers: only_tiers.cloned(),
            anchor_hinges: hinges
                .iter()
                .map(|(&index, hinge)| (index, hinge.point))
                .collect(),
            ..OptimizeOptions::default()
        };
        let free = free_tier_indices_with(design, &options);
        if free.is_empty() {
            return Err(EnvError::NoMovableTiers);
        }
        let anchors = free
            .iter()
            .filter(|&&index| {
                matches!(
                    design.tiers[index].constraint,
                    MeetConstraint::ScaleReference(_)
                )
            })
            .filter_map(|&index| hinges.get(&index).map(|hinge| (index, *hinge)))
            .collect();
        Ok(Self {
            free,
            anchors,
            baseline,
            original: design.clone(),
            policy,
            requested_bounds: requested_bounds.clone(),
            vary_anchored,
            only_tiers: only_tiers.cloned(),
        })
    }

    pub(crate) fn free(&self) -> &[usize] {
        &self.free
    }

    pub(crate) fn is_free(&self, index: usize) -> bool {
        self.free.binary_search(&index).is_ok()
    }

    pub(crate) fn requested_bounds(&self, index: usize) -> (f64, f64) {
        self.requested_bounds
            .get(&index)
            .copied()
            .unwrap_or((-MAX_SAFE_CANDIDATE_ANGLE_DEG, MAX_SAFE_CANDIDATE_ANGLE_DEG))
    }

    pub(crate) fn certify(
        &self,
        design: &Design,
    ) -> Result<indicatrix_cut_core::meet_preservation::PreservationFacts, String> {
        self.baseline
            .validate(design, self.policy)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn baseline(&self) -> Arc<MeetPreservationBaseline> {
        self.baseline.clone()
    }
}

/// Applies `action` to a copy of `design`.
///
/// The action is per tier (see [`ContinuousTierAction`]); only the movable tiers read their
/// entry, clamped to `+-max_step_deg`. Anchored tiers get the mast through their hinge, and
/// tiers that follow a relation are brought to the angle their relation gives. The proposal is
/// then projected onto the states that preserve the episode's original meets ([`project`]);
/// a proposal the projection cannot certify is [`StepRejection::PreservationViolation`].
pub(crate) fn apply_action(
    space: &MoveSpace,
    design: &Design,
    action: &ContinuousTierAction,
    max_step_deg: f64,
) -> Result<Design, StepRejection> {
    let candidate = propose(space, design, action, max_step_deg)?;
    if candidate == *design {
        return Ok(candidate);
    }
    project(space, design, &candidate, max_step_deg)
        .map_err(|_| StepRejection::PreservationViolation)
}

/// Why the meet-preserving projection of a proposal yielded no candidate.
#[derive(Debug)]
enum ProjectionRefusal {
    /// The gate refused every state the projection reached (the cut-core violation, kept for
    /// diagnosis: `apply_action` reports it only as a `PreservationViolation`).
    Gate(indicatrix_cut_core::meet_preservation::PreservationViolation),
    /// The projected state left the configured per-tier angle bounds.
    OutsideRequestedBounds,
    /// The projected state moved a free tier by more than the step limit.
    ExceedsStep,
}

impl std::fmt::Display for ProjectionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gate(violation) => {
                write!(
                    f,
                    "meet-preserving gate ({}): {violation}",
                    violation.kind()
                )
            }
            Self::OutsideRequestedBounds => {
                f.write_str("the projected state leaves the configured angle bounds")
            }
            Self::ExceedsStep => {
                f.write_str("the projected state moves a free tier by more than the step limit")
            }
        }
    }
}

/// Why `apply_action` refuses `action` at the projection stage, as text: the cut-core violation
/// or the post-check that failed. `None` when the projection is not what refuses it (the action
/// is accepted, is a no-op, or is refused before the projection).
#[cfg(test)]
pub(crate) fn preservation_refusal(
    space: &MoveSpace,
    design: &Design,
    action: &ContinuousTierAction,
    max_step_deg: f64,
) -> Option<String> {
    let candidate = propose(space, design, action, max_step_deg).ok()?;
    if candidate == *design {
        return None;
    }
    project(space, design, &candidate, max_step_deg)
        .err()
        .map(|refusal| refusal.to_string())
}

/// The proposal of `action` on `design` before the meet-preserving projection: the movable
/// tiers at their clamped new angles with the masts through their hinges, relations settled.
/// A zero action proposes `design` itself.
fn propose(
    space: &MoveSpace,
    design: &Design,
    action: &ContinuousTierAction,
    max_step_deg: f64,
) -> Result<Design, StepRejection> {
    if action.deltas_deg.len() != design.tiers.len() {
        return Err(StepRejection::WrongActionLength {
            expected: design.tiers.len(),
            got: action.deltas_deg.len(),
        });
    }
    if action.deltas_deg.iter().all(|delta| *delta == 0.0) {
        return Ok(design.clone());
    }
    let mut candidate = design.clone();
    for &index in &space.free {
        let delta = action.deltas_deg[index];
        if !delta.is_finite() {
            return Err(StepRejection::NonFiniteDelta);
        }
        let old = candidate.tiers[index].angle_deg;
        let (mut low, mut high) = space
            .requested_bounds
            .get(&index)
            .copied()
            .unwrap_or((-MAX_SAFE_CANDIDATE_ANGLE_DEG, MAX_SAFE_CANDIDATE_ANGLE_DEG));
        if low > high {
            std::mem::swap(&mut low, &mut high);
        }
        if delta == 0.0 {
            low = old;
            high = old;
        }
        let requested = (old + delta.clamp(-max_step_deg, max_step_deg)).clamp(low, high);
        let new = requested;
        if new.to_bits() == old.to_bits() {
            continue;
        }
        if !angle_is_safe(old, new) {
            return Err(StepRejection::UnsafeAngle);
        }
        candidate.tiers[index].angle_deg = new;
        if let Some(anchor) = space.anchors.get(&index) {
            let mast = mast_through(
                new,
                anchor.azimuth_rad,
                FacetSide::of_angle_deg(new),
                anchor.point,
            );
            if !(mast.is_finite() && mast > MIN_HINGE_MAST) {
                return Err(StepRejection::NoUsableMast);
            }
            candidate.tiers[index].constraint = MeetConstraint::ScaleReference(mast);
        }
    }
    settle_relations(&mut candidate)?;
    Ok(candidate)
}

/// Projects the proposal `candidate` (built from `design` by [`propose`]) onto the states that
/// preserve the meets of the episode's original design, with every free tier held within
/// `max_step_deg` of `design` and inside its configured bounds.
fn project(
    space: &MoveSpace,
    design: &Design,
    candidate: &Design,
    max_step_deg: f64,
) -> Result<Design, ProjectionRefusal> {
    let angle_bounds = space
        .free
        .iter()
        .map(|&i| {
            let old = design.tiers[i].angle_deg;
            let step = (old - max_step_deg, old + max_step_deg);
            let requested = space.requested_bounds(i);
            let lo = step.0.max(requested.0);
            let hi = step.1.min(requested.1);
            (i, if lo <= hi { (lo, hi) } else { (old, old) })
        })
        .collect();
    let options = OptimizeOptions {
        preservation_baseline: Some(space.baseline.clone()),
        preservation_policy: space.policy,
        vary_anchored: space.vary_anchored,
        only_tiers: space.only_tiers.clone(),
        anchor_hinges: space.anchors.iter().map(|(&i, h)| (i, h.point)).collect(),
        angle_bounds,
        ..OptimizeOptions::default()
    };
    let projected = indicatrix_cut_core::optimize::project_preserved_design(
        &space.original,
        candidate,
        &options,
        None,
    )
    .map_err(ProjectionRefusal::Gate)?;
    let violates_requested = space.requested_bounds.iter().any(|(&i, &(lo, hi))| {
        projected
            .tiers
            .get(i)
            .is_none_or(|tier| tier.angle_deg < lo - 1e-9 || tier.angle_deg > hi + 1e-9)
    });
    if violates_requested {
        return Err(ProjectionRefusal::OutsideRequestedBounds);
    }
    let exceeds_step = space.free.iter().any(|&i| {
        (projected.tiers[i].angle_deg - design.tiers[i].angle_deg).abs() > max_step_deg + 1e-9
    });
    if exceeds_step {
        return Err(ProjectionRefusal::ExceedsStep);
    }
    Ok(projected)
}

/// Moves every tier that follows a relation to the angle its relation gives now.
fn settle_relations(design: &mut Design) -> Result<(), StepRejection> {
    let updates = design
        .evaluate_relations()
        .map_err(|_| StepRejection::RelationsUnsolvable)?;
    for (position, angle) in updates {
        let Some(tier) = design.tiers.get_mut(position) else {
            return Err(StepRejection::RelationsUnsolvable);
        };
        if tier.angle_deg.to_bits() == angle.to_bits() {
            continue;
        }
        if !angle_is_safe(tier.angle_deg, angle) {
            return Err(StepRejection::RelationsUnsolvable);
        }
        tier.angle_deg = angle;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_designs_have_four_movable_tiers() {
        let design = Design::concave_fixture();
        let space = MoveSpace::measure(&design, true).expect("the fixture has movable tiers");
        assert_eq!(space.free(), &[1, 2, 3, 4]);
    }

    #[test]
    fn pinned_imports_do_not_move_without_anchoring() {
        let design = Design::concave_fixture();
        assert!(matches!(
            MoveSpace::measure(&design, false),
            Err(EnvError::NoMovableTiers)
        ));
    }

    #[test]
    fn a_small_feasible_action_moves_and_preserves_the_exact_meet_baseline() {
        let design = Design::concave_fixture();
        let space = MoveSpace::measure(&design, true).expect("movable");
        let mut deltas = vec![0.0; design.tiers.len()];
        deltas[2] = 0.05;
        let candidate = apply_action(&space, &design, &ContinuousTierAction::new(deltas), 0.1)
            .expect("small feasible tier move");
        assert_ne!(candidate, design, "the test must demonstrate useful motion");
        assert!((candidate.tiers[2].angle_deg - design.tiers[2].angle_deg).abs() > 1e-6);
        space
            .certify(&candidate)
            .expect("candidate preserves original meets");
        let mut second_delta = vec![0.0; candidate.tiers.len()];
        second_delta[2] = 0.05;
        let next = apply_action(
            &space,
            &candidate,
            &ContinuousTierAction::new(second_delta),
            0.1,
        )
        .expect("second step uses the same immutable baseline");
        assert_ne!(next, candidate, "the second feasible step should also move");
        space
            .certify(&next)
            .expect("both steps certify against the original baseline");
        let no_op = apply_action(
            &space,
            &next,
            &ContinuousTierAction::new(vec![0.0; next.tiers.len()]),
            0.1,
        )
        .expect("zero action");
        assert_eq!(no_op, next, "zero action must not reanchor masts");
    }

    #[test]
    fn large_request_is_clamped_to_a_feasible_step_limit() {
        let design = Design::concave_fixture();
        let space = MoveSpace::measure(&design, true).expect("movable");
        let mut deltas = vec![0.0; design.tiers.len()];
        deltas[2] = 5.0;
        let candidate = apply_action(&space, &design, &ContinuousTierAction::new(deltas), 0.1)
            .expect("small clamped step is feasible");
        let moved = (candidate.tiers[2].angle_deg - design.tiers[2].angle_deg).abs();
        assert!(
            moved > 1e-6 && moved <= 0.1 + 1e-9,
            "projected displacement {moved}"
        );
    }

    #[test]
    fn explicit_tier_scope_survives_measurement_and_projection() {
        let design = Design::concave_fixture();
        let only_tiers = BTreeSet::from([2]);
        let space = MoveSpace::measure_with(
            &design,
            true,
            Some(&only_tiers),
            PreservationPolicy::OPTIMIZE,
            &BTreeMap::new(),
        )
        .expect("scoped tier is movable");
        assert_eq!(space.free(), &[2]);
        let mut deltas = vec![0.0; design.tiers.len()];
        deltas[2] = 0.05;
        let candidate = apply_action(&space, &design, &ContinuousTierAction::new(deltas), 0.1)
            .expect("scoped move remains feasible");
        assert_ne!(candidate, design);
        space
            .certify(&candidate)
            .expect("preservation policy is retained");
    }

    #[test]
    fn configured_bounds_clip_a_useful_step_without_exceeding_the_request() {
        let design = Design::concave_fixture();
        let angle = design.tiers[2].angle_deg;
        let bounds = BTreeMap::from([(2, (angle - 0.1, angle + 0.1))]);
        let space =
            MoveSpace::measure_with(&design, true, None, PreservationPolicy::OPTIMIZE, &bounds)
                .expect("the current design is inside the request");
        let mut deltas = vec![0.0; design.tiers.len()];
        deltas[2] = 0.5;
        let candidate = apply_action(&space, &design, &ContinuousTierAction::new(deltas), 0.5)
            .expect("projection stays in configured bounds");
        assert!((angle..=angle + 0.1 + 1e-9).contains(&candidate.tiers[2].angle_deg));
        assert!(candidate.tiers[2].angle_deg > angle + 1e-6);
    }

    #[test]
    fn wrong_length_and_non_finite_actions_are_rejected() {
        let design = Design::concave_fixture();
        let space = MoveSpace::measure(&design, true).expect("movable");
        let short = ContinuousTierAction::new(vec![0.1; design.tiers.len() - 1]);
        assert!(matches!(
            apply_action(&space, &design, &short, 0.75),
            Err(StepRejection::WrongActionLength { .. })
        ));
        let mut deltas = vec![0.0; design.tiers.len()];
        deltas[2] = f64::NAN;
        assert_eq!(
            apply_action(&space, &design, &ContinuousTierAction::new(deltas), 0.75).err(),
            Some(StepRejection::NonFiniteDelta)
        );
    }

    #[test]
    fn a_tier_cannot_cross_the_girdle() {
        let design = Design::concave_fixture();
        let space = MoveSpace::measure(&design, true).expect("movable");
        // Tier 1 is the -42 degree pavilion; a huge step limit lets the delta reach zero.
        let mut deltas = vec![0.0; design.tiers.len()];
        deltas[1] = 100.0;
        assert_eq!(
            apply_action(&space, &design, &ContinuousTierAction::new(deltas), 100.0).err(),
            Some(StepRejection::UnsafeAngle)
        );
    }
}
