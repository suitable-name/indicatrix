//! Small closed shapes: a box (the calibration cube) and a sphere-like shell (a located
//! point).

use glam::DVec3;

use crate::rough_plan::shape::{MeshError, RoughMesh};

/// The 12 triangles of a box over the corners of [`box_points`], wound outward.
const BOX_TRIANGLES: [[u32; 3]; 12] = [
    [4, 5, 7],
    [4, 7, 6],
    [0, 2, 3],
    [0, 3, 1],
    [0, 1, 5],
    [0, 5, 4],
    [3, 2, 6],
    [3, 6, 7],
    [2, 0, 4],
    [2, 4, 6],
    [1, 3, 7],
    [1, 7, 5],
];

/// The 20 triangles of an icosahedron over the vertices built in [`sphere_shell`], wound outward.
const ICOSAHEDRON_TRIANGLES: [[u32; 3]; 20] = [
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

/// The 8 corners of the box with these half extents, centred at the origin. Corner `c` has
/// `x = +` when bit 0 of `c` is set, `y = +` for bit 1 and `z = +` for bit 2.
#[must_use]
pub fn box_points(half: DVec3) -> Vec<DVec3> {
    let side = |bit: u32, extent: f64| if bit == 0 { -extent } else { extent };
    (0..8_u32)
        .map(|c| {
            DVec3::new(
                side(c & 1, half.x),
                side((c >> 1) & 1, half.y),
                side((c >> 2) & 1, half.z),
            )
        })
        .collect()
}

/// A box with these half extents centred at the origin, as a [`RoughMesh`].
///
/// # Errors
///
/// [`MeshError`] when an extent is zero, negative or not finite.
pub fn box_mesh(half: DVec3) -> Result<RoughMesh, MeshError> {
    RoughMesh::new(&box_points(half), &BOX_TRIANGLES)
}

/// The corners and triangles of a closed shell around `centre` that contains a sphere.
///
/// The shell is an icosahedron whose faces touch the sphere of radius `radius_mm`. It over-covers the sphere
/// by a few percent in radius, never under-covers it, which is the safe side for an inclusion.
#[must_use]
pub fn sphere_shell(centre: DVec3, radius_mm: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let phi = f64::midpoint(1.0, 5.0_f64.sqrt());
    let raw = [
        DVec3::new(-1.0, phi, 0.0),
        DVec3::new(1.0, phi, 0.0),
        DVec3::new(-1.0, -phi, 0.0),
        DVec3::new(1.0, -phi, 0.0),
        DVec3::new(0.0, -1.0, phi),
        DVec3::new(0.0, 1.0, phi),
        DVec3::new(0.0, -1.0, -phi),
        DVec3::new(0.0, 1.0, -phi),
        DVec3::new(phi, 0.0, -1.0),
        DVec3::new(phi, 0.0, 1.0),
        DVec3::new(-phi, 0.0, -1.0),
        DVec3::new(-phi, 0.0, 1.0),
    ];
    let inradius_over_circumradius = ((1.0 + 2.0 / 5.0_f64.sqrt()) / 3.0).sqrt();
    let circumradius = radius_mm / inradius_over_circumradius;
    let points = raw
        .iter()
        .map(|v| centre + v.normalize() * circumradius)
        .collect();
    (points, ICOSAHEDRON_TRIANGLES.to_vec())
}
