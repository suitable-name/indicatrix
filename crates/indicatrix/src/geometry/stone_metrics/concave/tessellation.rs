//! Tessellating a tool into a convex polytope (and its bounding box).
//!
//! These are the half-space
//! lists the carved-mesh construction in the parent module clips against. Split from the
//! parent module so the file stays readable; the functions are unchanged.

use super::Plane;
use crate::geometry::{
    stone_metrics::mesh::TOOL_ICOSPHERE_LEVEL,
    tool::{ToolKind, ToolPrimitive, ToolSweep},
};
use glam::DVec3;
use std::{
    collections::BTreeMap,
    f64::consts::{PI, TAU},
};

/// A tool's convex polytope and a bounding sphere around it.
pub(in crate::geometry::stone_metrics) struct Polytope {
    /// Outward half-spaces `n . x <= m`, unit `n`, no duplicates.
    pub planes: Vec<Plane>,
    /// Bounding-sphere centre (the tool's origin).
    pub centre: DVec3,
    /// Bounding-sphere radius: every point of the polytope is within it.
    pub radius: f64,
}

fn dvec3(v: [f32; 4]) -> DVec3 {
    DVec3::new(f64::from(v[0]), f64::from(v[1]), f64::from(v[2]))
}

/// Adds the half-space `n_un . (x - c) <= off` (any positive length of `n_un`),
/// skipping a duplicate of an existing plane.
///
/// Duplicates arise for collinear profile runs and for a zero-length stroke; two
/// coincident planes would otherwise yield two coincident faces, i.e. a doubled
/// surface.
fn add_plane(planes: &mut Vec<Plane>, n_un: DVec3, off: f64, c: DVec3) {
    let len = n_un.length();
    if len <= 1e-12 || !len.is_finite() {
        return;
    }
    let n = n_un / len;
    let m = off / len + n.dot(c);
    if planes
        .iter()
        .any(|&(n2, m2)| n.dot(n2) > 1.0 - 1e-12 && (m - m2).abs() < 1e-9)
    {
        return;
    }
    planes.push((n, m));
}

/// Unit icosphere (vertices on the unit sphere, faces wound outward).
///
/// Deterministic: fixed base mesh, midpoints cached in a `BTreeMap`.
fn icosphere(level: u32) -> (Vec<DVec3>, Vec<[usize; 3]>) {
    let t = f64::midpoint(1.0, 5.0_f64.sqrt());
    let mut verts: Vec<DVec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| DVec3::new(x, y, z).normalize())
    .collect();
    let mut faces: Vec<[usize; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..level {
        let mut mids: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let mut mid = |verts: &mut Vec<DVec3>, a: usize, b: usize| -> usize {
            let key = (a.min(b), a.max(b));
            *mids.entry(key).or_insert_with(|| {
                verts.push((verts[a] + verts[b]).normalize());
                verts.len() - 1
            })
        };
        let mut next = Vec::with_capacity(faces.len() * 4);
        for &[a, b, c] in &faces {
            let ab = mid(&mut verts, a, b);
            let bc = mid(&mut verts, b, c);
            let ca = mid(&mut verts, c, a);
            next.push([a, ab, ca]);
            next.push([b, bc, ab]);
            next.push([c, ca, bc]);
            next.push([ab, bc, ca]);
        }
        faces = next;
    }
    // The base list's handedness is not relied on: orient every face outward.
    for f in &mut faces {
        let [a, b, c] = *f;
        if (verts[b] - verts[a])
            .cross(verts[c] - verts[a])
            .dot(verts[a])
            < 0.0
        {
            *f = [a, c, b];
        }
    }
    (verts, faces)
}

/// The radial profile `(z, r)` of a tool along its axis, a concave polyline.
///
/// `stroke` is the along-axis half-stroke: the Minkowski sum with an axial
/// segment shifts each rising run by `-stroke`, each falling run by `+stroke`
/// and inserts a plateau at the peak (the same sliding-window rule
/// `ToolPrimitive` intersects). With `stroke == 0` it is the plain profile.
fn axial_profile(kind: ToolKind, tool: &ToolPrimitive, stroke: f64) -> Vec<(f64, f64)> {
    let hl = f64::from(tool.axis[3]);
    let rim = f64::from(tool.origin[3]);
    let (rn, rp) = (f64::from(tool.profile[0]), f64::from(tool.profile[1]));
    let s = stroke;
    let raw: Vec<(f64, f64)> = match kind {
        // A ball never reaches here; the arm keeps the match exhaustive.
        ToolKind::Ball | ToolKind::Cylinder => vec![(-hl - s, rim), (hl + s, rim)],
        ToolKind::Frustum if rp >= rn => vec![(-hl - s, rn), (hl - s, rp), (hl + s, rp)],
        ToolKind::Frustum => vec![(-hl - s, rn), (-hl + s, rn), (hl + s, rp)],
        ToolKind::Bicone => vec![(-hl - s, rn), (-s, rim), (s, rim), (hl + s, rp)],
    };
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(raw.len());
    for p in raw {
        if out.last().is_none_or(|l| (p.0 - l.0).abs() > 1e-12) {
            out.push(p);
        }
    }
    out
}

/// Tessellates `tool` into a convex polytope; `None` for an invalid tool.
#[expect(
    clippy::too_many_lines,
    reason = "one linear construction per tool family (ball, profile of revolution); splitting it would scatter the shared frame and factors"
)]
pub(in crate::geometry::stone_metrics) fn polytope(
    tool: &ToolPrimitive,
    segments: usize,
) -> Option<Polytope> {
    tool.validate().ok()?;
    let kind = tool.kind()?;
    let sweep = tool.sweep()?;
    let n = segments.max(3);
    let nf = n as f64;
    // Equal-area polygon radius factor, and the matching apothem (what the
    // half-spaces actually sit at).
    let equal_area = (TAU / (nf * (TAU / nf).sin())).sqrt();
    let apothem = equal_area * (PI / nf).cos();
    let c = dvec3(tool.origin);
    let a = dvec3(tool.axis).normalize();
    let stroke = f64::from(tool.profile[2]);
    let swept = sweep != ToolSweep::None && stroke > 0.0;
    let mut planes = Vec::new();

    if kind == ToolKind::Ball {
        let r = f64::from(tool.origin[3]);
        let (verts, faces) = icosphere(TOOL_ICOSPHERE_LEVEL);
        // Scale the icosphere to the true sphere's volume so a dimple removes
        // the right amount of stone (the polytope's own volume constant).
        let unit_volume: f64 = faces
            .iter()
            .map(|&[i, j, k]| verts[i].dot(verts[j].cross(verts[k])) / 6.0)
            .sum();
        let scale = r * (4.0 * PI / 3.0 / unit_volume).cbrt();
        let dir = swept.then(|| {
            if sweep == ToolSweep::AlongAxis {
                a
            } else {
                dvec3(tool.sweep_dir).normalize()
            }
        });
        for &[i, j, k] in &faces {
            let (p, q, s) = (verts[i] * scale, verts[j] * scale, verts[k] * scale);
            let nrm = (q - p).cross(s - p).normalize();
            // Support function of the Minkowski sum with the segment.
            let shift = dir.map_or(0.0, |d| stroke * nrm.dot(d).abs());
            add_plane(&mut planes, nrm, nrm.dot(p) + shift, c);
        }
        if let Some(d) = dir {
            // The cylinder part of the capsule: edge normals of the polygon
            // around the sweep direction. Same radius factor as the ball ends
            // (`scale`, not `r`), so the flats meet the icosphere with no waist.
            let u = d.any_orthonormal_vector();
            let v = d.cross(u);
            for j in 0..n {
                let phi = TAU * (j as f64 + 0.5) / nf;
                add_plane(
                    &mut planes,
                    u * phi.cos() + v * phi.sin(),
                    apothem * scale,
                    c,
                );
            }
        }
        let radius = (scale + if swept { stroke } else { 0.0 }) * (1.0 + 1e-9);
        return Some(Polytope {
            planes,
            centre: c,
            radius,
        });
    }

    let across = swept && sweep == ToolSweep::AcrossAxis;
    let along = if swept && sweep == ToolSweep::AlongAxis {
        stroke
    } else {
        0.0
    };
    let profile = axial_profile(kind, tool, along);
    // Cross-section basis. Across a sweep the polygon's first vertex points
    // along the stroke so the two flats parallel to it are polygon vertices.
    let d = if across {
        dvec3(tool.sweep_dir).normalize()
    } else {
        a.any_orthonormal_vector()
    };
    let v = a.cross(d);
    for seg in profile.windows(2) {
        let ((z0, r0), (z1, r1)) = (seg[0], seg[1]);
        let k = (r1 - r0) / (z1 - z0);
        for j in 0..n {
            let phi = TAU * (j as f64 + 0.5) / nf;
            let rho = d * phi.cos() + v * phi.sin();
            let shift = if across {
                stroke * rho.dot(d).abs()
            } else {
                0.0
            };
            // `rho . q - apothem k z <= apothem (r0 - k z0)`: the polygon's
            // edge plane at the radius the profile has at height `z`.
            #[allow(
                clippy::suboptimal_flops,
                reason = "the plane offset is pinned (identity pins, golden volumes); an outer `mul_add` fuses the shift and moves the last bit"
            )]
            add_plane(
                &mut planes,
                rho - a * (apothem * k),
                apothem * k.mul_add(-z0, r0) + shift,
                c,
            );
        }
        if across {
            // The flats a stadium has parallel to the stroke: the polygon's
            // extreme vertices along `+-v`, which no edge normal covers.
            let (mut e_pos, mut e_neg) = (0.0_f64, 0.0_f64);
            for j in 0..n {
                let s = (TAU * j as f64 / nf).sin();
                e_pos = e_pos.max(s);
                e_neg = e_neg.max(-s);
            }
            for (sign, e) in [(1.0, e_pos), (-1.0, e_neg)] {
                let fe = equal_area * e;
                add_plane(
                    &mut planes,
                    v * sign - a * (fe * k),
                    fe * k.mul_add(-z0, r0),
                    c,
                );
            }
        }
    }
    let (z_lo, z_hi) = (profile[0].0, profile[profile.len() - 1].0);
    add_plane(&mut planes, -a, -z_lo, c);
    add_plane(&mut planes, a, z_hi, c);

    let z_ext = profile.iter().fold(0.0_f64, |m, p| m.max(p.0.abs()));
    let r_ext = profile.iter().fold(0.0_f64, |m, p| m.max(p.1));
    let radius =
        (z_ext.hypot(equal_area * r_ext) + if across { stroke } else { 0.0 }) * (1.0 + 1e-9);
    Some(Polytope {
        planes,
        centre: c,
        radius,
    })
}

/// An oriented box that contains a tool's tessellated polytope.
///
/// A quick, conservative stand-in for the polytope when a caller only needs to know
/// where it cannot be: separated boxes mean separated tools, and a plane the whole box
/// lies inside cannot cut the tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolBounds {
    /// The box centre: the tool's origin.
    pub centre: DVec3,
    /// Orthonormal box axes: the tool axis, then the sweep direction (any perpendicular
    /// of the axis for a tool that does not sweep across it), then their cross product.
    pub axes: [DVec3; 3],
    /// Half the box's length along each of [`Self::axes`].
    pub half_extents: [f64; 3],
}

/// The [`ToolBounds`] of the polytope [`polytope`] builds for `tool` at `segments`;
/// `None` for an invalid tool (which has no polytope either).
///
/// Mirrors [`polytope`]'s own construction: the polygon around the axis has the
/// equal-area circumradius, a swept tool adds its half-stroke along the stroke, and a
/// ball's icosphere is scaled to the sphere's volume. A relative `1e-9` keeps the box
/// outside the polytope against rounding.
pub(in crate::geometry::stone_metrics) fn bounds(
    tool: &ToolPrimitive,
    segments: usize,
) -> Option<ToolBounds> {
    tool.validate().ok()?;
    let kind = tool.kind()?;
    let sweep = tool.sweep()?;
    let nf = segments.max(3) as f64;
    let equal_area = (TAU / (nf * (TAU / nf).sin())).sqrt();
    let c = dvec3(tool.origin);
    let a = dvec3(tool.axis).normalize();
    let stroke = f64::from(tool.profile[2]);
    let swept = sweep != ToolSweep::None && stroke > 0.0;
    let across = swept && sweep == ToolSweep::AcrossAxis;
    let along = if swept && sweep == ToolSweep::AlongAxis {
        stroke
    } else {
        0.0
    };
    let d = if across {
        dvec3(tool.sweep_dir).normalize()
    } else {
        a.any_orthonormal_vector()
    };
    let v = a.cross(d);
    let half: [f64; 3] = if kind == ToolKind::Ball {
        let (verts, faces) = icosphere(TOOL_ICOSPHERE_LEVEL);
        let unit_volume: f64 = faces
            .iter()
            .map(|&[i, j, k]| verts[i].dot(verts[j].cross(verts[k])) / 6.0)
            .sum();
        let scale = f64::from(tool.origin[3]) * (4.0 * PI / 3.0 / unit_volume).cbrt();
        // A swept ball is a capsule: its flats sit on a polygon of the equal-area
        // circumradius around the stroke, and the ends reach `stroke` further.
        let wide = if swept { equal_area * scale } else { scale };
        let long = if swept { scale + stroke } else { scale };
        match sweep {
            ToolSweep::AlongAxis if swept => [long, wide, wide],
            ToolSweep::AcrossAxis if swept => [wide, long, wide],
            _ => [scale; 3],
        }
    } else {
        let profile = axial_profile(kind, tool, along);
        let z_ext = profile.iter().fold(0.0_f64, |m, p| m.max(p.0.abs()));
        let r_ext = profile.iter().fold(0.0_f64, |m, p| m.max(p.1));
        let radial = equal_area * r_ext;
        [z_ext, radial + if across { stroke } else { 0.0 }, radial]
    };
    Some(ToolBounds {
        centre: c,
        axes: [a, d, v],
        half_extents: half.map(|h| h * (1.0 + 1e-9)),
    })
}
