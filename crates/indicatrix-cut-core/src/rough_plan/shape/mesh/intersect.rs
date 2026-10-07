//! Detection of a mesh that crosses itself.
//!
//! A scan whose surface passes through itself has no well-defined inside: the parity of a
//! ray's crossings depends on which sheet it meets first. [`first_self_intersections`] finds
//! the triangle pairs that really cross, so the import can refuse the mesh instead of
//! planning stones into a notch that is not there.
//!
//! The test is a hand-rolled one in `f64` with a length tolerance (no exact-predicate
//! crate): two triangles cross when each has vertices clearly on both sides of the other's
//! plane and the stretches in which they cut that common line overlap by more than the
//! tolerance. Touching is not crossing: a vertex or an edge resting on another triangle, or
//! two triangles meeting along a line, are inside the tolerance and do not count. Triangles
//! that share a vertex (neighbours, or shells touching at a corner after the weld) are not
//! compared. Two triangles in one plane (within the tolerance) cross when their outlines
//! cross properly or one has a point strictly inside the other.
//!
//! Everything is deterministic: triangles in index order, candidates in ascending order.

use glam::DVec3;

use super::{
    BvhNode,
    geometry::{bounds_of, build_bvh},
};

/// The length tolerance, as a fraction of the mesh's size. Distinct vertices are further
/// apart than the weld's `1e-7`, so this is far below any real feature.
pub(super) const TOLERANCE_FRACTION: f64 = 1e-9;

/// The first (at most `limit`) pairs `(i, j)`, `i < j`, of triangles of `tris` that cross, in
/// ascending order of `i` and then `j`. `scale` is the mesh's size.
pub(super) fn first_self_intersections(
    verts: &[DVec3],
    tris: &[[u32; 3]],
    scale: f64,
    limit: usize,
) -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    if tris.len() < 2 || limit == 0 {
        return found;
    }
    let tolerance = TOLERANCE_FRACTION * scale;
    let (bvh, order) = build_bvh(verts, tris);
    let corners = |t: u32| tris[t as usize].map(|v| verts[v as usize]);
    let mut candidates: Vec<u32> = Vec::new();
    for i in 0..tris.len() as u32 {
        let (lo, hi) = bounds_of(corners(i).into_iter());
        let (lo, hi) = (lo - DVec3::splat(tolerance), hi + DVec3::splat(tolerance));
        candidates.clear();
        walk(
            &bvh,
            &order,
            |node| node.min.cmple(hi).all() && node.max.cmpge(lo).all(),
            |j| {
                if j > i {
                    candidates.push(j);
                }
            },
        );
        candidates.sort_unstable();
        for &j in &candidates {
            if shares_vertex(tris[i as usize], tris[j as usize]) {
                continue;
            }
            if triangles_cross(corners(i), corners(j), tolerance) {
                found.push((i, j));
                if found.len() >= limit {
                    return found;
                }
            }
        }
    }
    found
}

/// Visits the triangles of every leaf under a node `enter` accepts.
fn walk(
    bvh: &[BvhNode],
    order: &[u32],
    enter: impl Fn(&BvhNode) -> bool,
    mut visit: impl FnMut(u32),
) {
    let mut stack = vec![0_u32];
    while let Some(index) = stack.pop() {
        let node = &bvh[index as usize];
        if !enter(node) {
            continue;
        }
        if node.count > 0 {
            for &t in &order[node.first as usize..(node.first + node.count) as usize] {
                visit(t);
            }
        } else {
            stack.push(node.first + 1);
            stack.push(node.first);
        }
    }
}

/// Whether the two triangles name a common vertex.
fn shares_vertex(a: [u32; 3], b: [u32; 3]) -> bool {
    a.iter().any(|v| b.contains(v))
}

/// `x`, or zero when it is within `tolerance` of it.
fn snapped(x: f64, tolerance: f64) -> f64 {
    if x.abs() <= tolerance { 0.0 } else { x }
}

/// The unit normal of the triangle and the distances of `points` from its plane, those
/// within `tolerance` of it snapped to zero; `None` for a triangle with no area.
fn plane_distances(
    tri: [DVec3; 3],
    points: [DVec3; 3],
    tolerance: f64,
) -> Option<(DVec3, [f64; 3])> {
    let normal = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
    let length = normal.length();
    if length <= 0.0 || !length.is_finite() {
        return None;
    }
    let normal = normal / length;
    let distances = points.map(|p| snapped(normal.dot(p - tri[0]), tolerance));
    Some((normal, distances))
}

/// Whether the distances have a clearly positive and a clearly negative one.
fn straddles(distances: [f64; 3]) -> bool {
    distances.iter().any(|&d| d > 0.0) && distances.iter().any(|&d| d < 0.0)
}

/// The stretch of the line direction `dir` that the triangle `tri` covers where it cuts the
/// plane its vertices have the signed `distances` from.
fn interval(tri: [DVec3; 3], distances: [f64; 3], dir: DVec3) -> (f64, f64) {
    let along = tri.map(|p| dir.dot(p));
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for k in 0..3 {
        let next = (k + 1) % 3;
        if distances[k] == 0.0 {
            lo = lo.min(along[k]);
            hi = hi.max(along[k]);
        }
        if distances[k] * distances[next] < 0.0 {
            let s = distances[k] / (distances[k] - distances[next]);
            let x = (along[next] - along[k]).mul_add(s, along[k]);
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    (lo, hi)
}

/// Whether the triangles `a` and `b` cross (see the module documentation).
pub(super) fn triangles_cross(a: [DVec3; 3], b: [DVec3; 3], tolerance: f64) -> bool {
    let Some((na, db)) = plane_distances(a, b, tolerance) else {
        return false;
    };
    if db.iter().all(|&d| d == 0.0) {
        return coplanar_overlap(a, b, na, tolerance);
    }
    if !straddles(db) {
        return false;
    }
    let Some((nb, da)) = plane_distances(b, a, tolerance) else {
        return false;
    };
    if !straddles(da) {
        return false;
    }
    let line = na.cross(nb);
    let length = line.length();
    if length <= 1e-12 {
        return false;
    }
    let line = line / length;
    let (a_lo, a_hi) = interval(a, da, line);
    let (b_lo, b_hi) = interval(b, db, line);
    a_hi.min(b_hi) - a_lo.max(b_lo) > tolerance
}

/// The signed distance of `r` from the line through `p` and `q` (zero for a point-like edge).
fn side(p: [f64; 2], q: [f64; 2], r: [f64; 2]) -> f64 {
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let length = dx.hypot(dy);
    if length <= 0.0 {
        return 0.0;
    }
    dx.mul_add(r[1] - p[1], -(dy * (r[0] - p[0]))) / length
}

/// Whether the segments `a1 a2` and `b1 b2` cross properly, each end clearly on both sides.
fn segments_cross(a1: [f64; 2], a2: [f64; 2], b1: [f64; 2], b2: [f64; 2], tolerance: f64) -> bool {
    let apart =
        |x: f64, y: f64| (x > tolerance && y < -tolerance) || (x < -tolerance && y > tolerance);
    apart(side(a1, a2, b1), side(a1, a2, b2)) && apart(side(b1, b2, a1), side(b1, b2, a2))
}

/// Whether `p` is inside the triangle `tri` by more than `tolerance` from every edge.
fn strictly_inside(tri: [[f64; 2]; 3], p: [f64; 2], tolerance: f64) -> bool {
    let sides = [
        side(tri[0], tri[1], p),
        side(tri[1], tri[2], p),
        side(tri[2], tri[0], p),
    ];
    sides.iter().all(|&s| s > tolerance) || sides.iter().all(|&s| s < -tolerance)
}

/// Whether two triangles in the plane of unit normal `normal` overlap in an area: some pair
/// of edges crosses properly, or a vertex or the centroid of one is strictly inside the
/// other. Drops the axis the normal points along, which keeps the shapes.
fn coplanar_overlap(a: [DVec3; 3], b: [DVec3; 3], normal: DVec3, tolerance: f64) -> bool {
    let n = normal.abs();
    let axis = if n.x >= n.y && n.x >= n.z {
        0
    } else if n.y >= n.z {
        1
    } else {
        2
    };
    let flat = |p: DVec3| match axis {
        0 => [p.y, p.z],
        1 => [p.x, p.z],
        _ => [p.x, p.y],
    };
    let (a, b) = (a.map(flat), b.map(flat));
    for i in 0..3 {
        for j in 0..3 {
            if segments_cross(a[i], a[(i + 1) % 3], b[j], b[(j + 1) % 3], tolerance) {
                return true;
            }
        }
    }
    let centre = |t: [[f64; 2]; 3]| {
        [
            (t[0][0] + t[1][0] + t[2][0]) / 3.0,
            (t[0][1] + t[1][1] + t[2][1]) / 3.0,
        ]
    };
    b.iter().any(|&p| strictly_inside(a, p, tolerance))
        || a.iter().any(|&p| strictly_inside(b, p, tolerance))
        || strictly_inside(a, centre(b), tolerance)
        || strictly_inside(b, centre(a), tolerance)
}
