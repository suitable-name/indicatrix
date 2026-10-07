//! A [`FacetMap`] for a stone that is only partly cut: the Cut slider's rough, and
//! every step in between.
//!
//! The planes a partly cut stone is drawn from are the finished stone's planes with
//! the hidden tiers' slices removed (see `Design::try_planes_for_visible_tiers`): the
//! dedup runs over the whole schedule first, and the survivors of a hidden tier are
//! dropped afterwards. A facet id the rasterizer reports is therefore an index into
//! THAT shorter list. A map built for the finished stone numbers the same facets
//! differently as soon as a hidden tier sat in the middle of the cutting order (a
//! design with concave tiers), and its concave facets start at the finished plane
//! count instead of the drawn one. [`FacetMap::from_design_cut`] numbers the facets
//! the way the drawn stone does.

use super::{FacetInfo, FacetMap};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;

impl FacetMap {
    /// [`Self::from_design_with_tools`] for a stone drawn with only the flat tiers
    /// `visible_tiers` marks.
    ///
    /// `visible_tiers` is indexed like `Design::tiers`; a missing entry counts as
    /// visible and `None` means every tier (the finished stone, exactly
    /// [`Self::from_design_with_tools`]). `placements` is the `(concave tier,
    /// placement)` list of the tools drawn with these planes, so tool `k` is facet
    /// `drawn_flat_count + k`.
    #[must_use]
    pub fn from_design_cut(
        design: &Design,
        solved: &[SolvedTier],
        placements: &[(usize, usize)],
        visible_tiers: Option<&[bool]>,
    ) -> Self {
        let Some(visible) = visible_tiers else {
            return Self::from_design_with_tools(design, solved, placements);
        };
        Self::from_design(design, solved)
            .restricted_to(visible)
            .with_tools(design, placements)
    }

    /// The map of the preform's planes plus the facets of the tiers `visible` marks,
    /// renumbered consecutively in their original order. `self` must be a flat-only
    /// map (no concave facets appended yet).
    fn restricted_to(&self, visible: &[bool]) -> Self {
        let shown = |info: &FacetInfo| {
            info.tier_index
                .is_none_or(|tier| visible.get(tier).copied().unwrap_or(true))
        };
        let mut facets: Vec<FacetInfo> = Vec::with_capacity(self.flat_facet_count);
        let mut orbits: Vec<Vec<u32>> = vec![Vec::new(); self.orbits.len()];
        for info in self.facets.iter().take(self.flat_facet_count) {
            if !shown(info) {
                continue;
            }
            if let Some(orbit) = info.tier_index.and_then(|tier| orbits.get_mut(tier)) {
                orbit.push(facets.len() as u32);
            }
            facets.push(info.clone());
        }
        Self {
            preform_plane_count: self.preform_plane_count,
            flat_facet_count: facets.len(),
            facets,
            orbits,
            concave_hover: Vec::new(),
        }
    }
}
