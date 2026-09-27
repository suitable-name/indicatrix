//! The two public entry points: [`solve_meet_points`] (plain) and
//! [`solve_meet_points_with`] (cancellable, progress-reporting).

use std::collections::BTreeMap;

use super::{
    super::{MAX_PLANES, MeetTierInput, SolveControl, SolveError, SolvedTier},
    context::SolveContext,
};

/// Solves every tier's mast distance.
///
/// See the module docs for the model (vertex incidence), what the caller must
/// anchor (one scale reference per crown/pavilion/girdle block), and the
/// three-phase algorithm.
///
/// `gear_teeth_abs` is the index wheel's tooth count (see
/// [`indicatrix_formats::asc::AscSchedule::gear_teeth_abs`]). `tiers` should be in the
/// schedule's own file order (needed only to resolve an unsigned-zero angle's
/// crown/pavilion side); the solve itself is order-independent.
///
/// Deterministic: two calls with identical inputs produce identical outputs. No
/// hashed iteration and no convex-hull library anywhere in this path.
///
/// Latency: `O(P^3)` per refinement sweep in the plane count `P`; see the
/// module docs' "Cost envelope" section for the measured figures. Never
/// cancels and reports no progress -- see [`solve_meet_points_with`] for
/// that; this is a thin wrapper passing a no-op [`SolveControl`], and above
/// [`MAX_PLANES`] it keeps the legacy behavior of returning an all-
/// [`SolveStrategy::Failed`](super::super::SolveStrategy::Failed) result instead of an error.
#[must_use]
pub fn solve_meet_points(gear_teeth_abs: u32, tiers: &[MeetTierInput]) -> Vec<SolvedTier> {
    match solve_meet_points_with(gear_teeth_abs, tiers, &SolveControl::default()) {
        Ok(solved) => solved,
        Err(SolveError::TooManyPlanes { .. }) => {
            SolveContext::new(gear_teeth_abs, tiers).failed_solved()
        }
        Err(SolveError::Cancelled) => {
            unreachable!("SolveControl::default() never sets cancel")
        }
    }
}

/// Cancellable, progress-reporting sibling of [`solve_meet_points`].
///
/// Same algorithm and same determinism guarantee: an unused `control`, i.e.
/// [`SolveControl::default`], reproduces [`solve_meet_points`] exactly, bit
/// for bit. Checks `control` at every cancel point (see the module docs,
/// "Cancellation and progress") and reports a [`SolveProgress`](super::super::SolveProgress)
/// at every point that has one.
///
/// # Errors
///
/// [`SolveError::TooManyPlanes`] when the design has more than [`MAX_PLANES`]
/// facet-plane instances (checked before any solving starts, so this returns
/// immediately); [`SolveError::Cancelled`] the first time `control`'s cancel
/// flag is observed set.
pub fn solve_meet_points_with(
    gear_teeth_abs: u32,
    tiers: &[MeetTierInput],
    control: &SolveControl<'_>,
) -> Result<Vec<SolvedTier>, SolveError> {
    let ctx = SolveContext::new(gear_teeth_abs, tiers);
    if ctx.total_planes > MAX_PLANES {
        return Err(SolveError::TooManyPlanes {
            planes: ctx.total_planes,
            max: MAX_PLANES,
        });
    }
    if control.is_cancelled() {
        return Err(SolveError::Cancelled);
    }
    let result = ctx.run_pipeline(&BTreeMap::new(), &BTreeMap::new(), control)?;
    Ok(ctx.to_solved(&result))
}
