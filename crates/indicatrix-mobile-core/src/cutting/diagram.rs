//! The three-panel faceting diagram with one tier highlighted, for the Cut page.
//!
//! [`diagram_job`] copies the design and its solve out of the state; the [`DiagramJob`] is
//! `Send` and [`DiagramJob::render`] draws on any thread.

use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use indicatrix_cut_core::Design;
use indicatrix_solid::{
    diagram2d::{self, DiagramConfig, DiagramFrame, DiagramStyle},
    facet_map::FacetMap,
};

use super::{NotSolved, plan};
use crate::app_state::AppState;

/// A diagram to draw: the design, its solve, the tier to highlight and the frame size.
#[derive(Debug, Clone)]
pub struct DiagramJob {
    design: Design,
    solved: Vec<SolvedTier>,
    /// The flat tier (its position in `design.tiers`) to highlight; `None` highlights none.
    tier: Option<usize>,
    width: u32,
    height: u32,
}

impl DiagramJob {
    /// Draws the diagram. `None` when the design does not close to a solid.
    #[must_use]
    pub fn render(&self) -> Option<DiagramFrame> {
        render_highlighted(
            &self.design,
            &self.solved,
            self.tier,
            self.width,
            self.height,
        )
    }
}

/// Copies what the diagram of the open design with `tier` highlighted needs.
///
/// # Errors
/// [`NotSolved`] when the design has no solve of its current generation (or no tiers).
pub fn diagram_job(
    state: &AppState,
    tier: Option<usize>,
    width: u32,
    height: u32,
) -> Result<DiagramJob, NotSolved> {
    let _plan = plan(state)?;
    let solved = state
        .solve
        .current_tiers(state.session.current_generation())
        .ok_or(NotSolved)?
        .to_vec();
    Ok(DiagramJob {
        design: state.session.design.clone(),
        solved,
        tier,
        width,
        height,
    })
}

/// Draws `design`'s three-panel diagram at `width` x `height` with the facets of flat tier
/// `tier` highlighted (and the radial line to the tier's index on the wheel).
///
/// `None` when `solved` does not have one entry per tier, or the design does not close to
/// a solid.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "the gear's reference angle is a small number of degrees; the diagram works in f32"
)]
pub fn render_highlighted(
    design: &Design,
    solved: &[SolvedTier],
    tier: Option<usize>,
    width: u32,
    height: u32,
) -> Option<DiagramFrame> {
    if solved.len() != design.tiers.len() {
        return None;
    }
    let planes = design.planes_from_solved(solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return None;
    };
    let config = DiagramConfig {
        width,
        height,
        gear_teeth: design.meta.gear_teeth_abs(),
        gear_reference_angle: design.meta.gear_reference_angle as f32,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
    };
    let map = FacetMap::from_design(design, solved);
    let mut style = DiagramStyle {
        facet_labels: (0..map.facet_count())
            .map(|id| map.facet_label(id))
            .collect(),
        ..DiagramStyle::default()
    };
    if let Some(tier) = tier {
        let facets = map.facets_of_tier(tier);
        let mut selected = vec![false; map.facet_count()];
        for &facet in facets {
            if let Some(flag) = selected.get_mut(facet as usize) {
                *flag = true;
            }
        }
        style.selected = selected;
        style.selected_facet = facets.first().copied();
        style.facet_index_on_gear = (0..map.facet_count())
            .map(|id| map.index_on_gear(id))
            .collect();
    }
    Some(diagram2d::render_diagram(&mesh, &config, &style))
}
