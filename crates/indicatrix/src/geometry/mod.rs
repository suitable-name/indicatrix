pub mod brep;
pub mod cuts;
pub mod girdle;
pub mod meet_solver;
pub mod plane;
pub mod stone_metrics;

pub use brep::GemPolyhedron;
pub use cuts::{FacetSpec, StandardGemCuts};
pub use girdle::{classify_girdle_plane_indices, girdle_facet_finishes};
pub use meet_solver::{
    MeetConstraint, MeetTierInput, SolveStrategy, SolvedTier, build_reconstructed_schedule,
    meet_tier_inputs_from_asc, solve_meet_points, vertex_meet_groups,
};
pub use plane::GpuFacetPlane;
