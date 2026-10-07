//! Triangulation of the flat faces a cut leaves on a mesh, for display.
//!
//! The outline of a cut can be a thousand-sided polygon with holes, so the ear clipping
//! here is linear in practice (see [`ear_clip`]).

use glam::DVec3;

use super::geometry::area_vector;

/// Triangulates the face bounded by `loops` (counter-clockwise outlines about `normal`,
/// clockwise holes) by ear clipping, after joining each hole to its outline by a bridge.
/// Triangles smaller than `min_area` (doubled) are dropped; a loop that cannot be
/// triangulated is skipped, which only thins a display.
pub(super) fn triangulate(loops: &[Vec<DVec3>], normal: DVec3, min_area: f64) -> Vec<[DVec3; 3]> {
    let helper = if normal.x.abs() < 0.9 {
        DVec3::X
    } else {
        DVec3::Y
    };
    let u = normal.cross(helper).normalize_or_zero();
    let v = normal.cross(u);
    let flat = |p: DVec3| (p.dot(u), p.dot(v));
    let signed = |poly: &[DVec3]| normal.dot(area_vector(poly));

    let mut outlines: Vec<Vec<DVec3>> = Vec::new();
    let mut holes: Vec<Vec<DVec3>> = Vec::new();
    for outline in loops {
        if signed(outline) > 0.0 {
            outlines.push(outline.clone());
        } else if signed(outline) < 0.0 {
            holes.push(outline.clone());
        }
    }
    // A hole goes into the first outline that contains it, rightmost hole first.
    holes.sort_by(|a, b| {
        let right = |h: &[DVec3]| {
            h.iter()
                .map(|&p| flat(p).0)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        right(b).total_cmp(&right(a))
    });
    for hole in holes {
        let probe = flat(hole[0]);
        let home = outlines.iter().position(|outline| {
            let ring: Vec<(f64, f64)> = outline.iter().map(|&p| flat(p)).collect();
            point_in_ring(probe, &ring)
        });
        if let Some(home) = home {
            outlines[home] = bridge(&outlines[home], &hole, &flat);
        }
    }

    let mut out = Vec::new();
    for outline in &outlines {
        ear_clip(outline, &flat, min_area, normal, &mut out);
    }
    out
}

/// Whether `p` is inside the ring (even-odd).
fn point_in_ring(p: (f64, f64), ring: &[(f64, f64)]) -> bool {
    let mut inside = false;
    for (i, &(x1, y1)) in ring.iter().enumerate() {
        let (x2, y2) = ring[(i + 1) % ring.len()];
        if (y1 > p.1) != (y2 > p.1) && p.0 < (x2 - x1) * (p.1 - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
    }
    inside
}

/// Whether the open segments `a1 a2` and `b1 b2` cross.
fn segments_cross(a1: (f64, f64), a2: (f64, f64), b1: (f64, f64), b2: (f64, f64)) -> bool {
    let side = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
        (q.0 - p.0).mul_add(r.1 - p.1, -((q.1 - p.1) * (r.0 - p.0)))
    };
    let (d1, d2) = (side(a1, a2, b1), side(a1, a2, b2));
    let (d3, d4) = (side(b1, b2, a1), side(b1, b2, a2));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// `outline` with `hole` joined in along the shortest bridge from the hole's rightmost
/// vertex to an outline vertex that sees it.
fn bridge(outline: &[DVec3], hole: &[DVec3], flat: &impl Fn(DVec3) -> (f64, f64)) -> Vec<DVec3> {
    let hi = (0..hole.len())
        .max_by(|&a, &b| flat(hole[a]).0.total_cmp(&flat(hole[b]).0))
        .unwrap_or(0);
    let from = flat(hole[hi]);
    let dist = |p: DVec3| {
        let q = flat(p);
        (q.0 - from.0).hypot(q.1 - from.1)
    };
    let mut candidates: Vec<usize> = (0..outline.len()).collect();
    candidates.sort_by(|&a, &b| {
        dist(outline[a])
            .total_cmp(&dist(outline[b]))
            .then(a.cmp(&b))
    });
    let sees = |at: usize| {
        let to = flat(outline[at]);
        let clear_of = |ring: &[DVec3]| {
            (0..ring.len())
                .all(|i| !segments_cross(from, to, flat(ring[i]), flat(ring[(i + 1) % ring.len()])))
        };
        clear_of(outline) && clear_of(hole)
    };
    let at = candidates
        .iter()
        .copied()
        .find(|&c| sees(c))
        .or_else(|| candidates.first().copied())
        .unwrap_or(0);
    let mut joined = Vec::with_capacity(outline.len() + hole.len() + 2);
    joined.extend_from_slice(&outline[..=at]);
    for k in 0..=hole.len() {
        joined.push(hole[(hi + k) % hole.len()]);
    }
    joined.push(outline[at]);
    joined.extend_from_slice(&outline[at + 1..]);
    joined
}

/// Ear-clips the counter-clockwise ring `ring` into `out`.
///
/// The ring is a doubly linked list, so clipping an ear costs nothing, and only reflex
/// vertices are tested for lying inside a candidate ear (a convex vertex cannot be inside
/// the ear of a simple polygon). A clip can only turn a reflex neighbour convex, so the
/// reflex list shrinks as the ring does. The work is O(n r), `r` the reflex count, which is
/// O(n) for the outline of a cut through a smooth scan; the loop gives up after a whole
/// lap without a clip, which only thins a display.
fn ear_clip(
    ring: &[DVec3],
    flat: &impl Fn(DVec3) -> (f64, f64),
    min_area: f64,
    normal: DVec3,
    out: &mut Vec<[DVec3; 3]>,
) {
    let n = ring.len();
    if n < 3 {
        return;
    }
    let plan: Vec<(f64, f64)> = ring.iter().map(|&p| flat(p)).collect();
    let cross = |a: usize, b: usize, c: usize| {
        let (pa, pb, pc) = (plan[a], plan[b], plan[c]);
        (pb.0 - pa.0).mul_add(pc.1 - pa.1, -((pb.1 - pa.1) * (pc.0 - pa.0)))
    };
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = vec![true; n];
    let mut is_reflex: Vec<bool> = (0..n).map(|i| cross(prev[i], i, next[i]) < 0.0).collect();
    let mut reflex: Vec<usize> = (0..n).filter(|&i| is_reflex[i]).collect();
    let mut live = n;
    let mut at = 0;
    let mut idle = 0;
    while live >= 3 && idle <= live {
        let (a, c) = (prev[at], next[at]);
        let turn = cross(a, at, c);
        // A zero-width spike or a collinear vertex has no area: drop it, emitting nothing.
        let spike = turn.abs() <= min_area;
        let ear = !spike
            && turn > 0.0
            && !reflex.iter().any(|&k| {
                if !(alive[k] && is_reflex[k]) {
                    return false;
                }
                let same = |q: usize| (ring[k] - ring[q]).length_squared() <= 1e-24;
                if same(a) || same(at) || same(c) {
                    return false;
                }
                cross(a, at, k) >= 0.0 && cross(at, c, k) >= 0.0 && cross(c, a, k) >= 0.0
            });
        if !(spike || ear) {
            at = c;
            idle += 1;
            continue;
        }
        if ear {
            let tri = [ring[a], ring[at], ring[c]];
            // Counter-clockwise about the normal means the triangle's own normal agrees.
            if normal.dot((tri[1] - tri[0]).cross(tri[2] - tri[0])) > 0.0 {
                out.push(tri);
            }
        }
        alive[at] = false;
        is_reflex[at] = false;
        next[a] = c;
        prev[c] = a;
        live -= 1;
        idle = 0;
        for k in [a, c] {
            let now = live >= 3 && cross(prev[k], k, next[k]) < 0.0;
            if now && !is_reflex[k] {
                reflex.push(k);
            }
            is_reflex[k] = now;
        }
        if reflex.len() > 2 * live + 8 {
            reflex.retain(|&k| alive[k] && is_reflex[k]);
        }
        at = c;
    }
}
