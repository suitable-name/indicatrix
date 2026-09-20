//! [`Design`]'s already-solved-to-output side: building an
//! [`indicatrix_formats::asc::AscSchedule`], the full plane arrangement, and the meshed
//! solid/its measurements -- see [`Design::to_asc_schedule_from_solved`]'s doc
//! comment for why each has an "already solved" variant alongside the
//! solve-it-yourself convenience wrapper.

use super::{ConstraintTier, Design, MissingAnchor};
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
    /// # Errors
    ///
    /// Propagates [`Self::solve`]'s error.
    pub fn to_asc_schedule(&self) -> Result<AscSchedule, MissingAnchor> {
        let solved = self.solve()?;
        Ok(self.to_asc_schedule_from_solved(&solved))
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
    /// # Panics
    ///
    /// `solved` must have one entry per tier `self` currently has, in the same order
    /// -- the same alignment contract [`Self::resolve_dirty`] documents on its own
    /// `previous` parameter. Mismatched lengths panic here rather than zipping a
    /// tier to the wrong mast.
    #[must_use]
    pub fn to_asc_schedule_from_solved(&self, solved: &[SolvedTier]) -> AscSchedule {
        assert_eq!(
            solved.len(),
            self.tiers.len(),
            "to_asc_schedule_from_solved: `solved` ({} masts) is not aligned with this design's \
             current {} tier(s) -- only valid for a mast list this exact design (or one that \
             differs only via ModifyTier/SetConstraint) actually produced",
            solved.len(),
            self.tiers.len()
        );
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
        AscSchedule {
            gemcad_version: self.meta.gemcad_version.clone(),
            gear_teeth: self.meta.gear_teeth,
            gear_reference_angle: self.meta.gear_reference_angle,
            symmetry_order: self.meta.symmetry_order,
            mirror: self.meta.mirror,
            // The effective RI (see `Self::effective_refractive_index`), not the raw legacy field.
            refractive_index: self.effective_refractive_index(),
            headers: self.meta.headers.clone(),
            footnotes: self.meta.footnotes.clone(),
            tiers,
        }
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
    pub fn planes(&self) -> Result<Vec<(DVec3, f64)>, MissingAnchor> {
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
        planes.extend(
            StandardGemCuts::from_asc_schedule(&schedule)
                .into_iter()
                .map(GpuFacetPlane::to_halfspace_f64),
        );
        planes
    }

    /// The solid this design currently bounds, or which planes are keeping it from
    /// closing -- see [`SolidStatus`]. Recomputed from scratch every call (meshing
    /// is cheap relative to a render pass); an editor calls this after every edit.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::planes`]'s error.
    pub fn status(&self) -> Result<SolidStatus, MissingAnchor> {
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
    pub fn measure(&self) -> Result<Option<SolidMetrics>, MissingAnchor> {
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
        assert_eq!(
            solved.len(),
            self.tiers.len(),
            "planes_through_tier: `solved` ({} masts) is not aligned with this design's current \
             {} tier(s)",
            solved.len(),
            self.tiers.len()
        );
        // `saturating_add`, not `+`: `through_tier` comes from a UI slider, and a
        // caller passing `usize::MAX` to mean "everything" would otherwise overflow
        // and panic in a debug build. Saturating gives that caller exactly what it
        // asked for, since the `min` below clamps to the real tier count anyway.
        let cutoff = through_tier.saturating_add(1).min(self.tiers.len());
        let mut schedule = self.to_asc_schedule_from_solved(solved);
        schedule.tiers.truncate(cutoff);
        let mut planes = self.preform.planes_offset(self.preform_y_offset);
        planes.extend(
            StandardGemCuts::from_asc_schedule(&schedule)
                .into_iter()
                .map(GpuFacetPlane::to_halfspace_f64),
        );
        planes
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
///   `indicatrix_formats::asc::MeetInstruction::Meet` parses.
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

    /// `preform_y_offset` (CAD audit item 208) must actually shift the
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
}
