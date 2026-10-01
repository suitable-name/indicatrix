//! Rotating-caliper extents over a 2D outline, used by
//! [`measure_solid`](super::measure_solid) as the cross-check against the
//! axis-aligned width/length convention the printed proportions actually use.

/// Rotating-caliper width and length of a 2D point set: the smallest directional
/// extent over all convex-outline edge directions, and the extent along the
/// perpendicular direction. Returns `None` when the outline is degenerate
/// (fewer than three distinct hull points). Widths equal within a relative
/// `1e-12` resolve to the edge with the smaller direction angle.
pub(super) fn caliper_extents(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let hull = convex_hull_2d(points);
    if hull.len() < 3 {
        return None;
    }
    best_edge(&hull).map(|m| {
        if m.width <= m.length {
            (m.width, m.length)
        } else {
            (m.length, m.width)
        }
    })
}

/// Relative width difference under which two candidate edges are a tie
/// (last-bit noise from differently rotated but geometrically equal extents).
const TIE_REL: f64 = 1e-12;

/// One hull edge's caliper measurement.
#[derive(Clone, Copy)]
struct EdgeMeasure {
    /// Extent across the edge direction.
    width: f64,
    /// Extent along the edge direction.
    length: f64,
    /// Unit edge direction `(dx, dz)`.
    dir: (f64, f64),
}

/// Measures the hull against the direction of edge `i`; `None` for a
/// zero-length edge. The edge length is `sqrt(fma(ez, ez, ex*ex))`, built only
/// from correctly rounded operations, so the result does not depend on the platform libm.
/// [`caliper_extents`] and [`caliper_frame`] both go through here, which keeps
/// them bit-identical.
fn measure_edge(hull: &[(f64, f64)], i: usize) -> Option<EdgeMeasure> {
    let (px, pz) = hull[i];
    let (qx, qz) = hull[(i + 1) % hull.len()];
    let (ex, ez) = (qx - px, qz - pz);
    let ex_sq = ex * ex;
    let len = ez.mul_add(ez, ex_sq).sqrt();
    if len < 1e-12 {
        return None;
    }
    let (dx, dz) = (ex / len, ez / len);
    let mut along = (f64::INFINITY, f64::NEG_INFINITY);
    let mut across = (f64::INFINITY, f64::NEG_INFINITY);
    for &(x, z) in hull {
        let a = x.mul_add(dx, z * dz);
        let c = x.mul_add(-dz, z * dx);
        along = (along.0.min(a), along.1.max(a));
        across = (across.0.min(c), across.1.max(c));
    }
    Some(EdgeMeasure {
        width: across.1 - across.0,
        length: along.1 - along.0,
        dir: (dx, dz),
    })
}

/// True when direction `a` has a strictly smaller angle than `b` in `[0, 2*pi)`,
/// decided by half-plane and cross product (no trigonometry).
fn angle_less(a: (f64, f64), b: (f64, f64)) -> bool {
    let half = |d: (f64, f64)| u8::from(!(d.1 > 0.0 || (d.1 == 0.0 && d.0 > 0.0)));
    let (ha, hb) = (half(a), half(b));
    if ha == hb {
        a.0.mul_add(b.1, -(a.1 * b.0)) > 0.0
    } else {
        ha < hb
    }
}

/// Whether `cand` replaces `best`: a clearly smaller width wins; widths within
/// [`TIE_REL`] of each other are a tie that goes to the smaller edge angle.
fn edge_wins(cand: &EdgeMeasure, best: &EdgeMeasure) -> bool {
    let tol = TIE_REL * cand.width.max(best.width);
    if (cand.width - best.width).abs() <= tol {
        angle_less(cand.dir, best.dir)
    } else {
        cand.width < best.width
    }
}

/// The winning edge of the rotating-caliper sweep over a hull.
fn best_edge(hull: &[(f64, f64)]) -> Option<EdgeMeasure> {
    let mut best: Option<EdgeMeasure> = None;
    for i in 0..hull.len() {
        if let Some(m) = measure_edge(hull, i)
            && best.as_ref().is_none_or(|b| edge_wins(&m, b))
        {
            best = Some(m);
        }
    }
    best
}

/// Result of [`caliper_frame`]: the outline's minimum-width frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaliperFrame {
    /// Rotating-caliper width: minimum directional extent across the outline.
    pub width: f64,
    /// Rotating-caliper length: extent along the perpendicular direction.
    pub length: f64,
    /// 2D unit vector along the width direction in the outline's plane.
    pub width_dir: [f64; 2],
}

/// The rotating-caliper minimum-width frame of an outline: width, length (as
/// [`caliper_extents`] returns them) and the unit direction of the width.
///
/// Returns `None` when the outline is degenerate (fewer than three distinct hull
/// points).
///
/// Shares the edge sweep, tie rule and arithmetic with [`caliper_extents`], so
/// the two agree bit for bit.
#[must_use]
pub fn caliper_frame(outline: &[(f64, f64)]) -> Option<CaliperFrame> {
    let hull = convex_hull_2d(outline);
    if hull.len() < 3 {
        return None;
    }
    best_edge(&hull).map(|m| {
        let (dx, dz) = m.dir;
        if m.width <= m.length {
            CaliperFrame {
                width: m.width,
                length: m.length,
                width_dir: [-dz, dx],
            }
        } else {
            CaliperFrame {
                width: m.length,
                length: m.width,
                width_dir: [dx, dz],
            }
        }
    })
}

/// Andrew's monotone-chain convex hull over 2D points, counterclockwise.
/// Deterministic: total-order lexicographic sort, no hashing.
fn convex_hull_2d(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = points.to_vec();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| -> f64 {
        (a.0 - o.0).mul_add(b.1 - o.1, -((a.1 - o.1) * (b.0 - o.0)))
    };
    let mut lower: Vec<(f64, f64)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caliper_frame_matches_caliper_extents_and_has_unit_dir() {
        let square = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
        let rect = [(2.0, 0.5), (-2.0, 0.5), (-2.0, -0.5), (2.0, -0.5)];
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let rotated = [(s, 0.0), (0.0, s), (-s, 0.0), (0.0, -s)];
        let triangle = [(0.0, 1.0), (2.0, -1.0), (-1.0, -0.5)];

        for (i, outline) in [&square[..], &rect[..], &rotated[..], &triangle[..]]
            .iter()
            .enumerate()
        {
            let extents =
                caliper_extents(outline).unwrap_or_else(|| panic!("fixture {i} must measure"));
            let frame =
                caliper_frame(outline).unwrap_or_else(|| panic!("fixture {i} frame must measure"));

            assert!(
                (frame.width - extents.0).abs() < 1e-12,
                "fixture {i}: width mismatch ({} vs {})",
                frame.width,
                extents.0
            );
            assert!(
                (frame.length - extents.1).abs() < 1e-12,
                "fixture {i}: length mismatch ({} vs {})",
                frame.length,
                extents.1
            );
            let len = frame.width_dir[0].hypot(frame.width_dir[1]);
            assert!(
                (len - 1.0).abs() < 1e-12,
                "fixture {i}: width_dir is not a unit vector (norm {len})"
            );
        }
    }

    /// Extent of `outline` along the unit direction `dir`.
    fn extent_along(outline: &[(f64, f64)], dir: [f64; 2]) -> f64 {
        let (lo, hi) = outline
            .iter()
            .map(|&(x, z)| x.mul_add(dir[0], z * dir[1]))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| {
                (lo.min(t), hi.max(t))
            });
        hi - lo
    }

    #[test]
    fn caliper_frame_direction_matches_width_and_length_on_a_rotated_rectangle() {
        // A 4 x 1 rectangle turned by 30 degrees: the width direction must be
        // the rectangle's short axis, not an axis of the coordinate frame.
        let (sin, cos) = 30.0_f64.to_radians().sin_cos();
        let corners: [(f64, f64); 4] = [(2.0, 0.5), (-2.0, 0.5), (-2.0, -0.5), (2.0, -0.5)];
        let rotated: Vec<(f64, f64)> = corners
            .iter()
            .map(|&(x, z)| (x.mul_add(cos, -z * sin), x.mul_add(sin, z * cos)))
            .collect();
        let frame = caliper_frame(&rotated).expect("rectangle must measure");
        assert!((frame.width - 1.0).abs() < 1e-12, "width {}", frame.width);
        assert!(
            (frame.length - 4.0).abs() < 1e-12,
            "length {}",
            frame.length
        );

        let across = extent_along(&rotated, frame.width_dir);
        let perpendicular = [-frame.width_dir[1], frame.width_dir[0]];
        let along = extent_along(&rotated, perpendicular);
        assert!((across - frame.width).abs() < 1e-12, "across {across}");
        assert!((along - frame.length).abs() < 1e-12, "along {along}");
        // The short axis of the rectangle is (-sin, cos), up to sign.
        let alignment = frame.width_dir[0].mul_add(-sin, frame.width_dir[1] * cos);
        assert!(
            (alignment.abs() - 1.0).abs() < 1e-12,
            "alignment {alignment}"
        );
    }

    #[test]
    fn caliper_frame_direction_property_holds_on_every_fixture() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let fixtures: [&[(f64, f64)]; 4] = [
            &[(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)],
            &[(2.0, 0.5), (-2.0, 0.5), (-2.0, -0.5), (2.0, -0.5)],
            &[(s, 0.0), (0.0, s), (-s, 0.0), (0.0, -s)],
            &[(0.0, 1.0), (2.0, -1.0), (-1.0, -0.5)],
        ];
        for (i, outline) in fixtures.iter().enumerate() {
            let frame = caliper_frame(outline).expect("fixture must measure");
            let across = extent_along(outline, frame.width_dir);
            let perpendicular = [-frame.width_dir[1], frame.width_dir[0]];
            let along = extent_along(outline, perpendicular);
            assert!(
                (across - frame.width).abs() < 1e-12,
                "fixture {i}: {across}"
            );
            assert!((along - frame.length).abs() < 1e-12, "fixture {i}: {along}");
        }
    }

    /// A unit square has four edges of equal width; the tie goes to the edge with
    /// the smallest direction angle. The hull is counterclockwise from the
    /// lowest-left point, so that edge is (1, 0) and the width direction is
    /// (0, 1), whatever the input order.
    #[test]
    fn exact_width_ties_resolve_to_the_smallest_edge_angle() {
        let a = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let b = [(1.0, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0)];
        for outline in [a, b] {
            let frame = caliper_frame(&outline).expect("square must measure");
            assert!(frame.width_dir[0].abs() < 1e-12, "{:?}", frame.width_dir);
            assert!((frame.width_dir[1] - 1.0).abs() < 1e-12);
            assert!((frame.width - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn caliper_frame_degenerate_returns_none() {
        let empty: [(f64, f64); 0] = [];
        let one = [(1.0, 2.0)];
        let two = [(0.0, 0.0), (1.0, 1.0)];
        assert!(caliper_frame(&empty).is_none());
        assert!(caliper_frame(&one).is_none());
        assert!(caliper_frame(&two).is_none());
    }
}
