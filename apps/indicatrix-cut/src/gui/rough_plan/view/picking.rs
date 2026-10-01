//! Hover and click picking on the rough block: which edge or corner of its bounding box
//! the pointer is over.

use crate::gui::rough_plan::cut_faces::{
    CORNERS, EDGES, default_corner, default_edge, faces_label,
};
use glam::Vec3;
use indicatrix::optics::raytracer::Camera;
use indicatrix_cut_core::rough_plan::{BoxFace, RoughCut};
use indicatrix_solid::raster::project_point;

/// How close, in logical pixels, the pointer must be to an edge or corner to pick it.
pub(super) const PICK_RADIUS_PX: f32 = 10.0;

/// An edge or a corner of the rough's bounding box, by index into the fixed edge and
/// corner tables of the cut editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoxTarget {
    /// An edge: an index into [`EDGES`].
    Edge(usize),
    /// A corner: an index into [`CORNERS`].
    Corner(usize),
}

impl BoxTarget {
    /// "Click to cut corner Top-Front-Right".
    #[must_use]
    pub(super) fn hint(self) -> String {
        match self {
            Self::Edge(i) => format!("Click to cut edge {}", faces_label(&EDGES[i])),
            Self::Corner(i) => format!("Click to cut corner {}", faces_label(&CORNERS[i])),
        }
    }

    /// The cut a click on the target adds, with the default setbacks for a block of
    /// `extents_mm`.
    #[must_use]
    pub(super) fn default_cut(self, extents_mm: [f64; 3]) -> RoughCut {
        match self {
            Self::Edge(i) => default_edge(EDGES[i], extents_mm),
            Self::Corner(i) => default_corner(CORNERS[i], extents_mm),
        }
    }

    /// The target's points in the scene, for a box of half size `half`: two for an edge,
    /// one for a corner.
    #[must_use]
    pub(super) fn points(self, half: [f32; 3]) -> Vec<Vec3> {
        match self {
            Self::Corner(i) => vec![corner_point(half, &CORNERS[i])],
            Self::Edge(i) => {
                let faces = EDGES[i];
                let fixed = faces.map(|face| face.axis_and_side().0);
                let free = 3 - fixed[0] - fixed[1];
                let base = corner_point(half, &faces);
                let mut low = base;
                let mut high = base;
                low[free] = -half[free];
                high[free] = half[free];
                vec![low, high]
            }
        }
    }

    /// Whether any face at the target faces `camera`; a target on the far side of the
    /// box is not offered.
    fn visible(self, camera: &Camera, half: [f32; 3]) -> bool {
        let facing = |faces: &[BoxFace]| faces.iter().any(|&face| faces_camera(camera, half, face));
        match self {
            Self::Edge(i) => facing(&EDGES[i]),
            Self::Corner(i) => facing(&CORNERS[i]),
        }
    }
}

/// The point of a box of half size `half` named by `faces`; an axis no face names stays
/// at the centre.
fn corner_point(half: [f32; 3], faces: &[BoxFace]) -> Vec3 {
    let mut point = Vec3::ZERO;
    for face in faces {
        let (axis, high) = face.axis_and_side();
        point[axis] = if high { half[axis] } else { -half[axis] };
    }
    point
}

/// Whether `face` of a box of half size `half` (centred at the origin) faces the camera.
fn faces_camera(camera: &Camera, half: [f32; 3], face: BoxFace) -> bool {
    let (axis, high) = face.axis_and_side();
    let mut normal = Vec3::ZERO;
    normal[axis] = if high { 1.0 } else { -1.0 };
    let on_face = normal * half[axis];
    normal.dot(camera.origin - on_face) > 0.0
}

/// The distance from `p` to the segment `a`-`b`.
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let (apx, apy) = (p.0 - a.0, p.1 - a.1);
    let length_squared = abx.mul_add(abx, aby * aby);
    let t = if length_squared > f32::EPSILON {
        (apx.mul_add(abx, apy * aby) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.0 - t.mul_add(abx, a.0)).hypot(p.1 - t.mul_add(aby, a.1))
}

/// How far outside a cut's plane, in world units, a point of the uncut box may lie and
/// still count as a point of the cut solid.
const ON_SOLID_TOLERANCE: f32 = 1e-4;

/// The shortest stretch of an edge, in world units, that must be left after the cuts for
/// the edge to be offered.
const MIN_EDGE_LEFT: f32 = 1e-3;

/// The part of `target` that still exists after the cuts `cuts` (`n . p <= m` in world
/// units): a corner only when it is inside every cut's half-space, an edge as the stretch
/// that is. `None` when nothing of the target is left, for example a corner that a corner
/// cut removed. With no cuts this is [`BoxTarget::points`].
#[must_use]
pub(super) fn surviving_points(
    target: BoxTarget,
    half: [f32; 3],
    cuts: &[(Vec3, f32)],
) -> Option<Vec<Vec3>> {
    let points = target.points(half);
    match *points.as_slice() {
        [corner] => cuts
            .iter()
            .all(|&(normal, offset)| normal.dot(corner) <= offset + ON_SOLID_TOLERANCE)
            .then(|| vec![corner]),
        [a, b] => {
            let (low, high) = clip_segment(a, b, cuts)?;
            Some(vec![low, high])
        }
        _ => None,
    }
}

/// The part of the segment `a`-`b` inside every half-space of `cuts`, at least
/// [`MIN_EDGE_LEFT`] long.
fn clip_segment(a: Vec3, b: Vec3, cuts: &[(Vec3, f32)]) -> Option<(Vec3, Vec3)> {
    let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
    for &(normal, offset) in cuts {
        let at_a = normal.dot(a) - offset - ON_SOLID_TOLERANCE;
        let at_b = normal.dot(b) - offset - ON_SOLID_TOLERANCE;
        if at_a > 0.0 && at_b > 0.0 {
            return None;
        }
        if at_a <= 0.0 && at_b <= 0.0 {
            continue;
        }
        let crossing = at_a / (at_a - at_b);
        if at_a > 0.0 {
            enter = enter.max(crossing);
        } else {
            leave = leave.min(crossing);
        }
    }
    let direction = b - a;
    ((leave - enter) * direction.length() >= MIN_EDGE_LEFT)
        .then(|| (a + direction * enter, a + direction * leave))
}

/// The edge or corner of the box of half size `half` nearest to `cursor`, within
/// `radius` pixels, seen by `camera` in a `view` sized image. A corner beats an edge
/// when both are in reach. Targets the cuts `cuts` have removed are not offered, and an
/// edge that is cut short is measured along what is left of it.
#[must_use]
pub(super) fn pick_box(
    camera: &Camera,
    view: (u32, u32),
    half: [f32; 3],
    cuts: &[(Vec3, f32)],
    cursor: (f32, f32),
    radius: f32,
) -> Option<BoxTarget> {
    let screen = |point: Vec3| project_point(camera, point, view.0, view.1).map(|p| (p.0, p.1));

    let mut best_corner: Option<(f32, BoxTarget)> = None;
    for target in (0..CORNERS.len()).map(BoxTarget::Corner) {
        if !target.visible(camera, half) {
            continue;
        }
        let Some(points) = surviving_points(target, half, cuts) else {
            continue;
        };
        let Some(at) = screen(points[0]) else {
            continue;
        };
        let distance = (at.0 - cursor.0).hypot(at.1 - cursor.1);
        if distance <= radius && best_corner.is_none_or(|(best, _)| distance < best) {
            best_corner = Some((distance, target));
        }
    }
    if let Some((_, target)) = best_corner {
        return Some(target);
    }

    let mut best_edge: Option<(f32, BoxTarget)> = None;
    for target in (0..EDGES.len()).map(BoxTarget::Edge) {
        if !target.visible(camera, half) {
            continue;
        }
        let Some(ends) = surviving_points(target, half, cuts) else {
            continue;
        };
        let (Some(a), Some(b)) = (screen(ends[0]), screen(ends[1])) else {
            continue;
        };
        let distance = segment_distance(cursor, a, b);
        if distance <= radius && best_edge.is_none_or(|(best, _)| distance < best) {
            best_edge = Some((distance, target));
        }
    }
    best_edge.map(|(_, target)| target)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: (u32, u32) = (800, 600);
    const HALF: [f32; 3] = [0.6, 0.5, 0.4];

    fn camera() -> Camera {
        Camera::new(0.6, 0.45, 3.0, 42.0)
    }

    fn screen_of(camera: &Camera, point: Vec3) -> (f32, f32) {
        let p = project_point(camera, point, VIEW.0, VIEW.1).expect("in front of the camera");
        (p.0, p.1)
    }

    /// The corners ordered by their distance from the camera, nearest first.
    fn corners_by_distance(camera: &Camera) -> Vec<usize> {
        let mut order: Vec<usize> = (0..CORNERS.len()).collect();
        order.sort_by(|&a, &b| {
            let da = camera.origin.distance(corner_point(HALF, &CORNERS[a]));
            let db = camera.origin.distance(corner_point(HALF, &CORNERS[b]));
            da.total_cmp(&db)
        });
        order
    }

    #[test]
    fn the_cursor_on_a_visible_corner_picks_that_corner() {
        let camera = camera();
        let index = corners_by_distance(&camera)[0];
        let at = screen_of(&camera, corner_point(HALF, &CORNERS[index]));
        let picked = pick_box(
            &camera,
            VIEW,
            HALF,
            &[],
            (at.0 + 4.0, at.1 - 3.0),
            PICK_RADIUS_PX,
        );
        assert_eq!(picked, Some(BoxTarget::Corner(index)));
    }

    #[test]
    fn the_middle_of_a_visible_edge_picks_that_edge() {
        let camera = camera();
        // Top-Front is edge 0: the camera is above and in front, so it is visible.
        let ends = BoxTarget::Edge(0).points(HALF);
        let a = screen_of(&camera, ends[0]);
        let b = screen_of(&camera, ends[1]);
        let middle = (a.0.midpoint(b.0), a.1.midpoint(b.1));
        let picked = pick_box(
            &camera,
            VIEW,
            HALF,
            &[],
            (middle.0, middle.1 + 5.0),
            PICK_RADIUS_PX,
        );
        assert_eq!(picked, Some(BoxTarget::Edge(0)));
    }

    #[test]
    fn a_corner_within_reach_beats_an_edge_in_reach() {
        let camera = camera();
        let index = corners_by_distance(&camera)[0];
        let corner = corner_point(HALF, &CORNERS[index]);
        // A point on an edge leaving that corner, a few pixels along it, is within reach
        // of the edge and of the corner; the corner wins.
        let along = BoxTarget::Edge(0).points(HALF);
        let start = screen_of(&camera, corner);
        let toward = if along[0].distance(corner) < along[1].distance(corner) {
            screen_of(&camera, along[1])
        } else {
            screen_of(&camera, along[0])
        };
        let dx = toward.0 - start.0;
        let dy = toward.1 - start.1;
        let length = dx.hypot(dy).max(1.0);
        let cursor = (
            (dx / length).mul_add(6.0, start.0),
            (dy / length).mul_add(6.0, start.1),
        );
        assert_eq!(
            pick_box(&camera, VIEW, HALF, &[], cursor, PICK_RADIUS_PX),
            Some(BoxTarget::Corner(index))
        );
    }

    #[test]
    fn far_from_the_box_nothing_is_picked() {
        assert_eq!(
            pick_box(&camera(), VIEW, HALF, &[], (5.0, 5.0), PICK_RADIUS_PX),
            None
        );
    }

    /// The cut plane that removes the corner `faces` names, 0.2 world units deep along the
    /// corner's diagonal: `n` is the unit diagonal, `m = n . corner - 0.2`.
    fn corner_cut(index: usize) -> (Vec3, f32) {
        let corner = corner_point(HALF, &CORNERS[index]);
        let normal = corner.signum().normalize();
        (normal, normal.dot(corner) - 0.2)
    }

    #[test]
    fn a_corner_a_cut_removed_is_not_offered_and_the_others_are() {
        let camera = camera();
        let order = corners_by_distance(&camera);
        let (removed, kept) = (order[0], order[1]);
        let cuts = [corner_cut(removed)];
        let at = screen_of(&camera, corner_point(HALF, &CORNERS[removed]));
        assert_ne!(
            pick_box(&camera, VIEW, HALF, &cuts, at, 2.0),
            Some(BoxTarget::Corner(removed)),
            "the cut-away corner floats in the air"
        );
        assert!(surviving_points(BoxTarget::Corner(removed), HALF, &cuts).is_none());
        assert!(surviving_points(BoxTarget::Corner(kept), HALF, &cuts).is_some());
        // Uncut, the same pointer position still picks the corner.
        assert_eq!(
            pick_box(&camera, VIEW, HALF, &[], at, 2.0),
            Some(BoxTarget::Corner(removed))
        );
    }

    #[test]
    fn an_edge_is_cut_short_or_removed_by_the_cuts() {
        // Top-Front runs from (-0.6, 0.5, 0.4) to (0.6, 0.5, 0.4). The plane through the
        // Top-Front-Right corner's diagonal at n . p = 1.5 / sqrt(3) - 0.2 crosses it where
        // -0.6 + 1.2 t = 0.2536 (t = 0.7113: n . a = 0.1732, n . b = 0.8660, and
        // 0.1732 + 0.6928 t = 0.6660).
        let cut = corner_cut(1);
        let points = surviving_points(BoxTarget::Edge(0), HALF, &[cut]).expect("most is left");
        assert!((points[0].x + 0.6).abs() < 1e-5, "{points:?}");
        assert!((points[1].x - 0.2536).abs() < 1e-3, "{points:?}");
        // A plane y <= 0.3 takes the whole top face's edges away.
        let flat = (Vec3::Y, 0.3);
        assert!(surviving_points(BoxTarget::Edge(0), HALF, &[flat]).is_none());
        // And a plane that touches neither end leaves the edge whole.
        let far = (Vec3::Y, 2.0);
        let whole = surviving_points(BoxTarget::Edge(0), HALF, &[far]).expect("nothing cut");
        for (got, want) in whole.iter().zip(BoxTarget::Edge(0).points(HALF)) {
            assert!(got.distance(want) < 1e-6, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn a_corner_on_the_far_side_is_not_offered() {
        let camera = camera();
        let hidden = *corners_by_distance(&camera).last().expect("eight corners");
        assert!(!BoxTarget::Corner(hidden).visible(&camera, HALF));
        let at = screen_of(&camera, corner_point(HALF, &CORNERS[hidden]));
        // Whatever is picked at the hidden corner's pixel, it is not that corner.
        assert_ne!(
            pick_box(&camera, VIEW, HALF, &[], at, 2.0),
            Some(BoxTarget::Corner(hidden))
        );
    }

    #[test]
    fn a_target_names_its_faces_and_makes_the_default_cut() {
        assert_eq!(BoxTarget::Edge(0).hint(), "Click to cut edge Top-Front");
        assert_eq!(
            BoxTarget::Corner(1).hint(),
            "Click to cut corner Top-Front-Right"
        );
        let extents = [10.0, 8.0, 6.0];
        assert_eq!(
            BoxTarget::Edge(0).default_cut(extents),
            default_edge(EDGES[0], extents)
        );
        assert_eq!(
            BoxTarget::Corner(1).default_cut(extents),
            default_corner(CORNERS[1], extents)
        );
    }

    #[test]
    fn edge_and_corner_points_lie_on_the_box() {
        // Top-Front-Right is the +x, +y, +z corner.
        assert_eq!(
            BoxTarget::Corner(1).points(HALF),
            vec![Vec3::new(0.6, 0.5, 0.4)]
        );
        // Top-Front runs along x at y = +0.5, z = +0.4.
        let ends = BoxTarget::Edge(0).points(HALF);
        assert_eq!(ends[0], Vec3::new(-0.6, 0.5, 0.4));
        assert_eq!(ends[1], Vec3::new(0.6, 0.5, 0.4));
    }

    #[test]
    fn distance_to_a_segment_clamps_at_its_ends() {
        let a = (0.0, 0.0);
        let b = (10.0, 0.0);
        assert!((segment_distance((5.0, 3.0), a, b) - 3.0).abs() < 1e-6);
        assert!((segment_distance((-4.0, 3.0), a, b) - 5.0).abs() < 1e-6);
        assert!((segment_distance((13.0, 4.0), a, b) - 5.0).abs() < 1e-6);
    }
}
