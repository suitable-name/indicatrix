//! The goal behind the last step of a "Build this design" lesson: the learner's design is
//! the lesson's target design, rebuilt.
//!
//! A rebuild is not compared tier by tier with its "Meets" settings. The learner states
//! different meets than the original (an imported original pins every tier to an exact
//! scale, a rebuild lets tiers meet one another), so only the cut itself and the solved
//! result can be compared. And a mast is a size, so the tolerance is a fraction of it.
//!
//! The comparison never solves anything: the masts of the learner's design come from the
//! [`GoalContext`] (the UI's own cached solve), and a design that has not been solved yet
//! simply is not a match yet.

use super::{GoalContext, same_index_set};
use indicatrix_cut_core::{ConstraintTier, Design};

/// The smallest target mast a relative tolerance is measured against, so a target mast
/// near zero does not demand an impossible absolute accuracy.
pub const MAST_FLOOR: f64 = 0.05;

/// An angle this close to zero is a table or a culet.
const ZERO_ANGLE_EPSILON: f64 = 1e-9;

/// An angle at least this steep is a girdle.
const GIRDLE_MIN_ANGLE_DEG: f64 = 90.0 - 1e-6;

/// Whether `actual` is within the fraction `rel_tol` of `wanted` (never tighter than
/// `rel_tol * MAST_FLOOR`).
#[must_use]
pub fn mast_within(actual: f64, wanted: f64, rel_tol: f64) -> bool {
    (actual - wanted).abs() <= rel_tol * wanted.abs().max(MAST_FLOOR)
}

/// Whether a blank Indices field and a lone `0` mean the same cut at this angle: true for
/// a table, a culet and a girdle, whose facet has no azimuth of its own that matters.
pub(super) fn blank_equals_zero(angle_deg: f64) -> bool {
    angle_deg.abs() < ZERO_ANGLE_EPSILON || angle_deg.abs() >= GIRDLE_MIN_ANGLE_DEG
}

/// The index list with a blank one read as the lone `0` it stands for.
fn blank_as_zero(indices: &[f64]) -> Vec<f64> {
    if indices.is_empty() {
        vec![0.0]
    } else {
        indices.to_vec()
    }
}

/// Whether `user` and `wanted` are the same index positions on a gear of `gear` teeth.
fn same_indices(angle_deg: f64, user: &[f64], wanted: &[f64], gear: f64) -> bool {
    if blank_equals_zero(angle_deg) {
        same_index_set(&blank_as_zero(user), &blank_as_zero(wanted), gear)
    } else if wanted.is_empty() {
        user.is_empty()
    } else {
        same_index_set(user, wanted, gear)
    }
}

/// Whether two angles name the same side of zero: a culet (`-0`) is not a table (`+0`).
fn same_zero_side(a: f64, b: f64) -> bool {
    if a.abs() < ZERO_ANGLE_EPSILON && b.abs() < ZERO_ANGLE_EPSILON {
        a.is_sign_negative() == b.is_sign_negative()
    } else {
        true
    }
}

/// Whether `user` is the cut `wanted` describes: the same name, the same angle within
/// `angle_tol_deg`, the same index positions.
fn same_cut(user: &ConstraintTier, wanted: &ConstraintTier, gear: f64, angle_tol_deg: f64) -> bool {
    user.name.trim().eq_ignore_ascii_case(wanted.name.trim())
        && (user.angle_deg - wanted.angle_deg).abs() <= angle_tol_deg
        && same_zero_side(user.angle_deg, wanted.angle_deg)
        && same_indices(wanted.angle_deg, &user.indices, &wanted.indices, gear)
}

/// The user tier each target tier pairs with (tiers pair by name, and by order among tiers
/// sharing one), or `None` when some target tier has no partner or the counts differ.
fn pair_cuts(
    user: &[ConstraintTier],
    target: &[ConstraintTier],
    gear: f64,
    angle_tol_deg: f64,
) -> Option<Vec<usize>> {
    if user.len() != target.len() {
        return None;
    }
    let mut used = vec![false; user.len()];
    let mut pairs = Vec::with_capacity(target.len());
    for wanted in target {
        let found = user
            .iter()
            .enumerate()
            .find(|(index, tier)| !used[*index] && same_cut(tier, wanted, gear, angle_tol_deg))?
            .0;
        used[found] = true;
        pairs.push(found);
    }
    Some(pairs)
}

/// The first thing wrong with a [`Goal::DesignRebuilt`](super::Goal::DesignRebuilt), or
/// `None` when it can be met.
pub(super) fn problem(
    target: &Design,
    angle_tol_deg: f64,
    mast_rel_tol: f64,
    target_masts: &[f64],
) -> Option<String> {
    if !angle_tol_deg.is_finite() || angle_tol_deg < 0.0 {
        Some("the angle tolerance is not a number".into())
    } else if !mast_rel_tol.is_finite() || mast_rel_tol < 0.0 {
        Some("the depth tolerance is not a number".into())
    } else if target.tiers.is_empty() {
        Some("the target design has no tiers".into())
    } else if !target_masts.is_empty() && target_masts.len() != target.tiers.len() {
        Some("target_masts must have one entry per target tier".into())
    } else if target_masts.iter().any(|mast| !mast.is_finite()) {
        Some("a target mast is not a number".into())
    } else {
        None
    }
}

/// Whether the design in `ctx` is `target` rebuilt: the same gear and symmetry, every
/// target tier cut (name, angle within `angle_tol_deg`, index positions), and, when
/// `target_masts` is not empty, the design solved to a closed solid whose masts agree with
/// `target_masts` within the fraction `mast_rel_tol`.
pub(super) fn met(
    ctx: &GoalContext<'_>,
    target: &Design,
    angle_tol_deg: f64,
    mast_rel_tol: f64,
    target_masts: &[f64],
) -> bool {
    let design = ctx.design;
    if design.meta.gear_teeth_abs() != target.meta.gear_teeth_abs()
        || design.meta.symmetry_order != target.meta.symmetry_order
        || design.meta.mirror != target.meta.mirror
    {
        return false;
    }
    let gear = f64::from(design.meta.gear_teeth_abs());
    let Some(pairs) = pair_cuts(&design.tiers, &target.tiers, gear, angle_tol_deg) else {
        return false;
    };
    if target_masts.is_empty() {
        return true;
    }
    if !ctx.solved_closed {
        return false;
    }
    let Some(masts) = ctx.solved_masts else {
        return false;
    };
    masts.len() == design.tiers.len()
        && pairs
            .iter()
            .zip(target_masts)
            .all(|(&user, &wanted)| mast_within(masts[user], wanted, mast_rel_tol))
}
