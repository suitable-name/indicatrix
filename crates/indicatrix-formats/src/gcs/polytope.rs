//! A small deterministic half-space intersection for convex polytopes: the faces
//! of `∩ {n_k·x <= d_k}`, one polygon per half-space, for the `.gcs` writer's
//! vertex lists.
//!
//! Each face starts as a large square on its plane and is clipped by every other
//! half-space in input order (Sutherland-Hodgman). No hashing, no sorting by
//! floating-point keys: the output depends only on the input order.

/// Points within this signed distance outside a half-space count as inside.
const CLIP_EPS: f64 = 1e-10;

/// Consecutive polygon points closer than this are merged.
const MERGE_EPS: f64 = 1e-9;

/// One half-space `normal·x <= offset` with a unit `normal`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HalfSpace {
    pub(super) normal: [f64; 3],
    pub(super) offset: f64,
}

/// Why no bounded polytope came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PolytopeError {
    /// Some face reaches the starting square's edge: the half-spaces do not
    /// enclose a bounded solid.
    Unbounded,
    /// Every face clipped away: the half-spaces have an empty intersection.
    Empty,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[2].mul_add(b[2], a[0].mul_add(b[0], a[1] * b[1]))
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1].mul_add(b[2], -(a[2] * b[1])),
        a[2].mul_add(b[0], -(a[0] * b[2])),
        a[0].mul_add(b[1], -(a[1] * b[0])),
    ]
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    a.map(|c| c * s)
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    dot(d, d).sqrt()
}

/// A square of half-size `half` centred on the plane's foot point, counter-clockwise
/// seen from outside (along `-normal`).
fn start_square(plane: HalfSpace, half: f64) -> Vec<[f64; 3]> {
    let n = plane.normal;
    // The axis least aligned with `n` gives a well-conditioned tangent.
    let axis = if n[0].abs() <= n[1].abs() && n[0].abs() <= n[2].abs() {
        [1.0, 0.0, 0.0]
    } else if n[1].abs() <= n[2].abs() {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let u = cross(n, axis);
    let u = scale(u, dot(u, u).sqrt().recip());
    let v = cross(n, u);
    let centre = scale(n, plane.offset);
    [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]
        .iter()
        .map(|&(a, b)| add(centre, add(scale(u, a * half), scale(v, b * half))))
        .collect()
}

/// Clips `polygon` to the half-space `clip` (Sutherland-Hodgman).
fn clip_polygon(polygon: &[[f64; 3]], clip: HalfSpace) -> Vec<[f64; 3]> {
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for (k, &current) in polygon.iter().enumerate() {
        let previous = polygon[(k + polygon.len() - 1) % polygon.len()];
        let d_cur = dot(clip.normal, current) - clip.offset;
        let d_prev = dot(clip.normal, previous) - clip.offset;
        let (cur_in, prev_in) = (d_cur <= CLIP_EPS, d_prev <= CLIP_EPS);
        if cur_in != prev_in {
            let t = d_prev / (d_prev - d_cur);
            out.push(add(
                previous,
                scale(
                    [
                        current[0] - previous[0],
                        current[1] - previous[1],
                        current[2] - previous[2],
                    ],
                    t,
                ),
            ));
        }
        if cur_in {
            out.push(current);
        }
    }
    out
}

/// Drops consecutive (and first/last) points closer than [`MERGE_EPS`], and
/// anything with fewer than three points left.
fn merge_close_points(polygon: Vec<[f64; 3]>) -> Vec<[f64; 3]> {
    let mut out: Vec<[f64; 3]> = Vec::with_capacity(polygon.len());
    for p in polygon {
        if out.last().is_none_or(|&q| distance(p, q) > MERGE_EPS) {
            out.push(p);
        }
    }
    while out.len() > 1 && distance(out[0], out[out.len() - 1]) <= MERGE_EPS {
        out.pop();
    }
    if out.len() < 3 { Vec::new() } else { out }
}

/// The faces of `∩ {n_k·x <= d_k}`: one polygon per half-space, in input order,
/// counter-clockwise about its outward normal. A half-space that does not touch the
/// polytope (a redundant plane) gets an empty polygon.
///
/// # Errors
///
/// [`PolytopeError::Unbounded`] when the half-spaces do not enclose a bounded
/// solid, [`PolytopeError::Empty`] when they have an empty intersection.
pub(super) fn polytope_faces(planes: &[HalfSpace]) -> Result<Vec<Vec<[f64; 3]>>, PolytopeError> {
    let reach = planes
        .iter()
        .map(|p| p.offset.abs())
        .fold(1.0_f64, f64::max);
    let half = 1e3 * reach;
    let mut faces = Vec::with_capacity(planes.len());
    for (k, &plane) in planes.iter().enumerate() {
        let mut polygon = start_square(plane, half);
        for (j, &other) in planes.iter().enumerate() {
            if j != k && !polygon.is_empty() {
                polygon = clip_polygon(&polygon, other);
            }
        }
        let polygon = merge_close_points(polygon);
        if polygon
            .iter()
            .any(|p| p.iter().any(|c| c.abs() > 0.5 * half))
        {
            return Err(PolytopeError::Unbounded);
        }
        faces.push(polygon);
    }
    if faces.iter().all(Vec::is_empty) {
        return Err(PolytopeError::Empty);
    }
    Ok(faces)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn half_space(n: [f64; 3], d: f64) -> HalfSpace {
        let len = dot(n, n).sqrt();
        HalfSpace {
            normal: scale(n, len.recip()),
            offset: d,
        }
    }

    /// Counter-clockwise about `normal`: the polygon's area vector points along it.
    fn is_ccw(polygon: &[[f64; 3]], normal: [f64; 3]) -> bool {
        let mut area = [0.0; 3];
        for k in 0..polygon.len() {
            area = add(area, cross(polygon[k], polygon[(k + 1) % polygon.len()]));
        }
        dot(area, normal) > 0.0
    }

    #[test]
    fn cube_faces_are_unit_squares() {
        let planes: Vec<HalfSpace> = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ]
        .iter()
        .map(|&n| half_space(n, 1.0))
        .collect();
        let faces = polytope_faces(&planes).expect("a cube is bounded");
        assert_eq!(faces.len(), 6);
        for (face, plane) in faces.iter().zip(&planes) {
            assert_eq!(face.len(), 4);
            assert!(is_ccw(face, plane.normal));
            for p in face {
                assert!((dot(plane.normal, *p) - 1.0).abs() < 1e-9);
                assert!(p.iter().all(|c| (c.abs() - 1.0).abs() < 1e-9));
            }
        }
    }

    #[test]
    fn octahedron_faces_are_triangles_on_the_axes() {
        let mut planes = Vec::new();
        for sx in [1.0, -1.0] {
            for sy in [1.0, -1.0] {
                for sz in [1.0, -1.0] {
                    planes.push(half_space([sx, sy, sz], 3.0_f64.sqrt().recip()));
                }
            }
        }
        let faces = polytope_faces(&planes).expect("an octahedron is bounded");
        for (face, plane) in faces.iter().zip(&planes) {
            assert_eq!(face.len(), 3, "{face:?}");
            assert!(is_ccw(face, plane.normal));
            for p in face {
                let mut sorted = p.map(f64::abs);
                sorted.sort_by(f64::total_cmp);
                assert!(sorted[0] < 1e-9 && sorted[1] < 1e-9);
                assert!((sorted[2] - 1.0).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn open_and_empty_sets_are_errors() {
        let open = [
            half_space([1.0, 0.0, 0.0], 1.0),
            half_space([0.0, 1.0, 0.0], 1.0),
        ];
        assert_eq!(polytope_faces(&open), Err(PolytopeError::Unbounded));
        let empty = [
            half_space([1.0, 0.0, 0.0], -1.0),
            half_space([-1.0, 0.0, 0.0], -1.0),
        ];
        assert_eq!(polytope_faces(&empty), Err(PolytopeError::Empty));
    }

    #[test]
    fn a_redundant_plane_gets_an_empty_face() {
        let mut planes: Vec<HalfSpace> = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ]
        .iter()
        .map(|&n| half_space(n, 1.0))
        .collect();
        planes.push(half_space([1.0, 1.0, 1.0], 5.0));
        let faces = polytope_faces(&planes).expect("bounded");
        assert_eq!(faces[6].len(), 0);
    }
}
