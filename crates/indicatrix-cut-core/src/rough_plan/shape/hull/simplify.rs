//! A simplified OUTER outline for a convex hull with too many planes (a smooth scan).
//!
//! Every plane of the result is a supporting plane of the points (`n . p <= d` for all of
//! them, with equality for the extreme point), so the polytope contains every point and is
//! never smaller than the true hull. The planner then plans in a grid that is a little
//! larger than the rough, and the mesh, kept beside the outline, decides what is material.
//!
//! The construction is normal clustering of the exact hull's triangles, deterministic:
//!
//! 1. The six axis-aligned supporting planes come first. They bound the polytope whatever
//!    the clusters do and make its bounding box exactly the points' box, so the frame of a
//!    mesh imported through it is the mesh's own frame (a saved plan reloads in it).
//! 2. The triangles are sorted by area, largest first, ties to the lowest index.
//! 3. For a growing angle theta (starting at 8 degrees) each triangle joins the first
//!    cluster, in creation order, whose seed normal is within theta of its own, else it
//!    founds a new one. The first theta that leaves room for at most `max_planes` planes
//!    wins. The seeds end up more than theta apart, so on a sphere the cluster count is
//!    about `1.09 / (1 - cos(theta / 2))` (random sequential packing of caps of radius
//!    theta / 2): the budget [`OUTLINE_PLANES`](super::OUTLINE_PLANES) of 64 planes leaves 58
//!    clusters, which is a theta of about 22 degrees, and the outline then stands a few
//!    percent of the radius off the surface at the worst. That is fine: it only bounds the
//!    grid, and the mesh decides where material is.
//! 4. A cluster's normal is its area-weighted normal, normalised, and its offset is the
//!    maximum of `n . p` over ALL the points.

use glam::DVec3;

use super::Face;

/// The smallest clustering angle, in degrees.
///
/// With a budget of a few dozen clusters every smaller angle only aborts (at the first
/// cluster over the budget), so the search starts where an outline of that size can first
/// succeed on a sphere (about 22 degrees for 58 clusters).
const START_DEGREES: f64 = 8.0;
/// The factor the angle grows by while there are too many clusters.
const GROWTH: f64 = 1.12;
/// The largest clustering angle, in degrees.
const END_DEGREES: f64 = 90.0;

/// At most `max_planes` supporting planes `n . p <= d` of `points` that bound them, the
/// outer outline of the convex hull whose triangles are `faces` (built with tolerance `eps`
/// from these very points).
///
/// The result depends only on the arguments: no hashed collection, sorts by total order
/// with the index as the last key.
pub(in crate::rough_plan) fn simplified_outer_planes(
    points: &[DVec3],
    faces: &[Face],
    max_planes: usize,
    eps: f64,
) -> Vec<(DVec3, f64)> {
    let mut planes = axis_planes(points);
    let budget = max_planes.saturating_sub(planes.len());
    let weights = face_weights(points, faces);
    let order = area_order(&weights);
    let mut degrees = START_DEGREES;
    let mut clusters = loop {
        if let Some(found) = cluster(faces, &weights, &order, degrees.to_radians().cos(), budget) {
            break found;
        }
        if degrees >= END_DEGREES {
            // Cannot happen for a sane budget (at 90 degrees four seeds remain at most), but
            // the planes stay supporting whatever is dropped, so truncating is sound.
            break cluster(faces, &weights, &order, 0.0, usize::MAX).unwrap_or_default();
        }
        degrees = (degrees * GROWTH).min(END_DEGREES);
    };
    clusters.truncate(budget);
    for sum in clusters {
        let n = sum.normalize_or_zero();
        if n == DVec3::ZERO {
            continue;
        }
        let d = support(points, n);
        let known = planes
            .iter()
            .any(|&(m, e)| m.dot(n) > 1.0 - 1e-9 && (e - d).abs() <= 10.0 * eps);
        if !known {
            planes.push((n, d));
        }
    }
    planes
}

/// The six axis-aligned planes touching the points' bounding box.
fn axis_planes(points: &[DVec3]) -> Vec<(DVec3, f64)> {
    [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ]
    .into_iter()
    .map(|n| (n, support(points, n)))
    .collect()
}

/// The support value of `points` along unit `n`: the largest `n . p`.
fn support(points: &[DVec3], n: DVec3) -> f64 {
    points
        .iter()
        .fold(f64::NEG_INFINITY, |best, &p| best.max(n.dot(p)))
}

/// The outward normal and the area of every triangle.
fn face_weights(points: &[DVec3], faces: &[Face]) -> Vec<(DVec3, f64)> {
    faces
        .iter()
        .map(|face| {
            let [a, b, c] = face.v.map(|i| points[i]);
            (face.n, 0.5 * (b - a).cross(c - a).length())
        })
        .collect()
}

/// Triangle indices, largest area first, equal areas by index.
fn area_order(weights: &[(DVec3, f64)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| {
        weights[b]
            .1
            .total_cmp(&weights[a].1)
            .then_with(|| a.cmp(&b))
    });
    order
}

/// One cluster: the normal of its seed and the sum of the area-weighted normals in it.
struct Cluster {
    seed: DVec3,
    sum: DVec3,
}

/// The area-weighted normal sums of the clusters at angle `cos_theta`, or `None` as soon as
/// there would be more than `budget` of them.
fn cluster(
    faces: &[Face],
    weights: &[(DVec3, f64)],
    order: &[usize],
    cos_theta: f64,
    budget: usize,
) -> Option<Vec<DVec3>> {
    let mut clusters: Vec<Cluster> = Vec::new();
    for &i in order {
        let (n, area) = weights[i];
        if faces[i].n == DVec3::ZERO {
            continue;
        }
        if let Some(found) = clusters.iter_mut().find(|c| c.seed.dot(n) >= cos_theta) {
            found.sum += n * area;
        } else {
            if clusters.len() >= budget {
                return None;
            }
            clusters.push(Cluster {
                seed: n,
                sum: n * area,
            });
        }
    }
    Some(clusters.into_iter().map(|c| c.sum).collect())
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;
    use crate::rough_plan::{
        RoughBase,
        shape::{
            hull::{OUTLINE_PLANES, hull_triangles, import_mesh, mesh, outline_note},
            mesh_fixture::{icosphere, pebble_scan},
        },
    };

    /// Compares two planes for a deterministic order.
    fn plane_order(a: &(DVec3, f64), b: &(DVec3, f64)) -> Ordering {
        a.1.total_cmp(&b.1)
            .then(a.0.x.total_cmp(&b.0.x))
            .then(a.0.y.total_cmp(&b.0.y))
            .then(a.0.z.total_cmp(&b.0.z))
    }

    /// The outline of `points` through the same path an import takes.
    fn outer(points: &[DVec3]) -> Vec<(DVec3, f64)> {
        let (faces, eps) = hull_triangles(points).expect("a hull");
        simplified_outer_planes(points, &faces, OUTLINE_PLANES, eps)
    }

    #[test]
    fn the_outline_holds_every_point_and_has_at_most_64_planes() {
        for (points, _) in [icosphere(3, 10.0), pebble_scan(4, 7)] {
            let planes = outer(&points);
            assert!(planes.len() <= OUTLINE_PLANES, "{} planes", planes.len());
            assert!(planes.len() >= 6);
            let scale = points.iter().map(|p| p.length()).fold(0.0, f64::max);
            for &(n, d) in &planes {
                assert!((n.length() - 1.0).abs() < 1e-12);
                for p in &points {
                    assert!(
                        n.dot(*p) <= 1e-9_f64.mul_add(scale, d),
                        "a point outside a plane"
                    );
                }
                // Supporting: some point lies on the plane.
                assert!(points.iter().any(|p| (n.dot(*p) - d).abs() < 1e-9 * scale));
            }
        }
    }

    #[test]
    fn the_outline_is_deterministic() {
        let (points, _) = pebble_scan(4, 3);
        let (mut a, mut b) = (outer(&points), outer(&points));
        assert_eq!(a, b);
        a.sort_by(plane_order);
        b.sort_by(plane_order);
        assert_eq!(a, b);
    }

    #[test]
    fn the_outline_box_is_the_points_box() {
        let (points, _) = pebble_scan(3, 11);
        let planes = outer(&points);
        let (lo, hi) = points.iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        let (metrics_corners, corners) =
            indicatrix::geometry::stone_metrics::measure_solid_with_vertices(&planes)
                .expect("bounded");
        assert!(metrics_corners.volume > 0.0);
        let clo = corners
            .iter()
            .fold(DVec3::splat(f64::INFINITY), |m, &v| m.min(v));
        let chi = corners
            .iter()
            .fold(DVec3::splat(f64::NEG_INFINITY), |m, &v| m.max(v));
        assert!((clo - lo).abs().max_element() < 1e-9, "{clo:?} vs {lo:?}");
        assert!((chi - hi).abs().max_element() < 1e-9, "{chi:?} vs {hi:?}");
    }

    #[test]
    fn smooth_scans_import_with_their_mesh_and_a_small_outline() {
        for (levels, triangles) in [(3, 1280), (4, 5120)] {
            let (points, tris) = icosphere(levels, 10.0);
            assert_eq!(tris.len(), triangles);
            let (base, note) = import_mesh(&points, &tris).expect("a smooth scan imports");
            assert_eq!(note, None, "a reload must not get a note");
            let RoughBase::Hull { id, .. } = base else {
                panic!("not a hull");
            };
            let kept = mesh(id).expect("the mesh is kept, though the sphere is convex");
            assert_eq!(kept.triangles().len(), triangles);
            let planes = base.to_halfspaces(true).expect("planes");
            assert!(planes.len() <= OUTLINE_PLANES, "{} planes", planes.len());
            let said = outline_note(id).expect("a simplified outline is announced");
            assert!(said.contains(&triangles.to_string()), "{said}");
            // The mesh volume, not the outline's, is the rough's.
            let model = crate::rough_plan::RoughModel::new(base, Vec::new());
            let volume = model.measure().expect("measures").volume_mm3;
            let sphere = 4.0 / 3.0 * std::f64::consts::PI * 1000.0;
            assert!(volume < sphere && volume > 0.9 * sphere, "{volume}");
            assert!((volume - kept.volume()).abs() < 1e-9 * volume);
        }
    }

    #[test]
    fn importing_a_smooth_scan_twice_gives_the_same_base_and_its_reload_the_same_shape() {
        let (points, tris) = pebble_scan(4, 5);
        let (first, _) = import_mesh(&points, &tris).expect("imports");
        let (second, _) = import_mesh(&points, &tris).expect("imports");
        assert_eq!(first, second);
        let RoughBase::Hull { id, .. } = first else {
            panic!("not a hull");
        };
        // What a saved plan stores: the mesh as the rough keeps it.
        let kept = mesh(id).expect("the mesh");
        let (again, note) = import_mesh(kept.vertices(), kept.triangles()).expect("reloads");
        assert_eq!(note, None);
        let RoughBase::Hull { id: again_id, .. } = again else {
            panic!("not a hull");
        };
        let reloaded = mesh(again_id).expect("a mesh plan reloads as one");
        assert_eq!(reloaded.triangles().len(), kept.triangles().len());
        assert!((reloaded.volume() - kept.volume()).abs() < 1e-9 * kept.volume());
        for (a, b) in first
            .bounding_box_extents()
            .into_iter()
            .zip(again.bounding_box_extents())
        {
            assert!((a - b).abs() < 1e-9, "{a} vs {b}");
        }
    }

    #[test]
    fn an_unusable_smooth_scan_is_refused_not_given_an_outline_alone() {
        use crate::rough_plan::shape::{HullError, MeshError};
        let (points, mut tris) = icosphere(3, 10.0);
        // A big hole: the cap above z = 4 is gone. (One missing triangle would be filled by
        // the mesh repair.)
        tris.retain(|tri| {
            let z = tri.iter().map(|&v| points[v as usize].z).sum::<f64>() / 3.0;
            z <= 4.0
        });
        assert!(matches!(
            import_mesh(&points, &tris),
            Err(HullError::ScanUnusable {
                reason: MeshError::Open,
                ..
            })
        ));
        // A cloud of points with no faces cannot keep a mesh either.
        assert!(matches!(
            import_mesh(&points, &[]),
            Err(HullError::TooComplex(n)) if n > 400
        ));
    }

    #[test]
    fn every_mesh_vertex_is_inside_the_outline_in_the_rough_frame() {
        let (points, tris) = pebble_scan(4, 9);
        let (base, _) = import_mesh(&points, &tris).expect("imports");
        let RoughBase::Hull { id, .. } = base else {
            panic!("not a hull");
        };
        let kept = mesh(id).expect("the mesh");
        let planes = base.to_halfspaces(true).expect("planes");
        assert!(planes.len() <= OUTLINE_PLANES, "{} planes", planes.len());
        // Every point of the mesh's own surface lies in the outline, in the rough frame.
        for &v in kept.vertices() {
            for &(n, d) in &planes {
                assert!(n.dot(v) <= d + 1e-7, "a vertex outside the outline");
            }
        }
        // And the centre is inside the mesh.
        let (lo, hi) = kept.bounds();
        assert!(kept.contains_point((lo + hi) * 0.5));
    }

    #[test]
    fn a_coarse_outline_is_never_inside_the_mesh() {
        let (points, tris) = pebble_scan(4, 7);
        let (base, _) = import_mesh(&points, &tris).expect("imports");
        let RoughBase::Hull { id, .. } = base else {
            panic!("not a hull");
        };
        let kept = mesh(id).expect("the mesh");
        let planes = base.to_halfspaces(true).expect("planes");
        // Six axis planes and some clusters, never more than the budget.
        assert!(planes.len() > 6, "{} planes", planes.len());
        assert!(planes.len() <= OUTLINE_PLANES, "{} planes", planes.len());
        let scale = kept
            .vertices()
            .iter()
            .map(|v| v.length())
            .fold(0.0, f64::max);
        for &v in kept.vertices() {
            for &(n, d) in &planes {
                assert!(
                    n.dot(v) <= 1e-9_f64.mul_add(scale, d),
                    "a mesh vertex outside a coarse outline plane"
                );
            }
        }
    }

    #[test]
    fn importing_a_scan_builds_the_outline_from_the_welded_vertices() {
        let (points, tris) = pebble_scan(4, 5);
        let (plain, _) = import_mesh(&points, &tris).expect("imports");
        // A vertex no triangle uses, far from the scan: the mesh drops it, so the outline,
        // the frame and the id are those of the scan alone.
        let mut with_stray = points;
        with_stray.push(DVec3::new(50.0, -40.0, 60.0));
        let (strayed, note) = import_mesh(&with_stray, &tris).expect("imports");
        assert_eq!(note, None);
        assert_eq!(plain, strayed);
    }
}
