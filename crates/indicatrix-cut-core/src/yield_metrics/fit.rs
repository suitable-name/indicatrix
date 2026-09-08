//! [`exceeds_preform`] and its finding, [`PreformFit`] -- whether a schedule's
//! own facet planes (preform aside) imply a solid bigger than the stated
//! rough. See the parent module's doc comment on this function for why the
//! preform-LESS arrangement is what has to be measured.

use crate::design::Design;
use indicatrix::geometry::{
    GpuFacetPlane,
    cuts::StandardGemCuts,
    meet_solver::SolvedTier,
    stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
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
    let preform_planes = design.preform.planes();
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
            if violation > FIT_EPS && worst.is_none_or(|(_, w)| violation > w) {
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
    pub design_width: f64,
    pub design_length: f64,
    pub design_height: f64,
    pub preform_width: f64,
    pub preform_length: f64,
    pub preform_height: f64,
    /// The exact, general fit test: `Some((plane index, max violation))` when
    /// some solved-stone vertex sits outside one of the preform's own
    /// half-space planes (indexing [`crate::preform::PreformSpec::planes`]'s
    /// output), by how much (model units), else `None`. See
    /// [`worst_halfspace_violation`]'s doc comment for why this can catch a
    /// non-box preform's corner-cutting where the extents comparison alone
    /// cannot; for a [`crate::preform::PreformShape::Block`] preform it
    /// always agrees with [`Self::exceeds_width`]/[`Self::exceeds_length`]/
    /// [`Self::exceeds_height`].
    pub outside_halfspace: Option<(usize, f64)>,
}

/// Absolute slack (model units) below which a design axis is treated as "fits" even
/// if numerically a hair over the preform's own figure -- absorbs the same
/// float/measurement noise `stone_metrics`'s own epsilon constants absorb, at a
/// comparable scale (mast units are of order 1).
const FIT_EPS: f64 = 1e-7;

impl PreformFit {
    /// `true` iff the design's own facet-plane-only width exceeds the preform's.
    #[must_use]
    pub fn exceeds_width(&self) -> bool {
        self.design_width > self.preform_width + FIT_EPS
    }

    /// `true` iff the design's own facet-plane-only length exceeds the preform's.
    #[must_use]
    pub fn exceeds_length(&self) -> bool {
        self.design_length > self.preform_length + FIT_EPS
    }

    /// `true` iff the design's own facet-plane-only total height exceeds the
    /// preform's.
    #[must_use]
    pub fn exceeds_height(&self) -> bool {
        self.design_height > self.preform_height + FIT_EPS
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
