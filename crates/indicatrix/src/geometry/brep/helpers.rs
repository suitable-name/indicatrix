//! Weld, ring, triangulation and validity helpers behind [`super::GemPolyhedron::from_planes`]:
//! the steps that turn the arrangement walk's triple solutions into welded vertices,
//! ordered facet rings, a triangle fan buffer and the manifold, Euler and volume gates.
//! Every helper is a pure function of its arguments; ordering is always `total_cmp` and
//! ascending indices, so the output does not depend on visiting order.

use super::{BrepError, MIN_TRIPLE_DET, MeetCandidate, WeldedVertex};
use glam::{DVec3, Vec3};
use std::{cmp::Ordering, collections::BTreeMap};

/// Root of `i` in the union-find forest, with path halving.
const fn find_root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Welds candidates closer than `radius` into vertices (connected components, so
/// the grouping does not depend on visiting order), then orders the vertices by
/// incident set, ties by position.
pub(super) fn weld_candidates(candidates: &[MeetCandidate], radius: f64) -> Vec<WeldedVertex> {
    let mut parent: Vec<usize> = (0..candidates.len()).collect();
    let mut by_x: Vec<usize> = (0..candidates.len()).collect();
    by_x.sort_unstable_by(|&p, &q| {
        candidates[p]
            .v
            .x
            .total_cmp(&candidates[q].v.x)
            .then(p.cmp(&q))
    });
    for (k, &p) in by_x.iter().enumerate() {
        for &q in &by_x[k + 1..] {
            if candidates[q].v.x - candidates[p].v.x >= radius {
                break;
            }
            if (candidates[q].v - candidates[p].v).length() < radius {
                let (rp, rq) = (find_root(&mut parent, p), find_root(&mut parent, q));
                parent[rp.max(rq)] = rp.min(rq);
            }
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for member in 0..candidates.len() {
        let root = find_root(&mut parent, member);
        groups.entry(root).or_default().push(member);
    }
    let mut welded: Vec<WeldedVertex> = groups
        .into_values()
        .map(|members| weld_group(candidates, &members))
        .collect();
    welded.sort_by(|p, q| {
        p.incident
            .cmp(&q.incident)
            .then_with(|| cmp_position(candidates[p.best].v, candidates[q.best].v))
    });
    welded
}

/// Builds one welded vertex from its (ascending) candidate indices. Candidates
/// arrive in lexicographic triple order, so the strict `>` keeps the
/// lexicographically smallest triple among equally conditioned ones.
fn weld_group(candidates: &[MeetCandidate], members: &[usize]) -> WeldedVertex {
    let mut incident: Vec<usize> = members.iter().flat_map(|&m| candidates[m].planes).collect();
    incident.sort_unstable();
    incident.dedup();
    let mut best = members[0];
    for &m in &members[1..] {
        if candidates[m].det.abs() > candidates[best].det.abs() {
            best = m;
        }
    }
    WeldedVertex { incident, best }
}

/// Total order on positions: `x`, then `y`, then `z`, by `total_cmp`.
fn cmp_position(p: DVec3, q: DVec3) -> Ordering {
    p.x.total_cmp(&q.x)
        .then_with(|| p.y.total_cmp(&q.y))
        .then_with(|| p.z.total_cmp(&q.z))
}

/// [`BrepError::IllConditionedTriple`] for the first vertex (in vertex order) whose
/// best triple is below `MIN_TRIPLE_DET`.
pub(super) fn check_conditioning(
    candidates: &[MeetCandidate],
    welded: &[WeldedVertex],
) -> Result<(), BrepError> {
    for w in welded {
        let best = &candidates[w.best];
        if best.det.abs() < MIN_TRIPLE_DET {
            let [a, b, c] = best.planes;
            return Err(BrepError::IllConditionedTriple {
                a,
                b,
                c,
                det: best.det,
            });
        }
    }
    Ok(())
}

/// Primal feasibility gate: every vertex must satisfy every plane within
/// `tolerance` (normalised frame). Reports the first `(vertex, plane)` pair.
pub(super) fn check_feasible(
    halfspaces: &[(DVec3, f64)],
    positions: &[DVec3],
    tolerance: f64,
    scale: f64,
) -> Result<(), BrepError> {
    for (vertex, &p) in positions.iter().enumerate() {
        for (plane, &(n, m)) in halfspaces.iter().enumerate() {
            let excess = n.dot(p) - m;
            if excess > tolerance {
                return Err(BrepError::InfeasibleVertex {
                    vertex,
                    plane,
                    excess: excess * scale,
                });
            }
        }
    }
    Ok(())
}

/// One ordered vertex ring per plane: the vertices incident to it (see
/// [`order_ring`]); empty when fewer than three are.
pub(super) fn facet_rings(
    halfspaces: &[(DVec3, f64)],
    positions: &[DVec3],
    welded: &[WeldedVertex],
) -> Vec<Vec<u32>> {
    halfspaces
        .iter()
        .enumerate()
        .map(|(plane, &(normal, _))| {
            let ring: Vec<u32> = welded
                .iter()
                .enumerate()
                .filter(|(_, w)| w.incident.binary_search(&plane).is_ok())
                .map(|(i, _)| i as u32)
                .collect();
            order_ring(normal, positions, ring)
        })
        .collect()
}

/// Sorts `ring` counter-clockwise about `normal` (seen from outside) by angle about
/// the ring's centroid, ties by vertex index, then rotates the smallest index to the
/// front. Returns an empty ring for fewer than three vertices.
fn order_ring(normal: DVec3, positions: &[DVec3], ring: Vec<u32>) -> Vec<u32> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let centroid = ring.iter().map(|&i| positions[i as usize]).sum::<DVec3>() / ring.len() as f64;
    // Deterministic in-plane basis, as `stone_metrics`' face rings: start from the
    // world axis least aligned with the normal. `(u, w, normal)` is right-handed, so
    // increasing angle runs counter-clockwise seen from outside.
    let seed = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
        DVec3::X
    } else if normal.y.abs() <= normal.z.abs() {
        DVec3::Y
    } else {
        DVec3::Z
    };
    let u = (seed - normal * normal.dot(seed)).normalize();
    let w = normal.cross(u);
    let mut keyed: Vec<(f64, u32)> = ring
        .into_iter()
        .map(|i| {
            let d = positions[i as usize] - centroid;
            (w.dot(d).atan2(u.dot(d)), i)
        })
        .collect();
    keyed.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.cmp(&q.1)));
    let mut ordered: Vec<u32> = keyed.into_iter().map(|(_, i)| i).collect();
    let first = ordered
        .iter()
        .enumerate()
        .min_by_key(|&(_, &i)| i)
        .map_or(0, |(k, _)| k);
    ordered.rotate_left(first);
    ordered
}

/// Fan-triangulates every facet polygon from its first vertex into the flat index
/// buffer used for rendering.
pub(super) fn triangulate_polygons(facet_polygons: &[Vec<u32>]) -> Vec<u32> {
    let mut triangle_indices = Vec::new();
    for poly in facet_polygons.iter().filter(|p| p.len() >= 3) {
        for pair in poly.windows(2).skip(1) {
            triangle_indices.extend_from_slice(&[poly[0], pair[0], pair[1]]);
        }
    }
    triangle_indices
}

/// Validity gate: every edge of a closed 2-manifold polyhedron must be shared by
/// exactly two facets, and Euler's formula `V - E + F = 2` must hold. Edges are
/// counted in a `BTreeMap`, so the reported non-manifold edge is the smallest one.
pub(super) fn check_euler(
    vertex_count: usize,
    facet_polygons: &[Vec<u32>],
) -> Result<(), BrepError> {
    let mut edge_counts: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for poly in facet_polygons.iter().filter(|p| p.len() >= 3) {
        for (k, &a) in poly.iter().enumerate() {
            let b = poly[(k + 1) % poly.len()];
            *edge_counts.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    if let Some((&edge, &count)) = edge_counts.iter().find(|&(_, &count)| count != 2) {
        return Err(BrepError::NonManifoldEdge { edge, count });
    }

    let face_count = facet_polygons.iter().filter(|p| p.len() >= 3).count();
    let edge_count = edge_counts.len();
    let euler = vertex_count as i64 - edge_count as i64 + face_count as i64;
    if euler != 2 {
        return Err(BrepError::EulerFormulaFailed {
            vertices: vertex_count,
            edges: edge_count,
            faces: face_count,
            euler,
        });
    }
    Ok(())
}

/// Divergence-theorem volume of a triangulated closed mesh, from the origin, summed
/// in `f64` in triangle order. Always non-negative.
pub(super) fn mesh_volume(positions: &[DVec3], triangle_indices: &[u32]) -> f64 {
    triangle_indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|tri| {
            let [a, b, c] = tri.map(|i| positions[i as usize]);
            a.dot(b.cross(c))
        })
        .sum::<f64>()
        .abs()
        / 6.0
}

/// Scales every vertex back to input units and narrows it to `f32`, reporting the
/// first vertex that leaves `f32` range.
pub(super) fn output_vertices(
    candidates: &[MeetCandidate],
    welded: &[WeldedVertex],
    scale: f64,
) -> Result<Vec<Vec3>, BrepError> {
    welded
        .iter()
        .map(|w| {
            let best = &candidates[w.best];
            let vertex = best.v * scale;
            let narrowed = vertex.as_vec3();
            if narrowed.is_finite() {
                Ok(narrowed)
            } else {
                let [a, b, c] = best.planes;
                Err(BrepError::NonFiniteVertex { a, b, c, vertex })
            }
        })
        .collect()
}
