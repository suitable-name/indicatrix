use glam::{DVec3, Vec3};

/// `true` when a tier at `angle_deg` sits on the crown side (its facet normal
/// points up), `false` for the pavilion side.
///
/// The one side rule every plane builder shares
/// (`StandardGemCuts::from_asc_schedule`, the meet solver's `tier_sides`, and
/// `indicatrix-solid`'s facet map).
///
/// Decided from the angle's sign ALONE: a positive angle is crown, a negative one
/// pavilion, and a zero angle is crown (the table) unless it is a sign-negative
/// zero (the culet). The `GemCAD` manual states the culet as angle `0` with a
/// negative distance ("This number will be positive unless the facet is a culet
/// (0° pavilion) facet", Windows manual p.21);
/// `indicatrix_formats::asc::parse_asc` turns that into a sign-negative zero
/// angle, and the editor authors culets as `-0.0`.
///
/// This replaced a file-order rule ("an unsigned zero inherits the previous tier's
/// side"), which the real catalogue (5,759 files, measured 2026-09-28) shows to be
/// wrong both ways: of the 148 zero-angle tiers with a negative (culet) distance, 8
/// follow a crown tier or open the file and were read as a second table; and of
/// the 5,153 positive-distance tables, 8 directly follow a pavilion tier (`GemCAD`'s
/// table-first sort order) and were read as culets.
#[must_use]
pub const fn tier_is_crown_side(angle_deg: f64) -> bool {
    angle_deg > 0.0 || (angle_deg == 0.0 && !angle_deg.is_sign_negative())
}

/// A facet plane in the `n . x + d <= 0` (origin-interior) half-space convention,
/// `#[repr(C)]` so it can be uploaded to the GPU verbatim.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GpuFacetPlane {
    /// Outward unit normal `n` of the facet (unit length is required by every
    /// consumer; [`GpuFacetPlane::new`] normalises it).
    pub normal: [f32; 3],
    /// Offset `d` in `n . x + d <= 0`: the negated distance of the plane from the
    /// origin along `normal`, negative when the origin is inside.
    pub d: f32,
}

impl GpuFacetPlane {
    /// Builds a plane from a (not-necessarily-normalized) normal and offset,
    /// normalizing `normal` first.
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
    /// This crate has two conventions for the same plane: the GPU/tracer and
    /// `GemPolyhedron::from_planes` inputs use `n . x + d <= 0` with `d < 0`
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

    /// The side comes from the sign alone, including the sign of a zero.
    #[test]
    fn tier_is_crown_side_reads_the_sign_of_zero() {
        assert!(tier_is_crown_side(41.0));
        assert!(tier_is_crown_side(90.0));
        assert!(tier_is_crown_side(0.0), "a positive zero is the table");
        assert!(
            !tier_is_crown_side(-0.0),
            "a sign-negative zero is the culet"
        );
        assert!(!tier_is_crown_side(-41.0));
        assert!(!tier_is_crown_side(-90.0));
    }

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
