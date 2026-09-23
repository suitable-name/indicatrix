//! [`Design::solve`] and [`Design::resolve_dirty`] -- deriving every tier's
//! mast from its authored [`MeetConstraint`], whole-design or subgraph. See
//! the parent module's doc comment ("Scale anchoring") for the missing-anchor
//! failure mode both gate on via [`missing_anchor_blocks`].

use super::{Design, DesignSolveError, MissingAnchor, SolveMismatch};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, MeetTierInput, SolveControl, SolveError, SolvedTier, classify_blocks,
    solve_meet_points, solve_meet_points_with,
};

impl Design {
    /// Solves every tier's mast from its authored [`MeetConstraint`] via
    /// `indicatrix::geometry::meet_solver::solve_meet_points` -- the plain,
    /// no-external-verification entry point; the corpus's
    /// `solve_meet_points_verified` repair search costs tens to hundreds of pipeline
    /// re-runs per design, appropriate for offline corpus validation, not an
    /// editor's edit loop.
    ///
    /// # Errors
    ///
    /// [`DesignSolveError::MissingAnchor`] naming every block that has tiers but
    /// no explicit [`MeetConstraint::ScaleReference`] among them, rather than
    /// letting the solver's own internal `DEFAULT_PLAUSIBLE_SCALE` fallback
    /// silently stand in for a real dimension -- see the module docs.
    /// [`DesignSolveError::Target`] when [`Self::tier_targets`] is non-empty and
    /// a target could not be resolved (missing girdle diameter, or a target the
    /// bisection search in `crate::design::targets` could not bracket); every
    /// existing design has an empty `tier_targets`, so this can only be reached
    /// by a caller that has started authoring [`crate::design::TierTarget`]s.
    /// See [`DesignSolveError`]'s own [`std::fmt::Display`] impl for the
    /// status-strip sentence each case renders as.
    pub fn solve(&self) -> Result<Vec<SolvedTier>, DesignSolveError> {
        match self.solve_with(&SolveControl::default()) {
            Ok(solved) => Ok(solved),
            // Legacy behavior above MAX_PLANES: solve_meet_points itself (not
            // solve_meet_points_with) returns an all-`Failed` result rather than
            // an error -- reproduce that exactly instead of surfacing the new
            // `_with`-only error variant here.
            Err(DesignSolveError::Solve(SolveError::TooManyPlanes { .. })) => Ok(
                solve_meet_points(self.meta.gear_teeth_abs(), &self.meet_tier_inputs()),
            ),
            Err(DesignSolveError::Solve(SolveError::Cancelled)) => {
                unreachable!("SolveControl::default() never sets cancel")
            }
            Err(DesignSolveError::Mismatch(_)) => {
                unreachable!("solve_with takes no previous/solved list, so never mismatches")
            }
            Err(e @ (DesignSolveError::MissingAnchor(_) | DesignSolveError::Target(_))) => Err(e),
        }
    }

    /// Cancellable, progress-reporting sibling of [`Self::solve`] -- same
    /// derivation and same determinism guarantee (an unused `control`
    /// reproduces the mast list [`Self::solve`] would produce), but checks
    /// `control` at every cancel point `indicatrix::geometry::meet_solver`
    /// exposes and reports its progress -- see
    /// `indicatrix::geometry::meet_solver::solve_meet_points_with`'s own doc
    /// comment for exactly where.
    ///
    /// # Errors
    ///
    /// [`DesignSolveError::MissingAnchor`] under the same condition as
    /// [`Self::solve`] (checked first, before any solving starts);
    /// [`DesignSolveError::Solve`] when the underlying solve is cancelled or
    /// exceeds `indicatrix::geometry::meet_solver`'s plane cap -- unlike
    /// [`Self::solve`], this surfaces the plane-cap case as a real error
    /// instead of silently falling back to an all-`Failed` result.
    pub fn solve_with(
        &self,
        control: &SolveControl<'_>,
    ) -> Result<Vec<SolvedTier>, DesignSolveError> {
        let inputs = self.resolved_meet_tier_inputs()?;
        let missing = missing_anchor_blocks(&inputs);
        if !missing.is_empty() {
            return Err(DesignSolveError::MissingAnchor(MissingAnchor {
                blocks: missing,
            }));
        }
        Ok(solve_meet_points_with(
            self.meta.gear_teeth_abs(),
            &inputs,
            control,
        )?)
    }

    /// Re-solves only the tiers that could actually change as a result of an edit
    /// touching `dirty` (see [`crate::resolve`]'s module docs for what "could
    /// actually change" means here -- wider than a naive dependency graph would
    /// suggest), reusing `previous`'s mast for every other tier.
    ///
    /// # The substitution
    ///
    /// [`solve_meet_points`] takes every tier and returns every tier -- there is no
    /// separate "solve a subgraph" entry point, and this crate deliberately does not
    /// add one (see [`crate::resolve`]'s module docs). Instead, every tier index
    /// [`crate::resolve::affected_tiers`] did *not* mark gets its [`MeetConstraint`]
    /// temporarily replaced with [`MeetConstraint::ScaleReference`] at `previous`'s
    /// mast, and the ordinary [`solve_meet_points`] runs over the whole (partially
    /// substituted) list -- a substituted tier's own returned mast is trivially
    /// `previous`'s value back unchanged, so the single returned `Vec` already has
    /// both kept and re-solved masts, index-for-index.
    ///
    /// # Why the missing-anchor check runs on the REAL constraints first
    ///
    /// Substituting every unaffected tier with a `ScaleReference` would, on its own,
    /// make [`missing_anchor_blocks`] spuriously report every block as anchored,
    /// hiding a real [`MissingAnchor`] a full [`Self::solve`] on the same edited
    /// design *would* report. So this method checks [`missing_anchor_blocks`]
    /// against `self`'s real, unsubstituted [`Self::meet_tier_inputs`] first (the
    /// same predicate [`Self::solve`] runs, factored out so the two can never drift
    /// apart) and only substitutes afterward.
    ///
    /// # Panics
    ///
    /// `previous` must have one entry per tier `self` currently has, in the same
    /// order -- a prior [`Self::solve`]/[`Self::resolve_dirty`] result for a design
    /// identical to `self` except at the positions `dirty` names. That holds for
    /// [`Edit::ModifyTier`](crate::edit::Edit::ModifyTier)/
    /// [`Edit::SetConstraint`](crate::edit::Edit::SetConstraint) but never
    /// [`Edit::AddTier`](crate::edit::Edit::AddTier)/
    /// [`Edit::RemoveTier`](crate::edit::Edit::RemoveTier), which is why
    /// [`crate::resolve::resolve_after_edit`] always calls [`Self::solve`] instead
    /// for those two.
    ///
    /// # Panics
    ///
    /// Panics if `previous` is not aligned with [`Self::tiers`] -- see
    /// [`Self::resolve_dirty_with`]'s [`DesignSolveError::Mismatch`] for the
    /// same condition surfaced as a real `Err` instead.
    ///
    /// # Errors
    ///
    /// [`DesignSolveError::MissingAnchor`] -- see "Why the missing-anchor
    /// check..." above. [`DesignSolveError::Target`] under the same condition
    /// [`Self::solve`]'s own `# Errors` section documents.
    pub fn resolve_dirty(
        &self,
        previous: &[SolvedTier],
        dirty: &std::collections::BTreeSet<usize>,
    ) -> Result<Vec<SolvedTier>, DesignSolveError> {
        match self.resolve_dirty_with(previous, dirty, &SolveControl::default()) {
            Ok(solved) => Ok(solved),
            Err(DesignSolveError::Mismatch(m)) => panic!(
                "resolve_dirty: `previous` ({} masts) is not aligned with this design's current \
                 {} tier(s) -- only valid after an index-preserving edit (ModifyTier/\
                 SetConstraint); AddTier/RemoveTier must use Self::solve instead",
                m.got_tiers, m.expected_tiers
            ),
            // Legacy behavior above MAX_PLANES: see the matching arm on `Self::solve`.
            Err(DesignSolveError::Solve(SolveError::TooManyPlanes { .. })) => {
                let substituted = self.substituted_inputs(previous, dirty);
                Ok(solve_meet_points(self.meta.gear_teeth_abs(), &substituted))
            }
            Err(DesignSolveError::Solve(SolveError::Cancelled)) => {
                unreachable!("SolveControl::default() never sets cancel")
            }
            Err(e @ (DesignSolveError::MissingAnchor(_) | DesignSolveError::Target(_))) => Err(e),
        }
    }

    /// Cancellable, progress-reporting sibling of [`Self::resolve_dirty`] --
    /// same substitution and same determinism guarantee, but returns
    /// [`DesignSolveError::Mismatch`] instead of panicking when `previous` is
    /// not aligned with [`Self::tiers`], and checks `control` at every cancel
    /// point the underlying solve exposes -- see [`Self::solve_with`]'s doc
    /// comment.
    ///
    /// # Errors
    ///
    /// [`DesignSolveError::Mismatch`] when `previous.len() != self.tiers.len()`
    /// (checked first, before anything else -- same order [`Self::resolve_dirty`]'s
    /// `panic!` runs in directly); [`DesignSolveError::MissingAnchor`] under the
    /// same condition as [`Self::solve`]; [`DesignSolveError::Solve`] when the
    /// underlying solve is cancelled or exceeds the plane cap (surfaced as a
    /// real error here, unlike [`Self::resolve_dirty`] -- see
    /// [`Self::solve_with`]'s doc comment on the same distinction).
    pub fn resolve_dirty_with(
        &self,
        previous: &[SolvedTier],
        dirty: &std::collections::BTreeSet<usize>,
        control: &SolveControl<'_>,
    ) -> Result<Vec<SolvedTier>, DesignSolveError> {
        if previous.len() != self.tiers.len() {
            return Err(DesignSolveError::Mismatch(SolveMismatch {
                expected_tiers: self.tiers.len(),
                got_tiers: previous.len(),
            }));
        }
        // Resolved (not raw) inputs: a tier target -- like a real authored
        // `ScaleReference` -- still needs converting to a mast before the
        // missing-anchor check and the dirty-substitution below can reason about
        // it. A no-op, byte-identical to `self.meet_tier_inputs()`, whenever
        // `self.tier_targets` is empty -- see `crate::design::targets`'s module
        // docs.
        let inputs = self.resolved_meet_tier_inputs()?;
        let missing = missing_anchor_blocks(&inputs);
        if !missing.is_empty() {
            return Err(DesignSolveError::MissingAnchor(MissingAnchor {
                blocks: missing,
            }));
        }

        let substituted = Self::substitute_inputs(inputs, previous, dirty);
        Ok(solve_meet_points_with(
            self.meta.gear_teeth_abs(),
            &substituted,
            control,
        )?)
    }

    /// [`Self::resolve_dirty`]'s own legacy plane-cap fallback: builds `inputs`
    /// from `self`'s RAW (not target-resolved) constraints -- see
    /// [`Self::substitute_inputs`]'s own doc comment for why `resolve_dirty_with`
    /// does not use this exact helper.
    fn substituted_inputs(
        &self,
        previous: &[SolvedTier],
        dirty: &std::collections::BTreeSet<usize>,
    ) -> Vec<MeetTierInput> {
        Self::substitute_inputs(self.meet_tier_inputs(), previous, dirty)
    }

    /// Shared by [`Self::resolve_dirty_with`] and [`Self::substituted_inputs`]:
    /// every tier [`crate::resolve::affected_tiers`] did NOT mark gets its
    /// [`MeetConstraint`] temporarily replaced with [`MeetConstraint::ScaleReference`]
    /// at `previous`'s mast -- see [`Self::resolve_dirty`]'s doc comment, "The
    /// substitution". Takes `inputs` already built (rather than deriving them
    /// itself) so [`Self::resolve_dirty_with`] can pass its own
    /// [`Self::resolved_meet_tier_inputs`] result through without resolving
    /// targets twice.
    fn substitute_inputs(
        inputs: Vec<MeetTierInput>,
        previous: &[SolvedTier],
        dirty: &std::collections::BTreeSet<usize>,
    ) -> Vec<MeetTierInput> {
        let affected = crate::resolve::affected_tiers(&inputs, dirty);
        inputs
            .into_iter()
            .enumerate()
            .map(|(i, mut input)| {
                if !affected.contains(&i) {
                    input.constraint = MeetConstraint::ScaleReference(previous[i].mast);
                }
                input
            })
            .collect()
    }

    /// This design's tiers as [`MeetTierInput`]s, for [`Self::solve`] and for a
    /// caller that wants to run a different `indicatrix::geometry::meet_solver`
    /// entry point directly, e.g. `indicatrix-cut`'s "Deep Solve" action
    /// (`solve_meet_points_verified`), which this crate deliberately does not wrap
    /// itself (see [`Self::solve`]). `pub` for exactly that reason -- read-only, so
    /// exposing it adds no way to mutate a [`Design`] outside [`crate::edit::History`].
    #[must_use]
    pub fn meet_tier_inputs(&self) -> Vec<MeetTierInput> {
        self.tiers
            .iter()
            .map(|t| MeetTierInput {
                angle_deg: t.angle_deg,
                indices: t.indices.clone(),
                constraint: t.constraint.clone(),
                names: t.names().into_iter().map(str::to_string).collect(),
            })
            .collect()
    }
}

/// Every crown/pavilion/girdle block present among `inputs` that has no explicit
/// [`MeetConstraint::ScaleReference`] tier of its own -- the exact predicate
/// [`Design::solve`] and [`Design::resolve_dirty`] both gate on, factored out to one
/// place so the two can never check different things.
fn missing_anchor_blocks(inputs: &[MeetTierInput]) -> Vec<Block> {
    let blocks = classify_blocks(inputs);
    [Block::Crown, Block::Pavilion, Block::Girdle]
        .into_iter()
        .filter(|&block| {
            let present = blocks.contains(&block);
            let anchored = inputs.iter().zip(&blocks).any(|(t, &b)| {
                b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_))
            });
            present && !anchored
        })
        .collect()
}
