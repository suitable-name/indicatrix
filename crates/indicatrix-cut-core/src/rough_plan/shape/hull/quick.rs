//! A fast, deterministic convex hull for the large point sets of a scan.
//!
//! [`hull_faces`](super::hull_faces) inserts the points one at a time and scans EVERY face
//! of the growing hull for each of them (and rebuilds the face list), so on a scan, where
//! almost every point is a hull vertex, it costs about `P^2`: seconds at 10,000 points,
//! minutes at 160,000. This is quickhull with conflict lists, which touches only the faces
//! around each new point: about `P log P`.
//!
//! # What it may NOT be used for
//!
//! Its triangulation of a plane with many coplanar points, and the rounding of its planes,
//! differ from [`hull_faces`](super::hull_faces) in the last bits. It therefore never produces
//! an EXACT outline, a stone's outline or a design's outline: those stay with `hull_faces`,
//! bit for bit. It only decides "this hull has more than `MAX_HULL_PLANES` planes" and
//! feeds the simplification, whose planes are supporting planes over ALL points and so do not
//! depend on which valid hull supplied the normals and areas.
//!
//! # Determinism
//!
//! Nothing is hashed and nothing depends on a float key without an index tie-break:
//! - the seed is the same tetrahedron as the exact hull's;
//! - a point goes to the FIRST face (lowest creation index) that sees it, the points of a list
//!   stay in ascending order;
//! - the next face is the lowest-indexed live face with a non-empty list (an ordered set), and
//!   its apex is the furthest point, the lowest index on a tie;
//! - the visible region is flooded breadth-first in the faces' winding order, so the horizon
//!   and the new faces come out in one fixed order;
//! - the orphaned points of the removed faces are re-tested in ascending order.
//!
//! The same points give the same faces, run after run and on any machine.

use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;

use super::{Face, HullError, first_tetrahedron};

/// One face of the hull under construction.
struct Node {
    face: Face,
    /// Still part of the hull.
    alive: bool,
    /// The not yet processed points this face sees by more than `eps`, ascending.
    conflict: Vec<usize>,
    /// The round of the flood that last classified this face.
    stamp: usize,
    /// Whether the apex of round `stamp` sees this face.
    lit: bool,
}

/// How far `p` is above the plane of `face`.
fn above(face: &Face, p: DVec3) -> f64 {
    face.n.dot(p) - face.d
}

/// Adds `face` to `nodes` and its directed edges to `edges`; its index.
fn push_node(
    nodes: &mut Vec<Node>,
    edges: &mut BTreeMap<(usize, usize), usize>,
    face: Face,
) -> usize {
    let index = nodes.len();
    for edge in face.edges() {
        edges.insert(edge, index);
    }
    nodes.push(Node {
        face,
        alive: true,
        conflict: Vec::new(),
        stamp: 0,
        lit: false,
    });
    index
}

/// The tolerance [`hull_triangles`](super::hull_triangles) uses for `points`, after the same
/// finite check.
pub(super) fn tolerance(points: &[DVec3]) -> Result<f64, HullError> {
    if points.iter().any(|p| !p.is_finite()) {
        return Err(HullError::NotFinite);
    }
    let lo = points
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |m, &p| m.min(p));
    let hi = points
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |m, &p| m.max(p));
    Ok(1e-9 * (hi - lo).max_element().max(1e-12))
}

/// The triangles of the convex hull of `pts`, outward wound, in creation order; `None` when
/// `pts` has no volume (as [`hull_faces`](super::hull_faces)). See the module docs for what
/// this may and may not be used for.
pub(super) fn quick_hull_faces(pts: &[DVec3], eps: f64) -> Option<Vec<Face>> {
    let (seed, seed_faces) = first_tetrahedron(pts, eps)?;
    let mut nodes: Vec<Node> = Vec::with_capacity(pts.len() * 6);
    // The directed edge (a, b) of a live face, to that face: the neighbour across it is the
    // owner of (b, a).
    let mut edges: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for face in seed_faces {
        push_node(&mut nodes, &mut edges, face);
    }
    for (index, &p) in pts.iter().enumerate() {
        if seed.contains(&index) {
            continue;
        }
        if let Some(node) = nodes.iter_mut().find(|n| above(&n.face, p) > eps) {
            node.conflict.push(index);
        }
    }
    // The live faces that still have points to place, by creation index.
    let mut pending: BTreeSet<usize> = (0..nodes.len())
        .filter(|&i| !nodes[i].conflict.is_empty())
        .collect();
    let mut round = 0;
    while let Some(&start) = pending.first() {
        round += 1;
        let apex = nodes[start]
            .conflict
            .iter()
            .copied()
            .fold((f64::NEG_INFINITY, usize::MAX), |best, i| {
                let height = above(&nodes[start].face, pts[i]);
                if height > best.0 { (height, i) } else { best }
            })
            .1;
        let p = pts[apex];

        // Flood the faces the apex sees, breadth first; every crossing from a seen face to
        // an unseen one is a horizon edge, in the seen face's winding direction.
        nodes[start].stamp = round;
        nodes[start].lit = true;
        let mut seen = vec![start];
        let mut horizon: Vec<(usize, usize)> = Vec::new();
        let mut cursor = 0;
        while cursor < seen.len() {
            let face = seen[cursor];
            cursor += 1;
            for (a, b) in nodes[face].face.edges() {
                let Some(&next) = edges.get(&(b, a)) else {
                    continue;
                };
                if nodes[next].stamp != round {
                    let height = above(&nodes[next].face, p);
                    nodes[next].stamp = round;
                    nodes[next].lit = height > eps;
                    if nodes[next].lit {
                        seen.push(next);
                        continue;
                    }
                } else if nodes[next].lit {
                    continue;
                }
                horizon.push((a, b));
            }
        }

        // Remove the seen faces, keeping their points to place again.
        let mut orphans: Vec<usize> = Vec::new();
        for &face in &seen {
            pending.remove(&face);
            for edge in nodes[face].face.edges() {
                edges.remove(&edge);
            }
            nodes[face].alive = false;
            orphans.append(&mut nodes[face].conflict);
        }
        orphans.retain(|&i| i != apex);
        orphans.sort_unstable();

        // The cone from the apex over the horizon. A point goes to the first new face that
        // sees it; one that no new face sees is inside the hull.
        let first_new = nodes.len();
        for (a, b) in horizon {
            push_node(&mut nodes, &mut edges, Face::new(pts, a, b, apex));
        }
        for i in orphans {
            let q = pts[i];
            if let Some(node) = nodes[first_new..]
                .iter_mut()
                .find(|n| above(&n.face, q) > eps)
            {
                node.conflict.push(i);
            }
        }
        pending.extend((first_new..nodes.len()).filter(|&i| !nodes[i].conflict.is_empty()));
    }
    Some(
        nodes
            .into_iter()
            .filter(|n| n.alive)
            .map(|n| n.face)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rough_plan::shape::mesh_fixture::{icosphere, pebble_scan};

    use super::super::{distinct_planes, hull_faces};

    /// Asserts that every plane of `a` has one in `b` within 1e-9, and the other way round,
    /// and that the counts agree.
    fn same_planes(a: &[(DVec3, f64)], b: &[(DVec3, f64)]) {
        assert_eq!(a.len(), b.len(), "plane counts");
        let near = |p: &(DVec3, f64), q: &(DVec3, f64)| {
            (p.0 - q.0).abs().max_element() <= 1e-9 && (p.1 - q.1).abs() <= 1e-9
        };
        for p in a {
            assert!(b.iter().any(|q| near(p, q)), "no match for {p:?}");
        }
        for q in b {
            assert!(a.iter().any(|p| near(p, q)), "no match for {q:?}");
        }
    }

    /// A 20 mm box centred at the origin, every face tessellated 30 x 30 (edges and corners
    /// repeated): 5,400 points on six planes, almost all of them coplanar with others.
    fn tessellated_box() -> Vec<DVec3> {
        let mut points = Vec::new();
        for axis in 0..3 {
            for side in [-10.0, 10.0] {
                for i in 0..30 {
                    for j in 0..30 {
                        let u = 20.0 * (f64::from(i) / 29.0 - 0.5);
                        let v = 20.0 * (f64::from(j) / 29.0 - 0.5);
                        points.push(match axis {
                            0 => DVec3::new(side, u, v),
                            1 => DVec3::new(u, side, v),
                            _ => DVec3::new(u, v, side),
                        });
                    }
                }
            }
        }
        points
    }

    #[test]
    fn the_quick_hull_has_the_planes_of_the_exact_hull() {
        for (name, points, same_faces) in [
            ("icosphere", icosphere(3, 10.0).0, true),
            ("pebble", pebble_scan(4, 3).0, true),
            ("box", tessellated_box(), false),
        ] {
            let eps = tolerance(&points).expect("finite");
            let exact = hull_faces(&points, eps).expect("a volume");
            let quick = quick_hull_faces(&points, eps).expect("a volume");
            same_planes(&distinct_planes(&exact, eps), &distinct_planes(&quick, eps));
            if same_faces {
                assert_eq!(exact.len(), quick.len(), "{name}: face counts");
            }
        }
        let eps = tolerance(&tessellated_box()).expect("finite");
        let quick = quick_hull_faces(&tessellated_box(), eps).expect("a volume");
        assert_eq!(distinct_planes(&quick, eps).len(), 6);
    }

    #[test]
    fn the_quick_hull_is_deterministic_and_order_independent_in_its_planes() {
        let (points, _) = pebble_scan(4, 5);
        let eps = tolerance(&points).expect("finite");
        let key = |faces: &[Face]| -> Vec<([usize; 3], DVec3, f64)> {
            faces.iter().map(|f| (f.v, f.n, f.d)).collect()
        };
        let first = quick_hull_faces(&points, eps).expect("a volume");
        let second = quick_hull_faces(&points, eps).expect("a volume");
        assert!(key(&first) == key(&second), "two runs differ");
        let mut reversed = points;
        reversed.reverse();
        let back = quick_hull_faces(&reversed, eps).expect("a volume");
        same_planes(&distinct_planes(&first, eps), &distinct_planes(&back, eps));
    }

    #[test]
    fn flat_points_have_no_quick_hull() {
        let flat = [DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::new(1.0, 1.0, 0.0)];
        assert!(quick_hull_faces(&flat, 1e-9).is_none());
    }
}
