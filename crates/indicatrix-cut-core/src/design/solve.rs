//! [`Design::solve`] and [`Design::resolve_dirty`] -- deriving every tier's
//! mast from its authored [`MeetConstraint`], whole-design or subgraph. See
//! the parent module's doc comment ("Scale anchoring") for the missing-anchor
//! failure mode both gate on via [`missing_anchor_blocks`].

use super::{Design, MissingAnchor};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, MeetTierInput, SolvedTier, classify_blocks, solve_meet_points,
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
    /// Returns [`MissingAnchor`] naming every block that has tiers but no
    /// explicit [`MeetConstraint::ScaleReference`] among them, rather than
    /// letting the solver's own internal `DEFAULT_PLAUSIBLE_SCALE` fallback
    /// silently stand in for a real dimension -- see the module docs.
    pub fn solve(&self) -> Result<Vec<SolvedTier>, MissingAnchor> {
        let inputs = self.meet_tier_inputs();
        let missing = missing_anchor_blocks(&inputs);
        if !missing.is_empty() {
            return Err(MissingAnchor { blocks: missing });
        }
        Ok(solve_meet_points(self.meta.gear_teeth_abs(), &inputs))
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
    /// for those two. Mismatched lengths panic rather than silently indexing the
    /// wrong tier's "previous" mast into a substitution.
    ///
    /// # Errors
    ///
    /// See "Why the missing-anchor check..." above.
    pub fn resolve_dirty(
        &self,
        previous: &[SolvedTier],
        dirty: &std::collections::BTreeSet<usize>,
    ) -> Result<Vec<SolvedTier>, MissingAnchor> {
        assert_eq!(
            previous.len(),
            self.tiers.len(),
            "resolve_dirty: `previous` ({} masts) is not aligned with this design's current \
             {} tier(s) -- only valid after an index-preserving edit (ModifyTier/SetConstraint); \
             AddTier/RemoveTier must use Self::solve instead",
            previous.len(),
            self.tiers.len()
        );
        let inputs = self.meet_tier_inputs();
        let missing = missing_anchor_blocks(&inputs);
        if !missing.is_empty() {
            return Err(MissingAnchor { blocks: missing });
        }

        let affected = crate::resolve::affected_tiers(&inputs, dirty);

        let substituted: Vec<MeetTierInput> = inputs
            .into_iter()
            .enumerate()
            .map(|(i, mut input)| {
                if !affected.contains(&i) {
                    input.constraint = MeetConstraint::ScaleReference(previous[i].mast);
                }
                input
            })
            .collect();

        Ok(solve_meet_points(self.meta.gear_teeth_abs(), &substituted))
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
