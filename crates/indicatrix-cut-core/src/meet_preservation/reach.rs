//! How far each tier's facets reach inward from the girdle outline, and the travel a
//! requested turn implies from it ([`super::PreservationPolicy::bounds_follow_turn`]).
//!
//! With the outline held, a facet turned from slope `t` to `t'` about the outline carries a
//! point `l` inward from it by `l (tan t' - tan t)`. The reach of a facet is therefore
//! measured from the outline's support along the facet's own horizontal azimuth (the largest
//! `a . p` over every junction of the stone, `a` the unit horizontal part of the facet's
//! normal) inward to the facet's innermost ring vertex (the smallest `a . p` over its own
//! ring). That is the lever arm of the facet's innermost point about a hinge on the outline,
//! whether or not the facet itself touches the outline: a brilliant's pavilion main stops
//! short of the girdle, but the editor's anchoring turns it about the outline through the
//! breaks it follows, and its culet end travels by the full `(R - r_culet) d tan`. A facet
//! hinged nearer than the outline travels less, so the reach is an upper bound for it.
//!
//! The reaches are read once, in the original solved geometry, so a candidate's own repair
//! cannot enlarge the allowance it is judged by.
use super::{Plane, provenance::SurfaceKey};
use crate::design::Design;
use glam::DVec3;

/// A facet normal whose horizontal part is shorter than this has no azimuth: it is a flat,
/// and a flat never turns.
const HORIZONTAL_EPS: f64 = 1e-9;

/// Each tier's inward reach from the outline in model units, `None` when the tier has no
/// readable live facet (no face cycle, or a vertical normal).
///
/// `surfaces` and `cycles` are the baseline's: a cycle names its surface and the junction
/// indices of its ring, `points` are the junction points, and a tier owns the surfaces keyed
/// [`SurfaceKey::Facet`] with its id. A tier with several live facets reads the largest.
pub(super) fn tier_reaches(
    design: &Design,
    surfaces: &[(SurfaceKey, Plane)],
    points: &[DVec3],
    cycles: &[(usize, Vec<usize>)],
) -> Vec<Option<f64>> {
    (0..design.tiers.len())
        .map(|index| {
            let id = design.tier_id_at_or_synthetic(index);
            let owned = |key: &SurfaceKey| matches!(key, SurfaceKey::Facet(tier, _) if *tier == id);
            surfaces
                .iter()
                .enumerate()
                .filter(|(_, (key, _))| owned(key))
                .flat_map(|(surface, (_, (normal, _)))| {
                    let normal = *normal;
                    cycles
                        .iter()
                        .filter(move |(owner, _)| *owner == surface)
                        .filter_map(move |(_, ring)| facet_reach(normal, ring, points))
                })
                .reduce(f64::max)
        })
        .collect()
}

/// The reach of one facet: from the outline's support along the facet's azimuth to the
/// facet's innermost ring vertex. `None` for an empty ring or a facet without an azimuth.
fn facet_reach(normal: DVec3, ring: &[usize], points: &[DVec3]) -> Option<f64> {
    let horizontal = normal.x.hypot(normal.z);
    if ring.is_empty() || horizontal <= HORIZONTAL_EPS {
        return None;
    }
    let azimuth = DVec3::new(normal.x / horizontal, 0.0, normal.z / horizontal);
    let outer = points
        .iter()
        .map(|p| azimuth.dot(*p))
        .fold(f64::NEG_INFINITY, f64::max);
    let inner = ring
        .iter()
        .map(|&j| azimuth.dot(points[j]))
        .fold(f64::INFINITY, f64::min);
    let reach = outer - inner;
    reach.is_finite().then_some(reach.max(0.0))
}

/// The travel a request implies, in model units.
///
/// It is the largest `reach_i x turn_i` over the tiers with a positive tangent change,
/// `fallback` standing in for a tier whose reach could not be read (the owner's fallback:
/// half the stone's length).
pub(super) fn turn_travel(reaches: &[Option<f64>], fallback: f64, turn: &[f64]) -> f64 {
    reaches
        .iter()
        .zip(turn)
        .filter(|(_, change)| **change > 0.0)
        .map(|(reach, change)| reach.unwrap_or(fallback) * change)
        .fold(0.0, f64::max)
}
