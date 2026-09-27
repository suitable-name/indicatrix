//! [`measure_solid`]: the external-verification entry point that reduces a
//! plane arrangement to the same dimensionless figures a faceting diagram
//! prints.

use glam::DVec3;

use super::{
    EPS_FACE, GIRDLE_NY,
    caliper::caliper_extents,
    mesh::face_area,
    types::SolidMetrics,
    vertices::{dedup_planes, feasible_vertices},
};

/// Measures the solid bounded by `planes` (`n . x <= m`, unit outward normals).
///
/// Returns `None` when the solid is degenerate or unbounded: fewer than four
/// distinct vertices, zero/negative volume, or any vertex escaping to the
/// bounding blank (a schedule missing its closing planes).
///
/// Deterministic by construction: plain nested loops over the given plane
/// order, total-order sorts, no hashing, no convex-hull library. Two calls with
/// identical inputs produce byte-identical results.
#[must_use]
pub fn measure_solid(planes: &[(DVec3, f64)]) -> Option<SolidMetrics> {
    let planes = dedup_planes(planes);
    let verts = feasible_vertices(&planes)?;
    if verts.len() < 4 {
        return None;
    }

    let volume: f64 = planes
        .iter()
        .map(|&(n, m)| m * face_area(n, m, &verts) / 3.0)
        .sum();
    if !(volume.is_finite() && volume > 0.0) {
        return None;
    }

    let y_max = verts
        .iter()
        .map(|s| s.v.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let y_min = verts.iter().map(|s| s.v.y).fold(f64::INFINITY, f64::min);

    // Girdle band: vertical extent of the vertices lying on any vertical plane.
    let mut girdle_top = f64::NEG_INFINITY;
    let mut girdle_bottom = f64::INFINITY;
    for &(n, m) in planes.iter().filter(|(n, _)| n.y.abs() <= GIRDLE_NY) {
        for s in &verts {
            if (n.dot(s.v) - m).abs() <= EPS_FACE {
                girdle_top = girdle_top.max(s.v.y);
                girdle_bottom = girdle_bottom.min(s.v.y);
            }
        }
    }
    let has_girdle = girdle_top >= girdle_bottom;

    let x_max = verts
        .iter()
        .map(|s| s.v.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let x_min = verts.iter().map(|s| s.v.x).fold(f64::INFINITY, f64::min);
    let z_max = verts
        .iter()
        .map(|s| s.v.z)
        .fold(f64::NEG_INFINITY, f64::max);
    let z_min = verts.iter().map(|s| s.v.z).fold(f64::INFINITY, f64::min);
    let (dx, dz) = (x_max - x_min, z_max - z_min);
    let (width_axis, length_axis) = if dx <= dz { (dx, dz) } else { (dz, dx) };

    let outline: Vec<(f64, f64)> = verts.iter().map(|s| (s.v.x, s.v.z)).collect();
    let (width_caliper, length_caliper) =
        caliper_extents(&outline).unwrap_or((width_axis, length_axis));

    Some(SolidMetrics {
        volume,
        width_axis,
        length_axis,
        width_caliper,
        length_caliper,
        total_height: y_max - y_min,
        crown_height: has_girdle.then_some(y_max - girdle_top),
        pavilion_depth: has_girdle.then_some(girdle_bottom - y_min),
        girdle_thickness: has_girdle.then_some(girdle_top - girdle_bottom),
        vertex_count: verts.len(),
    })
}
