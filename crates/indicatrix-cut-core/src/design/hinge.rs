//! Hinge points: where a tier's facet touches the girdle, and the mast that keeps it there.
//!
//! Changing a facet's angle while leaving its mast alone tilts the plane about the
//! stone's centre line, so the facet swings away from the girdle: a steeper crown drops
//! its girdle edge, a steeper pavilion lifts its own, and the girdle band between them
//! can shrink to nothing. A cutter re-cutting a facet at a new angle does the opposite:
//! the facet is tilted about the edge where it meets the girdle, and that edge stays put.
//!
//! This module supplies the geometry for that:
//!
//! - [`tier_hinges`] finds, for every flat crown or pavilion tier, the pivot point on the
//!   girdle side of the tier's first live facet in the SOLVED stone: for a facet that
//!   touches the girdle walls, its extreme wall vertex (the highest one for a crown
//!   facet, the lowest for a pavilion facet), so the stone's girdle top and bottom stay
//!   where they are; for any other facet, its vertex nearest the girdle (the midpoint of
//!   the edge when two vertices tie).
//! - [`mast_through`] gives the mast of the plane with a new angle that passes through
//!   such a point, using the same plane convention as
//!   `StandardGemCuts::from_asc_schedule`: normal `(sin t cos p, +-cos t, sin t sin p)`
//!   (plus for crown, minus for pavilion), plane `n . x = |mast|`.
//!
//! Only the point's distance from the centre line and its height matter: the plane
//! family with a fixed azimuth `p` and a changing `t` that passes through a point turns
//! about the horizontal line through that point, tangent to the girdle. Sliding the point
//! along that line changes nothing.
//!
//! The azimuth is read from the solved plane's own normal, not recomputed from the
//! index list, so a gear reference angle and a cheater offset are already in it.
//!
//! Also here: [`tier_plane_ranges`] and [`solid_facets_in`], which say which planes belong
//! to which tier and which of them reach the solid's surface. Retarget's validity checks
//! use them to tell a facet that survived from one that vanished.

use super::Design;
use glam::DVec3;
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    plane::tier_is_crown_side,
    stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// Two ring vertices whose heights differ by less than this belong to the same edge.
const EDGE_TIE_TOLERANCE: f64 = 1e-6;

/// Tiers this close to vertical (`|cos t|` at or below this) are girdle facets and have
/// no crown or pavilion hinge. Matches `classify_blocks`.
const GIRDLE_COS_EPS: f64 = 1e-6;

/// A plane whose normal has a vertical component this small is a girdle wall.
const WALL_NORMAL_Y_EPS: f64 = 1e-6;

/// A ring vertex this close to a wall plane lies on that wall.
const WALL_TOLERANCE: f64 = 1e-7;

/// Which side of the girdle a facet plane faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetSide {
    /// The normal points up (positive angle, or the table).
    Crown,
    /// The normal points down (negative angle, or the culet).
    Pavilion,
}

impl FacetSide {
    /// The side a tier at `angle_deg` is on, by the one rule every plane builder shares
    /// (`indicatrix::geometry::plane::tier_is_crown_side`).
    #[must_use]
    pub const fn of_angle_deg(angle_deg: f64) -> Self {
        if tier_is_crown_side(angle_deg) {
            Self::Crown
        } else {
            Self::Pavilion
        }
    }
}

/// The pivot of one tier's facet on the girdle side, in the solved stone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierHinge {
    /// The pivot point of the tier's first live facet (see the module docs for the rule).
    pub point: DVec3,
    /// The facet's azimuth, radians: the direction of its normal's horizontal part.
    pub azimuth_rad: f64,
    /// Which side of the girdle the facet is on.
    pub side: FacetSide,
    /// `true` when the facet has a whole edge on a girdle wall (two or more wall vertices):
    /// it is one of the facets that bound the girdle band, so it depends on no other facet
    /// for its position. A facet that only touches the wall at a point, or not at all,
    /// sits between those and the table or culet.
    pub wall_edge: bool,
}

/// The mast of the plane with slope `angle_deg` (magnitude; the side comes from `side`) and
/// azimuth `azimuth_rad` that passes through `hinge`.
///
/// The plane is `n . x = mast` with `n = (sin t cos p, +-cos t, sin t sin p)`, plus for
/// [`FacetSide::Crown`] and minus for [`FacetSide::Pavilion`] -- the convention
/// `StandardGemCuts::from_asc_schedule` builds its planes with. The result is the plane
/// offset, which is negative only when `hinge` lies on the far side of the stone's centre;
/// a caller that stores it as a mast should refuse such a value.
#[must_use]
pub fn mast_through(angle_deg: f64, azimuth_rad: f64, side: FacetSide, hinge: DVec3) -> f64 {
    let (sin_t, cos_t) = angle_deg.abs().to_radians().sin_cos();
    let (sin_p, cos_p) = azimuth_rad.sin_cos();
    let vertical = match side {
        FacetSide::Crown => cos_t,
        FacetSide::Pavilion => -cos_t,
    };
    let radial = cos_p.mul_add(hinge.x, sin_p * hinge.z);
    sin_t.mul_add(radial, vertical * hinge.y)
}

/// The planes each tier contributes, as index ranges into [`Design::planes_from_solved`]'s
/// combined list (the preform's planes come first, so the first range starts after them).
///
/// A tier whose plane duplicated an earlier tier's contributes fewer planes than it has
/// indices, so the ranges are the authoritative count, not `indices.len()`.
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`]: `solved` must have one entry
/// per tier.
#[must_use]
pub fn tier_plane_ranges(design: &Design, solved: &[SolvedTier]) -> Vec<Range<usize>> {
    let schedule = design.to_asc_schedule_from_solved(solved);
    let boundaries = crate::manufacturability::facet_plane_boundaries(&schedule);
    let preform_len = design.preform.planes().len();
    let mut start = 0;
    boundaries
        .iter()
        .map(|&end| {
            let range = preform_len + start..preform_len + end;
            start = end;
            range
        })
        .collect()
}

/// The set of plane indices whose face reaches the surface of `mesh`.
fn live_planes(mesh: &SolidMesh) -> BTreeSet<usize> {
    mesh.rings
        .iter()
        .filter(|(_, ring)| ring.len() >= 3)
        .map(|&(plane, _)| plane)
        .collect()
}

/// The vertex of `points` with the extreme height (the highest when `highest`, else the
/// lowest). Vertices tied in height are combined into the midpoint of their bounding box,
/// which is the midpoint of the edge when two vertices are tied and the vertex itself when
/// one is.
fn extreme_point(points: &[DVec3], highest: bool) -> Option<DVec3> {
    let key = |v: &DVec3| if highest { -v.y } else { v.y };
    let best = points.iter().map(key).fold(f64::INFINITY, f64::min);
    if !best.is_finite() {
        return None;
    }
    let (low, high) = points
        .iter()
        .filter(|v| key(v) <= best + EDGE_TIE_TOLERANCE)
        .fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), v| (lo.min(*v), hi.max(*v)),
        );
    Some((low + high) * 0.5)
}

/// The pivot of a facet ring.
///
/// A ring with vertices on a girdle wall pivots at its extreme wall vertex: the highest
/// for a crown facet, the lowest for a pavilion facet. Those are the vertices that set the
/// stone's girdle top and bottom, so keeping them keeps the measured girdle thickness. A
/// ring that touches no wall pivots at its vertex nearest the girdle: the lowest for a
/// crown facet, the highest for a pavilion facet.
///
/// The flag says whether the ring has a whole edge on a wall (two or more wall vertices).
fn hinge_of_ring(ring: &[DVec3], side: FacetSide, walls: &[(DVec3, f64)]) -> Option<(DVec3, bool)> {
    let on_wall: Vec<DVec3> = ring
        .iter()
        .copied()
        .filter(|vertex| {
            walls
                .iter()
                .any(|&(normal, offset)| (normal.dot(*vertex) - offset).abs() <= WALL_TOLERANCE)
        })
        .collect();
    if on_wall.is_empty() {
        extreme_point(ring, side == FacetSide::Pavilion).map(|point| (point, false))
    } else {
        extreme_point(&on_wall, side == FacetSide::Crown).map(|point| (point, on_wall.len() >= 2))
    }
}

/// [`tier_hinges`] over planes and a mesh the caller has already built (`planes` from
/// [`Design::planes_from_solved`], `mesh` from `build_solid_mesh(planes)`).
///
/// Maps a tier's position in [`Design::tiers`] to its hinge. Left out: horizontal tiers
/// (the table and the culet have no girdle edge to hinge on), girdle tiers, and any tier
/// none of whose facets reach the surface.
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`].
#[must_use]
pub fn tier_hinges_in(
    design: &Design,
    solved: &[SolvedTier],
    planes: &[(DVec3, f64)],
    mesh: &SolidMesh,
) -> BTreeMap<usize, TierHinge> {
    let ranges = tier_plane_ranges(design, solved);
    let rings: BTreeMap<usize, &Vec<DVec3>> = mesh
        .rings
        .iter()
        .filter(|(_, ring)| ring.len() >= 3)
        .map(|(plane, ring)| (*plane, ring))
        .collect();
    let walls: Vec<(DVec3, f64)> = planes
        .iter()
        .copied()
        .filter(|(normal, _)| normal.y.abs() <= WALL_NORMAL_Y_EPS)
        .collect();
    let mut hinges = BTreeMap::new();
    for (tier_index, (tier, range)) in design.tiers.iter().zip(&ranges).enumerate() {
        let theta = tier.angle_deg.abs().to_radians();
        if crate::optics_hints::is_horizontal_angle_deg(tier.angle_deg)
            || theta.cos().abs() <= GIRDLE_COS_EPS
        {
            continue;
        }
        let side = FacetSide::of_angle_deg(tier.angle_deg);
        let first_live = range
            .clone()
            .find_map(|plane| rings.get(&plane).map(|ring| (plane, *ring)));
        let Some((plane, ring)) = first_live else {
            continue;
        };
        let Some((point, wall_edge)) = hinge_of_ring(ring, side, &walls) else {
            continue;
        };
        let Some(&(normal, _)) = planes.get(plane) else {
            continue;
        };
        hinges.insert(
            tier_index,
            TierHinge {
                point,
                azimuth_rad: normal.z.atan2(normal.x),
                side,
                wall_edge,
            },
        );
    }
    hinges
}

/// The hinge of every flat crown or pavilion tier of the solved stone -- see
/// [`tier_hinges_in`] for what is left out.
///
/// Empty when the stone is not a closed solid.
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`].
#[must_use]
pub fn tier_hinges(design: &Design, solved: &[SolvedTier]) -> BTreeMap<usize, TierHinge> {
    let planes = design.planes_from_solved(solved);
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => tier_hinges_in(design, solved, &planes, &mesh),
        _ => BTreeMap::new(),
    }
}

/// [`tier_hinges`] reduced to the points: the midpoint of the girdle-side edge of each flat
/// tier's first live facet, keyed by the tier's position in [`Design::tiers`].
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`].
#[must_use]
pub fn tier_hinge_points(design: &Design, solved: &[SolvedTier]) -> BTreeMap<usize, DVec3> {
    tier_hinges(design, solved)
        .into_iter()
        .map(|(tier, hinge)| (tier, hinge.point))
        .collect()
}

/// How many of one tier's facet planes exist and how many reach the solid's surface.
#[derive(Debug, Clone, PartialEq)]
pub struct TierFacets {
    /// The facet planes the tier contributes.
    pub total: usize,
    /// How many of them have a face on the finished solid.
    pub alive: usize,
    /// The vertical component of the first live facet's normal: positive for a plane
    /// that faces up, negative for one that faces down. `None` without a live facet.
    pub first_live_normal_y: Option<f64>,
}

/// Which facets reach the surface of a solved, closed stone.
#[derive(Debug, Clone, PartialEq)]
pub struct SolidFacets {
    /// One entry per tier, in [`Design::tiers`] order.
    pub tiers: Vec<TierFacets>,
    /// How many of the preform's own planes show a face on the solid. Zero for a stone
    /// that sits entirely inside its rough.
    pub preform_alive: usize,
}

/// Counts the live facets of every tier, from planes and a mesh the caller has already
/// built (`planes` from [`Design::planes_from_solved`], `mesh` from `build_solid_mesh`).
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`].
#[must_use]
pub fn solid_facets_in(
    design: &Design,
    solved: &[SolvedTier],
    planes: &[(DVec3, f64)],
    mesh: &SolidMesh,
) -> SolidFacets {
    let live = live_planes(mesh);
    let preform_len = design.preform.planes().len();
    let tiers = tier_plane_ranges(design, solved)
        .into_iter()
        .map(|range| {
            let first_live = range.clone().find(|plane| live.contains(plane));
            TierFacets {
                total: range.len(),
                alive: range.filter(|plane| live.contains(plane)).count(),
                first_live_normal_y: first_live
                    .and_then(|plane| planes.get(plane))
                    .map(|&(normal, _)| normal.y),
            }
        })
        .collect();
    SolidFacets {
        tiers,
        preform_alive: live.iter().filter(|&&plane| plane < preform_len).count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design::{ConstraintTier, ScheduleMeta},
        edit::Edit,
        preform::PreformSpec,
    };
    use indicatrix::geometry::meet_solver::MeetConstraint;

    /// The standard round brilliant template in a rough deep enough that the preform never
    /// touches the stone: every tier pinned, so it solves and closes on its own.
    fn brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn tier_named(design: &Design, name: &str) -> usize {
        design
            .tiers
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no tier named {name}"))
    }

    #[test]
    fn a_kites_hinge_is_its_lowest_vertex() {
        let ring = [
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.8, 0.3, 0.2),
            DVec3::new(0.6, 0.5, 0.0),
            DVec3::new(0.8, 0.3, -0.2),
        ];
        assert_eq!(
            hinge_of_ring(&ring, FacetSide::Crown, &[]),
            Some((DVec3::new(1.0, 0.0, 0.0), false))
        );
    }

    #[test]
    fn a_crown_facets_hinge_is_the_midpoint_of_its_lowest_edge() {
        let ring = [
            DVec3::new(1.0, 0.1, -0.2),
            DVec3::new(1.0, 0.1, 0.2),
            DVec3::new(0.6, 0.5, 0.0),
        ];
        let (hinge, wall_edge) = hinge_of_ring(&ring, FacetSide::Crown, &[]).unwrap();
        assert!(
            (hinge - DVec3::new(1.0, 0.1, 0.0)).length() < 1e-12,
            "{hinge}"
        );
        assert!(!wall_edge);
    }

    #[test]
    fn a_facet_touching_a_wall_at_one_point_has_no_wall_edge() {
        let wall = [(DVec3::X, 1.0)];
        let ring = [
            DVec3::new(1.0, 0.02, 0.0),
            DVec3::new(0.8, 0.3, 0.2),
            DVec3::new(0.6, 0.5, 0.0),
            DVec3::new(0.8, 0.3, -0.2),
        ];
        assert_eq!(
            hinge_of_ring(&ring, FacetSide::Crown, &wall),
            Some((DVec3::new(1.0, 0.02, 0.0), false))
        );
    }

    #[test]
    fn a_pavilion_facets_hinge_is_the_midpoint_of_its_highest_edge() {
        let ring = [
            DVec3::new(1.0, -0.1, -0.2),
            DVec3::new(1.0, -0.1, 0.2),
            DVec3::new(0.0, -0.9, 0.0),
        ];
        let (hinge, _) = hinge_of_ring(&ring, FacetSide::Pavilion, &[]).unwrap();
        assert!(
            (hinge - DVec3::new(1.0, -0.1, 0.0)).length() < 1e-12,
            "{hinge}"
        );
    }

    #[test]
    fn an_empty_ring_has_no_hinge() {
        assert_eq!(hinge_of_ring(&[], FacetSide::Crown, &[]), None);
    }

    #[test]
    fn a_crown_facet_on_a_wall_pivots_at_its_highest_wall_vertex() {
        // A break facet whose bottom edge lies on the wall x = 1 and slopes up: the
        // girdle top is its higher end, so that is the pivot.
        let wall = [(DVec3::X, 1.0)];
        let ring = [
            DVec3::new(1.0, 0.01, -0.2),
            DVec3::new(1.0, 0.03, 0.2),
            DVec3::new(0.7, 0.2, 0.0),
        ];
        let hinge = hinge_of_ring(&ring, FacetSide::Crown, &wall).unwrap();
        assert_eq!(hinge, (DVec3::new(1.0, 0.03, 0.2), true));
    }

    #[test]
    fn a_pavilion_facet_on_a_wall_pivots_at_its_lowest_wall_vertex() {
        let wall = [(DVec3::X, 1.0)];
        let ring = [
            DVec3::new(1.0, -0.01, -0.2),
            DVec3::new(1.0, -0.03, 0.2),
            DVec3::new(0.2, -0.8, 0.0),
        ];
        let hinge = hinge_of_ring(&ring, FacetSide::Pavilion, &wall).unwrap();
        assert_eq!(hinge, (DVec3::new(1.0, -0.03, 0.2), true));
    }

    #[test]
    fn a_facet_that_misses_every_wall_ignores_them() {
        let wall = [(DVec3::X, 5.0)];
        let ring = [
            DVec3::new(0.9, 0.1, -0.2),
            DVec3::new(0.9, 0.1, 0.2),
            DVec3::new(0.6, 0.5, 0.0),
        ];
        let with_wall = hinge_of_ring(&ring, FacetSide::Crown, &wall);
        let without = hinge_of_ring(&ring, FacetSide::Crown, &[]);
        assert_eq!(with_wall, without);
    }

    #[test]
    fn mast_through_follows_the_asc_plane_convention() {
        // Crown, facing +x: n = (sin t, cos t, 0); through (1, 0.2, 0).
        let t = 30.0_f64.to_radians();
        let crown = mast_through(30.0, 0.0, FacetSide::Crown, DVec3::new(1.0, 0.2, 0.0));
        assert!((crown - 0.2_f64.mul_add(t.cos(), t.sin())).abs() < 1e-12);
        // Pavilion: n = (sin t, -cos t, 0); through (1, -0.2, 0).
        let pavilion = mast_through(-30.0, 0.0, FacetSide::Pavilion, DVec3::new(1.0, -0.2, 0.0));
        assert!((pavilion - 0.2_f64.mul_add(t.cos(), t.sin())).abs() < 1e-12);
        // Azimuth a quarter turn round: the radial direction is z.
        let quarter = mast_through(
            30.0,
            std::f64::consts::FRAC_PI_2,
            FacetSide::Crown,
            DVec3::new(0.0, 0.2, 1.0),
        );
        assert!((quarter - crown).abs() < 1e-12);
    }

    #[test]
    fn the_original_angle_through_its_own_hinge_gives_back_the_solved_mast() {
        let design = brilliant();
        let solved = design.solve().expect("every tier is pinned");
        let hinges = tier_hinges(&design, &solved);
        let mut flat: Vec<usize> = [
            "Star",
            "Crown Main",
            "Upper Girdle",
            "Pavilion Main",
            "Lower Girdle",
        ]
        .iter()
        .map(|name| tier_named(&design, name))
        .collect();
        flat.sort_unstable();
        assert_eq!(hinges.keys().copied().collect::<Vec<_>>(), flat);
        for (tier_index, hinge) in &hinges {
            let tier = &design.tiers[*tier_index];
            let mast = mast_through(tier.angle_deg, hinge.azimuth_rad, hinge.side, hinge.point);
            assert!(
                (mast - solved[*tier_index].mast).abs() < 1e-5,
                "{}: through its own hinge {mast}, solved {}",
                tier.name,
                solved[*tier_index].mast
            );
        }
    }

    #[test]
    fn the_table_the_girdle_and_the_culet_have_no_hinge() {
        let design = brilliant();
        let solved = design.solve().unwrap();
        let hinges = tier_hinges(&design, &solved);
        for name in ["Table", "Girdle", "Culet"] {
            assert!(
                !hinges.contains_key(&tier_named(&design, name)),
                "{name} must not have a hinge"
            );
        }
    }

    #[test]
    fn hinge_points_are_the_points_of_the_hinges() {
        let design = brilliant();
        let solved = design.solve().unwrap();
        let hinges = tier_hinges(&design, &solved);
        let points = tier_hinge_points(&design, &solved);
        assert_eq!(hinges.len(), points.len());
        for (tier, hinge) in &hinges {
            assert_eq!(points[tier], hinge.point);
        }
    }

    #[test]
    fn the_girdle_breaking_facets_hinge_on_the_girdle() {
        let design = brilliant();
        let solved = design.solve().unwrap();
        let hinges = tier_hinges(&design, &solved);
        // The girdle walls stand at a radius of 1.0 (the girdle facets' mast), so the
        // lowest point of an upper-girdle facet and the highest of a lower-girdle facet
        // sit on the girdle band, just either side of its middle.
        let upper = hinges[&tier_named(&design, "Upper Girdle")].point;
        let lower = hinges[&tier_named(&design, "Lower Girdle")].point;
        for point in [upper, lower] {
            let radius = point.x.hypot(point.z);
            assert!(radius > 0.95 && radius < 1.06, "radius {radius}");
            assert!(point.y.abs() < 0.1, "height {}", point.y);
        }
    }

    #[test]
    fn a_main_facets_hinge_is_on_its_side_of_the_girdle() {
        let design = brilliant();
        let solved = design.solve().unwrap();
        let hinges = tier_hinges(&design, &solved);
        // A main facet meets the girdle-breaking facets (or touches the girdle at a point),
        // so its hinge sits within the girdle radius and near the girdle band, the crown
        // main's above the pavilion main's.
        let crown = hinges[&tier_named(&design, "Crown Main")].point;
        let pavilion = hinges[&tier_named(&design, "Pavilion Main")].point;
        for point in [crown, pavilion] {
            let radius = point.x.hypot(point.z);
            assert!(radius > 0.3 && radius < 1.06, "radius {radius}");
            assert!(point.y.abs() < 0.5, "height {}", point.y);
        }
        assert!(
            crown.y > pavilion.y,
            "crown hinge {} vs pavilion hinge {}",
            crown.y,
            pavilion.y
        );
    }

    #[test]
    fn a_tilted_plane_through_the_hinge_keeps_the_hinge_on_it() {
        let mut design = brilliant();
        let solved = design.solve().unwrap();
        let hinges = tier_hinges(&design, &solved);
        for name in [
            "Crown Main",
            "Pavilion Main",
            "Upper Girdle",
            "Lower Girdle",
        ] {
            let tier_index = tier_named(&design, name);
            let hinge = hinges[&tier_index];
            let old_angle = design.tiers[tier_index].angle_deg;
            let new_angle = old_angle.signum().mul_add(3.0, old_angle);
            let mast = mast_through(new_angle, hinge.azimuth_rad, hinge.side, hinge.point);
            design
                .apply_edit(Edit::RetargetAngles {
                    changes: vec![(tier_index, old_angle, new_angle)],
                })
                .unwrap();
            design
                .apply_edit(Edit::SetConstraint {
                    index: tier_index,
                    constraint: MeetConstraint::ScaleReference(mast),
                })
                .unwrap();
            let resolved = design.solve().unwrap();
            let planes = design.planes_from_solved(&resolved);
            let plane = tier_plane_ranges(&design, &resolved)[tier_index].start;
            let (normal, offset) = planes[plane];
            assert!(
                (normal.dot(hinge.point) - offset).abs() < 1e-5,
                "{name}: the hinge left its plane by {}",
                normal.dot(hinge.point) - offset
            );
        }
    }

    #[test]
    fn plane_ranges_cover_every_tiers_facets_in_order() {
        let design = brilliant();
        let solved = design.solve().unwrap();
        let ranges = tier_plane_ranges(&design, &solved);
        assert_eq!(ranges.len(), design.tiers.len());
        let preform_len = design.preform.planes().len();
        assert_eq!(ranges[0].start, preform_len);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        for (tier, range) in design.tiers.iter().zip(&ranges) {
            assert_eq!(range.len(), tier.indices.len().max(1), "{}", tier.name);
        }
    }

    #[test]
    fn every_facet_of_the_brilliant_is_alive_and_the_preform_is_untouched() {
        let design = brilliant();
        let solved = design.solve().unwrap();
        let planes = design.planes_from_solved(&solved);
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            panic!("the brilliant must close");
        };
        let facets = solid_facets_in(&design, &solved, &planes, &mesh);
        assert_eq!(facets.preform_alive, 0);
        for (tier, counts) in design.tiers.iter().zip(&facets.tiers) {
            assert_eq!(counts.alive, counts.total, "{}", tier.name);
        }
        let table = &facets.tiers[tier_named(&design, "Table")];
        assert!(table.first_live_normal_y.unwrap() > 0.99);
        let culet = &facets.tiers[tier_named(&design, "Culet")];
        assert!(culet.first_live_normal_y.unwrap() < -0.99);
    }
}
