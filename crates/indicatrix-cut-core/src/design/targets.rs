//! [`TierTarget`]: authoring-level constraint kinds ("cut to 3.20 mm", "girdle
//! 2% of width", "table 4.10 mm wide") that resolve to a real
//! [`MeetConstraint::ScaleReference`] before solving, instead of a cutter
//! authoring a dimensionless mast directly.
//!
//! # Why these are not [`MeetConstraint`] variants
//!
//! [`MeetConstraint`] is owned by `indicatrix`'s geometry crate (the meet solver),
//! which this crate does not modify. [`TierTarget`] instead lives in
//! [`super::Design::tier_targets`], a `BTreeMap<TierId, TierTarget>` -- the exact
//! same "`Design`-level map, not a [`super::ConstraintTier`] field" pattern
//! [`super::Design::cheater_offsets_deg`] already uses, and for the same reason
//! (widening `ConstraintTier` would be a breaking change to dozens of construction
//! sites outside this crate). Keyed by [`super::TierId`] rather than position, so a
//! target survives `AddTier`/`RemoveTier`/`MoveTier` the same way
//! [`super::Design::tier_ids`] itself does.
//!
//! # Resolution
//!
//! [`Design::resolved_meet_tier_inputs`] is the one place a [`TierTarget`] ever
//! turns into a mast: called from [`super::Design::solve_with`]/
//! [`super::Design::resolve_dirty_with`] in place of the raw
//! [`super::Design::meet_tier_inputs`] whenever [`super::Design::tier_targets`] is
//! non-empty (a no-op, `self.meet_tier_inputs()` unchanged, whenever it is empty --
//! every existing design has an empty map, so this changes nothing for them).
//!
//! - [`TierTarget::DepthMm`] resolves in one correction pass: solve once with the
//!   target tier bootstrapped at mast `0.0` to measure the design's own
//!   `width_axis`, convert `depth_mm` to a mast via
//!   `crate::yield_metrics::mm_per_unit`, then use that mast as the tier's real
//!   `ScaleReference`. Not iterated to convergence -- see the method's own doc
//!   comment for why one pass is enough in practice and where it is not exact.
//! - [`TierTarget::GirdleThicknessMm`]/[`TierTarget::TableWidthMm`] resolve by
//!   bisecting the target tier's own mast (0 to a generous bound, at most 24
//!   iterations, tolerance `1e-4` of the girdle diameter) until the resulting
//!   solid's measured girdle thickness/table width lands within tolerance of the
//!   target, in millimetres. [`TargetResolveError::CannotBracket`] when the two
//!   bracket ends do not straddle the target (e.g. a target wider than the
//!   preform itself allows).
//!
//! # Legacy entry points resolve targets too
//!
//! [`super::Design::solve`]/[`super::Design::resolve_dirty`] call through
//! [`super::Design::solve_with`]/[`super::Design::resolve_dirty_with`]
//! internally, so they benefit from target resolution exactly like those `_with`
//! entry points. Both wrappers' `Err` type is
//! [`super::DesignSolveError`] (not the narrower [`super::MissingAnchor`] their
//! signatures used before this module existed) precisely so a resolution failure
//! -- missing girdle diameter, or a target that cannot be bracketed -- has
//! somewhere real to go instead of panicking; see [`TargetResolveError`]'s own
//! [`std::fmt::Display`] impl for the status-strip sentence each variant renders
//! as.

use super::{Design, DesignSolveError, TierId};
use crate::yield_metrics::mm_per_unit;
use indicatrix::geometry::{
    meet_solver::{MeetConstraint, MeetTierInput, SolveControl},
    stone_metrics::{SolidMetrics, SolidStatus, StoneProportions, build_solid_mesh, measure_solid},
};
use std::collections::BTreeMap;

/// One authoring-level target a cutter states in real units, resolved to a
/// [`MeetConstraint::ScaleReference`] before solving -- see the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TierTarget {
    /// "Cut this facet to depth `_` mm" -- the tier's own mast, converted via
    /// [`crate::yield_metrics::mm_per_unit`].
    DepthMm(f64),
    /// "This design's girdle should be `_` mm thick" -- bisects the target
    /// tier's own mast until [`StoneProportions::girdle_thickness`] (converted
    /// to mm) matches. Ordinarily authored on the tier
    /// `indicatrix::geometry::meet_solver::classify_blocks` would call the
    /// girdle (angle at or near 90 degrees), but this resolves the same way
    /// regardless of which tier carries it.
    GirdleThicknessMm(f64),
    /// "This design's table should be `_` mm wide" -- bisects the target
    /// tier's own mast until the table facet's own measured width (derived
    /// from [`StoneProportions::table_percent`] and the solid's own
    /// `width_axis`, converted to mm) matches.
    TableWidthMm(f64),
}

/// Every way [`Design::resolved_meet_tier_inputs`] can fail to turn
/// [`Design::tier_targets`] into real masts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetResolveError {
    /// A [`TierTarget`] is set but [`Design::girdle_diameter_mm`] is not -- there
    /// is no mm-per-unit scale to convert a millimetre target through. Surfaced
    /// as the status-strip problem text.
    MissingGirdleDiameter,
    /// [`TierTarget::GirdleThicknessMm`]/[`TierTarget::TableWidthMm`]'s bisection
    /// could not bracket the target within its search bound, or the design
    /// stopped measuring as a closed solid partway through the search --
    /// nothing this crate should loop forever chasing.
    CannotBracket { tier_index: usize },
    /// [`TierTarget::DepthMm`]'s bootstrap pass solved, but the design did not
    /// measure as a real closed solid (or measured a non-positive width), so
    /// there is no `width_axis` to derive a scale from.
    CannotMeasure { tier_index: usize },
    /// The bootstrap or bisection solve itself failed (a missing anchor
    /// elsewhere in the design, cancellation, or the solver's own plane cap).
    Anchor(Box<DesignSolveError>),
}

impl std::fmt::Display for TargetResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingGirdleDiameter => write!(
                f,
                "cut-to-depth needs a girdle diameter -- set one in Design settings"
            ),
            Self::CannotBracket { tier_index } => write!(
                f,
                "tier {}'s target could not be bracketed -- widen or remove it",
                tier_index + 1
            ),
            Self::CannotMeasure { tier_index } => write!(
                f,
                "tier {}'s target could not be resolved: the design does not currently measure \
                 as a closed solid",
                tier_index + 1
            ),
            Self::Anchor(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TargetResolveError {}

impl From<DesignSolveError> for TargetResolveError {
    fn from(e: DesignSolveError) -> Self {
        Self::Anchor(Box::new(e))
    }
}

/// Millimetre extent of one measured figure [`StoneProportions`] carries, for
/// [`bisect_tier_mast`]'s generic bracket search -- `girdle_thickness`/table width
/// share this shape (a mast-unit quantity derived from a solved-and-measured
/// candidate, converted to mm by the caller).
type MeasureFn = fn(&StoneProportions, &SolidMetrics) -> Option<f64>;

const fn girdle_thickness_mast(
    proportions: &StoneProportions,
    _metrics: &SolidMetrics,
) -> Option<f64> {
    proportions.girdle_thickness
}

fn table_width_mast(proportions: &StoneProportions, metrics: &SolidMetrics) -> Option<f64> {
    proportions
        .table_percent
        .map(|pct| pct / 100.0 * metrics.width_axis)
}

/// Solves `design` with tier `tier_index` pinned to `mast`, and returns
/// `extract`'s figure converted to millimetres via `girdle_mm`/the resulting
/// `width_axis` -- `None` for anything that stops this from being measurable
/// (solve failure, non-closed solid, non-positive width).
fn measured_value_mm(
    design: &Design,
    tier_index: usize,
    mast: f64,
    girdle_mm: f64,
    extract: MeasureFn,
) -> Option<f64> {
    let mut trial = design.clone();
    trial.tiers[tier_index].constraint = MeetConstraint::ScaleReference(mast);
    let solved = trial.solve_with(&SolveControl::default()).ok()?;
    let planes = trial.planes_from_solved(&solved);
    let metrics = measure_solid(&planes)?;
    if metrics.width_axis <= 1e-9 {
        return None;
    }
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return None;
    };
    let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
    let value_mast = extract(&proportions, &metrics)?;
    let scale = mm_per_unit(girdle_mm, metrics.width_axis)?;
    Some(value_mast * scale)
}

/// Bisects tier `tier_index`'s own mast (bracket `[0, hi]`, `hi` a generous
/// multiple of the preform's own largest plane offset) until `extract`'s
/// measured figure lands within `1e-4 * girdle_mm` of `target_mm` -- at most 24
/// iterations.
fn bisect_tier_mast(
    design: &Design,
    tier_index: usize,
    girdle_mm: f64,
    target_mm: f64,
    extract: MeasureFn,
) -> Result<f64, TargetResolveError> {
    let hi_bound = design
        .preform
        .planes()
        .iter()
        .fold(0.0_f64, |acc, &(_, m)| acc.max(m))
        .mul_add(4.0, 1.0);
    let bracket_err = || TargetResolveError::CannotBracket { tier_index };

    let lo_val =
        measured_value_mm(design, tier_index, 0.0, girdle_mm, extract).ok_or_else(bracket_err)?;
    let hi_val = measured_value_mm(design, tier_index, hi_bound, girdle_mm, extract)
        .ok_or_else(bracket_err)?;
    if (lo_val - target_mm) * (hi_val - target_mm) > 0.0 {
        return Err(bracket_err());
    }

    let tolerance = 1e-4 * girdle_mm.abs().max(1e-6);
    let mut lo = 0.0_f64;
    let mut lo_value = lo_val;
    let mut hi = hi_bound;
    for _ in 0..24 {
        let mid = f64::midpoint(lo, hi);
        let mid_val = measured_value_mm(design, tier_index, mid, girdle_mm, extract)
            .ok_or_else(bracket_err)?;
        if (mid_val - target_mm).abs() <= tolerance {
            return Ok(mid);
        }
        if (mid_val - target_mm).signum() == (lo_value - target_mm).signum() {
            lo = mid;
            lo_value = mid_val;
        } else {
            hi = mid;
        }
    }
    Ok(f64::midpoint(lo, hi))
}

impl Design {
    /// The [`TierTarget`] authored for the tier CURRENTLY at `index`, if any --
    /// looked up via [`Self::tier_ids`], the same positional-to-stable-id
    /// translation [`Self::cheater_offset_deg`]/[`Self::tier_note`] use for their
    /// own maps.
    #[must_use]
    pub fn tier_target(&self, index: usize) -> Option<TierTarget> {
        self.tier_ids
            .get(index)
            .and_then(|id| self.tier_targets.get(id))
            .copied()
    }

    /// The [`TierTarget`] for a specific [`TierId`], regardless of that tier's
    /// current position -- for a caller (e.g. [`Self::resolved_meet_tier_inputs`])
    /// iterating `tier_ids` directly.
    #[must_use]
    pub fn tier_target_for_id(&self, id: TierId) -> Option<TierTarget> {
        self.tier_targets.get(&id).copied()
    }

    /// [`Self::meet_tier_inputs`], but with every tier that carries a
    /// [`TierTarget`] resolved to a real [`MeetConstraint::ScaleReference`] first
    /// -- see the module docs for the resolution order (depth targets, then
    /// bisected girdle-thickness/table-width targets) and why this is a no-op,
    /// `self.meet_tier_inputs()` returned unchanged, whenever
    /// [`Self::tier_targets`] is empty.
    ///
    /// # Errors
    ///
    /// [`TargetResolveError`] -- see that type's own variants.
    pub fn resolved_meet_tier_inputs(&self) -> Result<Vec<MeetTierInput>, TargetResolveError> {
        if self.tier_targets.is_empty() {
            return Ok(self.meet_tier_inputs());
        }
        let girdle_mm = self
            .girdle_diameter_mm
            .ok_or(TargetResolveError::MissingGirdleDiameter)?;

        // Bootstrap: every target-bearing tier temporarily pinned at mast 0.0,
        // solved once so a `DepthMm` target has a real `width_axis` to convert
        // through. `tier_targets` is cleared on the clone FIRST -- critical, not
        // cosmetic: `Design::clone` carries `tier_targets` over verbatim, and
        // `Design::solve_with` calls back into this very method, so a clone that
        // still had entries would recurse into another bootstrap pass forever
        // instead of taking the (now-correct) empty-map fast path.
        let mut bootstrap = self.clone();
        bootstrap.tier_targets.clear();
        for index in 0..bootstrap.tiers.len() {
            if self.tier_target(index).is_some() {
                bootstrap.tiers[index].constraint = MeetConstraint::ScaleReference(0.0);
            }
        }
        let bootstrap_solved = bootstrap.solve_with(&SolveControl::default())?;
        let bootstrap_metrics = measure_solid(&bootstrap.planes_from_solved(&bootstrap_solved));

        let mut resolved = bootstrap;
        for index in 0..resolved.tiers.len() {
            if let Some(TierTarget::DepthMm(target_mm)) = self.tier_target(index) {
                let width_axis = bootstrap_metrics
                    .as_ref()
                    .map(|m| m.width_axis)
                    .filter(|w| *w > 1e-9)
                    .ok_or(TargetResolveError::CannotMeasure { tier_index: index })?;
                let scale = mm_per_unit(girdle_mm, width_axis)
                    .ok_or(TargetResolveError::CannotMeasure { tier_index: index })?;
                resolved.tiers[index].constraint =
                    MeetConstraint::ScaleReference(target_mm / scale);
            }
        }

        for index in 0..resolved.tiers.len() {
            let mast = match self.tier_target(index) {
                Some(TierTarget::GirdleThicknessMm(target_mm)) => Some(bisect_tier_mast(
                    &resolved,
                    index,
                    girdle_mm,
                    target_mm,
                    girdle_thickness_mast,
                )?),
                Some(TierTarget::TableWidthMm(target_mm)) => Some(bisect_tier_mast(
                    &resolved,
                    index,
                    girdle_mm,
                    target_mm,
                    table_width_mast,
                )?),
                _ => None,
            };
            if let Some(mast) = mast {
                resolved.tiers[index].constraint = MeetConstraint::ScaleReference(mast);
            }
        }

        Ok(resolved.meet_tier_inputs())
    }
}

/// Every tier's [`TierTarget`], keyed by [`TierId`] -- see the module docs.
/// `pub`, not `pub(crate)`: `targets` (this module) is itself private, so the
/// two are equivalent in visibility -- clippy's own `redundant_pub_crate`
/// prefers the plain form in that case.
pub type TierTargetMap = BTreeMap<TierId, TierTarget>;
