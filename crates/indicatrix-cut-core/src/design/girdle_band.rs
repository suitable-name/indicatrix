//! The thinnest point of the girdle band.
//!
//! `indicatrix::geometry::stone_metrics::SolidMetrics::girdle_thickness` is the height of the
//! highest girdle-wall vertex above the lowest one: the band's overall extent, read off two
//! extreme vertices. It says nothing about the band at the corners between two walls, which is
//! where a girdle runs out first. A retarget turns every wall-edge facet about exactly those two
//! extreme vertices ([`super::hinge`]), so it can never change that figure, yet the corners can
//! still thin to a knife edge or the crown-side and pavilion-side planes can cross there.
//!
//! [`girdle_band_in`] measures what the extent cannot see: for every live girdle wall, the
//! smallest vertical gap between its crown-side edge and its pavilion-side edge, which is the
//! thinnest the band gets anywhere around the outline.
//!
//! # Why the two ends of a wall are enough
//!
//! A wall facet is a convex polygon in a vertical plane. Its upper boundary (the crown-side
//! edges) is a concave function of the position along the wall and its lower boundary (the
//! pavilion-side edges) a convex one, so the gap between them is a concave function: its smallest
//! value over the wall is at one of the two ends, the corners shared with the neighbouring walls.
//! A wall whose crown-side and pavilion-side planes cross before the corner closes to a point
//! there, so its gap is zero.
//!
//! # Why a corner is read with a resolution
//!
//! Every plane of a design goes through `f32` (`GpuFacetPlane`), so the four planes that meet at a
//! wall's corner (two walls and two crown-side or pavilion-side facets) do not share one point
//! exactly. Each triple of them solves to a point a few times 1e-7 of the stone's size away from
//! the others, and the mesh keeps whichever came first. The corner's top vertex and its bottom
//! vertex therefore stand a little apart ALONG the wall even though the edge between them is
//! vertical. Measured on a level girdle (crown and pavilion facets at the walls' own positions, so
//! every wall is a rectangle): up to 3e-7 of the largest plane offset, on every wheel and scale
//! tried. The first version of this measure took only the vertices within 1e-6 of the wall's width
//! of its extreme end, which is 4e-7 on a 0.4 wide wall: one vertex of the corner fell outside,
//! the corner read as a point, and half the walls of a level girdle came out as a knife edge.
//!
//! The measure now takes every vertex within [`corner_resolution`] (1e-5 of the stone's size,
//! some 30 times the noise and 10 times the mesh's own vertex merge) of an end, and a gap that is
//! no larger than that resolution counts as exactly zero: a band that has truly run out leaves
//! its two corner vertices that close, and they must still read as a knife edge.
//!
//! # What the knife-edge floor is
//!
//! Because of that snap, every reading is either exactly `0.0` or larger than the resolution.
//! The real floor under which a band counts as a knife edge is therefore [`corner_resolution`]:
//! 1e-5 of the stone's size, a few thousandths of a percent of its width for a design with the
//! templates' preform. [`KNIFE_EDGE_GAP`] only backs up a [`GirdleBand`] that did not come from
//! [`girdle_band_in`]; it is never what separates a real reading from a knife edge.
//!
//! # What "the stone's size" is
//!
//! The size is the largest `|offset|` of the whole plane arrangement, the preform's own planes
//! included, because that is the scale the solid mesh normalises by and merges its vertices at.
//! The resolution therefore follows the PREFORM's extent, not the cut stone's: an oversized rough
//! block makes it coarser. A block 50 times the girdle's distance from the axis gives a resolution
//! of 5e-4 of that distance, which is a tenth of the corner thickness of a standard round
//! brilliant (a quarter of a percent of its width); at several hundred times, those corners would
//! read as zero, the original would count as a knife edge and the corner check would stop seeing
//! them. The templates and the fixtures stay within a few times the stone, far from that.

use super::Design;
use glam::DVec3;
use indicatrix::geometry::stone_metrics::SolidMesh;
use std::ops::Range;

/// Tiers this close to vertical (`|cos t|` at or below this) are girdle tiers. Matches
/// `classify_blocks` and [`super::hinge`].
const GIRDLE_COS_EPS: f64 = 1e-6;

/// A wall's normal needs at least this much horizontal part to define a position along the wall.
const MIN_HORIZONTAL_NORMAL: f64 = 1e-9;

/// The length below which two points of a solved stone cannot be told apart, as a fraction of the
/// stone's size (its largest plane offset). See the module docs for the measured noise.
const CORNER_RESOLUTION: f64 = 1e-5;

/// The share of a wall's width that one end may take. Keeps the two ends of a sliver wall from
/// merging into one when the resolution is wider than the wall.
const MAX_END_SHARE: f64 = 0.25;

/// A band whose thinnest gap is this small or smaller (in the design's own units) is a knife
/// edge. A backstop only.
///
/// [`girdle_band_in`] never reports a gap between zero and its [`corner_resolution`], so for a
/// stone of any realistic size the real floor is that resolution and this constant only matters to
/// a [`GirdleBand`] built some other way.
pub const KNIFE_EDGE_GAP: f64 = 1e-6;

/// What the live girdle walls of a solved stone say about its band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GirdleBand {
    /// The smallest vertical gap between the crown-side and pavilion-side edges of any live
    /// wall, in the design's own units. Zero when a wall closes to a point at a corner.
    pub min_thickness: f64,
    /// How many girdle walls reach the surface of the solid.
    pub live_walls: usize,
}

impl GirdleBand {
    /// `true` when the band runs out to nothing somewhere: a reading of [`girdle_band_in`] is
    /// `0.0` exactly then ([`KNIFE_EDGE_GAP`] backs up a band built another way).
    #[must_use]
    pub const fn is_knife_edge(self) -> bool {
        self.min_thickness <= KNIFE_EDGE_GAP
    }
}

/// The length below which two points of a solved stone cannot be told apart. It is also the real
/// knife-edge floor: a gap at or under it reads as exactly zero.
///
/// It is the stone's size (the largest `|offset|` among `planes`, the preform's own planes
/// included, which is the scale the mesh normalises by) times a fixed fraction. So it follows the
/// PREFORM's extent, not the cut stone's: a larger rough block makes it coarser. See the module
/// docs for what it is sized from. `1.0` stands in for the size of a stone with no finite,
/// positive offset.
#[must_use]
pub fn corner_resolution(planes: &[(DVec3, f64)]) -> f64 {
    let size = planes
        .iter()
        .map(|&(_, offset)| offset.abs())
        .filter(|offset| offset.is_finite())
        .fold(0.0_f64, f64::max);
    CORNER_RESOLUTION * if size > 0.0 { size } else { 1.0 }
}

/// The thinnest vertical gap of one wall facet: the smaller of the gaps at its two ends.
///
/// `ring` is the facet's polygon (any vertex order) and `normal` its plane's outward normal.
/// `None` for a ring with fewer than three vertices or a normal with no horizontal part (a
/// plane that is not a wall). `resolution` is the finest length the mesh resolves, the stone's
/// [`corner_resolution`].
///
/// The gap at an end is the height spanned by every ring vertex within `resolution` of that end
/// along the wall (never more than a quarter of the wall's width, so a sliver keeps two ends). A
/// result no larger than `resolution` is exactly `0.0`: the band closes to a point there.
#[must_use]
pub fn wall_thinnest_within(ring: &[DVec3], normal: DVec3, resolution: f64) -> Option<f64> {
    let horizontal = normal.x.hypot(normal.z);
    if ring.len() < 3 || horizontal < MIN_HORIZONTAL_NORMAL {
        return None;
    }
    let resolution = if resolution.is_finite() {
        resolution.max(0.0)
    } else {
        0.0
    };
    let tangent = DVec3::new(-normal.z / horizontal, 0.0, normal.x / horizontal);
    let along = |vertex: &DVec3| tangent.dot(*vertex);
    let (low, high) = ring
        .iter()
        .map(along)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), u| {
            (lo.min(u), hi.max(u))
        });
    let tolerance = resolution.min(MAX_END_SHARE * (high - low));
    let gap_at = |end: f64| {
        let (bottom, top) = ring
            .iter()
            .filter(|vertex| (along(vertex) - end).abs() <= tolerance)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), vertex| {
                (lo.min(vertex.y), hi.max(vertex.y))
            });
        top - bottom
    };
    let thinnest = gap_at(low).min(gap_at(high));
    Some(if thinnest <= resolution {
        0.0
    } else {
        thinnest
    })
}

/// One live girdle wall of a solved stone: a plane of a girdle tier that has a facet ring on the
/// solid.
#[derive(Debug, Clone, Copy)]
pub struct GirdleWall<'a> {
    /// The wall's index in the stone's plane arrangement.
    pub plane: usize,
    /// The plane's outward normal.
    pub normal: DVec3,
    /// The wall's facet polygon on the solid (any vertex order).
    pub ring: &'a [DVec3],
}

/// The live girdle walls of a solved, closed stone, in plane order.
///
/// `ranges` are the tiers' plane ranges ([`super::hinge::tier_plane_ranges`]), `planes` the
/// stone's planes and `mesh` the closed solid they bound. Every plane of a girdle tier (one
/// within [`GIRDLE_COS_EPS`] of vertical) that has a facet ring on the solid is a live wall.
pub fn girdle_walls<'a>(
    design: &'a Design,
    ranges: &'a [Range<usize>],
    planes: &'a [(DVec3, f64)],
    mesh: &'a SolidMesh,
) -> impl Iterator<Item = GirdleWall<'a>> + 'a {
    design
        .tiers
        .iter()
        .zip(ranges)
        .filter(|(tier, _)| tier.angle_deg.abs().to_radians().cos().abs() <= GIRDLE_COS_EPS)
        .flat_map(|(_, range)| range.clone())
        .filter_map(move |plane| {
            let &(normal, _) = planes.get(plane)?;
            let (_, ring) = mesh
                .rings
                .iter()
                .find(|(index, ring)| *index == plane && ring.len() >= 3)?;
            Some(GirdleWall {
                plane,
                normal,
                ring: ring.as_slice(),
            })
        })
}

/// The girdle band of a solved, closed stone: [`girdle_walls`] read at the stone's
/// [`corner_resolution`].
///
/// `None` when no girdle wall is live: the design has no girdle tier, or none of its planes
/// reaches the surface.
#[must_use]
pub fn girdle_band_in(
    design: &Design,
    ranges: &[Range<usize>],
    planes: &[(DVec3, f64)],
    mesh: &SolidMesh,
) -> Option<GirdleBand> {
    let resolution = corner_resolution(planes);
    let mut band: Option<GirdleBand> = None;
    for wall in girdle_walls(design, ranges, planes, mesh) {
        let Some(thinnest) = wall_thinnest_within(wall.ring, wall.normal, resolution) else {
            continue;
        };
        band = Some(band.map_or(
            GirdleBand {
                min_thickness: thinnest,
                live_walls: 1,
            },
            |so_far| GirdleBand {
                min_thickness: so_far.min_thickness.min(thinnest),
                live_walls: so_far.live_walls + 1,
            },
        ));
    }
    band
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design::{ConstraintTier, ScheduleMeta, hinge::tier_plane_ranges},
        preform::PreformSpec,
    };
    use indicatrix::geometry::{
        meet_solver::MeetConstraint,
        stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    };

    /// The 16 wall positions of a 96-tooth wheel.
    const WALLS: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];

    fn standard_brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn pinned(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: indices.to_vec(),
            constraint: MeetConstraint::ScaleReference(mast),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// A brilliant whose crown and pavilion facets stand at the same positions as the walls,
    /// so each facet cuts its wall along a level line: the band is as thick at every corner as
    /// anywhere else.
    fn level_girdle_brilliant() -> Design {
        level_girdle_brilliant_at(1.0)
    }

    /// [`level_girdle_brilliant`] authored at `scale` times the size: every mast and the
    /// preform's half-width and depth. The preform's `length_over_width` is a RATIO (the half
    /// length is `half_width * length_over_width`), so it stays 1.0: scaling it too made the
    /// block's half length `2 * scale^2`, which at 0.25 is a half length of 0.125 under the
    /// girdle's 0.25 and cut away 10 of the 16 walls.
    fn level_girdle_brilliant_at(scale: f64) -> Design {
        Design::new(
            PreformSpec::block(2.0 * scale, 1.0, 4.0 * scale),
            ScheduleMeta::standard_round_brilliant(),
            vec![
                pinned("Table", 0.0, &[], 0.32 * scale),
                pinned("Crown", 34.5, &WALLS, 0.59 * scale),
                pinned("Girdle", 90.0, &WALLS, scale),
                pinned("Pavilion", -41.0, &WALLS, 0.67 * scale),
                pinned("Culet", -0.0, &[], 0.88 * scale),
            ],
        )
    }

    /// Solves `design` and returns its plane arrangement, the closed solid they bound and the
    /// tiers' plane ranges.
    fn solid_of(design: &Design) -> (Vec<(DVec3, f64)>, SolidMesh, Vec<Range<usize>>) {
        let solved = design.solve().expect("every tier is pinned");
        let planes = design.planes_from_solved(&solved);
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            panic!("the design must close");
        };
        let ranges = tier_plane_ranges(design, &solved);
        (planes, mesh, ranges)
    }

    /// Solves `design` and returns what the band measure sees, plus the solid's own girdle
    /// thickness.
    fn band_of(design: &Design) -> (Option<GirdleBand>, f64) {
        let (planes, mesh, ranges) = solid_of(design);
        let measured = measure_solid(&planes)
            .and_then(|metrics| metrics.girdle_thickness)
            .expect("the design has a girdle");
        (girdle_band_in(design, &ranges, &planes, &mesh), measured)
    }

    /// One wall's thinnest point at the resolution its own ring implies (1e-5 of its largest
    /// coordinate), for the fixtures that are a ring and nothing else.
    fn wall_thinnest(ring: &[DVec3], normal: DVec3) -> Option<f64> {
        let size = ring
            .iter()
            .map(|vertex| vertex.abs().max_element())
            .fold(0.0_f64, f64::max);
        wall_thinnest_within(ring, normal, CORNER_RESOLUTION * size)
    }

    /// Every live girdle wall of `design` with its reading, one per line, under the preform and
    /// the resolution they were read at: what a failing assertion on the number of live walls
    /// needs to show which walls the mesh kept and how they read.
    fn wall_readings(design: &Design) -> String {
        let (planes, mesh, ranges) = solid_of(design);
        let resolution = corner_resolution(&planes);
        let walls: Vec<String> = girdle_walls(design, &ranges, &planes, &mesh)
            .map(|wall| {
                let reading = wall_thinnest_within(wall.ring, wall.normal, resolution);
                format!(
                    "plane {}: normal {:?}, thinnest {reading:?}",
                    wall.plane, wall.normal
                )
            })
            .collect();
        format!(
            "preform {:?}, resolution {resolution}, {} live walls:\n{}",
            design.preform,
            walls.len(),
            walls.join("\n")
        )
    }

    /// The live girdle walls of `design` whose thinnest point is not within `tolerance` of
    /// `expected`, one per line with plane, normal, reading and ring: what a failing assertion
    /// needs to show which wall misread and why. Empty when every wall reads `expected`.
    fn walls_off(design: &Design, expected: f64, tolerance: f64) -> String {
        let (planes, mesh, ranges) = solid_of(design);
        let resolution = corner_resolution(&planes);
        girdle_walls(design, &ranges, &planes, &mesh)
            .filter_map(|wall| {
                let thinnest = wall_thinnest_within(wall.ring, wall.normal, resolution)?;
                ((thinnest - expected).abs() > tolerance).then(|| {
                    format!(
                        "plane {}: normal {:?}, thinnest {thinnest}, ring {:?}",
                        wall.plane, wall.normal, wall.ring
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_rectangular_wall_is_as_thin_as_its_height() {
        let ring = [
            DVec3::new(1.0, 0.1, -0.2),
            DVec3::new(1.0, 0.1, 0.2),
            DVec3::new(1.0, -0.1, 0.2),
            DVec3::new(1.0, -0.1, -0.2),
        ];
        let thinnest = wall_thinnest(&ring, DVec3::X).expect("a wall");
        assert!((thinnest - 0.2).abs() < 1e-12, "{thinnest}");
    }

    #[test]
    fn a_wall_is_as_thin_as_its_thinner_end() {
        // Left end 0.1 high, right end 0.3 high: the band is thinnest at the left corner.
        let ring = [
            DVec3::new(1.0, 0.05, -0.2),
            DVec3::new(1.0, 0.15, 0.2),
            DVec3::new(1.0, -0.15, 0.2),
            DVec3::new(1.0, -0.05, -0.2),
        ];
        let thinnest = wall_thinnest(&ring, DVec3::X).expect("a wall");
        assert!((thinnest - 0.1).abs() < 1e-12, "{thinnest}");
        // The vertex order does not matter.
        let mut shuffled = ring;
        shuffled.reverse();
        assert_eq!(wall_thinnest(&shuffled, DVec3::X), Some(thinnest));
    }

    #[test]
    fn a_wall_that_closes_to_a_point_at_a_corner_has_no_thickness_there() {
        // A diamond: the crown-side and pavilion-side edges cross at both ends.
        let ring = [
            DVec3::new(1.0, 0.0, -0.2),
            DVec3::new(1.0, 0.1, 0.0),
            DVec3::new(1.0, 0.0, 0.2),
            DVec3::new(1.0, -0.1, 0.0),
        ];
        assert_eq!(wall_thinnest(&ring, DVec3::X), Some(0.0));
    }

    #[test]
    fn the_wall_may_face_any_way() {
        // The same rectangle on the wall z = 1 (normal +z) and on a diagonal wall.
        let on_z = [
            DVec3::new(-0.2, 0.1, 1.0),
            DVec3::new(0.2, 0.1, 1.0),
            DVec3::new(0.2, -0.1, 1.0),
            DVec3::new(-0.2, -0.1, 1.0),
        ];
        assert!((wall_thinnest(&on_z, DVec3::Z).unwrap() - 0.2).abs() < 1e-12);
        let diagonal = DVec3::new(1.0, 0.0, 1.0).normalize();
        let tangent = DVec3::new(-diagonal.z, 0.0, diagonal.x);
        let centre = diagonal;
        let on_diagonal: Vec<DVec3> = [(-0.2, 0.1), (0.2, 0.1), (0.2, -0.1), (-0.2, -0.1)]
            .iter()
            .map(|&(along, height)| centre + tangent * along + DVec3::Y * height)
            .collect();
        assert!((wall_thinnest(&on_diagonal, diagonal).unwrap() - 0.2).abs() < 1e-12);
    }

    #[test]
    fn a_plane_that_is_not_a_wall_or_a_ring_that_is_not_a_polygon_has_no_thinnest_point() {
        let ring = [
            DVec3::new(0.0, 0.3, 0.0),
            DVec3::new(1.0, 0.3, 0.0),
            DVec3::new(0.0, 0.3, 1.0),
        ];
        assert_eq!(wall_thinnest(&ring, DVec3::Y), None);
        assert_eq!(wall_thinnest(&ring[..2], DVec3::X), None);
    }

    #[test]
    fn a_knife_edge_is_a_gap_of_nothing() {
        let band = |gap| GirdleBand {
            min_thickness: gap,
            live_walls: 4,
        };
        assert!(band(0.0).is_knife_edge());
        assert!(band(KNIFE_EDGE_GAP).is_knife_edge());
        assert!(!band(0.01).is_knife_edge());
    }

    #[test]
    fn a_design_with_no_girdle_tier_has_no_band() {
        let design = standard_brilliant();
        assert_eq!(
            girdle_band_in(&design, &[], &[], &SolidMesh::default()),
            None
        );
    }

    /// A wall of the level 96-tooth girdle (normal `+x`, so the position along the wall is `z`)
    /// as a replica of the mesh build produces it from this fixture's `f32` planes (the same
    /// triple solve, vertex merge and face tolerances; the figures are not idealised). Each end's
    /// bottom and top vertex stand up to 5e-7 apart along the wall, because the four planes of a
    /// corner do not share one point, though the edge between them is vertical.
    fn level_wall_as_meshed() -> [DVec3; 4] {
        [
            DVec3::new(1.0, -0.018_471_969, -0.198_912_6),
            DVec3::new(1.0, 0.028_628_703, -0.198_912_6 + 5.047e-7),
            DVec3::new(1.0, 0.028_628_847, 0.198_912_5),
            DVec3::new(1.0, -0.018_472_055, 0.198_912_5),
        ]
    }

    #[test]
    fn a_corner_whose_vertices_are_noise_apart_along_the_wall_still_has_its_height() {
        // The left end's top vertex lies 5e-7 inside its bottom vertex. The first version of the
        // measure took only vertices within 1e-6 of the wall's width (4e-7) of the extreme end,
        // left the top vertex out, and read this wall of a level girdle as a knife edge.
        let ring = level_wall_as_meshed();
        let width = 0.397_825_1;
        assert_eq!(
            wall_thinnest_within(&ring, DVec3::X, 1e-6 * width),
            Some(0.0),
            "the end tolerance that misread the wall"
        );
        // The mesh's own resolution (1e-5 of its size 4, the preform's largest offset).
        let thinnest = wall_thinnest_within(&ring, DVec3::X, 4e-5).expect("a wall");
        assert!(
            (thinnest - 0.047_100_672).abs() < 1e-9,
            "thinnest {thinnest}, ring {ring:?}"
        );
        // And from the ring alone.
        let alone = wall_thinnest(&ring, DVec3::X).expect("a wall");
        assert!((alone - thinnest).abs() < 1e-9, "{alone}");
    }

    #[test]
    fn a_corner_that_has_truly_run_out_is_a_knife_edge_even_when_its_vertices_are_noise_apart() {
        // The band closes at the left end: its two vertices stand 3e-6 apart in height and
        // 4e-7 along the wall, which is how the mesh leaves a point that is not merged. The
        // right end is a healthy 0.1 high.
        let ring = [
            DVec3::new(1.0, 0.0, -0.2),
            DVec3::new(1.0, 3e-6, -0.2 + 4e-7),
            DVec3::new(1.0, 0.05, 0.2),
            DVec3::new(1.0, -0.05, 0.2),
        ];
        assert_eq!(wall_thinnest_within(&ring, DVec3::X, 4e-5), Some(0.0));
        assert_eq!(wall_thinnest(&ring, DVec3::X), Some(0.0));
    }

    #[test]
    fn a_thin_corner_above_the_resolution_is_reported_as_it_is() {
        // Half a thousandth is real: twelve times the resolution, so it is not rounded to zero.
        let ring = [
            DVec3::new(1.0, 0.0, -0.2),
            DVec3::new(1.0, 5e-4, -0.2 + 4e-7),
            DVec3::new(1.0, 0.05, 0.2),
            DVec3::new(1.0, -0.05, 0.2),
        ];
        let thinnest = wall_thinnest_within(&ring, DVec3::X, 4e-5).expect("a wall");
        assert!((thinnest - 5e-4).abs() < 1e-12, "{thinnest}");
    }

    #[test]
    fn a_reading_is_exactly_zero_or_above_the_resolution() {
        // The knife-edge floor is the resolution: no reading falls between zero and it.
        let resolution = 4e-5;
        for gap in [0.0, 1e-6, 3e-5, 4e-5, 4.1e-5, 1e-4, 5e-4] {
            let ring = [
                DVec3::new(1.0, 0.0, -0.2),
                DVec3::new(1.0, gap, -0.2 + 4e-7),
                DVec3::new(1.0, 0.05, 0.2),
                DVec3::new(1.0, -0.05, 0.2),
            ];
            let thinnest = wall_thinnest_within(&ring, DVec3::X, resolution).expect("a wall");
            let reads_zero = thinnest <= 0.0;
            assert_eq!(reads_zero, gap <= resolution, "gap {gap} read {thinnest}");
            assert!(
                reads_zero || thinnest > resolution,
                "gap {gap} read {thinnest}"
            );
        }
    }

    #[test]
    fn a_sliver_wall_keeps_its_two_ends_apart() {
        // The wall is narrower than the resolution: one end may take only a quarter of it, so
        // the thin end (0.1 high) is not merged with the thick one (0.2 high).
        let ring = [
            DVec3::new(1.0, 0.05, -5e-6),
            DVec3::new(1.0, -0.05, -5e-6),
            DVec3::new(1.0, 0.1, 5e-6),
            DVec3::new(1.0, -0.1, 5e-6),
        ];
        let thinnest = wall_thinnest_within(&ring, DVec3::X, 4e-5).expect("a wall");
        assert!((thinnest - 0.1).abs() < 1e-12, "{thinnest}");
    }

    #[test]
    fn the_resolution_follows_the_stones_size() {
        let plane = |offset| (DVec3::X, offset);
        assert!((corner_resolution(&[plane(2.0), plane(-4.0), plane(0.5)]) - 4e-5).abs() < 1e-18);
        assert!((corner_resolution(&[plane(0.25)]) - 2.5e-6).abs() < 1e-18);
        // A non-finite offset is ignored, and a stone with none stands for size one.
        assert!((corner_resolution(&[plane(f64::NAN), plane(1.0)]) - 1e-5).abs() < 1e-18);
        assert!((corner_resolution(&[]) - 1e-5).abs() < 1e-18);
    }

    #[test]
    fn a_level_girdle_is_as_thin_at_its_corners_as_it_measures() {
        let design = level_girdle_brilliant();
        let (band, measured) = band_of(&design);
        let band = band.expect("a live girdle");
        assert_eq!(band.live_walls, 16, "{}", wall_readings(&design));
        // The extreme vertices and the corners differ by the planes' `f32` noise, about 3e-7;
        // 1e-5 is far under the 0.047 being measured.
        let tolerance = 1e-5;
        assert!(
            (band.min_thickness - measured).abs() < tolerance,
            "thinnest {} against measured {measured}; the walls that read differently:\n{}",
            band.min_thickness,
            walls_off(&design, measured, tolerance)
        );
        assert!(!band.is_knife_edge());
    }

    #[test]
    fn the_scaled_fixture_is_the_same_stone_in_every_direction() {
        // The preform's length over width is a ratio and must not scale: the fixture once
        // squared it, so the block at 0.25 was 0.125 long under a girdle 0.25 from the axis.
        let unit = level_girdle_brilliant_at(1.0).preform.planes();
        for scale in [0.25, 4.0] {
            let scaled = level_girdle_brilliant_at(scale).preform.planes();
            assert_eq!(scaled.len(), unit.len(), "scale {scale}");
            for ((normal, offset), (unit_normal, unit_offset)) in scaled.iter().zip(&unit) {
                assert_eq!(normal, unit_normal, "scale {scale}");
                assert!(
                    (offset - unit_offset * scale).abs() < 1e-12,
                    "scale {scale}: preform plane {normal:?} at {offset}, expected {}",
                    unit_offset * scale
                );
            }
        }
    }

    #[test]
    fn every_wall_of_a_level_girdle_reads_the_same_at_any_authoring_scale() {
        // Half the walls of this girdle read as a knife edge before the corner resolution; the
        // noise and the mesh's resolution both grow with the stone, so every scale is checked.
        // The stone is the same at every scale, so what it reads relative to its size must be
        // the same too.
        let mut relative: Vec<(f64, f64)> = Vec::new();
        for scale in [0.25, 1.0, 4.0] {
            let design = level_girdle_brilliant_at(scale);
            let (band, measured) = band_of(&design);
            let band = band.expect("a live girdle");
            assert_eq!(
                band.live_walls,
                16,
                "scale {scale}: {}",
                wall_readings(&design)
            );
            let tolerance = 1e-5 * 4.0 * scale;
            let off = walls_off(&design, measured, tolerance);
            assert!(
                off.is_empty(),
                "scale {scale}: walls off the measured {measured}:\n{off}"
            );
            assert!(
                (band.min_thickness - measured).abs() < tolerance && !band.is_knife_edge(),
                "scale {scale}: thinnest {} against measured {measured}\n{}",
                band.min_thickness,
                wall_readings(&design)
            );
            relative.push((band.min_thickness / scale, measured / scale));
        }
        for &(thinnest, measured) in &relative[1..] {
            assert!(
                (thinnest - relative[0].0).abs() < 1e-4 && (measured - relative[0].1).abs() < 1e-4,
                "the readings per unit of scale differ between scales: {relative:?}"
            );
        }
    }

    #[test]
    fn the_standard_brilliants_girdle_is_thinnest_at_its_corners() {
        // The crown and pavilion facets stand between the walls' positions, so the band is
        // some 12 % of the stone's height at its thickest vertex pair but only half a percent
        // at the corners between the walls.
        let (band, measured) = band_of(&standard_brilliant());
        let band = band.expect("a live girdle");
        assert_eq!(band.live_walls, 16);
        assert!(band.min_thickness > 0.0 && !band.is_knife_edge());
        assert!(
            band.min_thickness < measured / 10.0,
            "thinnest {} against measured {measured}",
            band.min_thickness
        );
        assert!(
            (band.min_thickness - 0.005).abs() < 1e-3,
            "{}",
            band.min_thickness
        );
    }
}
