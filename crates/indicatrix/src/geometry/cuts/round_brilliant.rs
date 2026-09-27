//! Azimuth conversion and the 57-facet Standard Round Brilliant reference cut.

use std::f32::consts::PI;

use glam::Vec3;

use super::StandardGemCuts;
use crate::geometry::plane::GpuFacetPlane;

/// Girdle finish: the facet-index range of the girdle band.
///
/// Within [`StandardGemCuts::standard_round_brilliant`]'s own construction order, the 16
/// vertical prism facets making up the physical girdle band -- see that function's own
/// "5. 16 Girdle Facets (90.0° vertical cylinder of radius 1.0)" comment for the layout
/// this indexes into (table: 1, star: 8, crown main: 8, upper girdle break: 16 -- NONE
/// of which are the girdle itself, despite the name -- THEN the 16 true girdle facets,
/// at indices 33..=48). Exposed so a caller building a bruted/frosted-girdle variant of
/// this cut (e.g. `optics::raytracer::trace_spectral_ray_with_finish`'s
/// `facet_finishes` argument) knows which facet indices to mark
/// `optics::raytracer::FacetFinish::Frosted` without hand-deriving them -- and so that
/// derivation stays correct automatically if this cut's facet-push order ever changes.
pub const STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS: std::ops::Range<usize> = 33..49;

impl StandardGemCuts {
    /// Converts an index-wheel position to azimuth (radians about the stone's
    /// vertical axis), honoring a schedule's `g`-line reference angle.
    ///
    /// `index` and `gear_reference_angle` are both in **index-wheel tooth
    /// units** -- the same units [`indicatrix_formats::asc::AscTier::indices`] and
    /// [`indicatrix_formats::asc::AscSchedule::gear_reference_angle`] use, *not*
    /// degrees. That the reference angle is tooth-denominated (rather than a
    /// degrees-from-some-fixed-direction offset) is not stated anywhere in
    /// the `.asc` format's own sparse documentation; it was determined
    /// empirically against the real corpus (`facet_diagrams.sqlite`'s 5,758
    /// attached `.asc` files): of the 2,008 files (34.9%) with a nonzero
    /// `gear_reference_angle`, **every single one's magnitude sits inside
    /// `[0, gear_teeth]`** -- never once does it land in a degrees-style
    /// `0..360` range independent of that design's own tooth count. The
    /// value clusters almost entirely on two points: exactly `gear_teeth /
    /// 2` (1,574 files -- a deliberate half-turn re-reference of the whole
    /// index wheel) and exactly `gear_teeth` itself (424 files -- a full
    /// turn, geometrically a no-op); the rest split between `-gear_teeth /
    /// 2` (8 files) and `gear_teeth / 16` (2 files). Treating the field as
    /// tooth-denominated is the only interpretation consistent with values
    /// scaling with each design's own (varying: 96, 64, 80, 120, 72, ...)
    /// gear count instead of clustering near a fixed degrees range.
    ///
    /// Applied as a simple additive offset to the raw index -- `index 0`
    /// sits `gear_reference_angle` teeth around the wheel from the format's
    /// own zero direction -- rather than negated or applied to index growth
    /// direction, so this never flips the existing (load-bearing for every
    /// scene already reconstructed from these files) handedness of index
    /// growth; it only rotates the whole azimuth frame. At
    /// `gear_reference_angle == 0.0` (the default, and 65.1% of real files)
    /// this reduces to plain `2*pi*index/gear_teeth` bit-for-bit (`x + 0.0
    /// == x` for every finite `f32`), so every schedule that never set a
    /// reference angle renders exactly as before this fix.
    ///
    /// `gear_teeth` must already be the *absolute* tooth count (see
    /// [`indicatrix_formats::asc::AscSchedule::gear_teeth_abs`]) and non-zero --
    /// callers are responsible for that (mirrors the `.max(1)` guard already
    /// used at every call site below).
    #[must_use]
    pub fn index_to_azimuth(index: f32, gear_teeth: f32, gear_reference_angle: f32) -> f32 {
        2.0 * PI * (index + gear_reference_angle) / gear_teeth
    }

    /// Generates exact 3D half-space planes for a 57-facet Standard Round Brilliant (SRB) Diamond cut
    /// with standard ideal proportions (Crown height ~15% diameter, Table ~56% width, Pavilion depth ~43%).
    #[must_use]
    pub fn standard_round_brilliant() -> Vec<GpuFacetPlane> {
        let mut planes = Vec::with_capacity(57);
        let gear_teeth = 96.0f32;

        // 1. Crown Table Facet (Top flat facet at Y = +0.32)
        planes.push(GpuFacetPlane::new(Vec3::new(0.0, 1.0, 0.0), -0.32));

        // 2. 8 Crown Star Facets (15.0°, index 6, 18, 30, 42, 54, 66, 78, 90)
        let star_angle = 15.0f32.to_radians();
        for &g in &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                star_angle.sin() * phi.cos(),
                star_angle.cos(),
                star_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.45));
        }

        // 3. 8 Crown Kite / Main Facets (34.5°, index 96, 12, 24, 36, 48, 60, 72, 84)
        let crown_main_angle = 34.5f32.to_radians();
        for &g in &[0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                crown_main_angle.sin() * phi.cos(),
                crown_main_angle.cos(),
                crown_main_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.59));
        }

        // 4. 16 Upper Girdle Break Facets (41.0°, index 95, 1, 11, 13, 23, 25, 35, 37, 47, 49, 59, 61, 71, 73, 83, 85)
        let upper_girdle_angle = 41.0f32.to_radians();
        for &g in &[
            95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
            83.0, 85.0,
        ] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                upper_girdle_angle.sin() * phi.cos(),
                upper_girdle_angle.cos(),
                upper_girdle_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.67));
        }

        // 5. 16 Girdle Facets (90.0° vertical cylinder of radius 1.0)
        for i in 0..16 {
            let phi = 2.0 * PI * (i as f32) / 16.0;
            let n = Vec3::new(phi.cos(), 0.0, phi.sin());
            planes.push(GpuFacetPlane::new(n, -1.0));
        }

        // 6. 8 Pavilion Main Facets (-41.0°, index 96, 12, 24, 36, 48, 60, 72, 84)
        let pav_main_angle = 41.0f32.to_radians();
        for &g in &[0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                pav_main_angle.sin() * phi.cos(),
                -pav_main_angle.cos(),
                pav_main_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.67));
        }

        // 7. 16 Lower Girdle Break Facets (-42.5°, index 95, 1, 11, 13, 23, 25, 35, 37, 47, 49, 59, 61, 71, 73, 83, 85)
        let lower_girdle_angle = 42.5f32.to_radians();
        for &g in &[
            95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
            83.0, 85.0,
        ] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                lower_girdle_angle.sin() * phi.cos(),
                -lower_girdle_angle.cos(),
                lower_girdle_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.68));
        }

        // 8. Culet (Bottom point at Y = -0.88)
        planes.push(GpuFacetPlane::new(Vec3::new(0.0, -1.0, 0.0), -0.88));

        planes
    }
}
