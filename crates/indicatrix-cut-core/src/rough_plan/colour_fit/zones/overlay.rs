//! The overlay of the zone boundaries on the photos: where each zone boundary surface meets the
//! mesh surface, drawn through the rig cameras.
//!
//! # Method
//!
//! * Every boundary of a zone is an implicit surface `f(p) = 0` in the zone frame (a plane for a
//!   half space or a slab face, a cylinder, a polygon gauge for a prism, an edge half-plane for a
//!   sector). The outer triangles of the mesh are marched against it: a triangle whose vertex
//!   values differ in sign gives one segment (linear interpolation along its edges, computed
//!   from the endpoints in a canonical order so that neighbouring triangles agree to the last
//!   bit). Planar surfaces are exact. A curved surface is 1-Lipschitz, so a triangle whose
//!   vertices are all farther from it than the triangle's longest edge cannot be crossed and is
//!   skipped; the others are subdivided (4 ways, up to [`OverlayOptions::max_subdivision`]
//!   levels, always to the full depth so neighbours stay compatible) and marched.
//! * A mesh-shell zone is cut against the stone's triangles exactly (triangle against triangle).
//! * Segments are chained into polylines through shared endpoints (quantised to 1e-6 of the mesh
//!   size, in key order, so the result is deterministic).
//! * A polyline is projected through each rig camera with `ViewPose::project` after the alignment
//!   to the rig frame. Refraction is ignored: this is the SURFACE trace (the line a marker pen
//!   would draw on the stone), not where the boundary appears through the glass. A segment is
//!   drawn only when its mesh triangle faces the camera ([`OverlayOptions::front_only`]);
//!   self-occlusion by a concave mesh is not tested.
//!
//! Pixels are in the coordinates of the full-resolution photo of the view (`u` right, `v` down).

use std::collections::BTreeMap;

use glam::DVec3;
use indicatrix::optics::zoning::{Zone, ZoneFrame, ZoneShape, ZonedAbsorption};

use super::geometry::{axis_reference, gauge, polygon_normals};
use crate::rough_plan::{
    locate::{Projection, Scene},
    shape::RoughMesh,
};

/// Settings of the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayOptions {
    /// Draw only the parts of the boundary on triangles that face the camera (default true).
    pub front_only: bool,
    /// Subdivision levels for curved boundaries (default 4).
    pub max_subdivision: u32,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            front_only: true,
            max_subdivision: 4,
        }
    }
}

/// One boundary surface of a zone, traced on the mesh, in the mesh frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceTrace {
    /// The zone index (1..).
    pub zone: usize,
    /// Which boundary surface of the zone (0, 1, ...: a slab has two, a hollow cylinder two,
    /// a sector two).
    pub boundary: usize,
    /// The vertices.
    pub points: Vec<DVec3>,
    /// The outward normal of the mesh triangle of every segment (one per segment).
    pub normals: Vec<DVec3>,
    /// Whether the last vertex joins the first (it is not repeated).
    pub closed: bool,
}

/// One polyline of the overlay of a view.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayPolyline {
    /// The zone index (1..).
    pub zone: usize,
    /// Which boundary surface of the zone.
    pub boundary: usize,
    /// The vertices in photo pixels `[u, v]`.
    pub pixels: Vec<[f64; 2]>,
    /// Whether the last vertex joins the first.
    pub closed: bool,
}

/// The overlay of one view.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewOverlay {
    /// The rig view index.
    pub view: usize,
    /// The polylines.
    pub polylines: Vec<OverlayPolyline>,
}

enum Surface {
    Plane {
        normal: DVec3,
        offset: f64,
    },
    Cylinder {
        point: DVec3,
        dir: DVec3,
        radius: f64,
    },
    Prism {
        point: DVec3,
        dir: DVec3,
        normals: Vec<DVec3>,
        apothem: f64,
    },
    Wedge {
        point: DVec3,
        normal: DVec3,
        along: DVec3,
    },
}

impl Surface {
    fn value(&self, q: DVec3) -> f64 {
        match self {
            Self::Plane { normal, offset } => normal.dot(q) - offset,
            Self::Cylinder { point, dir, radius } => {
                let w = q - *point;
                (w - *dir * w.dot(*dir)).length() - radius
            }
            Self::Prism {
                point,
                dir,
                normals,
                apothem,
            } => {
                let w = q - *point;
                gauge(normals, w - *dir * w.dot(*dir)) - apothem
            }
            Self::Wedge { point, normal, .. } => normal.dot(q - *point),
        }
    }

    /// The part of the segment `a`-`b` (zone frame) that belongs to the boundary: all of it, or
    /// for an edge half-plane of a wedge the part on the side of the edge direction.
    fn clip(&self, a: DVec3, b: DVec3) -> Option<(DVec3, DVec3)> {
        match self {
            Self::Wedge { point, along, .. } => {
                let (ga, gb) = ((a - *point).dot(*along), (b - *point).dot(*along));
                if ga >= 0.0 && gb >= 0.0 {
                    Some((a, b))
                } else if ga < 0.0 && gb < 0.0 {
                    None
                } else {
                    let cut = a + (b - a) * (ga / (ga - gb));
                    Some(if ga >= 0.0 { (a, cut) } else { (cut, b) })
                }
            }
            _ => Some((a, b)),
        }
    }

    const fn is_planar(&self) -> bool {
        matches!(self, Self::Plane { .. } | Self::Wedge { .. })
    }
}

/// The boundary surfaces of an implicit zone, in the zone frame. Empty for a mesh shell (see
/// `shell_triangles`) and for a full-turn sector.
fn boundary_surfaces(zone: &Zone) -> Vec<Surface> {
    match &zone.shape {
        ZoneShape::HalfSpace { normal, offset } => vec![Surface::Plane {
            normal: *normal,
            offset: *offset,
        }],
        ZoneShape::Slab {
            normal,
            offset_min,
            offset_max,
        } => vec![
            Surface::Plane {
                normal: *normal,
                offset: *offset_min,
            },
            Surface::Plane {
                normal: *normal,
                offset: *offset_max,
            },
        ],
        ZoneShape::CoaxialCylinder {
            axis_point,
            axis_dir,
            r_in,
            r_out,
        } => {
            let mut out = vec![Surface::Cylinder {
                point: *axis_point,
                dir: *axis_dir,
                radius: *r_out,
            }];
            if *r_in > 0.0 {
                out.push(Surface::Cylinder {
                    point: *axis_point,
                    dir: *axis_dir,
                    radius: *r_in,
                });
            }
            out
        }
        ZoneShape::CoaxialPrism {
            axis_point,
            axis_dir,
            n_sides,
            r_in,
            r_out,
            phase,
        } => {
            let (u, v) = axis_reference(*axis_dir);
            let normals = polygon_normals(*n_sides, *phase, u, v);
            let mut out = vec![Surface::Prism {
                point: *axis_point,
                dir: *axis_dir,
                normals: normals.clone(),
                apothem: *r_out,
            }];
            if *r_in > 0.0 {
                out.push(Surface::Prism {
                    point: *axis_point,
                    dir: *axis_dir,
                    normals,
                    apothem: *r_in,
                });
            }
            out
        }
        ZoneShape::Sector {
            axis_point,
            axis_dir,
            angle_from,
            angle_to,
        } => {
            if angle_to - angle_from >= std::f64::consts::TAU - 1e-9 {
                return Vec::new();
            }
            let (u, v) = axis_reference(*axis_dir);
            [*angle_from, *angle_to]
                .iter()
                .map(|&theta| {
                    let along = u * theta.cos() + v * theta.sin();
                    Surface::Wedge {
                        point: *axis_point,
                        normal: axis_dir.cross(along),
                        along,
                    }
                })
                .collect()
        }
        ZoneShape::MeshShell { .. } => Vec::new(),
    }
}

/// A traced segment with the normal of its mesh triangle.
struct Segment {
    a: DVec3,
    b: DVec3,
    normal: DVec3,
}

/// A mesh triangle: corners and outward normal.
struct Tri {
    corners: [DVec3; 3],
    normal: DVec3,
}

fn outer_triangles(mesh: &RoughMesh) -> Vec<Tri> {
    let verts = mesh.vertices();
    mesh.outer_triangles()
        .iter()
        .enumerate()
        .map(|(i, t)| Tri {
            corners: t.map(|v| verts[v as usize]),
            normal: mesh.triangle_plane(i as u32).0,
        })
        .collect()
}

fn lexicographically_less(p: DVec3, q: DVec3) -> bool {
    p.x.total_cmp(&q.x)
        .then(p.y.total_cmp(&q.y))
        .then(p.z.total_cmp(&q.z))
        .is_lt()
}

/// The zero of the linear interpolation along an edge, computed from the endpoints in a
/// canonical order so both triangles sharing the edge get the same bits.
fn edge_crossing(p: DVec3, fp: f64, q: DVec3, fq: f64) -> DVec3 {
    let (p, fp, q, fq) = if lexicographically_less(p, q) {
        (p, fp, q, fq)
    } else {
        (q, fq, p, fp)
    };
    let t = fp / (fp - fq);
    p + (q - p) * t
}

/// The segment where the zero set crosses a triangle with these vertex values (sign classes:
/// `>= 0` and `< 0`).
fn crossing_segment(corners: [DVec3; 3], f: [f64; 3]) -> Option<(DVec3, DVec3)> {
    let mut points = [DVec3::ZERO; 2];
    let mut count = 0;
    for (i, j) in [(0_usize, 1_usize), (1, 2), (2, 0)] {
        if (f[i] >= 0.0) != (f[j] >= 0.0) {
            if count == 2 {
                return None;
            }
            points[count] = edge_crossing(corners[i], f[i], corners[j], f[j]);
            count += 1;
        }
    }
    (count == 2).then(|| points.into())
}

fn longest_edge(c: &[DVec3; 3]) -> f64 {
    (c[0] - c[1])
        .length()
        .max((c[1] - c[2]).length())
        .max((c[2] - c[0]).length())
}

fn march(
    surface: &Surface,
    frame: &ZoneFrame,
    corners: [DVec3; 3],
    normal: DVec3,
    depth_left: u32,
    out: &mut Vec<Segment>,
) {
    let f = corners.map(|p| surface.value(frame.inverse_point(p)));
    let all_pos = f.iter().all(|v| *v >= 0.0);
    let all_neg = f.iter().all(|v| *v < 0.0);
    let same_sign = all_pos || all_neg;
    if same_sign {
        if surface.is_planar() {
            return;
        }
        let nearest = f.iter().map(|v| v.abs()).fold(f64::INFINITY, f64::min);
        if nearest > longest_edge(&corners) {
            return;
        }
    }
    if surface.is_planar() || depth_left == 0 {
        if !same_sign && let Some((a, b)) = crossing_segment(corners, f) {
            // The clip works in the zone frame; the cut points are mapped back.
            let local = surface.clip(frame.inverse_point(a), frame.inverse_point(b));
            if let Some((la, lb)) = local {
                let (a, b) = if la == frame.inverse_point(a) && lb == frame.inverse_point(b) {
                    (a, b)
                } else {
                    (frame.point(la), frame.point(lb))
                };
                out.push(Segment { a, b, normal });
            }
        }
        return;
    }
    let [a, b, c] = corners;
    let (ab, bc, ca) = (a.midpoint(b), b.midpoint(c), c.midpoint(a));
    for sub in [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]] {
        march(surface, frame, sub, normal, depth_left - 1, out);
    }
}

fn shell_triangles(zone: &Zone) -> Vec<[DVec3; 3]> {
    match &zone.shape {
        ZoneShape::MeshShell {
            vertices,
            triangles,
        } => triangles
            .iter()
            .filter_map(|t| {
                let p = |i: u32| vertices.get(i as usize).map(|v| DVec3::from_array(*v));
                Some([p(t[0])?, p(t[1])?, p(t[2])?])
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The segment where the triangle `m` meets the triangle `s`, if it does.
fn triangle_triangle(m: [DVec3; 3], s: [DVec3; 3]) -> Option<(DVec3, DVec3)> {
    let cross = (s[1] - s[0]).cross(s[2] - s[0]);
    let len = cross.length();
    if len < 1e-300 {
        return None;
    }
    let n = cross / len;
    let d = n.dot(s[0]);
    let f = m.map(|p| n.dot(p) - d);
    let (p0, p1) = crossing_segment(m, f)?;
    let dir = p1 - p0;
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for i in 0..3 {
        let a = s[i];
        let b = s[(i + 1) % 3];
        let third = s[(i + 2) % 3];
        let mut inward = n.cross(b - a);
        if inward.dot(third - a) < 0.0 {
            inward = -inward;
        }
        let num = inward.dot(p0 - a);
        let den = inward.dot(dir);
        if den.abs() < 1e-300 {
            if num < 0.0 {
                return None;
            }
            continue;
        }
        let t = -num / den;
        if den > 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
        if t0 >= t1 {
            return None;
        }
    }
    (t1 - t0 > 1e-12).then(|| (p0 + dir * t0, p0 + dir * t1))
}

fn bounds(c: &[DVec3; 3]) -> (DVec3, DVec3) {
    (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]))
}

fn overlaps(a: (DVec3, DVec3), b: (DVec3, DVec3)) -> bool {
    a.0.cmple(b.1).all() && b.0.cmple(a.1).all()
}

struct Chain {
    points: Vec<DVec3>,
    normals: Vec<DVec3>,
    closed: bool,
}

type Key = [i64; 3];

fn quantise(p: DVec3, tol: f64) -> Key {
    [
        (p.x / tol).round() as i64,
        (p.y / tol).round() as i64,
        (p.z / tol).round() as i64,
    ]
}

fn walk(
    segments: &[Segment],
    ends: &BTreeMap<Key, Vec<(usize, bool)>>,
    used: &mut [bool],
    start: usize,
    start_at_b: bool,
    tol: f64,
) -> Chain {
    let mut points = Vec::new();
    let mut normals = Vec::new();
    let mut s = start;
    let mut at_b = start_at_b;
    let start_point = if at_b { segments[s].b } else { segments[s].a };
    let start_key = quantise(start_point, tol);
    points.push(start_point);
    let mut closed = false;
    loop {
        used[s] = true;
        let seg = &segments[s];
        let next_point = if at_b { seg.a } else { seg.b };
        normals.push(seg.normal);
        let next_key = quantise(next_point, tol);
        let next = ends.get(&next_key).and_then(|list| {
            list.iter()
                .filter(|(i, _)| !used[*i])
                .min_by_key(|(i, _)| *i)
                .copied()
        });
        if let Some((n, n_at_b)) = next {
            points.push(next_point);
            s = n;
            at_b = n_at_b;
        } else {
            if next_key == start_key && points.len() > 2 {
                closed = true;
            } else {
                points.push(next_point);
            }
            break;
        }
    }
    Chain {
        points,
        normals,
        closed,
    }
}

fn chain_segments(segments: &[Segment], tol: f64) -> Vec<Chain> {
    let mut ends: BTreeMap<Key, Vec<(usize, bool)>> = BTreeMap::new();
    for (i, s) in segments.iter().enumerate() {
        ends.entry(quantise(s.a, tol)).or_default().push((i, false));
        ends.entry(quantise(s.b, tol)).or_default().push((i, true));
    }
    let mut used = vec![false; segments.len()];
    let mut chains = Vec::new();
    for list in ends.values() {
        if list.len() != 1 {
            continue;
        }
        let (s, at_b) = list[0];
        if !used[s] {
            chains.push(walk(segments, &ends, &mut used, s, at_b, tol));
        }
    }
    for s in 0..segments.len() {
        if !used[s] {
            chains.push(walk(segments, &ends, &mut used, s, false, tol));
        }
    }
    chains
}

/// Traces the boundary surfaces of every zone on the mesh surface (mesh frame).
///
/// The result is ordered by zone, then boundary, then by the chain order described in the module
/// docs. Zones whose boundary does not meet the mesh give nothing.
#[must_use]
pub fn surface_traces(
    zoned: &ZonedAbsorption,
    mesh: &RoughMesh,
    options: &OverlayOptions,
) -> Vec<SurfaceTrace> {
    let triangles = outer_triangles(mesh);
    let (lo, hi) = mesh.bounds();
    let scale = (hi - lo).length().max(1e-9);
    let tol = 1e-6 * scale;
    let mut traces = Vec::new();
    for (index, zone) in zoned.zones.iter().enumerate() {
        let zone_index = index + 1;
        let mut boundaries: Vec<Vec<Segment>> = Vec::new();
        let surfaces = boundary_surfaces(zone);
        for surface in &surfaces {
            let mut segments = Vec::new();
            for tri in &triangles {
                march(
                    surface,
                    &zoned.frame,
                    tri.corners,
                    tri.normal,
                    options.max_subdivision,
                    &mut segments,
                );
            }
            boundaries.push(segments);
        }
        let shell = shell_triangles(zone);
        if !shell.is_empty() {
            // The shell is in the zone frame; bring it to the mesh frame.
            let shell: Vec<[DVec3; 3]> = shell
                .iter()
                .map(|t| t.map(|p| zoned.frame.point(p)))
                .collect();
            let shell_bounds: Vec<(DVec3, DVec3)> = shell.iter().map(bounds).collect();
            let mut segments = Vec::new();
            for tri in &triangles {
                let tb = bounds(&tri.corners);
                for (s, sb) in shell.iter().zip(&shell_bounds) {
                    if overlaps(tb, *sb)
                        && let Some((a, b)) = triangle_triangle(tri.corners, *s)
                    {
                        segments.push(Segment {
                            a,
                            b,
                            normal: tri.normal,
                        });
                    }
                }
            }
            boundaries.push(segments);
        }
        for (boundary, mut segments) in boundaries.into_iter().enumerate() {
            segments.retain(|s| quantise(s.a, tol) != quantise(s.b, tol));
            for chain in chain_segments(&segments, tol) {
                traces.push(SurfaceTrace {
                    zone: zone_index,
                    boundary,
                    points: chain.points,
                    normals: chain.normals,
                    closed: chain.closed,
                });
            }
        }
    }
    traces
}

/// Projects one traced polyline through one view: the visible runs as pixel polylines.
fn project_trace(
    trace: &SurfaceTrace,
    scene: &Scene<'_>,
    view: usize,
    front_only: bool,
) -> Vec<(Vec<[f64; 2]>, bool)> {
    let Some(pose) = scene.rig.views.get(view) else {
        return Vec::new();
    };
    let n = trace.normals.len();
    let len = trace.points.len();
    if n == 0 || len < 2 {
        return Vec::new();
    }
    let pixel = |p: DVec3| {
        pose.project(scene.alignment.to_rig(p))
            .map(|v| v.to_array())
    };
    let visible = |i: usize| -> bool {
        let a = trace.points[i];
        let b = trace.points[(i + 1) % len];
        if pixel(a).is_none() || pixel(b).is_none() {
            return false;
        }
        if !front_only {
            return true;
        }
        let mid = scene.alignment.to_rig(a.midpoint(b));
        let normal = scene.alignment.dir_to_rig(trace.normals[i]);
        let direction = match pose.projection {
            Projection::Pinhole { .. } => (mid - pose.position_vec()).normalize_or_zero(),
            Projection::Orthographic { .. } => pose.basis().forward,
        };
        // A triangle seen exactly edge-on is not drawn.
        normal.dot(direction) < -1e-9
    };
    let flags: Vec<bool> = (0..n).map(visible).collect();
    if flags.iter().all(|v| !*v) {
        return Vec::new();
    }
    if flags.iter().all(|v| *v) {
        let pixels: Vec<[f64; 2]> = trace.points.iter().filter_map(|p| pixel(*p)).collect();
        return vec![(pixels, trace.closed)];
    }
    let start = if trace.closed {
        flags.iter().position(|v| !*v).map_or(0, |i| i + 1)
    } else {
        0
    };
    let mut runs: Vec<(Vec<[f64; 2]>, bool)> = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    for k in 0..n {
        let i = (start + k) % n;
        if flags[i] {
            if current.is_empty()
                && let Some(p) = pixel(trace.points[i])
            {
                current.push(p);
            }
            if let Some(p) = pixel(trace.points[(i + 1) % len]) {
                current.push(p);
            }
        } else if !current.is_empty() {
            runs.push((std::mem::take(&mut current), false));
        }
    }
    if !current.is_empty() {
        runs.push((current, false));
    }
    runs
}

/// The overlay of the zone boundaries on every view of the rig: the polylines where each boundary
/// meets the mesh surface, in photo pixels (refraction ignored, see the module docs).
///
/// One [`ViewOverlay`] per rig view, in view order.
#[must_use]
pub fn project_overlay(
    zoned: &ZonedAbsorption,
    scene: &Scene<'_>,
    options: &OverlayOptions,
) -> Vec<ViewOverlay> {
    let traces = surface_traces(zoned, scene.mesh, options);
    (0..scene.rig.views.len())
        .map(|view| {
            let mut polylines = Vec::new();
            for trace in &traces {
                for (pixels, closed) in project_trace(trace, scene, view, options.front_only) {
                    polylines.push(OverlayPolyline {
                        zone: trace.zone,
                        boundary: trace.boundary,
                        pixels,
                        closed,
                    });
                }
            }
            ViewOverlay { view, polylines }
        })
        .collect()
}
