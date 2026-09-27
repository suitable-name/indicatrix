//! Shared test fixtures for `GemPolyhedron` topology checks, used by both the B-Rep
//! reconstruction tests and the real `.asc`-design end-to-end tests.

use indicatrix::geometry::brep::GemPolyhedron;
use std::collections::HashSet;

/// Independently recomputes the polyhedron's edge set from its public
/// `facet_polygons` field (deliberately not trusting anything internal to
/// `from_planes`), and returns `(V, E, F)`. Panics if any edge is not shared by
/// exactly two facets, or if Euler's formula `V - E + F = 2` does not hold.
pub fn assert_euler_formula(hull: &GemPolyhedron) -> (usize, usize, usize) {
    let faces: Vec<&Vec<u32>> = hull
        .facet_polygons
        .iter()
        .filter(|p| p.len() >= 3)
        .collect();

    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut edge_hits: std::collections::HashMap<(u32, u32), u32> =
        std::collections::HashMap::new();
    for poly in &faces {
        for w in 0..poly.len() {
            let a = poly[w];
            let b = poly[(w + 1) % poly.len()];
            let key = if a < b { (a, b) } else { (b, a) };
            edges.insert(key);
            *edge_hits.entry(key).or_insert(0) += 1;
        }
    }
    for (edge, count) in &edge_hits {
        assert_eq!(
            *count, 2,
            "edge {edge:?} is shared by {count} facets, not exactly 2 (non-manifold mesh)"
        );
    }

    let v = hull.vertices.len();
    let e = edges.len();
    let f = faces.len();
    assert_eq!(
        v as i64 - e as i64 + f as i64,
        2,
        "Euler's formula V - E + F = 2 failed: V={v}, E={e}, F={f}"
    );
    (v, e, f)
}
