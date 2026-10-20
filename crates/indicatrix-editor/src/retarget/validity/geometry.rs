use super::{BTreeMap, DVec3, SolidMesh};

/// The area of a planar polygon ring.
#[must_use]
pub fn polygon_area(ring: &[DVec3]) -> f64 {
    let Some(&origin) = ring.first() else {
        return 0.0;
    };
    let mut sum = DVec3::ZERO;
    for pair in ring.windows(2).skip(1) {
        sum += (pair[0] - origin).cross(pair[1] - origin);
    }
    0.5 * sum.length()
}

pub(super) fn rings_by_plane(mesh: &SolidMesh) -> BTreeMap<usize, &Vec<DVec3>> {
    mesh.rings
        .iter()
        .filter(|(_, ring)| ring.len() >= 3)
        .map(|(plane, ring)| (*plane, ring))
        .collect()
}

pub(super) fn mean_height(ring: &[DVec3]) -> f64 {
    ring.iter().map(|v| v.y).sum::<f64>() / ring.len().max(1) as f64
}
