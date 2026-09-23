//! [`SolveMismatch`] -- the fallible counterpart to the `previous`/`solved`
//! length-alignment `panic!`s [`super::Design::resolve_dirty`],
//! [`super::Design::to_asc_schedule_from_solved`],
//! [`crate::cutting_sheet`]'s `Design::cutting_sheet` and
//! [`super::Design::planes_through_tier`] raise directly -- and
//! [`DesignSolveError`], the combined error [`super::Design::solve_with`] and
//! [`super::Design::resolve_dirty_with`] return.
//!
//! Each of the four `panic!`ing methods keeps its own exact signature and
//! panic message (so no `apps/**` caller needs to change), implemented as a
//! thin wrapper over a `try_*` sibling that returns [`SolveMismatch`] instead
//! -- see each method's own doc comment.

use super::{MissingAnchor, TargetResolveError};
use indicatrix::geometry::meet_solver::SolveError;

/// A tier list (`previous`/`solved`) was not aligned with `design.tiers`.
///
/// The length mismatch that [`super::Design::resolve_dirty`],
/// [`super::Design::to_asc_schedule_from_solved`],
/// [`super::Design::planes_through_tier`] and `Design::cutting_sheet`
/// (`crate::cutting_sheet`) `panic!` on directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolveMismatch {
    /// `design.tiers.len()` at the time of the call -- what the tier list
    /// needed to have.
    pub expected_tiers: usize,
    /// The length of the tier list the caller actually supplied.
    pub got_tiers: usize,
}

impl std::fmt::Display for SolveMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tier list has {} entries, but this design currently has {} tier(s) -- only valid \
             for a list this exact design (or one that differs only via ModifyTier/\
             SetConstraint) actually produced",
            self.got_tiers, self.expected_tiers
        )
    }
}

impl std::error::Error for SolveMismatch {}

/// Every way [`super::Design::solve_with`]/[`super::Design::resolve_dirty_with`]
/// can fail to produce a mast list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignSolveError {
    /// See [`MissingAnchor`].
    MissingAnchor(MissingAnchor),
    /// `previous` was not aligned with `self.tiers` -- see [`SolveMismatch`].
    /// Only [`super::Design::resolve_dirty_with`] can return this;
    /// [`super::Design::solve_with`] takes no such list.
    Mismatch(SolveMismatch),
    /// The solve itself was cancelled, or exceeded
    /// `indicatrix::geometry::meet_solver`'s plane cap -- see
    /// [`indicatrix::geometry::meet_solver::SolveError`].
    Solve(SolveError),
    /// A [`crate::design::TierTarget`] could not be resolved to a real mast
    /// before solving -- see [`TargetResolveError`]. Only ever returned when
    /// [`super::Design::tier_targets`] is non-empty; every existing design (an
    /// empty map) can never see this variant.
    Target(TargetResolveError),
}

impl std::fmt::Display for DesignSolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingAnchor(e) => write!(f, "{e}"),
            Self::Mismatch(e) => write!(f, "{e}"),
            Self::Solve(e) => write!(f, "{e}"),
            Self::Target(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DesignSolveError {}

impl From<MissingAnchor> for DesignSolveError {
    fn from(e: MissingAnchor) -> Self {
        Self::MissingAnchor(e)
    }
}

impl From<SolveMismatch> for DesignSolveError {
    fn from(e: SolveMismatch) -> Self {
        Self::Mismatch(e)
    }
}

impl From<SolveError> for DesignSolveError {
    fn from(e: SolveError) -> Self {
        Self::Solve(e)
    }
}

impl From<TargetResolveError> for DesignSolveError {
    fn from(e: TargetResolveError) -> Self {
        Self::Target(e)
    }
}
