//! Deterministic physical measurement of a faceted stone from its plane
//! arrangement.
//!
//! [`super::meet_solver`]'s corpus work established that wrong mast
//! configurations are *self-consistent*: every tier of a wrong solve is still
//! vertex-incident, so nothing internal to the arrangement separates right from
//! wrong. The proportions printed on a real diagram (`Vol/W^3`, `L/W`, `C/W`,
//! `P/W`, `H/W`) are **external** constraints: a candidate configuration either
//! reproduces them or it does not. [`measure_solid`] computes those same figures
//! from a plane arrangement -- deterministically, in `f64`, with no convex-hull
//! library ([`super::brep`]'s `chull` hull is nondeterministic and must stay out
//! of every solver decision path).
//!
//! The mechanism: enumerate the solid's vertices as feasible triple
//! intersections of the planes (the same primitive `meet_solver` uses), then
//! reconstruct each facet's polygon by collecting the vertices on its plane and
//! ordering them by angle. Volume comes from the divergence theorem
//! (`V = (1/3) * sum over faces of offset * area`, exact for outward-oriented
//! planes), heights from vertex `y` extents against the girdle band, and
//! width/length from the girdle outline's `x`/`z` extents (both axis-aligned
//! and rotating-caliper are computed; corpus measurement settled that the
//! printed figures use the axis convention -- see [`ExternalProportions`]).
//!
//! Split into: [`types`] (the reported data shapes), [`vertices`] (plane-
//! arrangement vertex enumeration and dedup, shared by both entry points),
//! [`measure`] ([`measure_solid`] itself), [`mesh`] ([`build_solid_mesh`] and
//! face reconstruction), [`caliper`] (the rotating-caliper cross-check), and
//! [`proportions`] ([`StoneProportions`], the cutter-facing readout).

mod caliper;
mod measure;
mod mesh;
mod proportions;
#[cfg(test)]
mod tests;
mod types;
mod vertices;

pub use measure::measure_solid;
pub use mesh::build_solid_mesh;
pub use proportions::StoneProportions;
pub use types::{ExternalProportions, SolidMesh, SolidMetrics, SolidStatus};

use super::girdle::GIRDLE_NORMAL_Y_EPSILON;

/// Half-extent of the bounding box standing in for the uncut rough. Matches
/// `meet_solver`'s own blank; a solid that reaches it is unbounded (a schedule
/// missing its closing planes), which [`measure_solid`] reports as `None`.
const BLANK_HALF_EXTENT: f64 = 64.0;

/// Feasibility slack: a vertex may poke this far (absolute; masts are ~1) beyond
/// a plane and still count as part of the solid. Matches `meet_solver::EPS_FEAS`.
const EPS_FEAS: f64 = 1e-5;

/// A vertex within this absolute distance of a plane counts as lying on its
/// face. Sized to cover [`EPS_FEAS`] feasibility drift and `VERTEX_DEDUP`
/// merging; the resulting polygon-corner error is quadratically small in it.
const EPS_FACE: f64 = 2e-5;

/// Minimum `|determinant|` for a triple of unit plane normals to define a
/// candidate vertex. Matches `meet_solver::MIN_TRIPLE_DET`.
const MIN_TRIPLE_DET: f64 = 1e-6;

/// Two candidate vertices within this distance (per axis) are one vertex.
const VERTEX_DEDUP: f64 = 1e-6;

/// `|normal.y|` at or below this means a vertical (girdle) plane.
///
/// Shares [`super::girdle::GIRDLE_NORMAL_Y_EPSILON`] rather than defining its own
/// separate threshold -- see that constant's doc comment for why a second,
/// independently-tuned value here would be wrong: this module's planes carry the
/// same f32-normalize rounding noise `girdle::is_girdle_plane`'s planes do, not the
/// exact-angle precision `meet_solver::classify_blocks` operates on, so a threshold
/// tuned for that exact-angle case could disagree with this one and classify a facet
/// as girdle in one module but not the other.
const GIRDLE_NY: f64 = GIRDLE_NORMAL_Y_EPSILON as f64;

/// `normal.y` at or above this counts as a flat table facet: an outward
/// normal within this of straight up (`+Y`) -- the horizontal-plane
/// counterpart to [`GIRDLE_NY`]'s vertical-plane threshold.
const TABLE_NY: f64 = 1.0 - 1e-6;
