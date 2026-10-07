//! The convex outline of a CARVED design: the corners and caliper extents of the hull of a
//! concave design's carved mesh, for the Rough planner.
//!
//! A concave design is its facet stone minus the curved tools. When a tool removes a vertex
//! that sets the stone's outline (the manufacturability check's `ToolRemovesHullVertex`), the
//! real outline is smaller than the flat one, and the planner can place the stone tighter.
//! The app measures a design WITH tools from the vertices of its carved mesh, reduced here to
//! their convex hull with a variant of the planner's own incremental hull (so the fit stage
//! walks the few hull corners, never the raw mesh vertices). A design WITHOUT tools never
//! comes through here.
//!
//! The carved mesh's vertices are not exact: the clipping snaps every polygon it touched to
//! a grid of about `1e-9` of the stone's width, while the polygons it did not touch keep
//! their exact corners. A box with a dimple on its top face therefore has its top corners
//! twice (exact and snapped), and a few dozen points on its top edges (where the tool's
//! tessellation planes cut the top polygon) that are a grid step below the exact corners.
//! That noise is below [`HULL_EPS`], but the hull's triangles through two such points a
//! short distance apart are tilted by noise over that distance, far more than the noise
//! itself. [`carved_hull_faces`] and [`is_corner`] are written for that input.
//!
//! The reduction is deterministic: a total-order sort, [`carved_hull_faces`] adds points in
//! index order, and the corners come out in the order of the sorted, de-duplicated input.

use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use indicatrix::geometry::stone_metrics::caliper_frame;

use super::{Face, first_tetrahedron};

/// The relative tolerance of the hull, as in [`import_hull`](super::import_hull).
const HULL_EPS: f64 = 1e-9;
/// Hull triangles whose unit normals are within this angle (radians) bound the same plane,
/// and three planes are independent when `|(a x b) . c|` of their normals exceeds it.
///
/// A triangle through two vertices `delta` apart that the clipping's rounding (about
/// [`HULL_EPS`] of the stone's width, `w`) put at different heights is tilted by about
/// `HULL_EPS * w / delta`: `1e-6` for vertices a thousandth of the stone apart, which is
/// common where a tool's tessellation plane cuts a facet near its corner. `1e-4` absorbs
/// vertices down to `1e-5 w` apart (a sliver beyond that costs at most one extra corner
/// on a hull edge), while no two facets of a design or a tool's tessellation meet at
/// `0.006` degrees.
const COPLANAR_ANGLE: f64 = 1e-4;
/// `cos(COPLANAR_ANGLE)`, to second order.
const COPLANAR_COS: f64 = 1.0 - COPLANAR_ANGLE * COPLANAR_ANGLE / 2.0;

/// The horizontal and vertical extents of a stone's outline corners, in the layout of the
/// stored solid extents. The stone's up axis is `y`; the outline is the `x`/`z` plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutlineExtents {
    /// Rotating-caliper width (the smaller horizontal extent).
    pub width_caliper: f64,
    /// Rotating-caliper length.
    pub length_caliper: f64,
    /// The smaller axis-aligned horizontal extent.
    pub width_axis: f64,
    /// The larger axis-aligned horizontal extent.
    pub length_axis: f64,
    /// The vertical extent, table to culet.
    pub height: f64,
}

/// A carved design's convex corners and their extents.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignOutline {
    /// The vertices of the convex hull of the carved mesh.
    pub corners: Vec<DVec3>,
    /// The extents of those corners.
    pub extents: OutlineExtents,
}

/// The vertices of the convex hull of `points`, deduplicated and in sorted order; `None` when
/// a coordinate is not finite or the points have no volume.
///
/// Every returned corner is one of the given points, bit for bit.
#[must_use]
pub fn convex_corners(points: &[DVec3]) -> Option<Vec<DVec3>> {
    if points.is_empty() || points.iter().any(|p| !p.is_finite()) {
        return None;
    }
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| {
        a.x.total_cmp(&b.x)
            .then(a.y.total_cmp(&b.y))
            .then(a.z.total_cmp(&b.z))
    });
    let lo = pts
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |m, &p| m.min(p));
    let hi = pts
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |m, &p| m.max(p));
    let eps = HULL_EPS * (hi - lo).max_element().max(1e-12);
    pts.dedup_by(|a, b| (*a - *b).abs().max_element() <= eps);
    let faces = carved_hull_faces(&pts, eps)?;
    // The hull keeps a vertex that a later point made coplanar with a face (or collinear
    // with an edge) when that point was within `eps` of the vertex's triangles: it is a
    // vertex of the triangulation, not a corner. A corner is where three planes with
    // independent normals meet.
    let mut normals: BTreeMap<usize, Vec<DVec3>> = BTreeMap::new();
    for face in faces.iter().filter(|f| f.n != DVec3::ZERO) {
        for i in face.v {
            normals.entry(i).or_default().push(face.n);
        }
    }
    Some(
        normals
            .into_iter()
            .filter(|(_, around)| is_corner(around))
            .map(|(i, _)| pts[i])
            .collect(),
    )
}

/// The triangles of the convex hull of `pts` (sorted and de-duplicated), outward wound;
/// `None` when the points have no volume. Points are added in index order, so the result
/// is deterministic.
///
/// [`hull_faces`](super::hull_faces) with one difference: the faces a point replaces are a
/// connected patch, the faces it is above by more than `eps` grown over neighbouring faces
/// it is within `eps` of. The carved mesh's vertices scatter around the real planes, so a
/// point on a hull plane is above some of that plane's tilted triangles and below others.
/// Replacing only the former leaves a horizon that is not one loop, and the faces built on
/// it point into the hull and are never replaced; they would hand [`is_corner`] planes that
/// bound nothing, and every vertex on a hull edge would count as a corner.
fn carved_hull_faces(pts: &[DVec3], eps: f64) -> Option<Vec<Face>> {
    let (seed, mut faces) = first_tetrahedron(pts, eps)?;
    for (index, &p) in pts.iter().enumerate() {
        if seed.contains(&index) {
            continue;
        }
        let height: Vec<f64> = faces.iter().map(|f| f.n.dot(p) - f.d).collect();
        let mut seen: Vec<bool> = height.iter().map(|&h| h > eps).collect();
        if !seen.contains(&true) {
            continue;
        }
        // Each directed edge belongs to one face; the face across it owns the reverse edge.
        let owner: BTreeMap<(usize, usize), usize> = faces
            .iter()
            .enumerate()
            .flat_map(|(i, f)| f.edges().into_iter().map(move |edge| (edge, i)))
            .collect();
        let mut queue: Vec<usize> = (0..faces.len()).filter(|&i| seen[i]).collect();
        while let Some(i) = queue.pop() {
            for (a, b) in faces[i].edges() {
                if let Some(&j) = owner.get(&(b, a))
                    && !seen[j]
                    && height[j] > -eps
                {
                    seen[j] = true;
                    queue.push(j);
                }
            }
        }
        let (gone, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut faces)
            .into_iter()
            .zip(seen)
            .partition(|&(_, s)| s);
        let lit: BTreeSet<(usize, usize)> = gone.iter().flat_map(|(f, _)| f.edges()).collect();
        faces = rest.into_iter().map(|(f, _)| f).collect();
        faces.extend(
            gone.iter()
                .flat_map(|(f, _)| f.edges())
                .filter(|&(a, b)| !lit.contains(&(b, a)))
                .map(|(a, b)| Face::new(pts, a, b, index)),
        );
    }
    Some(faces)
}

/// Whether a hull vertex whose triangles have the outward unit normals `around` is a true
/// corner: the normals bound at least three planes (normals within [`COPLANAR_ANGLE`] of
/// each other are one plane) and three of those planes are independent.
fn is_corner(around: &[DVec3]) -> bool {
    let mut planes: Vec<DVec3> = Vec::new();
    for &n in around {
        if !planes.iter().any(|p| p.dot(n) >= COPLANAR_COS) {
            planes.push(n);
        }
    }
    for (i, &a) in planes.iter().enumerate() {
        for (j, &b) in planes.iter().enumerate().skip(i + 1) {
            let ab = a.cross(b);
            if planes
                .iter()
                .skip(j + 1)
                .any(|&c| ab.dot(c).abs() > COPLANAR_ANGLE)
            {
                return true;
            }
        }
    }
    false
}

/// The caliper and axis extents of `corners`, as the planar measurement stores them.
///
/// Axis extents run over `x`/`z`, the caliper pair comes from the rotating-caliper sweep over
/// the `x`/`z` outline (the axis pair when the outline is degenerate), and the height is the
/// `y` extent. `None` without corners.
#[must_use]
pub fn outline_extents(corners: &[DVec3]) -> Option<OutlineExtents> {
    if corners.is_empty() {
        return None;
    }
    let lo = corners
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |m, &p| m.min(p));
    let hi = corners
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |m, &p| m.max(p));
    let (dx, dz) = (hi.x - lo.x, hi.z - lo.z);
    let (width_axis, length_axis) = if dx <= dz { (dx, dz) } else { (dz, dx) };
    let outline: Vec<(f64, f64)> = corners.iter().map(|c| (c.x, c.z)).collect();
    let (width_caliper, length_caliper) = caliper_frame(&outline)
        .map_or((width_axis, length_axis), |frame| {
            (frame.width, frame.length)
        });
    Some(OutlineExtents {
        width_caliper,
        length_caliper,
        width_axis,
        length_axis,
        height: hi.y - lo.y,
    })
}

/// The outline of a carved design: the convex hull of its carved mesh and its extents.
///
/// `None` when the vertices have no volume or are not finite, in which case the caller keeps
/// the flat stone's outline.
#[must_use]
pub fn design_outline(mesh_vertices: &[DVec3]) -> Option<DesignOutline> {
    let corners = convex_corners(mesh_vertices)?;
    let extents = outline_extents(&corners)?;
    Some(DesignOutline { corners, extents })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box's eight corners, `x` in `0..4`, `y` in `0..2`, `z` in `0..6`.
    fn box_corners() -> Vec<DVec3> {
        let mut out = Vec::new();
        for x in [0.0, 4.0] {
            for y in [0.0, 2.0] {
                for z in [0.0, 6.0] {
                    out.push(DVec3::new(x, y, z));
                }
            }
        }
        out
    }

    fn bits(points: &[DVec3]) -> BTreeSet<[u64; 3]> {
        points
            .iter()
            .map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])
            .collect()
    }

    #[test]
    fn a_convex_vertex_set_keeps_its_exact_corners_and_drops_interior_and_duplicate_points() {
        let corners = box_corners();
        let mut cloud = corners.clone();
        cloud.extend(corners.iter().copied());
        cloud.push(DVec3::new(2.0, 1.0, 3.0));
        cloud.push(DVec3::new(1.0, 2.0, 3.0));
        cloud.push(DVec3::new(0.7, 0.3, 5.1));
        let kept = convex_corners(&cloud).expect("a box has a hull");
        assert_eq!(kept.len(), 8);
        assert_eq!(
            bits(&kept),
            bits(&corners),
            "the corners are the input's, bit for bit"
        );
        let extents = outline_extents(&kept).expect("extents");
        assert!((extents.width_axis - 4.0).abs() < 1e-12);
        assert!((extents.length_axis - 6.0).abs() < 1e-12);
        assert!((extents.width_caliper - 4.0).abs() < 1e-12);
        assert!((extents.length_caliper - 6.0).abs() < 1e-12);
        assert!((extents.height - 2.0).abs() < 1e-12);
    }

    #[test]
    fn the_same_points_in_any_order_give_the_same_corners() {
        let mut cloud = box_corners();
        cloud.push(DVec3::new(2.0, 1.0, 3.0));
        let forward = convex_corners(&cloud).expect("hull");
        cloud.reverse();
        let backward = convex_corners(&cloud).expect("hull");
        assert_eq!(forward, backward);
        assert_eq!(forward, convex_corners(&cloud).expect("hull"));
    }

    #[test]
    fn a_flat_or_non_finite_vertex_set_has_no_outline() {
        let flat = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.0, 0.0, 1.0),
            DVec3::new(1.0, 0.0, 1.0),
        ];
        assert!(design_outline(&flat).is_none());
        assert!(design_outline(&[]).is_none());
        let mut bad = box_corners();
        bad.push(DVec3::new(f64::NAN, 0.0, 0.0));
        assert!(design_outline(&bad).is_none());
    }

    /// A pyramid (4 by 6 base, apex 3 high) and the same stone with its apex cut off flat at
    /// height 2.5 by a tool: the carved mesh has a small square where the apex was.
    fn pyramid(apex_cut: bool) -> Vec<DVec3> {
        let mut out = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(4.0, 0.0, 0.0),
            DVec3::new(0.0, 0.0, 6.0),
            DVec3::new(4.0, 0.0, 6.0),
        ];
        if apex_cut {
            for (x, z) in [(1.8, 2.8), (2.2, 2.8), (2.2, 3.2), (1.8, 3.2)] {
                out.push(DVec3::new(x, 2.5, z));
            }
        } else {
            out.push(DVec3::new(2.0, 3.0, 3.0));
        }
        out
    }

    #[test]
    fn a_tool_that_removes_the_vertex_setting_the_height_gives_a_smaller_extent() {
        let flat = design_outline(&pyramid(false)).expect("flat outline");
        let carved = design_outline(&pyramid(true)).expect("carved outline");
        assert_eq!(flat.corners.len(), 5);
        assert_eq!(carved.corners.len(), 8);
        assert!((flat.extents.height - 3.0).abs() < 1e-12);
        assert!((carved.extents.height - 2.5).abs() < 1e-12);
        assert!(carved.extents.height < flat.extents.height);
        // The base still sets the horizontal extents.
        assert!((carved.extents.width_axis - flat.extents.width_axis).abs() < 1e-12);
        assert!((carved.extents.length_caliper - flat.extents.length_caliper).abs() < 1e-12);
    }

    #[test]
    fn a_groove_inside_the_outline_changes_nothing() {
        // Extra vertices strictly inside the hull (the rim of a dimple on the top face and
        // its floor) leave the corners and the extents alone.
        let mut cloud = box_corners();
        cloud.extend([
            DVec3::new(1.8, 2.0, 2.8),
            DVec3::new(2.2, 2.0, 2.8),
            DVec3::new(2.0, 1.7, 3.0),
        ]);
        let with_dimple = design_outline(&cloud).expect("outline");
        let plain = design_outline(&box_corners()).expect("outline");
        assert_eq!(with_dimple.corners, plain.corners);
        assert_eq!(with_dimple.extents, plain.extents);
    }

    #[test]
    fn points_on_the_top_edges_a_rounding_step_below_the_corners_are_not_corners() {
        // What a carved mesh gives: the exact corners from the untouched side polygons, and
        // on the top edges the points where the tool's tessellation planes cut the top
        // polygon, snapped a few 1e-9 below the top plane, some very close to a corner. The
        // hull's triangles through a close pair are tilted by far more than the rounding,
        // which the corner test must not read as extra planes through the edge points.
        let mut cloud = box_corners();
        let y = 2.0 - 2e-9;
        for x in [
            0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 3.9, 3.98, 3.99, 3.995, 3.999,
        ] {
            cloud.push(DVec3::new(x, y, 0.0));
            cloud.push(DVec3::new(x, y, 6.0));
        }
        for z in [1.0, 2.0, 3.0, 4.0, 5.0] {
            cloud.push(DVec3::new(0.0, y, z));
            cloud.push(DVec3::new(4.0, y, z));
        }
        cloud.extend([
            DVec3::new(1.8, 2.0, 2.8),
            DVec3::new(2.2, 2.0, 2.8),
            DVec3::new(2.0, 1.7, 3.0),
        ]);
        let outline = design_outline(&cloud).expect("outline");
        assert_eq!(outline.corners.len(), 8, "{:?}", outline.corners);
        assert_eq!(bits(&outline.corners), bits(&box_corners()));
        assert_eq!(
            outline.extents,
            design_outline(&box_corners()).expect("outline").extents
        );
    }
}
