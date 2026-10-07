//! Splitting a polygon face of an OBJ file into triangles.
//!
//! A convex polygon is fanned from its first vertex, as always. A non-convex (or skew)
//! polygon, whose fan would overlap itself and could still pass the mesh's closedness check,
//! is ear-clipped in its own plane instead.

use glam::DVec3;

/// The triangles of the polygon with the 0-based vertex indices `corners` (at least three),
/// in the winding of `corners`; `None` when a non-convex polygon cannot be ear-clipped
/// (self-intersecting, or with no area).
pub(super) fn triangulate_face(points: &[DVec3], corners: &[u32]) -> Option<Vec<[u32; 3]>> {
    let fan = || {
        (1..corners.len() - 1)
            .map(|k| [corners[0], corners[k], corners[k + 1]])
            .collect()
    };
    if corners.len() == 3 {
        return Some(fan());
    }
    let ring: Vec<DVec3> = corners.iter().map(|&c| points[c as usize]).collect();
    let normal = newell_normal(&ring);
    let size = ring
        .iter()
        .fold(0.0_f64, |m, p| m.max(p.abs().max_element()));
    // A polygon with no area, or a coordinate that is not a number, is left to the mesh
    // check, which drops or refuses it as it always did.
    if !(normal.length() > 0.0 && normal.is_finite()) {
        return Some(fan());
    }
    let normal = normal.normalize();
    let tolerance = 1e-12 * size * size;
    let turns = turns(&ring, normal);
    if !turns.iter().any(|&t| t < -tolerance) {
        return Some(fan());
    }
    ear_clip(&ring, normal, tolerance).map(|local| {
        local
            .into_iter()
            .map(|tri| tri.map(|k| corners[k]))
            .collect()
    })
}

/// The Newell normal of the polygon `ring`: its area vector (twice the area, along the
/// normal the winding implies), robust for a skew polygon.
fn newell_normal(ring: &[DVec3]) -> DVec3 {
    let mut sum = DVec3::ZERO;
    for (i, &a) in ring.iter().enumerate() {
        sum += a.cross(ring[(i + 1) % ring.len()]);
    }
    sum
}

/// The turn at every vertex of `ring` about `normal`: positive for a left turn.
fn turns(ring: &[DVec3], normal: DVec3) -> Vec<f64> {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b, c) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            normal.dot((b - a).cross(c - b))
        })
        .collect()
}

/// Ear-clips the polygon `ring`, counter-clockwise about `normal`, into triangles of ring
/// positions; `None` when no ear is left before the polygon is used up.
#[expect(
    clippy::many_single_char_names,
    reason = "a, b, c, n and p are the textbook names of the ear-clipping vertices"
)]
fn ear_clip(ring: &[DVec3], normal: DVec3, tolerance: f64) -> Option<Vec<[usize; 3]>> {
    let n = ring.len();
    let helper = if normal.x.abs() < 0.9 {
        DVec3::X
    } else {
        DVec3::Y
    };
    let u = normal.cross(helper).normalize();
    let v = normal.cross(u);
    let flat: Vec<(f64, f64)> = ring.iter().map(|&p| (p.dot(u), p.dot(v))).collect();
    let cross = |a: usize, b: usize, c: usize| {
        (flat[b].0 - flat[a].0).mul_add(
            flat[c].1 - flat[a].1,
            -((flat[b].1 - flat[a].1) * (flat[c].0 - flat[a].0)),
        )
    };
    // A doubled area below the tolerance is no turn at all.
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut live = n;
    let mut at = 0;
    let mut out = Vec::with_capacity(n - 2);
    while live > 3 {
        let mut found = None;
        let mut i = at;
        for _ in 0..live {
            let (a, c) = (prev[i], next[i]);
            if cross(a, i, c) > tolerance {
                let blocked = {
                    let mut k = next[c];
                    let mut hit = false;
                    while k != a {
                        let same = |q: usize| (ring[k] - ring[q]).length_squared() <= 1e-24;
                        if !(same(a) || same(i) || same(c))
                            && cross(a, i, k) >= -tolerance
                            && cross(i, c, k) >= -tolerance
                            && cross(c, a, k) >= -tolerance
                        {
                            hit = true;
                            break;
                        }
                        k = next[k];
                    }
                    hit
                };
                if !blocked {
                    found = Some(i);
                    break;
                }
            }
            i = next[i];
        }
        let i = found?;
        let (a, c) = (prev[i], next[i]);
        out.push([a, i, c]);
        next[a] = c;
        prev[c] = a;
        live -= 1;
        at = a;
    }
    let a = at;
    let (b, c) = (next[a], next[next[a]]);
    if cross(a, b, c) > tolerance {
        out.push([a, b, c]);
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(points: &[DVec3], tris: &[[u32; 3]]) -> f64 {
        tris.iter()
            .map(|t| {
                let [a, b, c] = t.map(|k| points[k as usize]);
                (b - a).cross(c - a).z * 0.5
            })
            .sum()
    }

    fn pts(xy: &[(f64, f64)]) -> Vec<DVec3> {
        xy.iter().map(|&(x, y)| DVec3::new(x, y, 0.0)).collect()
    }

    #[test]
    fn a_convex_quad_is_fanned_unchanged() {
        let p = pts(&[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]);
        let tris = triangulate_face(&p, &[0, 1, 2, 3]).expect("fans");
        assert_eq!(tris, vec![[0, 1, 2], [0, 2, 3]]);
        let q = pts(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.5, 1.5), (0.0, 1.0)]);
        let tris = triangulate_face(&q, &[0, 1, 2, 3, 4]).expect("fans");
        assert_eq!(tris, vec![[0, 1, 2], [0, 2, 3], [0, 3, 4]]);
    }

    #[test]
    fn a_concave_quad_is_split_along_its_inner_diagonal() {
        // A dart: the corner (1, 1) is reflex, so the fan from vertex 0 flips a triangle.
        let p = pts(&[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0), (1.0, 1.0)]);
        assert!(
            area(&p, &[[0, 2, 3]]) < 0.0,
            "the fan's second triangle flips"
        );
        let tris = triangulate_face(&p, &[0, 1, 2, 3]).expect("ear-clips");
        assert_eq!(tris.len(), 2);
        // The dart is the triangle (0,0) (4,0) (0,4), area 8, less the dent of area 2.
        assert!((area(&p, &tris) - 6.0).abs() < 1e-12, "{tris:?}");
        assert!(tris.iter().all(|t| area(&p, &[*t]) > 0.0), "{tris:?}");
    }

    #[test]
    fn a_concave_pentagon_covers_exactly_its_area() {
        // An arrow with a reflex vertex whose fan from vertex 0 overlaps.
        let p = pts(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (2.0, 1.0), (0.0, 4.0)]);
        let corners = [0, 1, 2, 3, 4];
        let tris = triangulate_face(&p, &corners).expect("ear-clips");
        assert_eq!(tris.len(), 3);
        // Shoelace.
        let shoelace: f64 = (0..5)
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % 5]);
                a.x.mul_add(b.y, -(b.x * a.y))
            })
            .sum::<f64>()
            * 0.5;
        assert!((area(&p, &tris) - shoelace).abs() < 1e-12, "{tris:?}");
        assert!(tris.iter().all(|t| area(&p, &[*t]) > 0.0));
        // The fan would overlap: its signed areas do not add up to a positive outline.
        let fan_all_positive = (1..4).all(|k| area(&p, &[[0, k, k + 1]]) > 0.0);
        assert!(!fan_all_positive, "the fixture must defeat the fan");
    }

    #[test]
    fn a_self_intersecting_bowtie_is_refused() {
        let p = pts(&[(0.0, 0.0), (3.0, 3.0), (3.0, 0.0), (0.0, 1.0)]);
        assert_eq!(triangulate_face(&p, &[0, 1, 2, 3]), None);
    }
}
