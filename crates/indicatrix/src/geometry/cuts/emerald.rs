//! The Emerald Cut reference solid, computed from one shared profile so that
//! tiers meant to meet at an exact point do so to full `f32` precision.

use glam::Vec3;

use super::StandardGemCuts;
use crate::geometry::plane::GpuFacetPlane;

impl StandardGemCuts {
    /// Generates exact 3D half-space planes for an Emerald Cut Gemstone.
    ///
    /// Every tier offset (`d`) below is *computed*, not pasted as a rounded literal, from
    /// one small shared profile: the girdle band half-height, the crown/pavilion crease
    /// ring heights, and the girdle radii. Two tiers that are meant to meet at an exact
    /// point (e.g. a girdle-adjacent tier and its neighbor on the crease ring, or all the
    /// facets converging on a girdle corner) are placed through that *same computed point*
    /// to full `f32` precision, rather than each being rounded to four decimals
    /// independently. That distinction matters here: `GemPolyhedron::from_planes`
    /// reconstructs vertices as 3-plane meets and welds ones closer than
    /// `VERTEX_WELD_EPS` (1e-4); rounding this profile by hand instead can leave several
    /// such intended-coincident points ~1.5e-4 apart -- just outside the weld radius --
    /// which produces a dozen extra sliver vertices (60 instead of the true 48) even
    /// though every plane still contributes a facet and the volume is already correct.
    /// Deriving from the profile in code makes the coincidence exact by construction
    /// instead of by luck.
    #[must_use]
    pub fn emerald_cut() -> Vec<GpuFacetPlane> {
        // -- Shared profile -------------------------------------------------------------
        const TABLE_Y: f32 = 0.30; // crown table height
        const GIRDLE_HALF_BAND: f32 = 0.03; // girdle band spans y = -GIRDLE_HALF_BAND ..= +GIRDLE_HALF_BAND
        const CROWN_CREASE_Y: f32 = 0.15; // crown tiers meet each other on this ring
        const PAVILION_CREASE_MAG: f32 = 0.35; // pavilion tiers meet each other at y = -PAVILION_CREASE_MAG
        const GIRDLE_R_Z: f32 = 0.80;
        const GIRDLE_R_X: f32 = 1.10;
        const GIRDLE_R_DIAG: f32 = 1.00;
        // Pavilion keel truncation: just above the natural keel line where the two
        // keel-adjacent tiers (see `tier_d_at_crease` below, at PAVILION_CREASE_MAG) would
        // otherwise meet each other at x = z = 0 (that natural line sits at y ~= -0.8712),
        // so this plane slices through them just short of that point and leaves a real
        // keel flat instead of an untouched plane below the solid's deepest point.
        const PAVILION_KEEL_Y: f32 = -0.86;

        let diag = std::f32::consts::FRAC_1_SQRT_2;
        let mut planes = Vec::new();

        // Crown Table
        planes.push(GpuFacetPlane::new(Vec3::new(0.0, 1.0, 0.0), -TABLE_Y));

        // Crown tiers: the steeper (45 deg) tier is girdle-adjacent; the shallower
        // (35 deg) tier meets it exactly on the crown crease ring.
        let a1 = 35.0f32.to_radians(); // shallow, crease-ring-adjacent (Crown Step 1)
        let a2 = 45.0f32.to_radians(); // steep, girdle-adjacent (Crown Step 2)
        push_crown_tiers(
            &mut planes,
            a1,
            a2,
            GIRDLE_R_Z,
            GIRDLE_R_X,
            GIRDLE_HALF_BAND,
            CROWN_CREASE_Y,
        );

        // 4 Corner Crown Facets (40.0°), through the girdle's diagonal corner.
        let ac = 40.0f32.to_radians();
        push_corner_facets(&mut planes, ac, GIRDLE_R_DIAG, GIRDLE_HALF_BAND, diag, 1.0);

        // 8 Girdle Facets (90.0°)
        push_girdle_facets(&mut planes, GIRDLE_R_Z, GIRDLE_R_X, GIRDLE_R_DIAG, diag);

        // Pavilion tiers: the 53 deg tier is girdle-adjacent; the 43 deg tier meets it
        // exactly on the pavilion crease ring.
        //
        // NOTE: the tier angles are intentionally swapped from a naive reading of "step 1
        // then step 2" -- the shallower/more-horizontal 53 degree tier belongs immediately
        // below the girdle (its crease lands on the girdle edge), while the steeper/more-
        // vertical 43 degree tier belongs next to the keel. Assigning 43 degrees to the
        // girdle-adjacent tier instead would leave that tier's crease
        // strictly outside the girdle radius, so it would never touch the hull.
        let p1 = 53.0f32.to_radians(); // girdle-adjacent (Pavilion Step 1)
        let p2 = 43.0f32.to_radians(); // keel-adjacent, crease-ring-adjacent (Pavilion Step 2)
        push_pavilion_tiers(
            &mut planes,
            p1,
            p2,
            GIRDLE_R_Z,
            GIRDLE_R_X,
            GIRDLE_HALF_BAND,
            PAVILION_CREASE_MAG,
        );

        // 4 Corner Pavilion Facets (-48.0°), through the girdle's diagonal corner.
        let pc = 48.0f32.to_radians();
        push_corner_facets(&mut planes, pc, GIRDLE_R_DIAG, GIRDLE_HALF_BAND, diag, -1.0);

        // Keel line base -- just above the natural keel line formed by the pavilion
        // tiers, so it truncates them into a real keel flat instead of sitting below the
        // solid's deepest point (which would leave it untouched).
        planes.push(GpuFacetPlane::new(
            Vec3::new(0.0, -1.0, 0.0),
            PAVILION_KEEL_Y,
        ));

        planes
    }
}

/// `d` for a tier plane with normal `(0, +-angle.cos(), +-angle.sin())` that passes
/// through the girdle edge at radius `r`, `half_band` above (crown) or below
/// (pavilion) the girdle band -- i.e. whichever tier is girdle-adjacent (crown: the
/// steeper 45 deg tier; pavilion: the 53 deg tier). Shared by [`push_crown_tiers`]
/// and [`push_pavilion_tiers`].
fn tier_d_at_girdle(angle: f32, r: f32, half_band: f32) -> f32 {
    -angle.sin().mul_add(r, angle.cos() * half_band)
}

/// `d` for the tier that is NOT girdle-adjacent: it instead meets the girdle-
/// adjacent tier exactly on the crease ring `y = +-crease_mag`. Solves for the point
/// where the `girdle_angle` tier's own plane crosses that ring, then places this
/// tier's plane through that same point, so the two tiers share an exact edge.
fn tier_d_at_crease(
    girdle_angle: f32,
    crease_angle: f32,
    r: f32,
    crease_mag: f32,
    half_band: f32,
) -> f32 {
    let z_c = r - girdle_angle.cos() * (crease_mag - half_band) / girdle_angle.sin();
    -crease_angle
        .sin()
        .mul_add(z_c, crease_angle.cos() * crease_mag)
}

/// `d` for a diagonal corner facet through the girdle's diagonal corner point
/// `(r_diag, +-half_band)`. Works for both crown (+y normal) and pavilion (-y
/// normal) corners: the sign flip in the normal's y-component and the sign flip in
/// the girdle band's y-coordinate cancel out.
fn corner_d(angle: f32, r_diag: f32, half_band: f32) -> f32 {
    -angle.cos().mul_add(half_band, angle.sin() * r_diag)
}

/// Pushes the 8 crown tier facets: Crown Step 1 (`a1`, shallow, meets the crease
/// ring) and Crown Step 2 (`a2`, steep, girdle-adjacent). See [`tier_d_at_crease`]
/// and [`tier_d_at_girdle`] for how the two tiers are made to share an exact edge.
fn push_crown_tiers(
    planes: &mut Vec<GpuFacetPlane>,
    a1: f32,
    a2: f32,
    r_z: f32,
    r_x: f32,
    half_band: f32,
    crease_y: f32,
) {
    let d_a1_z = tier_d_at_crease(a2, a1, r_z, crease_y, half_band);
    let d_a1_x = tier_d_at_crease(a2, a1, r_x, crease_y, half_band);
    let d_a2_z = tier_d_at_girdle(a2, r_z, half_band);
    let d_a2_x = tier_d_at_girdle(a2, r_x, half_band);

    // Crown Step 1
    planes.push(GpuFacetPlane::new(
        Vec3::new(0.0, a1.cos(), a1.sin()),
        d_a1_z,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(0.0, a1.cos(), -a1.sin()),
        d_a1_z,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(a1.sin(), a1.cos(), 0.0),
        d_a1_x,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(-a1.sin(), a1.cos(), 0.0),
        d_a1_x,
    ));

    // Crown Step 2
    planes.push(GpuFacetPlane::new(
        Vec3::new(0.0, a2.cos(), a2.sin()),
        d_a2_z,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(0.0, a2.cos(), -a2.sin()),
        d_a2_z,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(a2.sin(), a2.cos(), 0.0),
        d_a2_x,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(-a2.sin(), a2.cos(), 0.0),
        d_a2_x,
    ));
}

/// Pushes the 8 pavilion tier facets: Pavilion Step 1 (`p1`, girdle-adjacent) and
/// Pavilion Step 2 (`p2`, keel-adjacent, meets the crease ring). Mirror image of
/// [`push_crown_tiers`] on the -y side.
fn push_pavilion_tiers(
    planes: &mut Vec<GpuFacetPlane>,
    p1: f32,
    p2: f32,
    r_z: f32,
    r_x: f32,
    half_band: f32,
    crease_mag: f32,
) {
    let d_p1_z = tier_d_at_girdle(p1, r_z, half_band);
    let d_p1_x = tier_d_at_girdle(p1, r_x, half_band);
    let d_p2_z = tier_d_at_crease(p1, p2, r_z, crease_mag, half_band);
    let d_p2_x = tier_d_at_crease(p1, p2, r_x, crease_mag, half_band);

    // Pavilion Step 1 (girdle-adjacent)
    planes.push(GpuFacetPlane::new(
        Vec3::new(0.0, -p1.cos(), p1.sin()),
        d_p1_z,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(0.0, -p1.cos(), -p1.sin()),
        d_p1_z,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(p1.sin(), -p1.cos(), 0.0),
        d_p1_x,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(-p1.sin(), -p1.cos(), 0.0),
        d_p1_x,
    ));

    // Pavilion Step 2 (keel-adjacent)
    planes.push(GpuFacetPlane::new(
        Vec3::new(0.0, -p2.cos(), p2.sin()),
        d_p2_z,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(0.0, -p2.cos(), -p2.sin()),
        d_p2_z,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(p2.sin(), -p2.cos(), 0.0),
        d_p2_x,
    ));
    planes.push(GpuFacetPlane::new(
        Vec3::new(-p2.sin(), -p2.cos(), 0.0),
        d_p2_x,
    ));
}

/// Pushes the 4 diagonal corner facets at `angle`, through the girdle's diagonal
/// corner point. `y_sign` is `1.0` for crown corners (+y normal) or `-1.0` for
/// pavilion corners (-y normal); see [`corner_d`] for why the same formula works
/// for both.
fn push_corner_facets(
    planes: &mut Vec<GpuFacetPlane>,
    angle: f32,
    r_diag: f32,
    half_band: f32,
    diag: f32,
    y_sign: f32,
) {
    let d = corner_d(angle, r_diag, half_band);
    let y = y_sign * angle.cos();
    let s = angle.sin() * diag;
    planes.push(GpuFacetPlane::new(Vec3::new(s, y, s), d));
    planes.push(GpuFacetPlane::new(Vec3::new(-s, y, s), d));
    planes.push(GpuFacetPlane::new(Vec3::new(s, y, -s), d));
    planes.push(GpuFacetPlane::new(Vec3::new(-s, y, -s), d));
}

/// Pushes the 8 vertical girdle facets (4 axis-aligned + 4 diagonal).
fn push_girdle_facets(planes: &mut Vec<GpuFacetPlane>, r_z: f32, r_x: f32, r_diag: f32, diag: f32) {
    planes.push(GpuFacetPlane::new(Vec3::new(0.0, 0.0, 1.0), -r_z));
    planes.push(GpuFacetPlane::new(Vec3::new(0.0, 0.0, -1.0), -r_z));
    planes.push(GpuFacetPlane::new(Vec3::new(1.0, 0.0, 0.0), -r_x));
    planes.push(GpuFacetPlane::new(Vec3::new(-1.0, 0.0, 0.0), -r_x));
    planes.push(GpuFacetPlane::new(Vec3::new(diag, 0.0, diag), -r_diag));
    planes.push(GpuFacetPlane::new(Vec3::new(-diag, 0.0, diag), -r_diag));
    planes.push(GpuFacetPlane::new(Vec3::new(diag, 0.0, -diag), -r_diag));
    planes.push(GpuFacetPlane::new(Vec3::new(-diag, 0.0, -diag), -r_diag));
}
