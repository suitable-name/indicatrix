//! Builds a [`super::CuttingSheet`] from a [`Design`] and its already-solved
//! masts ([`Design::cutting_sheet`] and siblings), and [`Design::facet_meets`]:
//! which other tiers a tier's own `MeetConstraint::MeetNamed` resolves to,
//! using the same [`MeetNameResolver`] the solver itself uses.

use super::sheet::{ConcaveRowInfo, CutSheetRow, CuttingSheet};
use crate::{
    design::{
        ConcaveTier, ConstraintTier, Design, SolveMismatch, TierLabelInfo, TierRef,
        cutting_order::meet_inputs,
    },
    edit::EditError,
};
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, MeetNameResolver, SolvedTier, TokenResolution},
    optics::materials::GemMaterial,
};

/// One name of a `MeetNamed` instruction as the sheet prints it: the code of the tier it
/// names (`Pavilion Main` becomes `P1`), `-`-joined for a compound vertex spec
/// (`1-2-G1` becomes `P1-P2-G1`). A word that names no tier (a meet-point word such as
/// `PCP`, connective prose, a name that does not resolve) stays as the designer wrote it.
fn meet_name_as_code(
    name: &str,
    resolver: &MeetNameResolver<'_>,
    codes: &[TierLabelInfo],
) -> String {
    if let TokenResolution::Tiers(tiers) = resolver.resolve_token(name) {
        let parts: Option<Vec<&str>> = tiers
            .iter()
            .map(|&tier| codes.get(tier).map(|label| label.code.as_str()))
            .collect();
        if let Some(parts) = parts.filter(|parts| !parts.is_empty()) {
            return parts.join("-");
        }
    }
    name.trim().to_owned()
}

/// Builds one tier's printable meet instruction -- see
/// [`super::sheet::CutSheetRow::meet_instruction`]. Mirrors
/// `design::export`'s own notes-vs-synthesized precedence (recorded notes
/// text while the constraint is still the pinned import value, else
/// synthesized from the constraint), but only ever accepts non-blank
/// recorded notes and always falls back to a readable phrase for
/// [`MeetConstraint::MeetExisting`] instead of an empty string.
///
/// A recorded note is the designer's own text and prints verbatim. `MeetNamed`
/// targets print as their tiers' `codes` (`Meet P1, P2, G1`), in the order the
/// constraint stores them.
pub(super) fn meet_instruction(
    tier: &ConstraintTier,
    resolver: &MeetNameResolver<'_>,
    codes: &[TierLabelInfo],
) -> String {
    if let (MeetConstraint::ScaleReference(_), Some(notes)) =
        (&tier.constraint, &tier.original_notes)
        && !notes.trim().is_empty()
    {
        return notes.clone();
    }
    match &tier.constraint {
        MeetConstraint::MeetExisting => "Meet at previously cut facets".to_string(),
        MeetConstraint::MeetNamed(names) => format!(
            "Meet {}",
            names
                .iter()
                .map(|name| meet_name_as_code(name, resolver, codes))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        MeetConstraint::ScaleReference(mast) => format!("Set to mast depth {mast:.4}"),
    }
}

/// A concave tier's facet-line row. It has no mast and no depth (its cut is
/// bounded by the tool, not by a solved plane), so both read as zero or absent
/// and the meet column prints the tier's own instructions, else "cut to depth"; the tool line's values ride in
/// [`CutSheetRow::concave`]. `code` is the tier's code (`P4`).
fn concave_row(tier: &ConcaveTier, code: &str, sequence: usize) -> CutSheetRow {
    CutSheetRow {
        sequence,
        code: code.to_owned(),
        name: tier.name.clone(),
        angle_deg: tier.angle_deg.abs(),
        indices: tier.indices.clone(),
        mast: 0.0,
        meet_instruction: if tier.instructions.trim().is_empty() {
            "cut to depth".to_string()
        } else {
            tier.instructions.clone()
        },
        meets_tiers: Vec::new(),
        cheater_offset_deg: None,
        angle_of_elevation_deg: tier.angle_deg.abs(),
        depth_mm: None,
        concave: Some(ConcaveRowInfo {
            tool: tier.tool,
            tool_azimuth_deg: tier.tool_azimuth_deg,
            displacement: tier.displacement,
            diameter_ratio: tier.diameter_ratio,
            tool_angle_deg: tier.tool_angle_deg,
            motion: tier.motion,
        }),
    }
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
        let inputs = meet_inputs(&self.tiers);
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

        let inputs = meet_inputs(&self.tiers);
        let resolver = MeetNameResolver::new(&inputs);
        let codes = self.tier_codes();
        let flat_row = |i: usize, sequence: usize| {
            let (tier, solved_tier) = (&self.tiers[i], &solved[i]);
            CutSheetRow {
                sequence,
                code: codes.flat[i].code.clone(),
                name: tier.name.clone(),
                angle_deg: tier.angle_deg.abs(),
                indices: tier.indices.clone(),
                mast: solved_tier.mast,
                meet_instruction: meet_instruction(tier, &resolver, &codes.flat),
                meets_tiers: resolve_meets(tier, &resolver),
                cheater_offset_deg: self.cheater_offset_deg(i),
                angle_of_elevation_deg: tier.angle_deg.abs(),
                depth_mm: mm_per_unit.map(|scale| solved_tier.mast * scale),
                concave: None,
            }
        };
        // Every design -- planar or not -- is printed in `cutting_order`: the pavilion
        // section first, each tool line at the end of its section, the table last.
        let rows = self
            .cutting_order()
            .into_iter()
            .enumerate()
            .map(|(position, tier_ref)| match tier_ref {
                TierRef::Flat(i) => flat_row(i, position + 1),
                TierRef::Concave(i) => {
                    concave_row(&self.concave_tiers[i], &codes.concave[i].code, position + 1)
                }
            })
            .collect();

        Ok(CuttingSheet { header, rows })
    }
}
