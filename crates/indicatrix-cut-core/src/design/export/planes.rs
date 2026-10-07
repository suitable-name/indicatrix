//! The full plane arrangement a [`Design`] bounds ([`Design::planes`] and
//! siblings), the solid it meshes to ([`Design::status`]/[`Design::measure`]),
//! and the cheater/azimuth-offset rotation ([`apply_cheater_offsets`]) that
//! keeps every production plane consumer agreeing with the cut sheet.

use super::{ConcaveResolveError, ToolPlacements};
use crate::design::{Design, DesignSolveError, SolveMismatch, TierRef};
use glam::DVec3;
use indicatrix::geometry::{
    GpuFacetPlane,
    cuts::StandardGemCuts,
    meet_solver::SolvedTier,
    stone_metrics::{
        SolidMetrics, SolidStatus, build_solid_mesh, build_solid_mesh_geom, measure_solid,
        mesh_volume,
    },
    tool::ToolPrimitive,
};
use indicatrix_formats::asc::AscSchedule;

impl Design {
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

    /// [`Self::status`]'s concave twin for an already-solved design: the mesh of
    /// the flat stone minus every concave tool ([`build_solid_mesh_geom`]).
    /// Identical to `build_solid_mesh(&self.planes_from_solved(solved))` when the
    /// design has no concave tiers.
    ///
    /// # Errors
    ///
    /// [`ConcaveResolveError`] when the concave tiers cannot be resolved.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`].
    pub fn status_from_solved_geom(
        &self,
        solved: &[SolvedTier],
    ) -> Result<SolidStatus, ConcaveResolveError> {
        let (planes, tools, _) = self.geometry_from_solved(solved)?;
        Ok(build_solid_mesh_geom(&planes, &tools))
    }

    /// [`Self::measure`]'s concave twin for an already-solved design: the flat
    /// stone's metrics with `volume` replaced by the carved mesh's volume, which
    /// is what carat weight and yield must see. Width, length and heights stay
    /// the flat stone's: concave cuts never add material, and `W` is defined on
    /// the flat stone (plan §4.4). Equal to `measure_solid` of the planes when the
    /// design has no concave tiers.
    ///
    /// `Ok(None)` when the flat stone is not a closed solid, or when the tools
    /// remove all of it.
    ///
    /// # Errors
    ///
    /// [`ConcaveResolveError`] when the concave tiers cannot be resolved.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`].
    pub fn measure_from_solved_geom(
        &self,
        solved: &[SolvedTier],
    ) -> Result<Option<SolidMetrics>, ConcaveResolveError> {
        let (planes, tools, _) = self.geometry_from_solved(solved)?;
        let flat = measure_solid(&planes);
        if tools.is_empty() {
            return Ok(flat);
        }
        Ok(match build_solid_mesh_geom(&planes, &tools) {
            SolidStatus::Closed(mesh) => flat.map(|metrics| SolidMetrics {
                volume: mesh_volume(&mesh),
                ..metrics
            }),
            _ => None,
        })
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

    /// The tiers the Cut slider walks, in the order they are cut: [`Self::cutting_order`], the
    /// order the printed cutting sheet, Cutting mode and "Build this design" follow, for a
    /// planar design as well as one with concave tiers. Flat and concave tiers each count as
    /// one step.
    ///
    /// A planar design's steps used to be its stored order, so the slider could show a
    /// crown before the pavilion; the shown tiers of a step are now a stored prefix only by
    /// accident, and [`Self::try_planes_after_steps`] builds them from the visible set.
    #[must_use]
    pub fn preview_steps(&self) -> Vec<TierRef> {
        self.cutting_order()
    }

    /// How many steps [`Self::preview_steps`] has: every flat tier plus every concave
    /// tier. The Cut slider's last position ("Finished") is this many steps.
    #[must_use]
    pub const fn preview_step_count(&self) -> usize {
        self.tiers.len() + self.concave_tiers.len()
    }

    /// The preform's planes plus the facet planes of exactly the flat tiers `visible`
    /// marks (indexed like [`Self::tiers`]; a missing entry counts as visible), in
    /// tier order. Unlike [`Self::try_planes_through_tier`] the shown tiers need not
    /// be a prefix, which a design with concave tiers needs: its cutting order
    /// interleaves the stored one.
    ///
    /// Cheater offsets are resolved against the full schedule, as in
    /// [`Self::try_planes_through_tier`], so a shown tier is rotated exactly as in the
    /// finished stone.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_planes_for_visible_tiers(
        &self,
        solved: &[SolvedTier],
        visible: &[bool],
    ) -> Result<Vec<(DVec3, f64)>, SolveMismatch> {
        let schedule = self.try_to_asc_schedule_from_solved(solved)?;
        let facet_planes: Vec<(DVec3, f64)> = StandardGemCuts::from_asc_schedule(&schedule)
            .into_iter()
            .map(GpuFacetPlane::to_halfspace_f64)
            .collect();
        let rotated = apply_cheater_offsets(self, &schedule, facet_planes);
        let boundaries = crate::manufacturability::facet_plane_boundaries(&schedule);
        let mut planes = self.preform.planes_offset(self.preform_y_offset);
        let mut start = 0usize;
        for (tier, &end) in boundaries.iter().enumerate() {
            let end = end.min(rotated.len()).max(start);
            if visible.get(tier).copied().unwrap_or(true) {
                planes.extend_from_slice(&rotated[start..end]);
            }
            start = end;
        }
        Ok(planes)
    }

    /// The plane arrangement of the stone after its first `steps`
    /// [`Self::preview_steps`]: `0` is the preform alone (the rough), and a `steps` at
    /// or past [`Self::preview_step_count`] is the finished stone, identical to
    /// [`Self::planes_from_solved`]. A step that is a concave tier adds no flat plane
    /// (its tool comes from [`Self::concave_tools_after_steps`]).
    ///
    /// The rough needs no masts, so `solved` is not checked for it.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when a non-empty stone is asked for and
    /// `solved.len() != self.tiers.len()`.
    pub fn try_planes_after_steps(
        &self,
        solved: &[SolvedTier],
        steps: usize,
    ) -> Result<Vec<(DVec3, f64)>, SolveMismatch> {
        let order = self.preview_steps();
        if steps >= order.len() {
            return self.try_planes_through_tier(solved, usize::MAX);
        }
        let mut visible = vec![false; self.tiers.len()];
        for step in &order[..steps] {
            if let TierRef::Flat(index) = step
                && let Some(slot) = visible.get_mut(*index)
            {
                *slot = true;
            }
        }
        let leading = visible.iter().take_while(|&&shown| shown).count();
        if !visible.contains(&true) {
            return Ok(self.preform.planes_offset(self.preform_y_offset));
        }
        if visible[leading..].iter().all(|&shown| !shown) {
            // The shown tiers are a stored prefix: the same truncation as
            // `planes_through_tier`, so a planar design is unchanged by the new slider.
            return self.try_planes_through_tier(solved, leading - 1);
        }
        self.try_planes_for_visible_tiers(solved, &visible)
    }

    /// The tools of the concave tiers among the first `steps`
    /// [`Self::preview_steps`]: the concave half of [`Self::try_planes_after_steps`].
    /// Empty for the rough and for a design without concave tiers.
    ///
    /// # Errors
    ///
    /// As [`Self::concave_tools_from_solved`].
    ///
    /// # Panics
    ///
    /// As [`Self::concave_tools_from_solved`] when a tool is asked for: `solved` must
    /// have one entry per flat tier.
    pub fn concave_tools_after_steps(
        &self,
        solved: &[SolvedTier],
        steps: usize,
    ) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError> {
        if self.concave_tiers.is_empty() || steps == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        match self.cutting_order().get(steps) {
            Some(&next) => self.concave_tools_through_tier(solved, next),
            None => self.concave_tools_from_solved(solved),
        }
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
/// preform's own planes) about the vertical axis by that tier's
/// [`Design::cheater_offset_deg`], if any -- the geometric half of a
/// "cheater"/azimuth-offset annotation. Without this rotation, an offset would
/// be persisted and printed on the cut sheet but completely ignored by the
/// solid, the tracer and the diagram; since [`Design::planes_from_solved`]/
/// [`Design::planes_through_tier`] are the one production path the solid preview
/// (`design_to_gpu_planes`), `solid_preview::facet_map` and the manufacturability
/// checks all read planes from, applying the rotation HERE makes every one of
/// those consumers agree with the cut sheet automatically, with no change needed
/// in any of them.
///
/// # Sign convention
///
/// A positive `offset_deg` moves a tier's facet(s) toward HIGHER index numbers:
/// each facet's azimuth `phi` (see
/// [`StandardGemCuts::index_to_azimuth`], `2 pi (index + reference) / gear`)
/// becomes `phi + offset_deg`, the same direction as the index shift
/// `Design::to_asc_schedule_from_solved_with_cheater_offsets` bakes into the
/// exported `.asc` indices. Facet normals are `(sin(theta) cos(phi), +-cos(theta),
/// sin(theta) sin(phi))`, so this is a rotation about `+Y` by `-offset_deg` in
/// `glam::DMat3::from_rotation_y`'s sense (which maps `phi` to `phi - angle`).
/// Neither `.asc` nor `GemCad` define a sign for a cheater angle; this is the
/// crate's one convention, shared by the solid, the tracer, the diagram and the
/// export.
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
            // Negated: `from_rotation_y(a)` maps azimuth `phi` to `phi - a`, and a
            // positive offset must move the facet to `phi + offset` (see above).
            let rotation = glam::DMat3::from_rotation_y(-offset_deg.to_radians());
            for plane in &mut facet_planes[prev..end] {
                plane.0 = rotation * plane.0;
            }
        }
        prev = end;
    }
    facet_planes
}

#[cfg(test)]
#[path = "planes_steps_tests.rs"]
mod steps_tests;
