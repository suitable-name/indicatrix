//! [`measure_solid`]: the external-verification entry point that reduces a
//! plane arrangement to the same dimensionless figures a faceting diagram
//! prints.

use glam::DVec3;

use super::{
    EPS_FACE, GIRDLE_NY,
    caliper::caliper_extents,
    mesh::face_area,
    pow2_scale_norm,
    types::SolidMetrics,
    vertices::{dedup_planes, feasible_vertices},
};

/// The largest `|m|` among `planes`, NaN-propagating.
///
/// Absolute magnitude fold used to pick [`pow2_scale_norm`]'s representative
/// scale: the largest `|m|` among `planes`, NaN-propagating so a non-finite
/// offset cannot silently vanish into a plausible-looking scale (mirrors
/// `meet_solver::SolveContext::new`'s anchor fold).
pub(in crate::geometry) fn max_abs_offset(planes: &[(DVec3, f64)]) -> f64 {
    planes
        .iter()
        .map(|&(_, m)| m.abs())
        .fold(0.0_f64, |acc, v| {
            if acc.is_nan() || v.is_nan() {
                f64::NAN
            } else {
                acc.max(v)
            }
        })
}

/// Measures the solid bounded by `planes` (`n . x <= m`, unit outward normals).
///
/// Returns `None` when the solid is degenerate or unbounded: fewer than four
/// distinct vertices, zero/negative volume, or any vertex escaping to the
/// bounding blank (a schedule missing its closing planes).
///
/// Deterministic by construction: plain nested loops over the given plane
/// order, total-order sorts, no hashing, no convex-hull library. Two calls with
/// identical inputs produce byte-identical results.
///
/// Internally normalises every plane offset by [`pow2_scale_norm`] of
/// the arrangement's own representative scale before running any of the
/// arrangement geometry below (all of it uses absolute, order-1-tuned
/// epsilons), then scales every length/volume figure in the returned
/// [`SolidMetrics`] back up by the same factor -- so the reported figures stay
/// in `planes`' own real mast units (as the type doc promises) regardless of
/// the design's absolute scale. A design whose own scale already rounds to
/// `2^0` measures bit-identically to before this normalisation existed.
///
/// Cached measurements: the Rough Planner stores the figures this function
/// (and the caliper pass it calls) produces in the catalogue database. Changing
/// the measuring rule -- the caliper algorithm, plane handling, epsilons or the
/// normalisation above -- means bumping
/// `indicatrix_vault::model::solid_extents::SOLID_EXTENTS_VERSION`, so every
/// stored row counts as missing and is re-measured on the next run.
/// Delegating to a shared helper does not alter the measuring rule; the rule in force
/// is the one the current `SOLID_EXTENTS_VERSION` names.
#[must_use]
pub fn measure_solid(planes: &[(DVec3, f64)]) -> Option<SolidMetrics> {
    measure_solid_inner(planes).map(|(metrics, _)| metrics)
}

/// [`measure_solid`] plus the solid's distinct vertices (in `planes`' own units).
///
/// Evaluates the plane arrangement identically to [`measure_solid`], returning
/// both the physical metrics and the deduplicated vertex positions in `planes`'
/// original coordinate frame.
///
/// Does not alter the measuring rule, so it needs no `SOLID_EXTENTS_VERSION` bump of its own.
#[must_use]
pub fn measure_solid_with_vertices(planes: &[(DVec3, f64)]) -> Option<(SolidMetrics, Vec<DVec3>)> {
    measure_solid_inner(planes)
}

fn measure_solid_inner(planes: &[(DVec3, f64)]) -> Option<(SolidMetrics, Vec<DVec3>)> {
    let planes = dedup_planes(planes);
    let scale = pow2_scale_norm(max_abs_offset(&planes));
    let planes: Vec<(DVec3, f64)> = planes.iter().map(|&(n, m)| (n, m / scale)).collect();
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

    let vertex_count = verts.len();
    let scaled_verts = verts.into_iter().map(|s| s.v * scale).collect();

    Some((
        SolidMetrics {
            volume: volume * scale * scale * scale,
            width_axis: width_axis * scale,
            length_axis: length_axis * scale,
            width_caliper: width_caliper * scale,
            length_caliper: length_caliper * scale,
            total_height: (y_max - y_min) * scale,
            crown_height: has_girdle.then_some((y_max - girdle_top) * scale),
            pavilion_depth: has_girdle.then_some((girdle_bottom - y_min) * scale),
            girdle_thickness: has_girdle.then_some((girdle_top - girdle_bottom) * scale),
            vertex_count,
        },
        scaled_verts,
    ))
}
