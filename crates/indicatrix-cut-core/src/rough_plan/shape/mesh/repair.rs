//! Repair of the small defects of a scanned surface, so that a mesh that is almost closed
//! is used as a mesh instead of falling back to its convex hull.
//!
//! Only a mesh that [`check_closed`] refuses comes here; a closed, consistently wound mesh
//! is never touched. Three repairs are tried, the least invasive first:
//!
//! 1. **Weld retry.** A gap that is only a hair wide (vertices that should be one but are a
//!    little further apart than the ordinary weld) is closed by welding again at `1e-6` and
//!    then `1e-5` of the mesh's size. Never coarser, so a real gap stays open.
//! 2. **Winding.** Triangles that run the wrong way round their neighbours are turned: per
//!    edge-connected component, a breadth-first walk from its lowest triangle makes every
//!    shared edge run in opposite directions. A component that cannot be wound consistently
//!    (a Moebius strip) is left alone. Which way a whole shell faces is decided later by
//!    `orient_shells`.
//! 3. **Holes.** A hole whose edge loop is short is filled: at most
//!    [`MAX_HOLE_PERIMETER_FRACTION`] of the bounding-box diagonal round and at most
//!    [`MAX_HOLE_EDGES`] edges. A flat hole is ear-clipped, any other gets a fan about a new
//!    vertex at the loop's centroid. A fill invents material, which is why only a small
//!    hole is filled.
//!
//! Every step is deterministic: ordered maps, loops in order of their lowest vertex, a new
//! centroid vertex appended after the existing ones.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use glam::DVec3;

use super::{
    MIN_AREA_FRACTION, MeshError,
    build::{check_closed, compact, drop_degenerate, weld_within},
    geometry::{area_vector, bounds_of},
    triangulate::triangulate,
};

/// The coarser welds tried on a mesh that stays open, as fractions of its size.
const WELD_RETRIES: [f64; 2] = [1e-6, 1e-5];
/// The longest edge loop a hole may have to be filled, as a fraction of the bounding-box
/// diagonal.
const MAX_HOLE_PERIMETER_FRACTION: f64 = 0.05;
/// The most edges a hole may have to be filled.
const MAX_HOLE_EDGES: usize = 64;
/// How far from its best-fit plane a loop's vertex may be, as a fraction of the size, for the
/// hole to count as flat.
const PLANAR_FRACTION: f64 = 1e-4;

/// A repaired mesh and what was done to it.
#[derive(Debug)]
pub(super) struct Repaired {
    pub verts: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    /// One sentence for the user.
    pub note: String,
}

/// The mesh `verts` and `tris` made closed and consistently wound when only small defects
/// stand in the way, or `None` (the caller keeps `error`, the original verdict).
///
/// `scale` is the mesh's size (the largest side of its bounding box). `error` is what
/// [`check_closed`] said: only an open or an inconsistently wound surface is repaired, a
/// non-manifold one never.
pub(super) fn repair(
    verts: &[DVec3],
    tris: &[[u32; 3]],
    scale: f64,
    error: MeshError,
) -> Option<Repaired> {
    if !matches!(error, MeshError::Open | MeshError::Inconsistent) {
        return None;
    }
    // The cheapest first: winding alone, then the same after a coarser weld, and a hole
    // filled only when joining vertices did not do.
    if let Some(done) = attempt(verts.to_vec(), tris.to_vec(), false, Vec::new(), scale) {
        return Some(done);
    }
    for fraction in WELD_RETRIES {
        let tolerance = fraction * scale;
        let (welded, mut joined) = weld_within(verts, tris, tolerance);
        drop_degenerate(&welded, &mut joined, scale);
        let (kept, compacted) = compact(&welded, &joined);
        let merged = verts.len().saturating_sub(kept.len());
        if merged == 0 || compacted.is_empty() {
            continue;
        }
        let note = format!(
            "closed gaps by joining {merged} vertices closer than {} mm",
            shown(tolerance)
        );
        if let Some(done) = attempt(kept, compacted, false, vec![note], scale) {
            return Some(done);
        }
    }
    attempt(verts.to_vec(), tris.to_vec(), true, Vec::new(), scale)
}

/// A length for a note: three decimals, or more when it is tiny.
fn shown(length: f64) -> String {
    if length >= 0.001 {
        format!("{length:.3}")
    } else {
        format!("{length:.1e}")
    }
}

/// Winds the mesh consistently and, when `fill`, fills its small holes; the result when that
/// leaves it closed.
fn attempt(
    mut verts: Vec<DVec3>,
    mut tris: Vec<[u32; 3]>,
    fill: bool,
    mut notes: Vec<String>,
    scale: f64,
) -> Option<Repaired> {
    match check_closed(&tris) {
        Ok(()) => return finish(verts, tris, &notes),
        Err(MeshError::Open | MeshError::Inconsistent) => {}
        Err(_) => return None,
    }
    let turned = wind_consistently(&mut tris)?;
    if turned > 0 {
        notes.push(format!(
            "turned {turned} {} to wind the surface consistently",
            if turned == 1 { "face" } else { "faces" }
        ));
    }
    if check_closed(&tris).is_ok() {
        return finish(verts, tris, &notes);
    }
    if !fill {
        return None;
    }
    let (holes, widest) = fill_holes(&mut verts, &mut tris, scale)?;
    notes.push(format!(
        "filled {holes} {}, the largest {} mm across",
        if holes == 1 { "hole" } else { "holes" },
        shown(widest)
    ));
    check_closed(&tris).ok()?;
    finish(verts, tris, &notes)
}

/// The repaired mesh with its notes joined into one sentence.
fn finish(verts: Vec<DVec3>, tris: Vec<[u32; 3]>, notes: &[String]) -> Option<Repaired> {
    if notes.is_empty() {
        // Nothing was changed, so nothing was repaired.
        return None;
    }
    let note = format!("The mesh was repaired: {}.", notes.join("; "));
    Some(Repaired { verts, tris, note })
}

/// Whether triangle `tri` runs along the edge `a -> b`.
fn runs_along(tri: [u32; 3], a: u32, b: u32) -> bool {
    (0..3).any(|k| tri[k] == a && tri[(k + 1) % 3] == b)
}

/// Turns triangles so that every edge shared by exactly two of them runs in opposite
/// directions, and returns how many were turned; `None` when some component cannot be wound
/// consistently.
///
/// Each edge-connected component is walked breadth first from its lowest triangle, which
/// keeps its winding, and a neighbour is turned when it runs along the shared edge the same
/// way. When that would turn more than half the component, the whole component is turned the
/// other way instead, which is as consistent and changes fewer faces. Edges with one or more
/// than two users do not connect anything.
pub(super) fn wind_consistently(tris: &mut [[u32; 3]]) -> Option<usize> {
    let mut users: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (t, tri) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            users.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    let mut turn: Vec<Option<bool>> = vec![None; tris.len()];
    let mut turned = 0;
    for start in 0..tris.len() {
        if turn[start].is_some() {
            continue;
        }
        turn[start] = Some(false);
        let mut component = vec![start];
        let mut queue = VecDeque::from([start]);
        while let Some(t) = queue.pop_front() {
            let mine = turn[t] == Some(true);
            let mut tri = tris[t];
            if mine {
                tri.swap(1, 2);
            }
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                let Some(sharing) = users.get(&(a.min(b), a.max(b))) else {
                    continue;
                };
                if sharing.len() != 2 {
                    continue;
                }
                let other = if sharing[0] == t {
                    sharing[1]
                } else {
                    sharing[0]
                };
                // The neighbour must end up running b -> a: turned when it runs a -> b now.
                let need = runs_along(tris[other], a, b);
                match turn[other] {
                    None => {
                        turn[other] = Some(need);
                        component.push(other);
                        queue.push_back(other);
                    }
                    Some(have) if have != need => return None,
                    Some(_) => {}
                }
            }
        }
        let count = component.iter().filter(|&&t| turn[t] == Some(true)).count();
        if 2 * count > component.len() {
            for &t in &component {
                turn[t] = Some(turn[t] != Some(true));
            }
            turned += component.len() - count;
        } else {
            turned += count;
        }
    }
    for (tri, turn) in tris.iter_mut().zip(&turn) {
        if *turn == Some(true) {
            tri.swap(1, 2);
        }
    }
    Some(turned)
}

/// The closed loops of the boundary edges of `tris`, each as its vertices in the direction
/// the triangles run along it, loops in order of their lowest vertex, each starting at it;
/// `None` when the boundary is not a set of simple loops (a vertex where two holes meet, or
/// an edge that runs the wrong way).
pub(super) fn boundary_loops(tris: &[[u32; 3]]) -> Option<Vec<Vec<u32>>> {
    let mut uses: BTreeMap<(u32, u32), [u32; 2]> = BTreeMap::new();
    for tri in tris {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            uses.entry((a.min(b), a.max(b))).or_default()[usize::from(a > b)] += 1;
        }
    }
    let mut next: BTreeMap<u32, u32> = BTreeMap::new();
    for (&(low, high), &[forward, backward]) in &uses {
        let edge = match [forward, backward] {
            [1, 0] => (low, high),
            [0, 1] => (high, low),
            [1, 1] => continue,
            _ => return None,
        };
        // Two boundary edges leaving one vertex: a pinch.
        if next.insert(edge.0, edge.1).is_some() {
            return None;
        }
    }
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut loops = Vec::new();
    for &start in next.keys() {
        if seen.contains(&start) {
            continue;
        }
        let mut ring = vec![start];
        seen.insert(start);
        let mut at = start;
        loop {
            let to = *next.get(&at)?;
            if to == start {
                break;
            }
            if !seen.insert(to) {
                return None;
            }
            ring.push(to);
            at = to;
        }
        loops.push(ring);
    }
    Some(loops)
}

/// Fills every hole of `tris`; the number of holes and the widest one's extent. `None` when
/// the boundary is not simple or some hole is too big to fill (see the module
/// documentation), in which case nothing was changed.
fn fill_holes(
    verts: &mut Vec<DVec3>,
    tris: &mut Vec<[u32; 3]>,
    scale: f64,
) -> Option<(usize, f64)> {
    let loops = boundary_loops(tris)?;
    if loops.is_empty() {
        return None;
    }
    let (lo, hi) = bounds_of(verts.iter().copied());
    let limit = MAX_HOLE_PERIMETER_FRACTION * (hi - lo).length();
    let mut widest = 0.0_f64;
    // The polygon of a loop runs against the boundary, so the fill meets the surface edges in
    // opposite directions.
    let polygons: Vec<Vec<u32>> = loops
        .iter()
        .map(|ring| ring.iter().rev().copied().collect())
        .collect();
    for ring in &polygons {
        if ring.len() > MAX_HOLE_EDGES {
            return None;
        }
        let points: Vec<DVec3> = ring.iter().map(|&v| verts[v as usize]).collect();
        let perimeter: f64 = (0..points.len())
            .map(|i| (points[(i + 1) % points.len()] - points[i]).length())
            .sum();
        if perimeter > limit {
            return None;
        }
        let (plo, phi) = bounds_of(points.iter().copied());
        widest = widest.max((phi - plo).length());
    }
    let mut added: Vec<[u32; 3]> = Vec::new();
    for ring in &polygons {
        let points: Vec<DVec3> = ring.iter().map(|&v| verts[v as usize]).collect();
        let corners = flat_fill(ring, &points, scale).unwrap_or_else(|| {
            // A fan about a new vertex at the centroid.
            let centroid = points.iter().copied().sum::<DVec3>() / points.len() as f64;
            verts.push(centroid);
            let c = (verts.len() - 1) as u32;
            (0..ring.len())
                .map(|i| [ring[i], ring[(i + 1) % ring.len()], c])
                .collect()
        });
        added.extend(corners);
    }
    tris.extend(added);
    Some((polygons.len(), widest))
}

/// The triangles (as vertex numbers) that fill the flat polygon `ring` (`points` its
/// positions), wound as the polygon is; `None` when it is not flat, not simple, or not
/// triangulated completely, so the caller fans it instead.
fn flat_fill(ring: &[u32], points: &[DVec3], scale: f64) -> Option<Vec<[u32; 3]>> {
    if let [a, b, c] = *ring {
        return Some(vec![[a, b, c]]);
    }
    let normal = area_vector(points);
    let length = normal.length();
    if length <= MIN_AREA_FRACTION * scale * scale {
        return None;
    }
    let normal = normal / length;
    let centroid = points.iter().copied().sum::<DVec3>() / points.len() as f64;
    if points
        .iter()
        .any(|&p| normal.dot(p - centroid).abs() > PLANAR_FRACTION * scale)
    {
        return None;
    }
    let triangles = triangulate(
        &[points.to_vec()],
        normal,
        MIN_AREA_FRACTION * scale * scale,
    );
    if triangles.len() + 2 != ring.len() {
        return None;
    }
    let id_of = |p: DVec3| points.iter().position(|&q| q == p).map(|i| ring[i]);
    triangles
        .iter()
        .map(|&[a, b, c]| Some([id_of(a)?, id_of(b)?, id_of(c)?]))
        .collect()
}
