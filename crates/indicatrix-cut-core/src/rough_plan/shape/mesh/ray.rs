//! Ray queries on a mesh's own surface, for locating inclusions from photos.
//!
//! The query walks the BVH the planner already built. Triangles that belong to inclusions
//! are skipped: a camera ray, and the ray inside the stone, leave the rough through its own
//! surface, whatever lies inside.

use std::cell::Cell;

use glam::DVec3;

use super::RoughMesh;

/// Where a ray first meets the rough's own surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The distance along the ray, in units of the direction vector's length.
    pub t: f64,
    /// The triangle that was hit.
    pub triangle: u32,
    /// The outward unit normal of that triangle.
    pub normal: DVec3,
}

/// Whether the ray `origin + t dir` meets the box at some `t` in `[0, limit]`; `inv` is
/// `1 / dir`.
fn ray_enters_box(origin: DVec3, inv: DVec3, min: DVec3, max: DVec3, limit: f64) -> bool {
    let near = (min - origin) * inv;
    let far = (max - origin) * inv;
    let t_enter = near.min(far).max_element();
    let t_exit = near.max(far).min_element();
    t_exit >= t_enter.max(0.0) && t_enter <= limit
}

/// The `t` at which the ray `origin + t dir` meets the triangle (Moller-Trumbore, both
/// sides), or `None` when it misses. `t` may be negative; the caller filters.
fn ray_triangle_distance(origin: DVec3, dir: DVec3, tri: [DVec3; 3]) -> Option<f64> {
    let [p0, p1, p2] = tri;
    let (edge1, edge2) = (p1 - p0, p2 - p0);
    let pvec = dir.cross(edge2);
    let det = edge1.dot(pvec);
    if det.abs() <= 1e-18 * edge1.length() * edge2.length() * dir.length() {
        return None;
    }
    let inv_det = 1.0 / det;
    let tvec = origin - p0;
    let bary_u = tvec.dot(pvec) * inv_det;
    if !(0.0..=1.0).contains(&bary_u) {
        return None;
    }
    let qvec = tvec.cross(edge1);
    let bary_v = dir.dot(qvec) * inv_det;
    if bary_v < 0.0 || bary_u + bary_v > 1.0 {
        return None;
    }
    Some(edge2.dot(qvec) * inv_det)
}

impl RoughMesh {
    /// The first place the ray `origin + t dir` with `t > t_min` meets the rough's own
    /// surface (inclusions are skipped), or `None` when it never does.
    ///
    /// `dir` need not be a unit vector, but `t` is then in units of its length. A hit exactly
    /// on an edge is reported once, for the first of its triangles the walk meets. Deterministic:
    /// the BVH is walked in a fixed order and ties keep the first triangle found.
    #[must_use]
    pub fn first_hit(&self, origin: DVec3, dir: DVec3, t_min: f64) -> Option<RayHit> {
        let outer = self
            .inclusion_shells
            .first()
            .map_or(self.tris.len(), |&first| first as usize);
        let inv = DVec3::ONE / dir;
        let best = Cell::new(f64::INFINITY);
        let found: Cell<Option<u32>> = Cell::new(None);
        self.walk(
            |node| ray_enters_box(origin, inv, node.min, node.max, best.get()),
            |t| {
                let hit = ((t as usize) < outer)
                    .then(|| ray_triangle_distance(origin, dir, self.corners(t)))
                    .flatten();
                if let Some(dist) = hit.filter(|&dist| dist > t_min && dist < best.get()) {
                    best.set(dist);
                    found.set(Some(t));
                }
                true
            },
        );
        found.get().map(|triangle| RayHit {
            t: best.get(),
            triangle,
            normal: self.planes[triangle as usize].0,
        })
    }
}
