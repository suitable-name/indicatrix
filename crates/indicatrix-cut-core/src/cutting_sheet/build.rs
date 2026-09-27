//! Builds a [`super::CuttingSheet`] from a [`Design`] and its already-solved
//! masts ([`Design::cutting_sheet`] and siblings), and [`Design::facet_meets`]:
//! which other tiers a tier's own `MeetConstraint::MeetNamed` resolves to,
//! using the same [`MeetNameResolver`] the solver itself uses.

use super::sheet::{CutSheetRow, CuttingSheet};
use crate::{
    design::{ConstraintTier, Design, SolveMismatch},
    edit::EditError,
};
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, MeetNameResolver, MeetTierInput, SolvedTier},
    optics::materials::GemMaterial,
};

/// Builds one tier's printable meet instruction -- see
/// [`super::sheet::CutSheetRow::meet_instruction`]. Mirrors
/// `design::export`'s own notes-vs-synthesized precedence (recorded notes
/// text while the constraint is still the pinned import value, else
/// synthesized from the constraint), but only ever accepts non-blank
/// recorded notes and always falls back to a readable phrase for
/// [`MeetConstraint::MeetExisting`] instead of an empty string.
fn meet_instruction(tier: &ConstraintTier) -> String {
    if let (MeetConstraint::ScaleReference(_), Some(notes)) =
        (&tier.constraint, &tier.original_notes)
        && !notes.trim().is_empty()
    {
        return notes.clone();
    }
    match &tier.constraint {
        MeetConstraint::MeetExisting => "Meet at previously cut facets".to_string(),
        MeetConstraint::MeetNamed(names) => format!("Meet {}", names.join(", ")),
        MeetConstraint::ScaleReference(mast) => format!("Set to mast depth {mast:.4}"),
    }
}

/// Builds one [`MeetTierInput`] per tier of `tiers`, for
/// [`MeetNameResolver`] -- the same conversion
/// `indicatrix::geometry::meet_solver::meet_tier_inputs_from_asc` does from a
/// parsed `.asc` schedule, done here directly from a [`Design`]'s own
/// authoritative [`ConstraintTier::constraint`] instead (no solved mast
/// needed: a tier's constraint already carries its `ScaleReference` value
/// when it has one, so this never has to solve first).
fn meet_tier_inputs(tiers: &[ConstraintTier]) -> Vec<MeetTierInput> {
    tiers
        .iter()
        .map(|tier| MeetTierInput {
            angle_deg: tier.angle_deg,
            indices: tier.indices.clone(),
            constraint: tier.constraint.clone(),
            names: tier.names().into_iter().map(str::to_string).collect(),
        })
        .collect()
}

/// Resolves `tier`'s own [`MeetConstraint::MeetNamed`] against an
/// already-built `resolver` -- the shared body of [`Design::facet_meets`]
/// and [`Design::cutting_sheet`], so the latter builds one
/// [`MeetNameResolver`] for the whole design instead of one per row.
/// `MeetExisting`/`ScaleReference` both resolve to an empty list, same as
/// [`Design::facet_meets`] documents.
fn resolve_meets(tier: &ConstraintTier, resolver: &MeetNameResolver<'_>) -> Vec<usize> {
    let MeetConstraint::MeetNamed(names) = &tier.constraint else {
        return Vec::new();
    };
    resolver.resolve_names(names).refs
}

impl Design {
    /// Which other tiers (by index into [`Self::tiers`]) the tier at
    /// `tier_index` actually meets, resolved the same way [`Self::solve`]
    /// itself resolves a `MeetNamed` instruction -- [`MeetNameResolver`], not
    /// a naive name match, so girdle/culet/table fallbacks, side-prefix and
    /// plural stripping, and compound vertex specs all behave identically to
    /// the solver.
    ///
    /// [`MeetConstraint::MeetExisting`] (the solver picks a candidate vertex
    /// without ever naming facets) and [`MeetConstraint::ScaleReference`]
    /// (an authored dimension, not a meet at all) both resolve to an empty
    /// list -- there is nothing named to report for either. Only
    /// [`MeetConstraint::MeetNamed`] has a real answer.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn facet_meets(&self, tier_index: usize) -> Result<Vec<usize>, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let inputs = meet_tier_inputs(&self.tiers);
        let resolver = MeetNameResolver::new(&inputs);
        Ok(resolve_meets(tier, &resolver))
    }

    /// Builds this design's printable cutting sequence from an
    /// already-[`Self::solve`]'d (or [`Self::resolve_dirty`]'d) mast list --
    /// see [`CuttingSheet`] for what it carries and why a cutter needs it.
    /// Never solves again itself, same reasoning as
    /// [`Self::to_asc_schedule_from_solved`].
    ///
    /// Built-ins-only for the printed "Refractive index" line -- see
    /// [`Self::effective_refractive_index`]'s own doc comment. A caller with a
    /// resolved custom catalogue on hand should call [`Self::cutting_sheet_with`]
    /// instead.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::to_asc_schedule_from_solved`]:
    /// `solved` must have one entry per tier `self` currently has, in the
    /// same order.
    #[must_use]
    pub fn cutting_sheet(&self, solved: &[SolvedTier]) -> CuttingSheet {
        self.cutting_sheet_with(solved, &[])
    }

    /// Catalogue-aware sibling of [`Self::cutting_sheet`]: also consults `custom` --
    /// see [`Self::effective_refractive_index_with`] -- so the printed "Refractive
    /// index" line reflects a CUSTOM catalogue material's own `n_D` instead of
    /// silently falling back to the legacy schedule RI. Passing `&[]` behaves
    /// exactly like [`Self::cutting_sheet`] (that method's own implementation, in
    /// fact).
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::cutting_sheet`].
    #[must_use]
    pub fn cutting_sheet_with(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> CuttingSheet {
        self.try_cutting_sheet_with(solved, custom)
            .unwrap_or_else(|e| {
                panic!(
                    "cutting_sheet: `solved` ({} masts) is not aligned with this design's current \
                 {} tier(s)",
                    e.got_tiers, e.expected_tiers
                )
            })
    }

    /// Fallible sibling of [`Self::cutting_sheet`]: returns [`SolveMismatch`]
    /// instead of panicking when `solved` is not aligned with
    /// [`Self::tiers`]. Identical construction otherwise.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_cutting_sheet(&self, solved: &[SolvedTier]) -> Result<CuttingSheet, SolveMismatch> {
        self.try_cutting_sheet_with(solved, &[])
    }

    /// Catalogue-aware, fallible sibling of [`Self::cutting_sheet_with`] -- returns
    /// [`SolveMismatch`] instead of panicking, exactly like
    /// [`Self::try_cutting_sheet`] but also consulting `custom` for the printed
    /// "Refractive index" line (see [`Self::effective_refractive_index_with`]).
    /// Every other cutting-sheet entry point in this impl ultimately calls through
    /// here, so this is the one place that line is actually decided.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_cutting_sheet_with(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> Result<CuttingSheet, SolveMismatch> {
        if solved.len() != self.tiers.len() {
            return Err(SolveMismatch {
                expected_tiers: self.tiers.len(),
                got_tiers: solved.len(),
            });
        }

        let mut header = vec![format!(
            "Material: {}",
            self.material.name.as_deref().unwrap_or("(unset)")
        )];
        header.push(format!(
            "Refractive index: {:.3}",
            self.effective_refractive_index_with(custom)
        ));
        header.push(format!(
            "Index gear: {} teeth, symmetry {}{}",
            self.meta.gear_teeth_abs(),
            self.meta.symmetry_order,
            if self.meta.mirror { ", mirrored" } else { "" }
        ));
        if let Some(mm) = self.girdle_diameter_mm {
            header.push(format!("Girdle diameter: {mm:.3} mm"));
        }
        // Carat weight, with the same understated-weight caveat
        // `crate::yield_metrics::report`'s own module doc comment documents
        // (facets that don't reach the preform's own walls leave `width_axis`
        // reading as the PREFORM's width, not the actual narrower girdle) --
        // shown next to the number instead of only in a doc comment nobody
        // printing a sheet ever reads.
        let yield_report = self.yield_report(solved);
        if let Some(carat) = yield_report.carat_weight {
            header.push(format!(
                "Carat weight (estimate): {carat:.3} ct -- assumes the authored facets reach \
                 the preform's own walls; a stone left smaller than its rough reads lighter \
                 than this"
            ));
        }
        let mm_per_unit = yield_report.mm_per_unit;

        let inputs = meet_tier_inputs(&self.tiers);
        let resolver = MeetNameResolver::new(&inputs);
        let rows = self
            .tiers
            .iter()
            .zip(solved)
            .enumerate()
            .map(|(i, (tier, solved_tier))| CutSheetRow {
                sequence: i + 1,
                name: tier.name.clone(),
                angle_deg: tier.angle_deg,
                indices: tier.indices.clone(),
                mast: solved_tier.mast,
                meet_instruction: meet_instruction(tier),
                meets_tiers: resolve_meets(tier, &resolver),
                cheater_offset_deg: self.cheater_offset_deg(i),
                angle_of_elevation_deg: tier.angle_deg.abs(),
                depth_mm: mm_per_unit.map(|scale| solved_tier.mast * scale),
            })
            .collect();

        Ok(CuttingSheet { header, rows })
    }
}
