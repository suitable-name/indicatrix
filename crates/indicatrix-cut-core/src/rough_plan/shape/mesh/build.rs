//! Turning raw points and triangles into a checked solid: welding, compaction, the closed
//! manifold check and the orientation of every shell.
//!
//! Everything is deterministic: ordered maps, first-use numbering and triangles in file
//! order, so a mesh never depends on anything but its input.

use std::collections::BTreeMap;

use glam::DVec3;

use super::{
    MIN_AREA_FRACTION, MeshError, RAY_DIRECTIONS, WELD_FRACTION,
    geometry::{bounds_of, ray_meets_triangle},
};

/// The vertices of `tris` welded by proximity, and the triangles over them.
///
/// A point joins the first earlier vertex that lies within the weld tolerance
/// (`WELD_FRACTION` of `scale`) of it on every axis; a point with no such vertex starts a
/// new one. Vertices are numbered in order of first use by a triangle, and a welded vertex
/// keeps the position of the first of its members, so the result depends on nothing but the
/// input. Proximity is looked up in a grid of one-tolerance cells and the 27 cells around
/// the point are searched, so two copies of a vertex are welded even when a cell boundary
/// runs between them. Welding does not chain: a point joins a vertex, never a point that
/// joined one.
pub(super) fn weld(points: &[DVec3], tris: &[[u32; 3]], scale: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    weld_within(points, tris, WELD_FRACTION * scale)
}

/// [`weld`] with the tolerance given as a length: the repair retries a coarser weld on a mesh
/// that stayed open.
pub(super) fn weld_within(
    points: &[DVec3],
    tris: &[[u32; 3]],
    quantum: f64,
) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    // An exact repeat of a position, the common case, skips the neighbourhood search.
    let mut exact: BTreeMap<[u64; 3], u32> = BTreeMap::new();
    // The vertices of every cell, in the order they were made.
    let mut cells: BTreeMap<[i64; 3], Vec<u32>> = BTreeMap::new();
    let mut verts: Vec<DVec3> = Vec::new();
    let mut welded = Vec::with_capacity(tris.len());
    for tri in tris {
        welded.push(tri.map(|v| {
            let p = points[v as usize];
            let bits = [p.x, p.y, p.z].map(f64::to_bits);
            if let Some(&id) = exact.get(&bits) {
                return id;
            }
            let cell = [p.x, p.y, p.z].map(|c| (c / quantum).floor() as i64);
            let id = near_vertex(&cells, &verts, cell, p, quantum).unwrap_or_else(|| {
                verts.push(p);
                let id = (verts.len() - 1) as u32;
                cells.entry(cell).or_default().push(id);
                id
            });
            exact.insert(bits, id);
            id
        }));
    }
    (verts, welded)
}

/// The first-made vertex within `tolerance` of `p` on every axis, searched in the 27 cells
/// around `cell`.
fn near_vertex(
    cells: &BTreeMap<[i64; 3], Vec<u32>>,
    verts: &[DVec3],
    cell: [i64; 3],
    p: DVec3,
    tolerance: f64,
) -> Option<u32> {
    let mut best: Option<u32> = None;
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                let key = [
                    cell[0].saturating_add(dx),
                    cell[1].saturating_add(dy),
                    cell[2].saturating_add(dz),
                ];
                let Some(ids) = cells.get(&key) else {
                    continue;
                };
                for &id in ids {
                    let near = (verts[id as usize] - p).abs().max_element() <= tolerance;
                    if near && best.is_none_or(|first| id < first) {
                        best = Some(id);
                    }
                }
            }
        }
    }
    best
}

/// Removes the triangles that name a vertex twice or whose doubled area is below
/// `MIN_AREA_FRACTION` of the squared `scale`.
pub(super) fn drop_degenerate(verts: &[DVec3], tris: &mut Vec<[u32; 3]>, scale: f64) {
    tris.retain(|tri| {
        let [a, b, c] = tri.map(|v| verts[v as usize]);
        let doubled_area = (b - a).cross(c - a).length();
        tri[0] != tri[1]
            && tri[1] != tri[2]
            && tri[0] != tri[2]
            && doubled_area > MIN_AREA_FRACTION * scale * scale
    });
}

/// `tris` renumbered over only the vertices they use, in order of first use.
pub(super) fn compact(verts: &[DVec3], tris: &[[u32; 3]]) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut renumber: BTreeMap<u32, u32> = BTreeMap::new();
    let mut kept = Vec::new();
    let mut out = Vec::with_capacity(tris.len());
    for tri in tris {
        out.push(tri.map(|v| {
            *renumber.entry(v).or_insert_with(|| {
                kept.push(verts[v as usize]);
                (kept.len() - 1) as u32
            })
        }));
    }
    (kept, out)
}

/// Checks that the triangles form a closed, consistently wound manifold: every undirected
/// edge in exactly two triangles, once per direction.
///
/// The test is per edge, so several disjoint closed surfaces (an outer surface and a cavity)
/// each pass; [`orient_shells`] then decides which way each faces.
pub(super) fn check_closed(tris: &[[u32; 3]]) -> Result<(), MeshError> {
    // Per undirected edge (low, high): uses low -> high and uses high -> low.
    let mut edges: BTreeMap<(u32, u32), [u32; 2]> = BTreeMap::new();
    for tri in tris {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let uses = edges.entry((a.min(b), a.max(b))).or_default();
            uses[usize::from(a > b)] += 1;
        }
    }
    let mut open = false;
    let mut inconsistent = false;
    for uses in edges.values() {
        match *uses {
            [1, 1] => {}
            [a, b] if a + b > 2 => return Err(MeshError::NonManifold),
            [a, b] if a + b == 1 => open = true,
            _ => inconsistent = true,
        }
    }
    if open {
        Err(MeshError::Open)
    } else if inconsistent {
        Err(MeshError::Inconsistent)
    } else {
        Ok(())
    }
}

/// The signed volume of the tetrahedron of triangle `tri` and `origin`.
fn triangle_volume(verts: &[DVec3], tri: [u32; 3], origin: DVec3) -> f64 {
    let [a, b, c] = tri.map(|v| verts[v as usize] - origin);
    a.dot(b.cross(c)) / 6.0
}

/// The signed volume the triangles enclose, measured from `origin` (a corner of the mesh,
/// for accuracy).
pub(super) fn signed_volume(verts: &[DVec3], tris: &[[u32; 3]], origin: DVec3) -> f64 {
    tris.iter()
        .map(|&tri| triangle_volume(verts, tri, origin))
        .sum()
}

/// The signed volume the triangles of `shell` (indices into `tris`) enclose.
fn shell_volume(verts: &[DVec3], tris: &[[u32; 3]], shell: &[usize], origin: DVec3) -> f64 {
    shell
        .iter()
        .map(|&t| triangle_volume(verts, tris[t], origin))
        .sum()
}

/// The representative of `i`'s set in the union-find forest `parent`, compressing the path.
const fn root_of(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// The closed shells of `tris`: the sets of triangles that share edges, each set ascending,
/// the sets in order of their first triangle.
fn shells_of(tris: &[[u32; 3]]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    let mut first_use: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for (t, tri) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let other = *first_use.entry((a.min(b), a.max(b))).or_insert(t);
            let (ra, rb) = (root_of(&mut parent, other), root_of(&mut parent, t));
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    let mut shell_of_root: BTreeMap<usize, usize> = BTreeMap::new();
    let mut shells: Vec<Vec<usize>> = Vec::new();
    for t in 0..tris.len() {
        let root = root_of(&mut parent, t);
        let index = *shell_of_root.entry(root).or_insert_with(|| {
            shells.push(Vec::new());
            shells.len() - 1
        });
        shells[index].push(t);
    }
    shells
}

/// Whether `p` is inside the closed surface made of the triangles of `shell`: the majority
/// of three ray parities, like [`RoughMesh::contains_point`](super::RoughMesh::contains_point)
/// but over one shell and without the BVH, which does not exist yet.
fn shell_contains(verts: &[DVec3], tris: &[[u32; 3]], shell: &[usize], p: DVec3) -> bool {
    let odd = RAY_DIRECTIONS
        .iter()
        .filter(|&&dir| {
            let crossings = shell
                .iter()
                .filter(|&&t| ray_meets_triangle(p, dir, tris[t].map(|v| verts[v as usize])))
                .count();
            crossings % 2 == 1
        })
        .count();
    odd >= 2
}

/// A point strictly inside a face of `shell`, to ask which other shells enclose it: the
/// centroid of its largest triangle (the first of equals).
///
/// A vertex of the shell is a poor probe. Two closed shells can touch at a vertex (two
/// bodies corner to corner, a cavity touching the outer surface at a point) or one shell's
/// vertex can rest on a face of another, and a point ON a surface has no defined ray parity
/// there. The centroid of the largest triangle lies well inside a face, away from the
/// vertices and edges of its shell, so it lies on another shell only where the two surfaces
/// overlap, which stays the documented limitation.
fn probe_point(verts: &[DVec3], tris: &[[u32; 3]], shell: &[usize]) -> DVec3 {
    let corners = |t: usize| tris[t].map(|v| verts[v as usize]);
    let area = |t: usize| {
        let [a, b, c] = corners(t);
        (b - a).cross(c - a).length_squared()
    };
    let largest = shell.iter().fold(
        shell[0],
        |best, &t| if area(t) > area(best) { t } else { best },
    );
    let [a, b, c] = corners(largest);
    (a + b + c) / 3.0
}

/// Turns every closed shell of `tris` so that its triangles face out of the material it
/// bounds.
///
/// A lone shell is turned outward when its signed volume is negative. With several shells
/// (a hollow rough has an outer surface and one surface per cavity, and a cavity can hold an
/// island) the nesting depth of a shell decides, the number of OTHER shells that contain it:
/// an even depth is a body, whose normals point outward (positive signed volume); an odd
/// depth is a cavity, whose normals point into the void (negative signed volume). The total
/// signed volume then adds the bodies and subtracts the cavities, whichever way the file
/// wound them, and a cavity's planes face the right way for the planner's cutting rows.
///
/// Containment is decided with one point inside a face of the shell ([`probe_point`]), so
/// shells that cross each other (a self-intersecting scan) have no defined depth; that stays
/// the documented limitation. Shells that merely touch, at a vertex or with a vertex on a
/// face, do not cross, and each keeps its own depth.
///
/// The work is the number of shells times the triangles of the shells whose bounding box
/// holds the probe, three rays each, without the BVH (it needs the oriented mesh). The
/// triangle cap bounds it, so a scan with very many tiny internal shells is slow to
/// import, not stuck.
pub(super) fn orient_shells(verts: &[DVec3], tris: &mut [[u32; 3]], origin: DVec3) {
    let shells = shells_of(tris);
    if let [only] = shells.as_slice() {
        if shell_volume(verts, tris, only, origin) < 0.0 {
            flip(tris, only);
        }
        return;
    }
    let boxes: Vec<(DVec3, DVec3)> = shells
        .iter()
        .map(|shell| {
            let corners = shell.iter().flat_map(|&t| tris[t]);
            bounds_of(corners.map(|v| verts[v as usize]))
        })
        .collect();
    // Every depth comes from the winding-blind containment test, so the flips can wait.
    let mut flips = Vec::new();
    for (i, shell) in shells.iter().enumerate() {
        let probe = probe_point(verts, tris, shell);
        let depth = shells
            .iter()
            .enumerate()
            .filter(|&(j, other)| {
                let (lo, hi) = boxes[j];
                j != i
                    && probe.cmpge(lo).all()
                    && probe.cmple(hi).all()
                    && shell_contains(verts, tris, other, probe)
            })
            .count();
        let body = depth.is_multiple_of(2);
        if (shell_volume(verts, tris, shell, origin) >= 0.0) != body {
            flips.push(i);
        }
    }
    for i in flips {
        flip(tris, &shells[i]);
    }
}

/// Reverses the winding of the triangles of `shell`.
fn flip(tris: &mut [[u32; 3]], shell: &[usize]) {
    for &t in shell {
        tris[t].swap(1, 2);
    }
}
