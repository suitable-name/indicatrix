//! [`Design`]'s already-solved-to-output side: building an
//! [`indicatrix_formats::asc::AscSchedule`], the full plane arrangement, and the meshed
//! solid/its measurements -- see [`Design::to_asc_schedule_from_solved`]'s doc
//! comment for why each has an "already solved" variant alongside the
//! solve-it-yourself convenience wrapper.

use super::{ConstraintTier, Design, DesignSolveError, SolveMismatch};
use glam::DVec3;
use indicatrix::{
    geometry::{
        GpuFacetPlane,
        cuts::StandardGemCuts,
        meet_solver::{MeetConstraint, SolvedTier},
        stone_metrics::{SolidMetrics, SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
};
use indicatrix_formats::asc::{AscSchedule, AscTier};

impl Design {
    /// This design's effective refractive index -- what [`Self::to_asc_schedule`]
    /// writes to the exported `.asc`'s `I` line, and what the optimizer/tilt
    /// curves/viewport derive their material from.
    ///
    /// Precedence: `self.material.refractive_index_override` when set, else the
    /// selected material's own resolved `n_D` (built-ins only -- a caller that wants
    /// custom catalogue materials folded in too should call
    /// [`crate::material::MaterialSelection::resolve`] directly instead), else the
    /// legacy [`ScheduleMeta::refractive_index`] this crate carried before a
    /// material model existed, read only for a design with neither an override nor a
    /// recognized material name (e.g. an untouched `.asc` import).
    #[must_use]
    pub fn effective_refractive_index(&self) -> f64 {
        self.material
            .refractive_index_override
            .or_else(|| {
                self.material
                    .name
                    .as_deref()
                    .and_then(crate::material::built_in_refractive_index)
            })
            .unwrap_or(self.meta.refractive_index)
    }

    /// Like [`Self::effective_refractive_index`], but also consults `custom` -- a
    /// caller's own resolved catalogue materials -- so a design whose
    /// [`crate::material::MaterialSelection::name`] names a CUSTOM catalogue entry
    /// (not one of the thirteen built-ins [`crate::material::built_in_refractive_index`]
    /// knows about) still scores against that material's real `n_D` instead of
    /// silently falling through to the legacy schedule field.
    ///
    /// Precedence: `self.material.refractive_index_override` when set, else a
    /// `custom` entry whose `name` matches `self.material.name`
    /// (ASCII-case-insensitively, the same match [`crate::material::MaterialLookup`]
    /// implementations in this codebase use), else the built-in table, else the
    /// legacy [`ScheduleMeta::refractive_index`] fallback -- exactly
    /// [`Self::effective_refractive_index`]'s own order with one more rung inserted
    /// between "override" and "built-in".
    ///
    /// A caller that already resolves a design's material against a full catalogue
    /// (built-ins plus custom) elsewhere -- the optimizer/tilt-curve/solid-preview
    /// paths that build an `EditorMaterialLookup`-style lookup over
    /// `RenderContext::custom_materials` -- should call this instead of
    /// [`Self::effective_refractive_index`], passing that same custom list;
    /// [`Self::to_asc_schedule`]/[`Self::to_asc_schedule_from_solved`] and the
    /// optimizer's own objective keep calling the built-ins-only version, since
    /// neither has a custom catalogue on hand.
    #[must_use]
    pub fn effective_refractive_index_with(&self, custom: &[GemMaterial]) -> f64 {
        self.material
            .refractive_index_override
            .or_else(|| {
                self.material.name.as_deref().and_then(|name| {
                    custom
                        .iter()
                        .find(|gem| gem.name.eq_ignore_ascii_case(name))
                        .map(|gem| f64::from(gem.dispersion.evaluate(589.3)))
                })
            })
            .or_else(|| {
                self.material
                    .name
                    .as_deref()
                    .and_then(crate::material::built_in_refractive_index)
            })
            .unwrap_or(self.meta.refractive_index)
    }

    /// Builds a real [`AscSchedule`] from this design's current state: every
    /// tier's mast is [`Self::solve`]'d, and its `G`-field notes text comes from
    /// [`constraint_notes`] -- verbatim from [`ConstraintTier::original_notes`] for
    /// as long as a tier's constraint is still the pinned
    /// [`MeetConstraint::ScaleReference`] import produced, else synthesized fresh
    /// from the tier's current [`MeetConstraint`] kind (parsing that synthesized text
    /// back reclassifies to the same variant, so this still round-trips
    /// semantically even once a tier has actually been edited). This is what makes
    /// `.asc` export and [`Self::planes`] both work from the same authored state.
    ///
    /// A thin wrapper over [`Self::to_asc_schedule_from_solved`] that solves first --
    /// see that method for the caller ([`crate::manufacturability`]) that needs this
    /// without paying for a second solve.
    ///
    /// Built-ins-only for its `I` line -- see [`Self::effective_refractive_index`]'s
    /// own doc comment. A caller with a resolved custom catalogue on hand should call
    /// [`Self::to_asc_schedule_with`] instead.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::solve`]'s error.
    pub fn to_asc_schedule(&self) -> Result<AscSchedule, DesignSolveError> {
        self.to_asc_schedule_with(&[])
    }

    /// Like [`Self::to_asc_schedule`], but also consults `custom` -- see
    /// [`Self::effective_refractive_index_with`] -- so the exported `I` line reflects
    /// a CUSTOM catalogue material's own `n_D` instead of silently falling back to
    /// the legacy schedule RI. Passing `&[]` behaves exactly like
    /// [`Self::to_asc_schedule`] (that method's own implementation, in fact).
    ///
    /// # Errors
    ///
    /// Propagates [`Self::solve`]'s error.
    pub fn to_asc_schedule_with(
        &self,
        custom: &[GemMaterial],
    ) -> Result<AscSchedule, DesignSolveError> {
        let solved = self.solve()?;
        Ok(self.to_asc_schedule_from_solved_with(&solved, custom))
    }

    /// [`Self::to_asc_schedule`]'s schedule-building half, taking an
    /// already-[`Self::solve`]'d (or [`Self::resolve_dirty`]'d) mast list instead of
    /// solving `self` again.
    ///
    /// Exists so [`crate::manufacturability::check_manufacturability`] can run
    /// checks off an already-solved design rather than forcing a fresh solve -- a
    /// design with real meet-derived structure costs 5-6 seconds to solve (see
    /// `Design::solve`), and an editor with a `Vec<SolvedTier>` already in hand
    /// should never pay that cost twice. [`Self::to_asc_schedule`] just solves and
    /// delegates here, so the two can never drift apart.
    ///
    /// Built-ins-only for its `I` line, same caveat as [`Self::to_asc_schedule`] --
    /// see [`Self::to_asc_schedule_from_solved_with`] for the catalogue-aware sibling.
    ///
    /// # Panics
    ///
    /// `solved` must have one entry per tier `self` currently has, in the same order
    /// -- the same alignment contract [`Self::resolve_dirty`] documents on its own
    /// `previous` parameter. Mismatched lengths panic here rather than zipping a
    /// tier to the wrong mast.
    #[must_use]
    pub fn to_asc_schedule_from_solved(&self, solved: &[SolvedTier]) -> AscSchedule {
        self.to_asc_schedule_from_solved_with(solved, &[])
    }

    /// Catalogue-aware sibling of [`Self::to_asc_schedule_from_solved`] -- see
    /// [`Self::to_asc_schedule_with`] for why this exists. Passing `custom` as `&[]`
    /// is exactly [`Self::to_asc_schedule_from_solved`] itself (that method's own
    /// implementation, in fact), so built-in-material behaviour is byte-identical
    /// between the two.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::to_asc_schedule_from_solved`].
    #[must_use]
    pub fn to_asc_schedule_from_solved_with(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> AscSchedule {
        self.try_to_asc_schedule_from_solved_with(solved, custom)
            .unwrap_or_else(|e| {
                panic!(
                    "to_asc_schedule_from_solved: `solved` ({} masts) is not aligned with this \
                     design's current {} tier(s) -- only valid for a mast list this exact \
                     design (or one that differs only via ModifyTier/SetConstraint) actually \
                     produced",
                    e.got_tiers, e.expected_tiers
                )
            })
    }

    /// Fallible sibling of [`Self::to_asc_schedule_from_solved`]: returns
    /// [`SolveMismatch`] instead of panicking when `solved` is not aligned
    /// with [`Self::tiers`]. Identical construction otherwise -- the two
    /// share this method's body, so they can never drift apart.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_to_asc_schedule_from_solved(
        &self,
        solved: &[SolvedTier],
    ) -> Result<AscSchedule, SolveMismatch> {
        self.try_to_asc_schedule_from_solved_with(solved, &[])
    }

    /// Catalogue-aware, fallible sibling of [`Self::to_asc_schedule_from_solved_with`]
    /// -- returns [`SolveMismatch`] instead of panicking, exactly like
    /// [`Self::try_to_asc_schedule_from_solved`] but also consulting `custom` for the
    /// `I` line (see [`Self::effective_refractive_index_with`]). Every other `_with`
    /// entry point in this module (and [`Self::try_to_asc_schedule_from_solved`]
    /// itself) ultimately calls through here, so this is the one place the `I` line
    /// is actually decided.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_to_asc_schedule_from_solved_with(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> Result<AscSchedule, SolveMismatch> {
        if solved.len() != self.tiers.len() {
            return Err(SolveMismatch {
                expected_tiers: self.tiers.len(),
                got_tiers: solved.len(),
            });
        }
        let tiers = self
            .tiers
            .iter()
            .zip(solved)
            .map(|(tier, solved)| AscTier {
                angle_deg: tier.angle_deg,
                mast: solved.mast,
                name: tier.name.clone(),
                indices: tier.indices.clone(),
                notes: constraint_notes(tier),
            })
            .collect();
        Ok(AscSchedule {
            gemcad_version: self.meta.gemcad_version.clone(),
            gear_teeth: self.meta.gear_teeth,
            gear_reference_angle: self.meta.gear_reference_angle,
            symmetry_order: self.meta.symmetry_order,
            mirror: self.meta.mirror,
            // The effective RI (see `Self::effective_refractive_index_with`), not the
            // raw legacy field -- catalogue-aware when `custom` names a match.
            refractive_index: self.effective_refractive_index_with(custom),
            headers: self.meta.headers.clone(),
            footnotes: self.meta.footnotes.clone(),
            tiers,
        })
    }

    /// The full plane arrangement this design bounds: the preform's own closed
    /// plane set (shifted by [`Self::preform_y_offset`] -- see
    /// [`crate::preform::PreformSpec::planes_offset`]), plus one plane per facet
    /// the schedule's tiers describe (via [`Self::to_asc_schedule`] then
    /// [`StandardGemCuts::from_asc_schedule`], the same production path a real
    /// `.asc` file's recorded masts go through).
    ///
    /// Plane order is preform planes first, then schedule planes in tier order.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::solve`]'s error -- see the module docs on why this crate
    /// refuses to invent a scale rather than returning some plane arrangement anyway.
    pub fn planes(&self) -> Result<Vec<(DVec3, f64)>, DesignSolveError> {
        let solved = self.solve()?;
        Ok(self.planes_from_solved(&solved))
    }

    /// [`Self::planes`]'s arrangement-building half, taking an already-solved mast
    /// list instead of solving `self` again -- see
    /// [`Self::to_asc_schedule_from_solved`] for why this exists and its (inherited)
    /// panic contract.
    #[must_use]
    pub fn planes_from_solved(&self, solved: &[SolvedTier]) -> Vec<(DVec3, f64)> {
        let schedule = self.to_asc_schedule_from_solved(solved);
        let mut planes = self.preform.planes_offset(self.preform_y_offset);
        let facet_planes: Vec<(DVec3, f64)> = StandardGemCuts::from_asc_schedule(&schedule)
            .into_iter()
            .map(GpuFacetPlane::to_halfspace_f64)
            .collect();
        planes.extend(apply_cheater_offsets(self, &schedule, facet_planes));
        planes
    }

    /// The solid this design currently bounds, or which planes are keeping it from
    /// closing -- see [`SolidStatus`]. Recomputed from scratch every call (meshing
    /// is cheap relative to a render pass); an editor calls this after every edit.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::planes`]'s error.
    pub fn status(&self) -> Result<SolidStatus, DesignSolveError> {
        Ok(build_solid_mesh(&self.planes()?))
    }

    /// Convenience: `true` iff [`Self::status`] is `Ok(SolidStatus::Closed(_))`.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        matches!(self.status(), Ok(SolidStatus::Closed(_)))
    }

    /// This design's physical measurements (volume, width/length, crown/pavilion/
    /// total height), or `None` when it isn't currently a closed solid. A thin
    /// wrapper over [`measure_solid`] on [`Self::planes`] for a caller that wants
    /// the scalar figures without the mesh.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::planes`]'s error.
    pub fn measure(&self) -> Result<Option<SolidMetrics>, DesignSolveError> {
        Ok(measure_solid(&self.planes()?))
    }

    /// [`Self::planes_from_solved`] restricted to the first `through_tier + 1`
    /// tiers (the preform's own planes, including [`Self::preform_y_offset`]'s
    /// shift, are always included) -- the plane arrangement a "show through
    /// tier N" viewport slider needs to preview
    /// the stone as cut up to and including that tier, without a second
    /// solve and without the caller re-deriving the schedule truncation
    /// itself.
    ///
    /// `through_tier >= self.tiers.len()` means "every tier", identical to
    /// [`Self::planes_from_solved`] itself.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`]: `solved`
    /// must have one entry per tier `self` currently has, in the same order.
    #[must_use]
    pub fn planes_through_tier(
        &self,
        solved: &[SolvedTier],
        through_tier: usize,
    ) -> Vec<(DVec3, f64)> {
        self.try_planes_through_tier(solved, through_tier)
            .unwrap_or_else(|e| {
                panic!(
                    "planes_through_tier: `solved` ({} masts) is not aligned with this design's \
                     current {} tier(s)",
                    e.got_tiers, e.expected_tiers
                )
            })
    }

    /// Fallible sibling of [`Self::planes_through_tier`]: returns
    /// [`SolveMismatch`] instead of panicking when `solved` is not aligned
    /// with [`Self::tiers`].
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_planes_through_tier(
        &self,
        solved: &[SolvedTier],
        through_tier: usize,
    ) -> Result<Vec<(DVec3, f64)>, SolveMismatch> {
        // `saturating_add`, not `+`: `through_tier` comes from a UI slider, and a
        // caller passing `usize::MAX` to mean "everything" would otherwise overflow
        // and panic in a debug build. Saturating gives that caller exactly what it
        // asked for, since the `min` below clamps to the real tier count anyway.
        let cutoff = through_tier.saturating_add(1).min(self.tiers.len());
        let full_schedule = self.try_to_asc_schedule_from_solved(solved)?;
        // Cheater offsets are resolved against the FULL (untruncated) schedule --
        // `apply_cheater_offsets` locates each tier's own facet-plane slice via
        // `facet_plane_boundaries(&full_schedule)`, which a truncated schedule
        // would misalign for every tier after the cutoff. The facet count through
        // `cutoff` is derived separately so the final arrangement still only
        // contains the first `cutoff` tiers' (rotated) planes.
        let mut planes = self.preform.planes_offset(self.preform_y_offset);
        let facet_planes: Vec<(DVec3, f64)> = StandardGemCuts::from_asc_schedule(&full_schedule)
            .into_iter()
            .map(GpuFacetPlane::to_halfspace_f64)
            .collect();
        let facet_count_through_cutoff = if cutoff == full_schedule.tiers.len() {
            facet_planes.len()
        } else {
            let mut truncated = full_schedule.clone();
            truncated.tiers.truncate(cutoff);
            StandardGemCuts::from_asc_schedule(&truncated).len()
        };
        let mut rotated = apply_cheater_offsets(self, &full_schedule, facet_planes);
        rotated.truncate(facet_count_through_cutoff);
        planes.extend(rotated);
        Ok(planes)
    }

    /// Which tier (index into [`Self::tiers`]) contributed the facet plane at
    /// `plane_index` of [`Self::planes_from_solved`]'s combined arrangement --
    /// e.g. one of the escaping indices named by
    /// [`indicatrix::geometry::stone_metrics::SolidStatus::Unbounded`]. Does
    /// the `self.preform.planes().len()` offset once, here, rather than
    /// leaving every caller to re-derive and subtract it themselves.
    ///
    /// Returns `None` for a preform plane (before any tier's own
    /// contribution starts) or an index past the end of the arrangement --
    /// both mean "not a schedule-tier facet", not an error.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`]: `solved` must
    /// have one entry per tier `self` currently has, in the same order.
    #[must_use]
    pub fn tier_for_plane_index(&self, solved: &[SolvedTier], plane_index: usize) -> Option<usize> {
        let local_index = plane_index.checked_sub(self.preform.planes().len())?;
        let schedule = self.to_asc_schedule_from_solved(solved);
        let boundaries = crate::manufacturability::facet_plane_boundaries(&schedule);
        boundaries.iter().position(|&end| local_index < end)
    }
}

/// Rotates each tier's own slice of `facet_planes` (in
/// [`StandardGemCuts::from_asc_schedule`]'s order, i.e. NOT including the
/// preform's own planes) around the vertical axis by that tier's
/// [`Design::cheater_offset_deg`], if any -- the geometric half of a
/// "cheater"/azimuth-offset annotation. Without this rotation, an offset would
/// be persisted and printed on the cut sheet but completely ignored by the
/// solid, the tracer and the diagram; since [`Self::planes_from_solved`]/
/// [`Self::planes_through_tier`] are the one production path the solid preview
/// (`design_to_gpu_planes`), `solid_preview::facet_map` and the manufacturability
/// checks all read planes from, applying the rotation HERE makes every one of
/// those consumers agree with the cut sheet automatically, with no change needed
/// in any of them.
///
/// # Sign convention
///
/// A positive `offset_deg` rotates a tier's facet(s) counter-clockwise about
/// `+Y` (the same right-handed sense `glam::DMat3::from_rotation_y` uses) --
/// this crate's own choice, since neither `.asc` nor `GemCad` define one for a
/// cheater angle; a caller displaying the value (the cut sheet, the inspector)
/// should say so once rather than leave the sign unexplained.
///
/// `n . x <= m`'s offset `m` is unchanged by any rotation about the origin
/// (rotation preserves distance from the axis), so only each plane's normal
/// moves.
///
/// Tiers whose plane count does not match `indices.len().max(1)` (the rare
/// dedup case [`facet_plane_boundaries`]'s own doc comment describes) still
/// rotate correctly: the boundaries are read from the SAME schedule the caller
/// built `facet_planes` from, so a tier's slice is always the boundary-derived
/// range, never a re-derived `indices.len()`.
fn apply_cheater_offsets(
    design: &Design,
    schedule: &AscSchedule,
    mut facet_planes: Vec<(DVec3, f64)>,
) -> Vec<(DVec3, f64)> {
    if design.cheater_offsets_deg.is_empty() {
        return facet_planes;
    }
    let boundaries = crate::manufacturability::facet_plane_boundaries(schedule);
    let mut prev = 0usize;
    for (tier_index, &end) in boundaries.iter().enumerate() {
        let end = end.min(facet_planes.len());
        if let Some(offset_deg) = design.cheater_offset_deg(tier_index)
            && offset_deg != 0.0
            && prev < end
        {
            let rotation = glam::DMat3::from_rotation_y(offset_deg.to_radians());
            for plane in &mut facet_planes[prev..end] {
                plane.0 = rotation * plane.0;
            }
        }
        prev = end;
    }
    facet_planes
}

/// Builds one tier's `.asc` `G`-field text.
///
/// While `tier.constraint` is still the pinned [`MeetConstraint::ScaleReference`]
/// [`Design::from_asc_schedule`] produced for it, this returns
/// [`ConstraintTier::original_notes`] verbatim (whatever the file actually said --
/// `"Cut to TCP"`, `"GMP"`, `"Cut to mast depth X."`, anything at all, not just the
/// handful of instructions this crate classifies), so an untouched import's schedule
/// re-exports with the exact notes a human reads while cutting. `original_notes` is
/// `None` for a tier [`Design::from_asc_schedule`] never built (a brand-new tier, or
/// one already replaced by an edit), which falls through to the synthesized text
/// below same as a tier whose constraint has actually changed.
///
/// Synthesized text reclassifies to the same [`MeetConstraint`] variant via
/// `AscTier::meet_instruction`/`meet_tier_inputs_from_asc`, so this still round-trips
/// semantically (not byte-for-byte) once a tier has actually been edited:
///
/// - [`MeetConstraint::MeetExisting`] -> empty (no `G` field -- the common case).
/// - [`MeetConstraint::MeetNamed`] -> `"Meet <names>"`, the exact instruction text
///   `indicatrix_formats::asc::MeetInstruction::Meet` parses -- PROVIDED every name in
///   the list passes [`meet_name_is_asc_safe`]; a name containing whitespace/`,`/
///   `;`, or with leading/trailing punctuation, splits into the wrong token(s) or a
///   different tier's name on re-parse (`indicatrix_formats::asc`'s own tokenizer
///   splits and trims exactly those characters -- see that module's
///   `extract_meet_names`). This function does not itself refuse such a name (the
///   export format is unchanged -- see
///   [`crate::manufacturability::check_meet_name_asc_safety`] for the warning that
///   flags it instead), so a caller relying on byte-for-byte reclassification must
///   check that separately.
/// - [`MeetConstraint::ScaleReference`] -> `"Set stone size."`, one of several real
///   phrasings `indicatrix_formats::asc::MeetInstruction::ScaleReference` recognizes (this
///   crate does not track which specific phrasing a freshly authored -- not
///   imported -- scale reference should use, only that it is one).
fn constraint_notes(tier: &ConstraintTier) -> String {
    if let (MeetConstraint::ScaleReference(_), Some(original_notes)) =
        (&tier.constraint, &tier.original_notes)
    {
        return original_notes.clone();
    }
    match &tier.constraint {
        MeetConstraint::MeetExisting => String::new(),
        MeetConstraint::MeetNamed(names) => format!("Meet {}", names.join(", ")),
        MeetConstraint::ScaleReference(_) => "Set stone size.".to_string(),
    }
}

/// `true` iff `name` would survive a plain `.asc` export/re-import as this exact,
/// single token -- i.e. [`constraint_notes`]'s `"Meet <names>"` text, re-parsed by
/// `indicatrix_formats::asc`'s own `G`-field tokenizer (splits a `Meet` instruction's
/// text on `,`/`;`/whitespace, then trims each token of leading/trailing
/// non-alphanumeric characters -- see that crate's `extract_meet_names`), would
/// resolve back to `name` and nothing else.
///
/// Unsafe: an empty name (nothing to resolve at all); one containing whitespace,
/// `,` or `;` (the parser's own token separators -- a compound tier name like
/// `"Crown Main"` would split into two unrelated tokens, neither of which names
/// this tier); or one with a leading or trailing character that is not
/// alphanumeric (stripped by the parser's `trim_matches`, so e.g. the mirrored-tier
/// convention `"Main'"` -- see [`ConstraintTier::mirrored_to_other_block`] --
/// reparses as `"Main"`, a DIFFERENT tier's name entirely).
///
/// Used by [`crate::manufacturability::check_meet_name_asc_safety`] to warn on an
/// authored [`MeetConstraint::MeetNamed`] target that would not survive this round
/// trip; [`constraint_notes`] itself keeps writing the name verbatim regardless
/// (the export format is unchanged -- see that function's own doc comment).
#[must_use]
pub fn meet_name_is_asc_safe(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c == ',' || c == ';')
    {
        return false;
    }
    let first = name.chars().next().expect("checked non-empty above");
    let last = name.chars().next_back().expect("checked non-empty above");
    first.is_alphanumeric() && last.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{design::ScheduleMeta, material::MaterialSelection, preform::PreformSpec};

    fn round_brilliant_design() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    /// `planes_through_tier` at the last tier index must match
    /// `planes_from_solved` exactly, and at tier 0 must include only the
    /// preform planes plus the first tier's own facet (the "Table" tier has
    /// no indices, so exactly one plane).
    #[test]
    fn planes_through_tier_truncates_the_schedule() {
        let design = round_brilliant_design();
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let full = design.planes_from_solved(&solved);
        let through_last = design.planes_through_tier(&solved, design.tiers.len() - 1);
        assert_eq!(full.len(), through_last.len());

        let preform_count = design.preform.planes().len();
        let through_first = design.planes_through_tier(&solved, 0);
        assert_eq!(through_first.len(), preform_count + 1);
    }

    /// A `through_tier` past the last tier index must behave exactly like
    /// `planes_from_solved` (every tier included).
    #[test]
    fn planes_through_tier_past_the_end_includes_everything() {
        let design = round_brilliant_design();
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let full = design.planes_from_solved(&solved);
        let past_end = design.planes_through_tier(&solved, 1000);
        assert_eq!(full.len(), past_end.len());
    }

    /// `preform_y_offset` must actually shift the
    /// preform's own two horizontal planes in BOTH `planes_from_solved` and
    /// `planes_through_tier` -- the two production entry points a real
    /// solid/preview build from -- while leaving every schedule-facet plane
    /// (and the plane count) completely unchanged, matching
    /// `PreformSpec::planes_offset`'s own contract.
    #[test]
    fn preform_y_offset_shifts_the_preforms_own_planes_in_every_production_arrangement() {
        let mut design = round_brilliant_design();
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let unshifted = design.planes_from_solved(&solved);
        let unshifted_through = design.planes_through_tier(&solved, design.tiers.len() - 1);

        design.preform_y_offset = 0.25;
        let shifted = design.planes_from_solved(&solved);
        let shifted_through = design.planes_through_tier(&solved, design.tiers.len() - 1);

        assert_eq!(unshifted.len(), shifted.len());
        assert_eq!(unshifted_through.len(), shifted_through.len());
        // Same plane count as the unshifted arrangement, so this is exactly
        // `preform.planes_offset(0.25)` followed by the same facet planes --
        // the preform's own two horizontal planes (pushed last by
        // `PreformSpec::planes`) must have moved; every other plane must not.
        let preform_count = design.preform.planes().len();
        for i in 0..preform_count - 2 {
            assert_eq!(unshifted[i], shifted[i], "non-vertical preform plane {i}");
        }
        assert_ne!(unshifted[preform_count - 2].1, shifted[preform_count - 2].1);
        assert_ne!(unshifted[preform_count - 1].1, shifted[preform_count - 1].1);
        for i in preform_count..unshifted.len() {
            assert_eq!(unshifted[i], shifted[i], "schedule facet plane {i}");
        }
    }

    /// A cheater offset recorded on exactly one tier must rotate that tier's OWN
    /// facet plane(s) about `+Y` and leave every other plane -- preform planes
    /// AND every other tier's facet planes -- byte-identical, in both
    /// `planes_from_solved` and `planes_through_tier` (the two production
    /// arrangements the solid/facet map/manufacturability checks all read from).
    /// Without this rotation, this offset would be persisted and printed but
    /// completely ignored by the geometry.
    #[test]
    fn cheater_offset_rotates_only_its_own_tiers_planes() {
        let mut design = round_brilliant_design();
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let unrotated = design.planes_from_solved(&solved);
        let unrotated_through = design.planes_through_tier(&solved, design.tiers.len() - 1);

        // Tier 2 ("Crown Main") gets a real cheater offset; every other tier stays
        // untouched.
        design.cheater_offsets_deg.insert(2, 0.5);
        let rotated = design.planes_from_solved(&solved);
        let rotated_through = design.planes_through_tier(&solved, design.tiers.len() - 1);

        assert_eq!(unrotated.len(), rotated.len());
        assert_eq!(unrotated_through.len(), rotated_through.len());

        let schedule = design.to_asc_schedule_from_solved(&solved);
        let boundaries = crate::manufacturability::facet_plane_boundaries(&schedule);
        let preform_len = design.preform.planes().len();
        let tier2_start = preform_len + boundaries[1];
        let tier2_end = preform_len + boundaries[2];
        assert!(
            tier2_end > tier2_start,
            "Crown Main must contribute at least one facet plane"
        );

        for i in 0..unrotated.len() {
            if (tier2_start..tier2_end).contains(&i) {
                assert_ne!(
                    unrotated[i].0, rotated[i].0,
                    "plane {i} (Crown Main's own facet) must have rotated"
                );
                // The offset only shifts azimuth: the plane's own distance from
                // the origin is unchanged.
                assert!((unrotated[i].1 - rotated[i].1).abs() < 1e-9);
            } else {
                assert_eq!(unrotated[i], rotated[i], "plane {i} must not have moved");
            }
        }
        for i in 0..unrotated_through.len() {
            assert_eq!(
                unrotated_through[i].1, rotated_through[i].1,
                "plane {i} offset must not have moved"
            );
            if (tier2_start..tier2_end).contains(&i) {
                assert_ne!(unrotated_through[i].0, rotated_through[i].0);
            } else {
                assert_eq!(unrotated_through[i].0, rotated_through[i].0);
            }
        }
    }

    /// A cheater offset of exactly `0.0` (the default -- no offset actually
    /// recorded, or one explicitly cleared back to zero) must leave every plane
    /// unchanged, matching "never print a value the picture ignores" the other
    /// way around: a zero offset the picture DOES apply is indistinguishable
    /// from no offset at all.
    #[test]
    fn zero_cheater_offset_changes_nothing() {
        let mut design = round_brilliant_design();
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let before = design.planes_from_solved(&solved);
        design.cheater_offsets_deg.insert(2, 0.0);
        let after = design.planes_from_solved(&solved);
        assert_eq!(before, after);
    }

    /// `gear_reference_angle` is NOT geometry-inert for a block
    /// preform (see [`crate::edit::Edit::SetMeta`]'s doc comment)
    /// -- it rotates every solved facet plane's azimuth (see
    /// `indicatrix::geometry::cuts::StandardGemCuts::index_to_azimuth`) while the
    /// block's own fixed walls do not rotate with it, so the combined plane
    /// arrangement genuinely changes, not just a relabeling of the same solid.
    #[test]
    fn gear_reference_angle_changes_planes_from_solved_on_a_block_preform() {
        let mut design = round_brilliant_design();
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let unrotated = design.planes_from_solved(&solved);

        design.meta.gear_reference_angle = 6.0;
        let rotated = design.planes_from_solved(&solved);

        assert_eq!(
            unrotated.len(),
            rotated.len(),
            "a reference-angle change must not add or drop planes"
        );
        assert_ne!(
            unrotated, rotated,
            "gear_reference_angle must rotate the facet planes on a block preform, \
             contrary to the old 'affects no geometry' doc"
        );
    }

    // --- meet_name_is_asc_safe ---

    #[test]
    fn meet_name_is_asc_safe_accepts_a_plain_alphanumeric_name() {
        assert!(meet_name_is_asc_safe("P1"));
        assert!(meet_name_is_asc_safe("Girdle"));
        assert!(meet_name_is_asc_safe("C1"));
    }

    #[test]
    fn meet_name_is_asc_safe_rejects_whitespace_and_separators() {
        assert!(!meet_name_is_asc_safe("Crown Main"));
        assert!(!meet_name_is_asc_safe("P1,P2"));
        assert!(!meet_name_is_asc_safe("P1;P2"));
        assert!(!meet_name_is_asc_safe(""));
    }

    #[test]
    fn meet_name_is_asc_safe_rejects_leading_or_trailing_punctuation() {
        // The mirrored-tier naming convention (`ConstraintTier::mirrored_to_other_block`)
        // is exactly the case this guards: `"Main'"` re-parses as `"Main"`.
        assert!(!meet_name_is_asc_safe("Main'"));
        assert!(!meet_name_is_asc_safe("'Main"));
    }

    // --- Design::effective_refractive_index_with ---

    fn custom_garnet(n_d: f32) -> GemMaterial {
        let mut gem = GemMaterial::diamond();
        gem.name = "My Garnet".to_string();
        gem.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
            a: n_d,
            b: 0.0,
            c: 0.0,
        };
        gem
    }

    /// An explicit RI override still wins even when a same-named custom material is
    /// also present -- the same top-of-precedence rule
    /// `effective_refractive_index` already follows.
    #[test]
    fn effective_refractive_index_with_prefers_the_override_over_a_custom_entry() {
        let mut design = round_brilliant_design();
        design.material = MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(1.70),
        };
        let custom = [custom_garnet(1.90)];
        assert_eq!(design.effective_refractive_index_with(&custom), 1.70);
    }

    /// A design named after a CUSTOM catalogue entry (not one of the thirteen
    /// built-ins) must resolve to that entry's own `n_D` -- the bug
    /// `effective_refractive_index` alone cannot fix, since it only ever consults
    /// the built-in table.
    #[test]
    fn effective_refractive_index_with_resolves_a_custom_material_by_name() {
        let mut design = round_brilliant_design();
        design.material = MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        // The built-ins-only version has no idea what "My Garnet" is and falls all
        // the way back to the legacy schedule RI.
        assert_eq!(
            design.effective_refractive_index(),
            design.meta.refractive_index
        );
        let custom = [custom_garnet(1.90)];
        assert!((design.effective_refractive_index_with(&custom) - 1.90).abs() < 1e-6);
    }

    /// A recognized built-in name still resolves correctly when `custom` has
    /// nothing matching it -- the "no custom catalogue loaded" case must behave
    /// exactly like `effective_refractive_index`.
    #[test]
    fn effective_refractive_index_with_falls_back_to_a_built_in_when_no_custom_entry_matches() {
        let mut design = round_brilliant_design();
        design.material = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let custom: [GemMaterial; 0] = [];
        assert_eq!(
            design.effective_refractive_index_with(&custom),
            design.effective_refractive_index()
        );
    }

    /// With no material selection and no matching custom entry at all, both
    /// functions must fall back to the same legacy schedule RI.
    #[test]
    fn effective_refractive_index_with_falls_back_to_the_legacy_value_when_nothing_resolves() {
        let design = round_brilliant_design();
        let custom: [GemMaterial; 0] = [];
        assert_eq!(
            design.effective_refractive_index_with(&custom),
            design.meta.refractive_index
        );
    }

    // --- to_asc_schedule_with / to_asc_schedule_from_solved_with (custom-material `I` line) ---

    /// A design on a CUSTOM catalogue material must export an `I` line matching
    /// that material's own `n_D` through the `_with` entry points -- the bug this
    /// module fixes. `to_asc_schedule` (built-ins-only) must still fall back to the
    /// legacy schedule RI for the exact same design, proving the two really do
    /// differ only in whether a custom catalogue was supplied.
    #[test]
    fn to_asc_schedule_with_resolves_a_custom_materials_own_refractive_index() {
        let mut design = round_brilliant_design();
        design.material = MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let custom = [custom_garnet(1.9)];

        let with_catalogue = design
            .to_asc_schedule_with(&custom)
            .expect("every tier is pinned via ScaleReference");
        assert!((with_catalogue.refractive_index - 1.9).abs() < 1e-6);

        let built_ins_only = design
            .to_asc_schedule()
            .expect("every tier is pinned via ScaleReference");
        assert_eq!(
            built_ins_only.refractive_index,
            design.meta.refractive_index
        );
    }

    /// Same check via the already-solved entry points
    /// (`to_asc_schedule_from_solved_with`/`to_asc_schedule_from_solved`), and via
    /// the fallible `try_*_with` sibling.
    #[test]
    fn to_asc_schedule_from_solved_with_resolves_a_custom_materials_own_refractive_index() {
        let mut design = round_brilliant_design();
        design.material = MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let custom = [custom_garnet(1.9)];
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");

        let with_catalogue = design.to_asc_schedule_from_solved_with(&solved, &custom);
        assert!((with_catalogue.refractive_index - 1.9).abs() < 1e-6);

        let via_try = design
            .try_to_asc_schedule_from_solved_with(&solved, &custom)
            .expect("solved is aligned with design.tiers");
        assert!((via_try.refractive_index - 1.9).abs() < 1e-6);

        let built_ins_only = design.to_asc_schedule_from_solved(&solved);
        assert_eq!(
            built_ins_only.refractive_index,
            design.meta.refractive_index
        );
    }

    /// A design on a recognized built-in ("Diamond") must export the exact same `I`
    /// line -- and in fact the exact same full `.asc` text -- through both the old
    /// (built-ins-only) and new (`_with`, `custom` empty or irrelevant) entry
    /// points, matching this crate's own determinism requirement.
    #[test]
    fn to_asc_schedule_with_matches_the_old_entry_point_byte_for_byte_on_a_built_in() {
        let mut design = round_brilliant_design();
        design.material = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        // A custom catalogue with an unrelated entry must not perturb a design named
        // after a built-in -- built-ins take precedence over nothing here, since
        // `custom` has no "Diamond" entry of its own to shadow it with.
        let custom = [custom_garnet(1.9)];

        let old = design
            .to_asc_schedule()
            .expect("every tier is pinned via ScaleReference");
        let new = design
            .to_asc_schedule_with(&custom)
            .expect("every tier is pinned via ScaleReference");
        assert!((old.refractive_index - 2.417).abs() < 1e-3);
        assert_eq!(old.refractive_index, new.refractive_index);
        assert_eq!(
            indicatrix_formats::asc::to_asc_string(&old),
            indicatrix_formats::asc::to_asc_string(&new)
        );
    }

    // --- SolveMismatch (panic-to-error conversion) ---

    /// `try_to_asc_schedule_from_solved` must return [`SolveMismatch`]
    /// instead of panicking on a misaligned `solved`, and
    /// `to_asc_schedule_from_solved` must still panic on the exact same
    /// input.
    #[test]
    fn try_to_asc_schedule_from_solved_reports_a_mismatch_instead_of_panicking() {
        let design = round_brilliant_design();
        let bogus_solved: Vec<SolvedTier> = Vec::new();
        let err = design
            .try_to_asc_schedule_from_solved(&bogus_solved)
            .expect_err("empty solved list must not align with 8 tiers");
        assert_eq!(err.expected_tiers, design.tiers.len());
        assert_eq!(err.got_tiers, 0);
    }

    #[test]
    #[should_panic(expected = "to_asc_schedule_from_solved: `solved`")]
    fn to_asc_schedule_from_solved_still_panics_on_a_mismatch() {
        let design = round_brilliant_design();
        let bogus_solved: Vec<SolvedTier> = Vec::new();
        let _ = design.to_asc_schedule_from_solved(&bogus_solved);
    }

    /// Same pair of checks for `planes_through_tier`/`try_planes_through_tier`.
    #[test]
    fn try_planes_through_tier_reports_a_mismatch_instead_of_panicking() {
        let design = round_brilliant_design();
        let bogus_solved: Vec<SolvedTier> = Vec::new();
        let err = design
            .try_planes_through_tier(&bogus_solved, 0)
            .expect_err("empty solved list must not align with 8 tiers");
        assert_eq!(err.expected_tiers, design.tiers.len());
        assert_eq!(err.got_tiers, 0);
    }

    #[test]
    #[should_panic(expected = "planes_through_tier: `solved`")]
    fn planes_through_tier_still_panics_on_a_mismatch() {
        let design = round_brilliant_design();
        let bogus_solved: Vec<SolvedTier> = Vec::new();
        let _ = design.planes_through_tier(&bogus_solved, 0);
    }
}
