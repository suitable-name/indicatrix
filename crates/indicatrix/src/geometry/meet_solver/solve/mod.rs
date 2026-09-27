//! The three-phase solve pipeline: [`SolveContext`] precomputes everything a
//! design's solve shares (normals, block classification, resolved named
//! references, anchors, priors) once, and [`SolveContext::run_pipeline`] runs
//! the constructive pass / block estimate / refinement sweeps described in the
//! module docs any number of times with different decision overrides -- the
//! reuse [`solve_meet_points_verified`](super::solve_meet_points_verified)'s
//! repair search depends on. [`solve_meet_points`] is the plain, no-override
//! entry point.
//!
//! Split into: [`context`] ([`SolveContext`]'s state and construction),
//! [`pipeline`] (the three-phase algorithm itself, [`PipelineResult`], and
//! the phase-2 [`Origin`](pipeline::Origin) bookkeeping), [`reporting`]
//! (mapping a pipeline run to [`super::SolvedTier`]s and the external
//! verification score), and [`entry`] (the two public entry points).

mod context;
mod entry;
mod pipeline;
mod reporting;
#[cfg(test)]
mod tests;

pub(super) use context::SolveContext;
pub use entry::{solve_meet_points, solve_meet_points_with};
pub(super) use pipeline::PipelineResult;
