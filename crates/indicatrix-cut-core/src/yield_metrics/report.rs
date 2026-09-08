//! [`YieldReport`] and [`Design::yield_report`], the one entry point that
//! ties [`super::scale`]'s figures and [`super::fit::exceeds_preform`]
//! together against a design's current, already-solved state.

use super::{
    fit::{PreformFit, exceeds_preform},
    scale::{carat_weight, mm_per_unit, volume_mm3, volumetric_yield},
};
use crate::design::Design;
use indicatrix::geometry::{meet_solver::SolvedTier, stone_metrics::measure_solid};

/// Every figure for one already-[`Design::solve`]'d design, bundled for a
/// caller (the editor UI) that wants them all at once -- see [`Design::yield_report`].
#[derive(Debug, Clone, PartialEq)]
pub struct YieldReport {
    /// [`super::scale::volumetric_yield`] of finished volume over preform volume.
    /// `None` only when the design isn't currently a closed solid, or the preform
    /// itself fails to measure (never `None` because of a missing mm scale or
    /// material -- see the parent module's doc comment).
    pub volumetric_yield: Option<f64>,
    /// [`super::scale::mm_per_unit`]'s scale factor, when
    /// [`Design::girdle_diameter_mm`] is set and the design measures. `None`
    /// otherwise -- and whenever this is `None`, `finished_volume_mm3`/
    /// `carat_weight` below are too (both depend on it).
    pub mm_per_unit: Option<f64>,
    /// The finished solid's own volume, converted to real mm^3 via `mm_per_unit`.
    pub finished_volume_mm3: Option<f64>,
    /// [`crate::material::MaterialSelection::effective_specific_gravity`] for
    /// `design.material` -- the SG actually used below, whether from a preset or a
    /// user override (see that method's own doc comment).
    pub specific_gravity_used: Option<f64>,
    /// [`super::scale::carat_weight`] of `finished_volume_mm3` at
    /// `specific_gravity_used`. `None` whenever either input is `None`.
    pub carat_weight: Option<f64>,
    /// [`exceeds_preform`]'s finding, when the design's own facet planes (preform
    /// aside) form a real solid that is bigger than the stated rough. `None` means
    /// "no fit problem detected" -- either it genuinely fits, or (the common case)
    /// the facets alone don't close into a solid at all, see that function's own doc
    /// comment.
    pub preform_fit: Option<PreformFit>,
}

impl Design {
    /// Every figure (volumetric yield, mm-anchored carat weight, and the
    /// preform-fit check) for this design's current state, computed from `solved` --
    /// an already-[`Self::solve`]'d (or [`Self::resolve_dirty`]'d) mast list -- rather
    /// than forcing a second solve. Same reasoning, and the same alignment contract,
    /// as [`crate::manufacturability::check_manufacturability`]'s own `solved`
    /// parameter: a real meet-derived design costs seconds to solve, and an editor
    /// that already has a `Vec<SolvedTier>` from its own "Solve" action should never
    /// pay that cost twice just to show a yield figure.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`]: `solved` must have
    /// one entry per tier `self` currently has, in the same order.
    #[must_use]
    pub fn yield_report(&self, solved: &[SolvedTier]) -> YieldReport {
        let finished = measure_solid(&self.planes_from_solved(solved));
        let preform = measure_solid(&self.preform.planes());

        let vol_yield = match (&finished, &preform) {
            (Some(f), Some(p)) => volumetric_yield(f.volume, p.volume),
            _ => None,
        };

        let scale = match (self.girdle_diameter_mm, &finished) {
            (Some(mm), Some(f)) => mm_per_unit(mm, f.width_axis),
            _ => None,
        };

        let finished_volume_mm3 = match (&finished, scale) {
            (Some(f), Some(scale)) => Some(volume_mm3(f.volume, scale)),
            _ => None,
        };

        let specific_gravity_used = self.material.effective_specific_gravity();

        let carat = match (finished_volume_mm3, specific_gravity_used) {
            (Some(v), Some(sg)) => Some(carat_weight(v, sg)),
            _ => None,
        };

        YieldReport {
            volumetric_yield: vol_yield,
            mm_per_unit: scale,
            finished_volume_mm3,
            specific_gravity_used,
            carat_weight: carat,
            preform_fit: exceeds_preform(self, solved),
        }
    }
}
