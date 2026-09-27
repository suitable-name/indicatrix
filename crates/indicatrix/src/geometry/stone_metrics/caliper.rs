//! Rotating-caliper extents over a 2D outline, used by
//! [`measure_solid`](super::measure_solid) as the cross-check against the
//! axis-aligned width/length convention the printed proportions actually use.

/// Rotating-caliper width and length of a 2D point set: the smallest directional
/// extent over all convex-outline edge directions, and the extent along the
/// perpendicular direction. Returns `None` when the outline is degenerate
/// (fewer than three distinct hull points).
pub(super) fn caliper_extents(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let hull = convex_hull_2d(points);
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f64, f64)> = None;
    for i in 0..hull.len() {
        let (px, pz) = hull[i];
        let (qx, qz) = hull[(i + 1) % hull.len()];
        let (ex, ez) = (qx - px, qz - pz);
        let len = ex.hypot(ez);
        if len < 1e-12 {
            continue;
        }
        let (dx, dz) = (ex / len, ez / len);
        let mut along = (f64::INFINITY, f64::NEG_INFINITY);
        let mut across = (f64::INFINITY, f64::NEG_INFINITY);
        for &(x, z) in &hull {
            let a = x.mul_add(dx, z * dz);
            let c = x.mul_add(-dz, z * dx);
            along = (along.0.min(a), along.1.max(a));
            across = (across.0.min(c), across.1.max(c));
        }
        let width = across.1 - across.0;
        let length = along.1 - along.0;
        if best.is_none_or(|(bw, _)| width < bw) {
            best = Some((width, length));
        }
    }
    best.map(|(w, l)| if w <= l { (w, l) } else { (l, w) })
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
