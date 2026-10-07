//! Bounding-volume tests that let the concave-tool check (check 6) skip work before it
//! builds a plane-arrangement mesh.
//!
//! Every mesh build of the check is cubic in the number of planes, and a ball tool alone
//! carries 320 of them, so the check asks these questions first:
//!
//! * is a tool wholly beyond one of the stone's planes (it cuts nothing)?
//! * do two tools' boxes miss each other (they cannot overlap)?
//! * which planes can a body never cut (a plane the whole of another body lies strictly
//!   inside adds nothing to the intersection, so it is left out of the arrangement)?
//!
//! Each answer is exact up to the box being at least as big as the body it bounds, so a
//! test only ever says "cannot touch" when that is true; it never changes a result, it only
//! avoids computing it.

use glam::DVec3;
use indicatrix::geometry::stone_metrics::ToolBounds;

/// A half-space `n . x <= m`.
pub(super) type Plane = (DVec3, f64);

/// A convex region known to contain a body: the stone's axis-aligned box or a tool's
/// oriented box.
#[derive(Clone, Copy)]
pub(super) enum Bound<'a> {
    /// The box `lo <= x <= hi`, component-wise.
    Aabb { lo: DVec3, hi: DVec3 },
    /// A tool's oriented box.
    Obb(&'a ToolBounds),
}

impl Bound<'_> {
    /// The largest `n . x` over the region.
    fn max_along(&self, n: DVec3) -> f64 {
        match self {
            Self::Aabb { lo, hi } => (n * *hi).max(n * *lo).element_sum(),
            Self::Obb(bounds) => {
                n.dot(bounds.centre)
                    + bounds
                        .axes
                        .iter()
                        .zip(bounds.half_extents)
                        .map(|(axis, half)| half * n.dot(*axis).abs())
                        .sum::<f64>()
            }
        }
    }

    /// The smallest `n . x` over the region.
    fn min_along(&self, n: DVec3) -> f64 {
        -self.max_along(-n)
    }
}

/// Positions in `planes` that no body of `others` makes redundant: a plane is left out when
/// some body lies strictly inside it (by `slack`), because the intersection of all the
/// bodies then never reaches it.
///
/// Dropping such planes leaves the intersection's geometry unchanged: walking from a point
/// of the intersection towards a point of the relaxed one, the first constraint violated
/// would have to be one of the dropped planes, which the body inside it forbids.
pub(super) fn needed_planes(planes: &[Plane], others: &[Bound<'_>], slack: f64) -> Vec<usize> {
    planes
        .iter()
        .enumerate()
        .filter(|&(_, &(n, m))| !others.iter().any(|body| body.max_along(n) < m - slack))
        .map(|(index, _)| index)
        .collect()
}

/// Whether `body` lies wholly outside the stone `planes` (beyond one of them by more than
/// `slack`), so nothing of the body is in the stone.
pub(super) fn outside_stone(body: &Bound<'_>, planes: &[Plane], slack: f64) -> bool {
    planes.iter().any(|&(n, m)| body.min_along(n) > m + slack)
}

/// Whether the oriented boxes `a` and `b` are separated (by more than `slack`) along one of
/// the fifteen axes of the separating-axis test, so nothing inside them can overlap.
pub(super) fn boxes_separated(a: &ToolBounds, b: &ToolBounds, slack: f64) -> bool {
    let offset = b.centre - a.centre;
    let reach = |bounds: &ToolBounds, axis: DVec3| -> f64 {
        bounds
            .axes
            .iter()
            .zip(bounds.half_extents)
            .map(|(own, half)| half * axis.dot(*own).abs())
            .sum()
    };
    let separates = |axis: DVec3| {
        let length = axis.length();
        if length <= 1e-12 {
            // Parallel edges give no axis of their own; the face axes cover them.
            return false;
        }
        let unit = axis / length;
        offset.dot(unit).abs() > reach(a, unit) + reach(b, unit) + slack
    };
    a.axes.iter().chain(&b.axes).any(|&axis| separates(axis))
        || a.axes
            .iter()
            .any(|&x| b.axes.iter().any(|&y| separates(x.cross(y))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upright(centre: DVec3, half: [f64; 3]) -> ToolBounds {
        ToolBounds {
            centre,
            axes: [DVec3::X, DVec3::Y, DVec3::Z],
            half_extents: half,
        }
    }

    fn unit_cube() -> Vec<Plane> {
        vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 1.0),
            (DVec3::NEG_Y, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ]
    }

    #[test]
    fn a_box_beyond_a_plane_is_outside_the_stone_and_a_touching_one_is_not() {
        let cube = unit_cube();
        let far = upright(DVec3::new(3.0, 0.0, 0.0), [0.5; 3]);
        assert!(outside_stone(&Bound::Obb(&far), &cube, 1e-9));
        let touching = upright(DVec3::new(1.5, 0.0, 0.0), [0.5; 3]);
        assert!(!outside_stone(&Bound::Obb(&touching), &cube, 1e-9));
        let inside = upright(DVec3::ZERO, [0.5; 3]);
        assert!(!outside_stone(&Bound::Obb(&inside), &cube, 1e-9));
    }

    #[test]
    fn planes_a_body_lies_strictly_inside_are_not_needed() {
        let cube = unit_cube();
        let inside = upright(DVec3::ZERO, [0.5; 3]);
        assert_eq!(
            needed_planes(&cube, &[Bound::Obb(&inside)], 1e-9),
            [] as [usize; 0]
        );
        // A box poking through +x keeps that plane only.
        let poking = upright(DVec3::new(0.8, 0.0, 0.0), [0.5, 0.2, 0.2]);
        assert_eq!(needed_planes(&cube, &[Bound::Obb(&poking)], 1e-9), vec![0]);
        // The stone's own box makes a plane beyond it redundant.
        let stone = Bound::Aabb {
            lo: DVec3::splat(-1.0),
            hi: DVec3::splat(1.0),
        };
        let far_planes = vec![(DVec3::X, 2.0), (DVec3::X, 1.0)];
        assert_eq!(needed_planes(&far_planes, &[stone], 1e-9), vec![1]);
    }

    #[test]
    fn separated_boxes_are_found_and_overlapping_or_touching_ones_are_not() {
        let a = upright(DVec3::ZERO, [1.0; 3]);
        let apart = upright(DVec3::new(3.0, 0.0, 0.0), [1.0; 3]);
        assert!(boxes_separated(&a, &apart, 1e-9));
        let touching = upright(DVec3::new(2.0, 0.0, 0.0), [1.0; 3]);
        assert!(!boxes_separated(&a, &touching, 1e-9));
        let overlapping = upright(DVec3::new(1.0, 1.0, 1.0), [1.0; 3]);
        assert!(!boxes_separated(&a, &overlapping, 1e-9));
        // A box turned 45 degrees about z reaches sqrt(2) along x: its corner pokes into
        // the upright box at 2.3 and clears it at 2.5.
        let turned = |x: f64| {
            let (s, c) = std::f64::consts::FRAC_PI_4.sin_cos();
            ToolBounds {
                centre: DVec3::new(x, 0.0, 0.0),
                axes: [DVec3::new(c, s, 0.0), DVec3::new(-s, c, 0.0), DVec3::Z],
                half_extents: [1.0; 3],
            }
        };
        assert!(!boxes_separated(&a, &turned(2.3), 1e-9));
        assert!(boxes_separated(&a, &turned(2.5), 1e-9));
        // Slack makes the test more cautious, never less.
        assert!(!boxes_separated(&a, &turned(2.5), 0.2));
    }
}
