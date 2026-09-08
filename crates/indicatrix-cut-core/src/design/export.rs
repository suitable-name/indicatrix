//! [`Design`]'s already-solved-to-output side: building an
//! [`indicatrix_formats::asc::AscSchedule`], the full plane arrangement, and the meshed
//! solid/its measurements -- see [`Design::to_asc_schedule_from_solved`]'s doc
//! comment for why each has an "already solved" variant alongside the
//! solve-it-yourself convenience wrapper.

use super::{Design, MissingAnchor};
use glam::DVec3;
use indicatrix::geometry::{
    GpuFacetPlane,
    cuts::StandardGemCuts,
    meet_solver::{MeetConstraint, SolvedTier},
    stone_metrics::{SolidMetrics, SolidStatus, build_solid_mesh, measure_solid},
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

    /// Builds a real [`AscSchedule`] from this design's current state: every
    /// tier's mast is [`Self::solve`]'d, and its `G`-field notes text is
    /// synthesized from the tier's [`MeetConstraint`] kind (not preserved verbatim
    /// from import -- this round-trips semantically, not byte-for-byte: parsing the
    /// synthesized notes back reclassifies to the same [`MeetConstraint`] variant).
    /// This is what makes `.asc` export and [`Self::planes`] both work from the same
    /// authored state.
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
                notes: constraint_notes(&tier.constraint),
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
    /// plane set, plus one plane per facet the schedule's tiers describe (via
    /// [`Self::to_asc_schedule`] then [`StandardGemCuts::from_asc_schedule`], the
    /// same production path a real `.asc` file's recorded masts go through).
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
        let mut planes = self.preform.planes();
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
}

/// Synthesizes one tier's `.asc` `G`-field text from its [`MeetConstraint`] -- see
/// [`Design::to_asc_schedule`] for the semantic (not byte-for-byte) round-trip this
/// supports. Each arm's text reclassifies to the same variant via
/// `AscTier::meet_instruction`/`meet_tier_inputs_from_asc`:
///
/// - [`MeetConstraint::MeetExisting`] -> empty (no `G` field -- the common case).
/// - [`MeetConstraint::MeetNamed`] -> `"Meet <names>"`, the exact instruction text
///   `indicatrix_formats::asc::MeetInstruction::Meet` parses.
/// - [`MeetConstraint::ScaleReference`] -> `"Set stone size."`, one of several real
///   phrasings `indicatrix_formats::asc::MeetInstruction::ScaleReference` recognizes (this
///   crate does not track which specific phrasing an imported tier originally used,
///   only that it is a scale reference).
fn constraint_notes(constraint: &MeetConstraint) -> String {
    match constraint {
        MeetConstraint::MeetExisting => String::new(),
        MeetConstraint::MeetNamed(names) => format!("Meet {}", names.join(", ")),
        MeetConstraint::ScaleReference(_) => "Set stone size.".to_string(),
    }
}
