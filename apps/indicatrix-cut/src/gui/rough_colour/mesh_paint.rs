//! Painting the polished windows on the 3D mesh of the Rough Planner's rough view (`zoning`
//! feature only).
//!
//! The wizard's Surfaces step paints on the photo (a brush back-projected through each pixel's
//! first mesh hit). This module is the same brush on the 3D view: a click in the view becomes a
//! ray in the rough frame (`gui::rough_plan::zoning_hooks::view_ray`), the first hit of that ray
//! on the mesh is a triangle, and the brush paints the triangles around the hit point that face
//! the same way. The painted triangles go into the wizard's one set of window triangles
//! (`State::windows`, which `fitwork::surface_map` turns into the `SurfaceMap`), so the photo
//! brush and the mesh brush edit the same set.
//!
//! Window-free: rays, meshes and triangle numbers only.

use glam::DVec3;
use indicatrix_cut_core::rough_plan::shape::RoughMesh;

/// A brush this share of the mesh's extent wide is what a brush radius of one working pixel
/// paints, so the default 4 px brush covers about 2 % of the rough.
const EXTENT_PER_BRUSH_PIXEL: f64 = 0.005;

/// The triangles of a brush must face the surface it landed on to within this cosine.
const MIN_FACING: f64 = 0.3;

/// A ray in the rough's frame (the frame of the mesh), mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshRay {
    /// Where the ray starts.
    pub origin: DVec3,
    /// Its direction (not necessarily a unit vector).
    pub dir: DVec3,
}

/// The brush radius in millimetres for the wizard's brush radius (working pixels) on a mesh
/// `extent_mm` across. At least a tenth of a millimetre, so a tiny rough still paints.
#[must_use]
pub fn brush_radius_mm(radius_px: f64, extent_mm: f64) -> f64 {
    let radius = if radius_px.is_finite() {
        radius_px.max(0.5)
    } else {
        4.0
    };
    let extent = if extent_mm.is_finite() && extent_mm > 0.0 {
        extent_mm
    } else {
        10.0
    };
    (radius * extent * EXTENT_PER_BRUSH_PIXEL).max(0.1)
}

/// The triangles a brush of `radius_mm` paints for a click along `ray`.
///
/// The triangle the ray hits first, and every triangle whose centre lies within the radius of
/// the hit point, close to the hit triangle's plane and facing the same way (so the brush does
/// not reach through a thin stone to its back). Ascending triangle numbers; empty when the ray
/// misses.
#[must_use]
pub fn triangles_under_brush(mesh: &RoughMesh, ray: &MeshRay, radius_mm: f64) -> Vec<u32> {
    if !(ray.dir.is_finite() && ray.origin.is_finite() && ray.dir.length() > 0.0) {
        return Vec::new();
    }
    let Some(hit) = mesh.first_hit(ray.origin, ray.dir, 0.0) else {
        return Vec::new();
    };
    let point = ray.origin + ray.dir * hit.t;
    let radius = radius_mm.max(0.0);
    let vertices = mesh.vertices();
    let mut out = vec![hit.triangle];
    for (index, tri) in mesh.triangles().iter().enumerate() {
        let index = index as u32;
        if index == hit.triangle {
            continue;
        }
        let [a, b, c] = tri.map(|i| vertices[i as usize]);
        let centre = (a + b + c) / 3.0;
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if (centre - point).length() <= radius
            && normal.dot(hit.normal) >= MIN_FACING
            && hit.normal.dot(centre - point).abs() <= 0.5 * radius
        {
            out.push(index);
        }
    }
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_colour::wizard::fitwork::{SurfaceChoice, surface_map};
    use indicatrix_cut_core::rough_plan::colour_fit::forward::SurfaceClass;
    use std::collections::BTreeSet;

    /// An axis-aligned cube of edge 10 mm centred on the origin, two triangles per face, wound
    /// outward.
    fn cube() -> RoughMesh {
        let h = 5.0;
        let points: Vec<DVec3> = (0..8)
            .map(|i| {
                DVec3::new(
                    if i & 1 == 0 { -h } else { h },
                    if i & 2 == 0 { -h } else { h },
                    if i & 4 == 0 { -h } else { h },
                )
            })
            .collect();
        let quads: [[u32; 4]; 6] = [
            [1, 3, 7, 5], // +x
            [0, 4, 6, 2], // -x
            [2, 6, 7, 3], // +y
            [0, 1, 5, 4], // -y
            [4, 5, 7, 6], // +z
            [0, 2, 3, 1], // -z
        ];
        let tris: Vec<[u32; 3]> = quads
            .iter()
            .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
            .collect();
        RoughMesh::new(&points, &tris).expect("a closed cube")
    }

    /// The triangles of the +z face, found by their planes (the mesh may number them its own way).
    fn top_face(mesh: &RoughMesh) -> Vec<u32> {
        (0..mesh.triangles().len() as u32)
            .filter(|&t| mesh.triangle_plane(t).0.z > 0.9)
            .collect()
    }

    fn ray_at_plus_z(x: f64, y: f64) -> MeshRay {
        MeshRay {
            origin: DVec3::new(x, y, 20.0),
            dir: DVec3::NEG_Z,
        }
    }

    #[test]
    fn a_ray_that_misses_paints_nothing() {
        let mesh = cube();
        assert_eq!(
            triangles_under_brush(&mesh, &ray_at_plus_z(50.0, 0.0), 2.0),
            [] as [u32; 0]
        );
        let degenerate = MeshRay {
            origin: DVec3::ZERO,
            dir: DVec3::ZERO,
        };
        assert_eq!(
            triangles_under_brush(&mesh, &degenerate, 2.0),
            [] as [u32; 0]
        );
    }

    #[test]
    fn a_small_brush_paints_the_hit_triangle_and_a_big_one_the_whole_face_but_no_other() {
        let mesh = cube();
        let top = top_face(&mesh);
        assert_eq!(top.len(), 2);
        let small = triangles_under_brush(&mesh, &ray_at_plus_z(1.0, -1.0), 0.01);
        assert_eq!(small.len(), 1);
        assert!(top.contains(&small[0]), "hit {small:?}");
        let face = triangles_under_brush(&mesh, &ray_at_plus_z(1.0, -1.0), 20.0);
        assert_eq!(face, top, "both +z triangles, nothing of the other faces");
    }

    #[test]
    fn a_brush_does_not_reach_the_faces_around_the_corner() {
        let mesh = cube();
        // A click on the +z face next to the +x edge, with a brush wider than the distance to
        // the +x face: the +x triangles face another way and stay unpainted.
        let near_edge = triangles_under_brush(&mesh, &ray_at_plus_z(4.5, 0.0), 6.0);
        let top = top_face(&mesh);
        assert!(near_edge.iter().all(|t| top.contains(t)), "{near_edge:?}");
    }

    #[test]
    fn the_brush_radius_follows_the_extent_and_has_a_floor() {
        assert!((brush_radius_mm(4.0, 100.0) - 2.0).abs() < 1e-12);
        assert!((brush_radius_mm(8.0, 100.0) - 4.0).abs() < 1e-12);
        assert!((brush_radius_mm(4.0, 1.0) - 0.1).abs() < 1e-12, "floor");
        assert!(brush_radius_mm(f64::NAN, f64::NAN) > 0.0);
    }

    #[test]
    fn mesh_painted_triangles_become_polished_windows_in_the_surface_map() {
        let mesh = cube();
        // The wizard keeps the painted triangles in one ordered set; a mesh click adds to it
        // exactly like a photo click does (`State::paint_triangles`).
        let mut windows: BTreeSet<u32> = BTreeSet::new();
        let painted = triangles_under_brush(&mesh, &ray_at_plus_z(0.0, 0.0), 20.0);
        windows.extend(painted.iter().copied());
        let list: Vec<u32> = windows.iter().copied().collect();
        let map = surface_map(SurfaceChoice::Frosted(0.2), None, &list);
        let frosted = SurfaceClass::Frosted { roughness: 0.2 };
        for t in 0..mesh.triangles().len() as u32 {
            let expected = if painted.contains(&t) {
                SurfaceClass::Polished
            } else {
                frosted
            };
            assert_eq!(map.class_of(t), expected, "triangle {t}");
        }
        assert_eq!(painted, top_face(&mesh));
        // Erasing takes the override away again.
        for t in &painted {
            windows.remove(t);
        }
        let list: Vec<u32> = windows.iter().copied().collect();
        let map = surface_map(SurfaceChoice::Frosted(0.2), None, &list);
        assert_eq!(map.class_of(painted[0]), frosted);
    }
}
