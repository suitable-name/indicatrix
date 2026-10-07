//! The space one optimizer run searches: per-tier angle bounds, the hinges that let a
//! `ScaleReference` tier turn without leaving its point, and the tier relations that
//! follow a changed angle.
//!
//! # Turning an anchored tier about its hinge
//!
//! A `ScaleReference` tier carries its own mast, so changing only its angle would
//! swing the plane about the axis and move every vertex on it. The optimizer instead
//! keeps one point of the tier's facet fixed (the *hinge*, normally the vertex of the
//! tier's first live facet that sets the girdle edge, see
//! [`crate::design::tier_hinge_points`]) and recomputes the mast so the plane still
//! passes through it. The formula is the one retarget uses,
//! [`crate::design::mast_through`]: the plane is `n . x = mast` with the unit normal
//! `(sin(theta) cos(phi), +-cos(theta), sin(theta) sin(phi))` (`+` on the crown side,
//! `-` on the pavilion side).
//!
//! The azimuth `phi` is read from the starting design's own solved plane that the hinge
//! lies on ([`measure_anchors`]), exactly as `design::hinge` reads it, so the gear
//! reference angle and a cheater offset are already in it and a hinge on a facet other
//! than the tier's first plane still gets that facet's azimuth.
//!
//! # Tiers that follow a relation
//!
//! A tier whose angle follows a relation is never a free variable (see
//! [`super::free_tier_indices_with`]). When a candidate changes a tier it reads,
//! [`SearchSpace::write_angle`] moves the followers to the angles their relations give,
//! so the candidate that gets solved and scored is the design the user would really get.

use super::{
    candidate::{MAX_SAFE_CANDIDATE_ANGLE_DEG, candidate_angle_is_safe},
    options::OptimizeOptions,
};
use crate::design::{Design, FacetSide, mast_through, tier_plane_ranges};
use glam::DVec3;
use indicatrix::geometry::meet_solver::{MeetConstraint, SolvedTier};
use std::collections::BTreeMap;

/// The smallest mast a hinge may produce. A hinge behind the plane (or on the axis)
/// gives a mast at or below this and the candidate is dropped.
const MIN_HINGE_MAST: f64 = 1e-9;

/// How far either side of a tier's current angle a multi-start run draws starting
/// angles from when the request gives the tier no bounds: the editor's default range.
pub(super) const DEFAULT_START_RANGE_DEG: f64 = 5.0;

/// The smallest angle magnitude the default start box reaches, so a draw never lands on
/// the girdle plane.
const MIN_START_MAGNITUDE_DEG: f64 = 0.5;

/// Where one anchored tier turns: the hinge point and the azimuth of the facet plane the
/// hinge lies on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Anchor {
    /// The point (in design units) the facet turns about.
    hinge: DVec3,
    /// The azimuth of that facet's normal, radians.
    azimuth_rad: f64,
}

/// The anchor of every tier in `free` that `options` lets turn about a hinge, measured on
/// the starting design's solve (`solved`) and plane arrangement (`planes`, from
/// `Design::planes_from_solved`).
///
/// A tier whose planes were all deduplicated against earlier tiers has no plane to read
/// an azimuth from and gets no anchor.
pub(super) fn measure_anchors(
    design: &Design,
    options: &OptimizeOptions,
    free: &[usize],
    solved: &[SolvedTier],
    planes: &[(DVec3, f64)],
) -> BTreeMap<usize, Anchor> {
    let wanted: Vec<(usize, DVec3)> = free
        .iter()
        .filter_map(|&tier| {
            options
                .anchored_hinge(design, tier)
                .map(|hinge| (tier, hinge))
        })
        .collect();
    if wanted.is_empty() {
        return BTreeMap::new();
    }
    let ranges = tier_plane_ranges(design, solved);
    wanted
        .into_iter()
        .filter_map(|(tier, hinge)| {
            let range = ranges.get(tier)?;
            let (normal, _) = range
                .clone()
                .filter_map(|plane| planes.get(plane))
                .min_by(|a, b| plane_residual(a, hinge).total_cmp(&plane_residual(b, hinge)))?;
            Some((
                tier,
                Anchor {
                    hinge,
                    azimuth_rad: normal.z.atan2(normal.x),
                },
            ))
        })
        .collect()
}

/// How far `hinge` is from the plane `(normal, offset)`.
fn plane_residual(plane: &(DVec3, f64), hinge: DVec3) -> f64 {
    (plane.0.dot(hinge) - plane.1).abs()
}

/// Moves every tier that follows a relation to the angle its relation gives now.
///
/// `false` when the relations cannot be worked out, or give a follower an angle the
/// search may not use (see [`candidate_angle_is_safe`]): the caller drops the design. A
/// design without relations is left alone.
pub(super) fn settle_relations(design: &mut Design) -> bool {
    if design.tier_relations.is_empty() {
        return true;
    }
    let Ok(updates) = design.evaluate_relations() else {
        return false;
    };
    for (position, angle_deg) in updates {
        let Some(tier) = design.tiers.get_mut(position) else {
            return false;
        };
        if tier.angle_deg.to_bits() == angle_deg.to_bits() {
            continue;
        }
        if !candidate_angle_is_safe(tier.angle_deg, angle_deg) {
            return false;
        }
        tier.angle_deg = angle_deg;
    }
    true
}

/// The angle bounds, hinges and relation handling of one run, resolved against the design
/// it searches.
#[derive(Debug, Clone, Default)]
pub(super) struct SearchSpace {
    /// Tier index to inclusive `(low, high)` angle range, normalised so `low <= high`.
    bounds: BTreeMap<usize, (f64, f64)>,
    /// Anchor of every free tier that is varied as an anchored tier.
    anchors: BTreeMap<usize, Anchor>,
}

impl SearchSpace {
    /// Resolves the angle bounds of `options` and takes the measured `anchors` (see
    /// [`measure_anchors`]).
    pub(super) fn new(options: &OptimizeOptions, anchors: BTreeMap<usize, Anchor>) -> Self {
        let bounds = options
            .angle_bounds
            .iter()
            .filter(|(_, (low, high))| low.is_finite() && high.is_finite())
            .map(|(&tier, &(low, high))| (tier, (low.min(high), low.max(high))))
            .collect();
        Self { bounds, anchors }
    }

    /// Whether `angle_deg` is inside tier `tier_index`'s bounds (always, without any).
    pub(super) fn contains(&self, tier_index: usize, angle_deg: f64) -> bool {
        self.bounds
            .get(&tier_index)
            .is_none_or(|&(low, high)| angle_deg >= low && angle_deg <= high)
    }

    /// `angle_deg` pulled into tier `tier_index`'s bounds (unchanged without any).
    pub(super) fn clamp(&self, tier_index: usize, angle_deg: f64) -> f64 {
        self.bounds
            .get(&tier_index)
            .map_or(angle_deg, |&(low, high)| angle_deg.clamp(low, high))
    }

    /// The box a multi-start run draws tier `tier_index`'s starting angles from, as an
    /// inclusive `(low, high)` pair with `low <= high`: the tier's own bounds when it has
    /// some, otherwise [`DEFAULT_START_RANGE_DEG`] either side of `current_deg`, kept on
    /// the tier's own side of the girdle between `0.5` and
    /// [`MAX_SAFE_CANDIDATE_ANGLE_DEG`] degrees (the editor's default range rule).
    pub(super) fn start_box(&self, tier_index: usize, current_deg: f64) -> (f64, f64) {
        if let Some(&bounds) = self.bounds.get(&tier_index) {
            return bounds;
        }
        let magnitude = current_deg.abs();
        let low = (magnitude - DEFAULT_START_RANGE_DEG).max(MIN_START_MAGNITUDE_DEG);
        let high = (magnitude + DEFAULT_START_RANGE_DEG).min(MAX_SAFE_CANDIDATE_ANGLE_DEG);
        // A tier already outside the clamp keeps a degenerate box at its own angle.
        let (low, high) = if low <= high {
            (low, high)
        } else {
            (magnitude, magnitude)
        };
        if current_deg.is_sign_negative() {
            (-high, -low)
        } else {
            (low, high)
        }
    }

    /// Sets tier `tier_index`'s angle on `design`; for an anchored tier recomputes its mast
    /// so the facet still passes through the hinge, and moves every tier that follows a
    /// relation to its relation's angle. `false` (with the angle already written) when no
    /// usable mast exists or the relations cannot be satisfied; the caller drops the design.
    pub(super) fn write_angle(
        &self,
        design: &mut Design,
        tier_index: usize,
        angle_deg: f64,
    ) -> bool {
        design.tiers[tier_index].angle_deg = angle_deg;
        if let Some(anchor) = self.anchors.get(&tier_index) {
            let mast = mast_through(
                angle_deg,
                anchor.azimuth_rad,
                FacetSide::of_angle_deg(angle_deg),
                anchor.hinge,
            );
            if !(mast.is_finite() && mast > MIN_HINGE_MAST) {
                return false;
            }
            design.tiers[tier_index].constraint = MeetConstraint::ScaleReference(mast);
        }
        settle_relations(design)
    }
}
