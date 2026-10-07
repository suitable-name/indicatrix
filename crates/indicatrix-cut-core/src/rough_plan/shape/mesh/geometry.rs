//! The geometric predicates of a [`RoughMesh`](super::RoughMesh): the bounding-volume
//! hierarchy, the ray, box and triangle tests, and polygon clipping.
//!
//! Plain functions over points and triangles; none of them knows the mesh it serves.

use glam::DVec3;

use super::{BvhNode, LEAF_SIZE};

/// The minimum and maximum corner of `points`.
pub(super) fn bounds_of(points: impl Iterator<Item = DVec3>) -> (DVec3, DVec3) {
    points.fold(
        (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    )
}

/// The BVH over `tris`: a median split on the longest axis of the centroids, ties by
/// triangle index.
pub(super) fn build_bvh(verts: &[DVec3], tris: &[[u32; 3]]) -> (Vec<BvhNode>, Vec<u32>) {
    let boxes: Vec<(DVec3, DVec3)> = tris
        .iter()
        .map(|tri| bounds_of(tri.iter().map(|&v| verts[v as usize])))
        .collect();
    let centroids: Vec<DVec3> = boxes.iter().map(|&(lo, hi)| (lo + hi) * 0.5).collect();
    let mut order: Vec<u32> = (0..tris.len() as u32).collect();
    let mut nodes = vec![BvhNode::EMPTY];
    let mut work = vec![(0_usize, 0_usize, tris.len())];
    while let Some((slot, start, end)) = work.pop() {
        let (min, max) = order[start..end].iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), &t| (lo.min(boxes[t as usize].0), hi.max(boxes[t as usize].1)),
        );
        if end - start <= LEAF_SIZE {
            nodes[slot] = BvhNode {
                min,
                max,
                first: start as u32,
                count: (end - start) as u32,
            };
            continue;
        }
        let (c_lo, c_hi) = bounds_of(order[start..end].iter().map(|&t| centroids[t as usize]));
        let spread = c_hi - c_lo;
        let axis = if spread.x >= spread.y && spread.x >= spread.z {
            0
        } else if spread.y >= spread.z {
            1
        } else {
            2
        };
        order[start..end].sort_by(|&a, &b| {
            centroids[a as usize][axis]
                .total_cmp(&centroids[b as usize][axis])
                .then(a.cmp(&b))
        });
        let mid = start + (end - start) / 2;
        let left = nodes.len();
        nodes.push(BvhNode::EMPTY);
        nodes.push(BvhNode::EMPTY);
        nodes[slot] = BvhNode {
            min,
            max,
            first: left as u32,
            count: 0,
        };
        work.push((left + 1, mid, end));
        work.push((left, start, mid));
    }
    (nodes, order)
}

/// Whether the ray `origin + t dir`, `t >= 0`, meets the box; `inv` is `1 / dir`.
pub(super) fn ray_meets_box(origin: DVec3, inv: DVec3, min: DVec3, max: DVec3) -> bool {
    let t1 = (min - origin) * inv;
    let t2 = (max - origin) * inv;
    let t_enter = t1.min(t2).max_element();
    let t_exit = t1.max(t2).min_element();
    t_exit >= t_enter.max(0.0)
}

/// Whether the ray `origin + t dir`, `t > 0`, meets the triangle (Moller-Trumbore, both
/// sides). A hit exactly on an edge may count for both triangles of it; the callers vote
/// over three rays instead of trusting one.
#[expect(
    clippy::many_single_char_names,
    reason = "Moller-Trumbore's own variable names"
)]
pub(super) fn ray_meets_triangle(origin: DVec3, dir: DVec3, [a, b, c]: [DVec3; 3]) -> bool {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() <= 1e-18 * e1.length() * e2.length() * dir.length() {
        return false;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    e2.dot(q) * inv > 0.0
}

/// Whether the triangle meets the closed box with the given centre and half extents
/// (Akenine-Moller: the box's three axes, the triangle's plane and the nine cross
/// products of edges and axes).
pub(super) fn triangle_meets_box(centre: DVec3, half: DVec3, tri: [DVec3; 3]) -> bool {
    let v = tri.map(|p| p - centre);
    for axis in 0..3 {
        let lo = v[0][axis].min(v[1][axis]).min(v[2][axis]);
        let hi = v[0][axis].max(v[1][axis]).max(v[2][axis]);
        if lo > half[axis] || hi < -half[axis] {
            return false;
        }
    }
    let edges = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    let normal = edges[0].cross(edges[1]);
    if normal.dot(v[0]).abs() > half.dot(normal.abs()) {
        return false;
    }
    for edge in edges {
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            let sep = axis.cross(edge);
            if sep == DVec3::ZERO {
                continue;
            }
            let p = v.map(|q| sep.dot(q));
            let lo = p[0].min(p[1]).min(p[2]);
            let hi = p[0].max(p[1]).max(p[2]);
            let reach = half.dot(sep.abs());
            if lo > reach || hi < -reach {
                return false;
            }
        }
    }
    true
}

/// Writes into `out` the part of the polygon `poly` with `n . p <= limit`
/// (Sutherland-Hodgman). For a non-convex polygon the result may carry zero-width bridges
/// along the clipping line, which change neither its area nor its fan volume.
pub(super) fn clip_halfspace(poly: &[DVec3], n: DVec3, limit: f64, out: &mut Vec<DVec3>) {
    out.clear();
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let (sa, sb) = (n.dot(a) - limit, n.dot(b) - limit);
        if sa <= 0.0 {
            out.push(a);
        }
        if (sa <= 0.0) != (sb <= 0.0) {
            out.push(a + (b - a) * (sa / (sa - sb)));
        }
    }
}

/// Half the sum of `a x b` over the edges of the closed polygon: its area times its
/// normal.
pub(super) fn area_vector(poly: &[DVec3]) -> DVec3 {
    let mut sum = DVec3::ZERO;
    for (i, &a) in poly.iter().enumerate() {
        sum += a.cross(poly[(i + 1) % poly.len()]);
    }
    sum * 0.5
}
