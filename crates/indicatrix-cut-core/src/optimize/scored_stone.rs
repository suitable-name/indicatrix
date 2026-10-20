//! [`ScoredStone`]: the stone every optimizer score measures, planes and concave tools.

use super::objective::to_gpu_planes;
use crate::design::Design;
use glam::DVec3;
use indicatrix::geometry::{
    GpuFacetPlane, StoneGeometry, stone_metrics::SolidMetrics, tool::ToolPrimitive,
};

/// The stone a design is scored on: its flat facets as the tracer takes them, and the
/// concave tools cut out of them.
///
/// Every scoring path of the optimizer measures this, so the figures describe the stone
/// the Compare window draws and not its flat shell. A design with no concave tiers (or
/// whose tiers do not resolve, see [`Design::tools_for_scoring`]) has no tools, and then
/// the geometry is the planes alone, bit for bit what the optimizer always scored.
pub(super) struct ScoredStone {
    planes: Vec<GpuFacetPlane>,
    tools: Vec<ToolPrimitive>,
}

impl ScoredStone {
    /// The stone of `design` whose flat arrangement is `planes`
    /// ([`Design::planes_from_solved`]).
    pub(super) fn of(design: &Design, planes: &[(DVec3, f64)]) -> Self {
        Self {
            planes: to_gpu_planes(planes),
            tools: design.tools_for_scoring(planes),
        }
    }

    /// The borrowed geometry the tracer takes.
    pub(super) fn geometry(&self) -> StoneGeometry<'_> {
        StoneGeometry {
            planes: &self.planes,
            tools: &self.tools,
        }
    }

    /// `flat` (the `measure_solid` of `planes`, the flat arrangement this stone was built
    /// from) with the volume of the stone carved by its tools: what the yield term reads, so
    /// a groove or a dimple costs yield the way it costs the cutter material.
    ///
    /// Without tools this is `flat` itself, bit for bit. When the tools leave no closed stone
    /// to measure the flat figures stand (as an unresolved tier scores flat): a yield loss of
    /// `0.0` would read as a free cut. Only the volume differs; the widths, heights and the
    /// table the shape penalty reads are the flat stone's by the carved measure's own
    /// definition ([`Design::carved_metrics`]), so the shape penalty has no carved variant.
    pub(super) fn yield_metrics(
        &self,
        planes: &[(DVec3, f64)],
        flat: Option<&SolidMetrics>,
    ) -> Option<SolidMetrics> {
        if self.tools.is_empty() {
            return flat.copied();
        }
        Design::carved_metrics(flat.copied(), planes, &self.tools).or_else(|| flat.copied())
    }
}
