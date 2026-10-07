//! [`FacetFrame`]: the local axes of one facet (normal, angle tangent, index tangent),
//! built with exactly the same f32 arithmetic the solid rasterizer's facet map uses.

use glam::Vec3;
use indicatrix::geometry::plane::tier_is_crown_side;
use indicatrix_cut_core::ConstraintTier;

/// `(theta, phi)` in radians, op for op as `indicatrix_solid::facet_map` builds them:
/// `theta = |angle|`, `phi = 2 pi index / gear`, both narrowed to f32 first.
fn theta_phi(angle_deg: f64, index_on_gear: f64, gear_teeth: u32) -> (f32, f32) {
    let gear = gear_teeth.max(1) as f32;
    let theta = (angle_deg.abs() as f32).to_radians();
    let phi = 2.0 * std::f32::consts::PI * (index_on_gear as f32) / gear;
    (theta, phi)
}

/// `(sin phi, cos phi)` for a facet: the ordinary azimuth for a tier with index-wheel
/// positions, and exactly `(1, 0)` for a tier with NONE -- `facet_map/build.rs` gives
/// such a tier the normal `(0, +-cos t, sin t)`, which is `phi = pi/2` with an exact
/// zero `x` component rather than `cos(pi/2)`'s rounding residue.
fn sin_cos_phi(phi: f32, indexless: bool) -> (f32, f32) {
    if indexless {
        (1.0, 0.0)
    } else {
        (phi.sin(), phi.cos())
    }
}

/// The outward unit normal of a facet at `angle_deg` on tooth `index_on_gear` of a
/// `gear_teeth` gear -- bit-identical to `facet_map/build.rs` for a tier with index-wheel
/// positions. A tier with none is handled by [`FacetFrame::from_tier`].
pub(super) fn facet_normal(angle_deg: f64, index_on_gear: f64, gear_teeth: u32) -> Vec3 {
    facet_normal_with(angle_deg, index_on_gear, gear_teeth, false)
}

/// [`facet_normal`] with the indexless special case -- see [`sin_cos_phi`].
fn facet_normal_with(angle_deg: f64, index_on_gear: f64, gear_teeth: u32, indexless: bool) -> Vec3 {
    let (theta, phi) = theta_phi(angle_deg, index_on_gear, gear_teeth);
    let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
    let (sin_phi, cos_phi) = sin_cos_phi(phi, indexless);
    let normal = if tier_is_crown_side(angle_deg) {
        Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
    } else {
        Vec3::new(sin_theta * cos_phi, -cos_theta, sin_theta * sin_phi)
    };
    normal.normalize()
}

/// One facet's local frame in world space (`+Y` toward the crown, index 0 toward `+X`,
/// `phi` growing toward `+Z`).
///
/// [`Self::normal`] is exactly the vector `indicatrix_solid::facet_map` builds for the
/// facet; the two tangents are its partial derivatives, so a screen-space drag along a
/// tangent's projection maps to a change of angle or of index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FacetFrame {
    /// The facet's centroid (world space): where the handles are anchored.
    pub centroid: Vec3,
    /// The facet's outward unit normal.
    pub normal: Vec3,
    /// The tier's signed angle in degrees (negative is pavilion, `-0.0` the culet).
    pub angle_deg: f64,
    /// The facet's index-wheel position (a whole tooth for a facet of the mesh).
    pub index_on_gear: f64,
    /// The gear's tooth count (at least 1).
    pub gear_teeth: u32,
    /// Whether the tier has no index-wheel positions at all (a lone facet the facet
    /// map places at `phi = pi/2` exactly) -- see [`sin_cos_phi`].
    indexless: bool,
}

impl FacetFrame {
    /// The frame of `tier`'s facet at `index_on_gear` on a `gear_teeth` gear, anchored
    /// at `centroid`. The normal is constructed exactly as `facet_map/build.rs` does,
    /// including its special case for a tier with no index-wheel positions
    /// (`index_on_gear` is ignored for such a tier; the facet map reports `0` for it).
    #[must_use]
    pub fn from_tier(
        tier: &ConstraintTier,
        index_on_gear: f64,
        gear_teeth: u32,
        centroid: Vec3,
    ) -> Self {
        let gear_teeth = gear_teeth.max(1);
        let indexless = tier.indices.is_empty();
        Self {
            centroid,
            normal: facet_normal_with(tier.angle_deg, index_on_gear, gear_teeth, indexless),
            angle_deg: tier.angle_deg,
            index_on_gear,
            gear_teeth,
            indexless,
        }
    }

    /// [`Self::from_tier`] for a design whose index wheel has a gear reference angle.
    ///
    /// The facet map puts a facet at the azimuth `2 pi (index + reference) / teeth`
    /// (`StandardGemCuts::index_to_azimuth`, with the reference angle narrowed to `f32`
    /// first), so a frame built from the plain index would sit on a different facet than the
    /// one drawn whenever the reference is not zero. `gear_reference_angle` is that
    /// narrowed value (`design.meta.gear_reference_angle as f32`, or a `DiagramLayout`'s
    /// own). For a whole tooth the sum is exact in `f64`, so the narrowing to `f32` inside
    /// [`Self::from_tier`] rounds exactly as the facet map's `f32` addition does and the
    /// normal stays bit-identical. A tier with no index-wheel positions ignores it, as
    /// the facet map does.
    #[must_use]
    pub fn from_tier_with_reference(
        tier: &ConstraintTier,
        index_on_gear: f64,
        gear_reference_angle: f32,
        gear_teeth: u32,
        centroid: Vec3,
    ) -> Self {
        Self::from_tier(
            tier,
            index_on_gear + f64::from(gear_reference_angle),
            gear_teeth,
            centroid,
        )
    }

    /// Whether the tier has no index-wheel positions (its index handle turns nothing).
    #[must_use]
    pub const fn is_indexless(&self) -> bool {
        self.indexless
    }

    /// Whether the facet is on the crown side (a positive angle or `+0.0`).
    #[must_use]
    pub const fn is_crown(&self) -> bool {
        tier_is_crown_side(self.angle_deg)
    }

    /// `d normal / d theta` (`theta = |angle|` in radians): the direction the normal
    /// moves as the facet gets steeper. Crown: `(cos t cos p, -sin t, cos t sin p)`;
    /// pavilion: `(cos t cos p, sin t, cos t sin p)`. A unit vector.
    #[must_use]
    pub fn tangent_theta(&self) -> Vec3 {
        let (theta, phi) = theta_phi(self.angle_deg, self.index_on_gear, self.gear_teeth);
        let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
        let (sin_phi, cos_phi) = sin_cos_phi(phi, self.indexless);
        let y = if self.is_crown() {
            -sin_theta
        } else {
            sin_theta
        };
        Vec3::new(cos_theta * cos_phi, y, cos_theta * sin_phi)
    }

    /// `d normal / d phi` direction: `(-sin p, 0, cos p)`, the way the facet's normal
    /// turns around the stone as the index grows. A unit vector.
    #[must_use]
    pub fn tangent_phi(&self) -> Vec3 {
        let (_, phi) = theta_phi(self.angle_deg, self.index_on_gear, self.gear_teeth);
        let (sin_phi, cos_phi) = sin_cos_phi(phi, self.indexless);
        Vec3::new(-sin_phi, 0.0, cos_phi)
    }
}
