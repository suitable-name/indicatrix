use glam::{DVec3, Vec3};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GpuFacetPlane {
    pub normal: [f32; 3],
    pub d: f32,
}

impl GpuFacetPlane {
    #[must_use]
    pub fn new(normal: Vec3, d: f32) -> Self {
        let n = normal.normalize();
        Self {
            normal: [n.x, n.y, n.z],
            d,
        }
    }

    /// Converts to the `n . x <= m` half-space convention used by
    /// [`super::stone_metrics::measure_solid`] and
    /// [`super::stone_metrics::build_solid_mesh`], widening to `f64`.
    ///
    /// This crate has two conventions for the same plane: dual-space convex
    /// hull (`GemPolyhedron::from_planes`) uses `n . x + d <= 0` with `d < 0`
    /// (origin strictly interior); the SIMD feasibility scan uses `n . x <=
    /// m` with `m` the positive offset along the outward normal, i.e.
    /// `m = -d`. This is the sign flip between them, so callers can hand the
    /// same planes to either path.
    #[must_use]
    pub fn to_halfspace_f64(self) -> (DVec3, f64) {
        (
            DVec3::new(
                f64::from(self.normal[0]),
                f64::from(self.normal[1]),
                f64::from(self.normal[2]),
            ),
            -f64::from(self.d),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks in that `to_halfspace_f64` inverts `from_asc_schedule`'s
    /// `d = -mast.abs()` convention (expect `m = mast`). A wrong sign would
    /// silently mirror the solid through the origin -- vertices and faces
    /// would still all close, so only comparing against the known mast value
    /// catches it.
    #[test]
    fn to_halfspace_f64_inverts_the_gpu_facet_plane_sign_convention() {
        let mast = 0.6_f32;
        let plane = GpuFacetPlane::new(Vec3::Y, -mast);
        let (n, m) = plane.to_halfspace_f64();
        assert!((n - DVec3::Y).length() < 1e-12);
        assert!((m - f64::from(mast)).abs() < 1e-12);
    }
}
