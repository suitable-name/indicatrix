//! The picture Cutting Mode shows beside the numerals.
//!
//! One panel of the faceting diagram
//! (crown, pavilion or profile, whichever shows the step), enlarged, with the tier being
//! cut highlighted and labelled.
//!
//! [`DiagramScene`] holds everything that does not change from step to step (the solid
//! mesh, the facet map and the labels), so stepping through a design only redraws. It is
//! plain data and `Send + Sync`: build it on the UI thread from the current solve, hand it
//! to a background thread and render there. The pixels come back as RGBA8
//! ([`DiagramPixels`]); the app turns them into a Slint image.

use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh},
};
use indicatrix_cut_core::Design;
use indicatrix_editor::cutting_mode::StepSide;
use indicatrix_solid::{
    diagram2d::{self, DiagramConfig, DiagramStyle, PanelKind},
    facet_map::FacetMap,
};

/// The highlight color of the tier being cut (RGB): a vivid orange that reads on the
/// diagram's dark background and next to the default light facet fill.
const STEP_COLOR: [u8; 3] = [255, 159, 28];

/// The panel that shows a step best: the crown from above for a crown step, the pavilion
/// from below for a pavilion step, the side profile for a girdle step.
#[must_use]
pub const fn panel_for(side: StepSide) -> PanelKind {
    match side {
        StepSide::Crown => PanelKind::Crown,
        StepSide::Pavilion => PanelKind::Pavilion,
        StepSide::Girdle => PanelKind::Profile,
    }
}

/// A rendered diagram: RGBA8, row-major, `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagramPixels {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed RGBA8 pixels.
    pub rgba: Vec<u8>,
}

/// The step-independent part of the diagram of one solved design.
#[derive(Debug, Clone)]
pub struct DiagramScene {
    mesh: SolidMesh,
    facet_map: FacetMap,
    labels: Vec<String>,
    index_on_gear: Vec<u32>,
    gear_teeth: u32,
    gear_reference_angle: f32,
    symmetry_order: u32,
    mirror: bool,
}

impl DiagramScene {
    /// The scene of `design` at its solved masts `solved` (one per flat tier).
    ///
    /// `None` when `solved` does not fit the design's tiers, or the planes do not close
    /// to a real solid: there is no diagram to draw for a stone that is not closed.
    #[must_use]
    pub fn build(design: &Design, solved: &[SolvedTier]) -> Option<Self> {
        if solved.len() != design.tiers.len() {
            return None;
        }
        let planes = design.planes_from_solved(solved);
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            return None;
        };
        let facet_map = FacetMap::from_design(design, solved);
        let count = facet_map.facet_count();
        let labels = (0..count).map(|id| facet_map.facet_label(id)).collect();
        let index_on_gear = (0..count).map(|id| facet_map.index_on_gear(id)).collect();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the render precision; a gear reference angle is a small degree value"
        )]
        let gear_reference_angle = design.meta.gear_reference_angle as f32;
        Some(Self {
            mesh,
            facet_map,
            labels,
            index_on_gear,
            gear_teeth: design.meta.gear_teeth_abs(),
            gear_reference_angle,
            symmetry_order: design.meta.symmetry_order,
            mirror: design.meta.mirror,
        })
    }

    /// Draws one panel of the diagram, enlarged to fill `width` x `height` pixels, with
    /// the facets of flat tier `tier` (an index into `design.tiers`) highlighted; no
    /// highlight for `None` or for a tier the diagram has no facet for (a concave tier).
    /// A cutter sees one panel at a time: [`panel_for`] picks the one that shows a step.
    #[must_use]
    pub fn render(
        &self,
        tier: Option<usize>,
        panel: PanelKind,
        width: u32,
        height: u32,
    ) -> DiagramPixels {
        let config = DiagramConfig {
            width,
            height,
            gear_teeth: self.gear_teeth,
            gear_reference_angle: self.gear_reference_angle,
            symmetry_order: self.symmetry_order,
            mirror: self.mirror,
        };
        let mut style = DiagramStyle {
            selected_color: STEP_COLOR,
            facet_labels: self.labels.clone(),
            facet_index_on_gear: self.index_on_gear.clone(),
            ..DiagramStyle::default()
        };
        if let Some(tier) = tier {
            let mut selected = vec![false; self.facet_map.facet_count()];
            for &facet in self.facet_map.facets_of_tier(tier) {
                if let Some(flag) = selected.get_mut(facet as usize) {
                    *flag = true;
                }
            }
            style.selected = selected;
        }
        let frame = diagram2d::render_diagram_single_panel(&self.mesh, &config, &style, panel);
        DiagramPixels {
            width: frame.width,
            height: frame.height,
            rgba: frame.color,
        }
    }
}
