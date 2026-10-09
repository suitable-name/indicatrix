//! Closed triangle shells for [`ZoneShape::MeshShell`](super::ZoneShape::MeshShell): the
//! validity check and the segment-line intersection (f64, sharp edges).
//!
//! Containment is by crossing parity along the whole line through the segment: with a closed
//! shell the line is outside before its first crossing, so it is inside between crossings 1
//! and 2, 3 and 4, and so on. There is no acceleration structure: the shell has at most
//! [`MAX_MESH_TRIANGLES`](super::MAX_MESH_TRIANGLES) triangles and every query tests all of them
//! (Moller-Trumbore). At that size a BVH would cost more in set-up and branching than it saves.

use super::{MAX_MESH_TRIANGLES, MeshProblem};
use std::collections::BTreeMap;

/// Crossings closer than this in the segment parameter are one crossing (a line through a
/// shared edge or vertex hits several triangles).
const MERGE_T: f64 = 1e-12;
/// Barycentric slack so that a line through a shared edge hits at least one of its triangles.
const BARY_EPS: f64 = 1e-12;

/// A copy of a shell's geometry, as held by a [`ZoneKernel`](super::ZoneKernel).
#[derive(Debug, Clone)]
pub(super) struct MeshData {
    pub(super) vertices: Vec<[f64; 3]>,
    pub(super) triangles: Vec<[u32; 3]>,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[2].mul_add(b[2], a[1].mul_add(b[1], a[0] * b[0]))
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[2].mul_add(-b[1], a[1] * b[2]),
        a[0].mul_add(-b[2], a[2] * b[0]),
        a[1].mul_add(-b[0], a[0] * b[1]),
    ]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Checks that the shell is a closed, consistently wound, non-degenerate triangle mesh.
pub(super) fn check(vertices: &[[f64; 3]], triangles: &[[u32; 3]]) -> Result<(), MeshProblem> {
    if triangles.len() < 4 {
        return Err(MeshProblem::TooFewTriangles);
    }
    if triangles.len() > MAX_MESH_TRIANGLES {
        return Err(MeshProblem::TooManyTriangles);
    }
    if vertices.iter().any(|v| !v.iter().all(|c| c.is_finite())) {
        return Err(MeshProblem::NotFinite);
    }
    let n = vertices.len();
    let mut volume6 = 0.0;
    let mut edges: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for tri in triangles {
        if tri.iter().any(|&i| (i as usize) >= n) {
            return Err(MeshProblem::IndexOutOfRange);
        }
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            return Err(MeshProblem::DegenerateTriangle);
        }
        let v0 = vertices[tri[0] as usize];
        let v1 = vertices[tri[1] as usize];
        let v2 = vertices[tri[2] as usize];
        let e1 = sub(v1, v0);
        let e2 = sub(v2, v0);
        let area2 = norm(cross(e1, e2));
        if area2 <= 1e-12 * norm(e1) * norm(e2) || area2 == 0.0 {
            return Err(MeshProblem::DegenerateTriangle);
        }
        volume6 += dot(v0, cross(v1, v2));
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            *edges.entry((a, b)).or_insert(0) += 1;
        }
    }
    if edges.values().any(|&count| count > 1) {
        return Err(MeshProblem::InconsistentWinding);
    }
    if edges.keys().any(|&(a, b)| !edges.contains_key(&(b, a))) {
        return Err(MeshProblem::NotClosed);
    }
    if volume6 == 0.0 {
        return Err(MeshProblem::ZeroVolume);
    }
    Ok(())
}

/// The sorted spans of `t` in `[0, 1]` where the segment `p0 + t dv` is inside the shell.
pub(super) fn line_spans(mesh: &MeshData, p0: [f64; 3], dv: [f64; 3]) -> Vec<(f64, f64)> {
    let dv_len = norm(dv);
    let mut hits: Vec<f64> = Vec::new();
    for tri in &mesh.triangles {
        let v0 = mesh.vertices[tri[0] as usize];
        let v1 = mesh.vertices[tri[1] as usize];
        let v2 = mesh.vertices[tri[2] as usize];
        let e1 = sub(v1, v0);
        let e2 = sub(v2, v0);
        let pvec = cross(dv, e2);
        let det = dot(e1, pvec);
        if det.abs() <= 1e-14 * norm(e1) * norm(e2) * dv_len {
            continue; // parallel to the triangle plane
        }
        let inv = 1.0 / det;
        let tvec = sub(p0, v0);
        let u = dot(tvec, pvec) * inv;
        if !(-BARY_EPS..=1.0 + BARY_EPS).contains(&u) {
            continue;
        }
        let qvec = cross(tvec, e1);
        let v = dot(dv, qvec) * inv;
        if v < -BARY_EPS || u + v > 1.0 + BARY_EPS {
            continue;
        }
        hits.push(dot(e2, qvec) * inv);
    }
    hits.sort_by(f64::total_cmp);

    // Merge crossings that are the same hit seen through several triangles.
    let mut crossings: Vec<f64> = Vec::with_capacity(hits.len());
    for t in hits {
        match crossings.last() {
            Some(&last) if t - last <= MERGE_T => {}
            _ => crossings.push(t),
        }
    }

    let mut spans = Vec::new();
    let mut i = 0;
    while i < crossings.len() {
        let lo = crossings[i];
        // An odd leftover crossing leaves the line inside for good.
        let hi = crossings.get(i + 1).copied().unwrap_or(f64::INFINITY);
        let a = lo.max(0.0);
        let b = hi.min(1.0);
        if b > a {
            spans.push((a, b));
        }
        i += 2;
    }
    spans
}
