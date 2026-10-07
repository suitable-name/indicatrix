//! Multi-stone planning in shaped rough geometries.
//!
//! Provides the once-per-plan model geometry ([`ctx`]), unit grid sizing ([`grid`]), spatial
//! piece classification and LP-based scaling ([`clip`], [`rows`]), positional 3-stage
//! guillotine DP ([`dp`]), tree placement ([`tree`]) with empty-piece merging ([`compact`]),
//! shaped uniform grids ([`uniform`]), and continuous position refinement ([`refine`]).
//! The stages that split their work over threads share one runner (`parallel`), which
//! returns results in job order so no result depends on the thread count.

pub mod clip;
pub mod compact;
pub mod ctx;
pub mod dp;
pub mod grid;
mod parallel;
pub mod refine;
pub mod rows;
pub mod tree;
pub mod uniform;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_brute;
#[cfg(test)]
mod tests_dp;
#[cfg(test)]
mod tests_grid_cap;
#[cfg(test)]
mod tests_hulls;
#[cfg(test)]
mod tests_mesh;
#[cfg(test)]
mod tests_mesh_perf;
#[cfg(test)]
mod tests_min_width;
#[cfg(test)]
mod tests_plan;

pub use clip::{
    BuildClipParams, CLASS_EXTERIOR, CLASS_INTERIOR, CLASS_PARTIAL, CLIP_SHORTLIST,
    CLIP_SHORTLIST_MAX, ClippedTable, GRID_POLL_PIECES, PLANE_EPS_MM, a_range_entry_range,
    a_range_jobs, build_clipped_a_range, build_clipped_table, build_clipped_table_jobs,
    build_clipped_table_lanes, classify_box, classify_box_in, classify_box_into, grid_poll_events,
    slice_entry_range,
};
pub use ctx::ShapedCtx;
pub use dp::{ShapedAltParams, ShapedOrderDp, plan_shaped_alternatives, plan_shaped_for_order};
pub use grid::{
    SHAPED_MAX_CELLS, SHAPED_MESH_PIECE_CAP, SHAPED_OP_CAP, SHAPED_PIECE_CAP, ShapedGrid,
    choose_shaped_grid, choose_shaped_grid_at, choose_shaped_grid_capped, piece_count,
    shaped_op_estimate,
};
pub use refine::refine_shaped;
pub use tree::layout_from_tree_shaped;
pub use uniform::shaped_uniform_layouts;
