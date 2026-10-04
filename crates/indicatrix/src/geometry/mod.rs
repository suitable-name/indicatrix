// `brep` carries its own module docs (algorithm, determinism guarantee).
pub mod brep;
/// Standard gemstone cut tables ([`StandardGemCuts`]).
///
/// Also reconstructs an `.asc` schedule into a validated B-rep.
pub mod cuts;
/// Classifies which facet planes form the girdle and looks up their finish.
pub mod girdle;
/// Solves meet-point tier constraints (index/angle/mast schedules) into concrete
/// facet planes.
pub mod meet_solver;
/// [`GpuFacetPlane`], the GPU-friendly `(normal, d)` plane representation shared by
/// the B-rep, the SIMD feasibility scan, and the raytracer.
pub mod plane;
/// Solid-geometry measurements (volume, surface area, bounding depth) derived from a
/// solved [`GemPolyhedron`].
pub mod stone_metrics;
/// Concave-facet tool volumes ([`ToolPrimitive`]) and the [`StoneGeometry`] that
/// pairs them with a stone's flat planes.
pub mod tool;

pub use brep::GemPolyhedron;
pub use cuts::{FacetSpec, StandardGemCuts};
pub use girdle::{classify_girdle_plane_indices, girdle_facet_finishes};
pub use meet_solver::{
    MeetConstraint, MeetTierInput, SolveStrategy, SolvedTier, build_reconstructed_schedule,
    meet_tier_inputs_from_asc, solve_meet_points, vertex_meet_groups,
};
pub use plane::GpuFacetPlane;
pub use tool::{
    MAX_TOOL_PRIMITIVES, StoneGeometry, ToolKind, ToolPrimitive, ToolPrimitiveError, ToolSweep,
};
