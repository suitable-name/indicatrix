//! [`exceeds_preform`] and its finding, [`PreformFit`] -- whether a schedule's
//! own facet planes (preform aside) imply a solid bigger than the stated
//! rough. See the parent module's doc comment on this function for why the
//! preform-LESS arrangement is what has to be measured.

use crate::design::Design;
use indicatrix::geometry::{
    GpuFacetPlane,
    cuts::StandardGemCuts,
    meet_solver::SolvedTier,
    stone_metrics::{SolidMetrics, SolidStatus, build_solid_mesh, measure_solid},
};

/// Whether the schedule's own facet planes -- **ignoring the preform entirely** --
/// exceed the stated rough's own dimensions, and by how much.
///
/// # Why this needs the preform-LESS arrangement
///
/// `Design::planes` always includes the preform's own half-spaces alongside the
/// schedule's facet planes, so the combined arrangement's measured extent can never
/// exceed the preform's -- it is a subset by construction. The facet planes ALONE
/// are under no such obligation: most real schedules never author an explicit
/// closing wall, relying on the preform's vertical walls instead, so a facets-alone
/// arrangement is typically [`SolidStatus::Unbounded`] (normal, not a warning) and
/// only occasionally closes on its own. When it DOES close, its extents are a
/// genuine fact about what the schedule's constraints alone imply -- and if that
/// solid is bigger than the stated preform in any axis, the preform is silently
/// truncating the design.
///
/// Named-axis comparison ([`PreformFit::exceeds_width`]/etc.), not a single number:
/// a stone can be too wide without being too tall and vice versa.
///
/// # Returns
///
/// `None` when there is nothing meaningful to compare: the facets alone don't close
/// into a solid (the common case, not itself a fit problem), or either solid fails
/// to measure.
#[must_use]
pub fn exceeds_preform(design: &Design, solved: &[SolvedTier]) -> Option<PreformFit> {
    let schedule = design.to_asc_schedule_from_solved(solved);
    let facet_planes: Vec<(glam::DVec3, f64)> = StandardGemCuts::from_asc_schedule(&schedule)
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();

    let SolidStatus::Closed(mesh) = build_solid_mesh(&facet_planes) else {
        return None;
    };
    let facets_alone = measure_solid(&facet_planes)?;
    let preform_planes = design.preform.planes_offset(design.preform_y_offset);
    let preform = measure_solid(&preform_planes)?;

    let fit = PreformFit {
        design_width: facets_alone.width_axis,
        design_length: facets_alone.length_axis,
        design_height: facets_alone.total_height,
        preform_width: preform.width_axis,
        preform_length: preform.length_axis,
        preform_height: preform.total_height,
        outside_halfspace: worst_halfspace_violation(&mesh.positions, &preform_planes),
    };
    fit.exceeds_any().then_some(fit)
}

/// Metrics of the finished stone for the yield figures: the flat solid's, with
/// the volume of the concave-carved mesh when the design has concave tiers
/// ([`Design::measure_from_solved_geom`], i.e. `build_solid_mesh_geom`).
///
/// Without concave tiers this is exactly `measure_solid(&design.planes_from_solved(solved))`,
/// so planar yield figures do not move. Concave tiers that cannot be resolved
/// (an invalid tier, too many placements) fall back to the flat stone: a yield
/// figure that is slightly high is a better answer for a half-edited design than
/// none, and the validation error is reported elsewhere.
pub(super) fn finished_metrics(design: &Design, solved: &[SolvedTier]) -> Option<SolidMetrics> {
    if design.concave_tiers.is_empty() {
        return measure_solid(&design.planes_from_solved(solved));
    }
    design
        .measure_from_solved_geom(solved)
        .unwrap_or_else(|_| measure_solid(&design.planes_from_solved(solved)))
}

/// The worst violation of `planes`' half-spaces (`n . x <= m`) by any of
/// `vertices` -- the offending plane's index into `planes` and how far
/// outside it the worst-violating vertex sits -- or `None` when every
/// vertex satisfies every plane within [`FIT_EPS`].
///
/// This is the exact, general preform-fit test.
/// [`PreformFit::exceeds_width`]/[`PreformFit::exceeds_length`]/
/// [`PreformFit::exceeds_height`] compare axis-aligned extents, which is
/// only equivalent to this for [`crate::preform::PreformShape::Block`]
/// (an axis-aligned box, with the design never rotated relative to its
/// preform in this crate's convention -- so the box's own faces line up
/// with the extents being compared). For any other
/// [`crate::preform::PreformShape`] (e.g.
/// [`crate::preform::PreformShape::Cylinder`], whose wall planes are not
/// axis-aligned), a vertex can sit inside every extent yet outside a wall
/// plane -- a square cross-section's corner poking past an octagon
/// preform's cut corner, say, while both AABBs agree exactly. Checking
/// every solved vertex against every preform half-space directly is the
/// rule that always gets this right, of which the extents comparison is
/// just the box special case.
pub(super) fn worst_halfspace_violation(
    vertices: &[glam::DVec3],
    planes: &[(glam::DVec3, f64)],
) -> Option<(usize, f64)> {
    let mut worst: Option<(usize, f64)> = None;
    for &v in vertices {
        for (i, &(n, m)) in planes.iter().enumerate() {
            let violation = n.dot(v) - m;
            let eps = FIT_EPS_REL * m.abs();
            if violation > eps && worst.is_none_or(|(_, w)| violation > w) {
                worst = Some((i, violation));
            }
        }
    }
    worst
}

/// One [`exceeds_preform`] finding.
///
/// The schedule's own facet-plane-only measured extents against the stated preform's
/// own, all in model (mast) units. Only ever constructed (by [`exceeds_preform`])
/// when at least one axis genuinely exceeds -- see [`Self::exceeds_any`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreformFit {
    /// Width of the design, in mast units.
    pub design_width: f64,
    /// Length of the design, in mast units.
    pub design_length: f64,
    /// Height of the design, in mast units.
    pub design_height: f64,
    /// Width of the preform, in mast units.
    pub preform_width: f64,
    /// Length of the preform, in mast units.
    pub preform_length: f64,
    /// Height of the preform, in mast units.
    pub preform_height: f64,
    /// The exact, general fit test: `Some((plane index, max violation))` when
    /// some solved-stone vertex sits outside one of the preform's own
    /// half-space planes (indexing [`crate::preform::PreformSpec::planes`]'s
    /// output), by how much (model units), else `None`. See
    /// [`worst_halfspace_violation`]'s doc comment for why this can catch a
    /// non-box preform's corner-cutting where the extents comparison alone
    /// cannot; for a [`crate::preform::PreformShape::Block`] preform the two
    /// agree in every PRACTICAL case (both gate on the same
    /// [`FIT_EPS_REL`]-scaled slack), but not EXACTLY: [`worst_halfspace_violation`]
    /// scales its slack by each plane's own offset (a `Block` preform's own
    /// HALF-width/length/height), while [`Self::exceeds_width`]/
    /// [`Self::exceeds_length`]/[`Self::exceeds_height`] scale theirs by the
    /// FULL preform extent -- a factor of two apart -- so a violation sized
    /// squarely between the two scaled slacks can trip one check without the
    /// other.
    pub outside_halfspace: Option<(usize, f64)>,
}

/// Relative slack (a fraction of the design's own preform extent, or of a
/// half-space plane's own offset) below which a design axis -- or a solved
/// vertex against a preform half-space -- is treated as "fits" even if
/// numerically a hair over the preform's own figure.
///
/// A FIXED, absolute mast-unit slack (rather than the old `1e-7` `FIT_EPS`)
/// is the wrong shape for this: [`exceeds_preform`] narrows its facet planes to
/// `f32` via `GpuFacetPlane` before measuring them (while the preform's own
/// planes stay `f64`), so the measurement noise this needs to absorb scales
/// with the DESIGN's own magnitude, not with a fixed mast-unit constant --  a
/// girdle mast set to EXACTLY the block preform's own half-width could still
/// measure `2.000000105 > 2.0 + 1e-7`, a false "design exceeds its stated
/// rough". See `a_design_whose_own_width_exactly_matches_the_preform_is_not_reported_as_exceeding`.
const FIT_EPS_REL: f64 = 1e-6;

impl PreformFit {
    /// `true` iff the design's own facet-plane-only width exceeds the preform's,
    /// by more than [`FIT_EPS_REL`] of the preform's own width.
    #[must_use]
    pub fn exceeds_width(&self) -> bool {
        self.design_width > FIT_EPS_REL.mul_add(self.preform_width.abs(), self.preform_width)
    }

    /// `true` iff the design's own facet-plane-only length exceeds the preform's,
    /// by more than [`FIT_EPS_REL`] of the preform's own length.
    #[must_use]
    pub fn exceeds_length(&self) -> bool {
        self.design_length > FIT_EPS_REL.mul_add(self.preform_length.abs(), self.preform_length)
    }

    /// `true` iff the design's own facet-plane-only total height exceeds the
    /// preform's, by more than [`FIT_EPS_REL`] of the preform's own height.
    #[must_use]
    pub fn exceeds_height(&self) -> bool {
        self.design_height > FIT_EPS_REL.mul_add(self.preform_height.abs(), self.preform_height)
    }

    /// `true` iff some solved-stone vertex lies outside one of the preform's
    /// own half-space planes -- the exact, general test; see
    /// [`Self::outside_halfspace`]'s doc comment.
    #[must_use]
    pub const fn exceeds_halfspace(&self) -> bool {
        self.outside_halfspace.is_some()
    }

    /// `true` iff any of the three axes exceeds, or the exact half-space
    /// check catches a poking-out vertex the extents alone missed -- what
    /// [`exceeds_preform`] gates its `Some`/`None` on.
    #[must_use]
    pub fn exceeds_any(&self) -> bool {
        self.exceeds_width()
            || self.exceeds_length()
            || self.exceeds_height()
            || self.exceeds_halfspace()
    }

    /// Scales every field by `mm_per_unit` (a LINEAR factor -- these are extents, not
    /// volumes; contrast [`super::scale::volume_mm3`]'s cube) for a caller that wants
    /// to show the mismatch in real millimetres rather than mast units.
    #[must_use]
    pub fn to_mm(&self, mm_per_unit: f64) -> Self {
        Self {
            design_width: self.design_width * mm_per_unit,
            design_length: self.design_length * mm_per_unit,
            design_height: self.design_height * mm_per_unit,
            preform_width: self.preform_width * mm_per_unit,
            preform_length: self.preform_length * mm_per_unit,
            preform_height: self.preform_height * mm_per_unit,
            outside_halfspace: self
                .outside_halfspace
                .map(|(plane, violation)| (plane, violation * mm_per_unit)),
        }
    }
}

impl std::fmt::Display for PreformFit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "design exceeds its stated rough: ")?;
        let mut parts = Vec::new();
        if self.exceeds_width() {
            parts.push(format!(
                "width {:.4} > preform {:.4}",
                self.design_width, self.preform_width
            ));
        }
        if self.exceeds_length() {
            parts.push(format!(
                "length {:.4} > preform {:.4}",
                self.design_length, self.preform_length
            ));
        }
        if self.exceeds_height() {
            parts.push(format!(
                "height {:.4} > preform {:.4}",
                self.design_height, self.preform_height
            ));
        }
        if let Some((plane, violation)) = self.outside_halfspace {
            parts.push(format!(
                "vertex outside preform plane {plane} by {violation:.4}"
            ));
        }
        write!(f, "{}", parts.join(", "))
    }
}
