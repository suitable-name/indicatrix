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
//! library. (Since 2026-09-28 [`super::brep`] no longer uses one either: it is
//! built on this module's own [`vertices`] arrangement walk. The meet solver
//! still must not depend on `brep`.)
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
//! face reconstruction), [`concave`] (the tool-carved mesh behind
//! [`build_solid_mesh_geom`]), [`caliper`] (the rotating-caliper cross-check), and
//! [`proportions`] ([`StoneProportions`], the cutter-facing readout).

mod caliper;
mod concave;
mod measure;
mod mesh;
mod proportions;
#[cfg(test)]
mod tests;
mod types;
mod vertices;

pub use caliper::{CaliperFrame, caliper_frame};
pub use measure::{measure_solid, measure_solid_with_vertices};
pub use mesh::{
    TOOL_ICOSPHERE_LEVEL, TOOL_SEGMENTS, build_solid_mesh, build_solid_mesh_geom, mesh_volume,
    tessellate_tool,
};
pub use proportions::StoneProportions;
pub use types::{ExternalProportions, SolidMesh, SolidMetrics, SolidStatus};

// Shared with `geometry::brep`, which reconstructs its B-rep from the same
// deterministic arrangement walk (see that module's docs).
pub(super) use measure::max_abs_offset;
pub(super) use vertices::{escaping_plane_indices, for_each_feasible_triple};

use super::girdle::GIRDLE_NORMAL_Y_EPSILON;

// One definition of the blank box half-extent (a solid that reaches it is
// unbounded -- a schedule missing its closing planes -- which [`measure_solid`]
// reports as `None`), the feasibility slack (a vertex may poke this far beyond a
// plane and still count as part of the solid) and the minimum triple
// determinant, shared with the meet solver's own candidate enumeration.
pub(super) use super::meet_solver::{BLANK_HALF_EXTENT, EPS_FEAS, MIN_TRIPLE_DET};

/// A vertex within this absolute distance of a plane counts as lying on its
/// face. Sized to cover [`EPS_FEAS`] feasibility drift and `VERTEX_DEDUP`
/// merging; the resulting polygon-corner error is quadratically small in it.
const EPS_FACE: f64 = 2e-5;

/// Two candidate vertices within this distance (per axis) are one vertex.
const VERTEX_DEDUP: f64 = 1e-6;

/// `|normal.y|` at or below this means a vertical (girdle) plane.
///
/// Purpose: classify `f32`-derived plane normals widened to `f64`, hence the
/// `1e-3` noise floor; `meet_solver::classify_blocks` works on exact `f64`
/// schedule angles (`1e-6`) and `meet_solver::names` picks its girdle tier by an
/// 85 degree naming heuristic, so the three thresholds answer different questions.
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

/// Rounds a design's representative absolute scale (the largest `|m|`
/// among its planes) to the nearest power of two. [`measure_solid`] and
/// [`build_solid_mesh`] divide every plane offset by this before running any
/// vertex-arrangement geometry, and multiply their length/volume results back
/// by it afterward -- [`EPS_FEAS`], [`EPS_FACE`], [`VERTEX_DEDUP`] and
/// `BLANK_HALF_EXTENT` are all absolute constants tuned for masts of order 1,
/// so a design authored at a very different absolute scale (see
/// [`super::meet_solver`]'s sibling fix in `SolveContext::new`, same
/// mechanism, same reasoning) would otherwise make them too tight or too
/// loose. Non-finite or non-positive input (an empty plane list, or every
/// offset exactly `0.0`) falls back to `1.0`, i.e. no normalisation.
/// `scale == 1.0` (or already an exact power of two) makes this `1.0` exactly,
/// so every division/multiplication by it is a bit-exact no-op.
///
/// Computed from the IEEE exponent and mantissa (`scale = m * 2^k`, `m` in
/// `[1, 2)`; the result is `2^k` when `m < sqrt(2)`, else `2^(k+1)`), with no
/// platform `log2`/`powi`. The exponent is clamped to the normal range.
pub(super) fn pow2_scale_norm(scale: f64) -> f64 {
    const MANTISSA: u64 = (1 << 52) - 1;
    /// 2^64, lifts a subnormal into the normal range.
    const SUBNORMAL_LIFT: f64 = 18_446_744_073_709_551_616.0;
    if !(scale.is_finite() && scale > 0.0) {
        return 1.0;
    }
    // `scale` is positive, so the sign bit is clear and `bits >> 52` is the biased exponent.
    let raw = scale.to_bits();
    let (bits, lift) = if raw >> 52 == 0 {
        ((scale * SUBNORMAL_LIFT).to_bits(), 64_i64)
    } else {
        (raw, 0_i64)
    };
    let biased = i64::from(u16::try_from(bits >> 52).unwrap_or(1023));
    let mantissa = f64::from_bits((bits & MANTISSA) | (1023_u64 << 52));
    let mut exponent = biased - 1023 - lift;
    if mantissa >= std::f64::consts::SQRT_2 {
        exponent += 1;
    }
    let biased_out = u64::try_from((exponent + 1023).clamp(1, 2046)).unwrap_or(1023);
    f64::from_bits(biased_out << 52)
}
