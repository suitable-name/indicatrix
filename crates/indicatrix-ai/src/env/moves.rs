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
    optimize::MAX_SAFE_CANDIDATE_ANGLE_DEG,
};
use std::collections::BTreeMap;

/// Smallest angle magnitude (degrees) a movable tier may be written at. Below one degree a
/// facet is practically horizontal: the meet solver loses the crown/pavilion side and the
/// plane approaches the degenerate table/culet plane.
const MIN_SAFE_ANGLE_DEG: f64 = 1.0;

/// A hinge-anchored tier needs a mast above this to be a plane at all.
const MIN_HINGE_MAST: f64 = 1e-9;

/// Whether moving a tier from `old` to `new` degrees is safe: finite, magnitude within
/// `[MIN_SAFE_ANGLE_DEG, MAX_SAFE_CANDIDATE_ANGLE_DEG]`, and on the same side of the girdle
/// (the sign bit decides crown or pavilion in the meet solver).
fn angle_is_safe(old: f64, new: f64) -> bool {
    new.is_finite()
        && (MIN_SAFE_ANGLE_DEG..=MAX_SAFE_CANDIDATE_ANGLE_DEG).contains(&new.abs())
        && new.is_sign_negative() == old.is_sign_negative()
}

/// The tiers the policy may move, and the hinges anchored tiers turn about.
#[derive(Debug, Clone)]
pub(crate) struct MoveSpace {
    free: Vec<usize>,
    anchors: BTreeMap<usize, TierHinge>,
}

impl MoveSpace {
    /// Measures the movable set of the (closed) `design`.
    ///
    /// Every imported `.asc` design pins every tier to a `ScaleReference` mast, which only
    /// moves when it is varied as an anchored tier: with `vary_anchored` the tier turns about
    /// its measured hinge (its mast is recomputed so the plane keeps passing through it), the
    /// way the editor's Optimize does.
    pub(crate) fn measure(design: &Design, vary_anchored: bool) -> Result<Self, EnvError> {
        let solved = design.solve().map_err(EnvError::Unsolvable)?;
        let hinges = if vary_anchored {
            tier_hinges(design, &solved)
        } else {
            BTreeMap::new()
        };
        let options = OptimizeOptions {
            vary_anchored,
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
        Ok(Self { free, anchors })
    }

    pub(crate) fn free(&self) -> &[usize] {
        &self.free
    }

    pub(crate) fn is_free(&self, index: usize) -> bool {
        self.free.binary_search(&index).is_ok()
    }
}

/// Applies `action` to a copy of `design`.
///
/// The action is per tier (see [`ContinuousTierAction`]); only the movable tiers read their
/// entry, clamped to `+-max_step_deg`. Anchored tiers get the mast through their hinge, and
/// tiers that follow a relation are brought to the angle their relation gives.
pub(crate) fn apply_action(
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
    let mut candidate = design.clone();
    for &index in &space.free {
        let delta = action.deltas_deg[index];
        if !delta.is_finite() {
            return Err(StepRejection::NonFiniteDelta);
        }
        let old = candidate.tiers[index].angle_deg;
        let new = old + delta.clamp(-max_step_deg, max_step_deg);
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
    fn action_moves_exactly_the_free_tiers() {
        let design = Design::concave_fixture();
        let space = MoveSpace::measure(&design, true).expect("movable");
        let action = ContinuousTierAction::new(vec![0.5; design.tiers.len()]);
        let candidate = apply_action(&space, &design, &action, 0.75).expect("safe action");
        for (index, (before, after)) in design.tiers.iter().zip(&candidate.tiers).enumerate() {
            let moved = (after.angle_deg - before.angle_deg).abs() > 1e-12;
            assert_eq!(moved, space.free().contains(&index), "tier {index}");
        }
    }

    #[test]
    fn deltas_are_clamped_to_the_step_limit() {
        let design = Design::concave_fixture();
        let space = MoveSpace::measure(&design, true).expect("movable");
        let action = ContinuousTierAction::new(vec![5.0; design.tiers.len()]);
        let candidate = apply_action(&space, &design, &action, 0.75).expect("safe action");
        assert!((candidate.tiers[3].angle_deg - design.tiers[3].angle_deg - 0.75).abs() < 1e-12);
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
