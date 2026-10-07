//! The illumination the metrics are scored under: whether a ray leaving the crown sees a
//! light source or only the dim surround.
//!
//! The test reads the same radiance the tracer lights the image with
//! ([`sample_studio_environment_with_rig`] for the analytic rigs, the map's own luminance for an
//! HDR panorama), so the lobes that make a facet flash in the render are the lobes that
//! count as "illuminated" here, and a preset change moves the numbers the way it moves
//! the picture.
//!
//! "Illuminated" is relative to the environment's ambient level, the median radiance over
//! a fixed set of directions spread evenly over the upper hemisphere. Two rules apply.
//!
//! - The `Studio` rigs (daylight, incandescent, ring lights, dark spotlight) are pinpoint
//!   sources on a dim charcoal surround: a direction is lit when its radiance exceeds
//!   [`SOURCE_OVER_AMBIENT`] times the ambient level, which separates the softboxes and
//!   ring pinpoints from the backdrop. An HDR map follows the same rule: with no region
//!   brighter than [`SOURCE_OVER_AMBIENT`] times its median luminance nothing in it
//!   counts as lit.
//! - Every other model (the light tent, the daylight dome, the ISO hemisphere) is a lit
//!   environment: its walls, sky or dome are the light. A direction is lit when its
//!   radiance exceeds [`UNIFORM_FRACTION_OF_AMBIENT`] times the ambient level, so the
//!   brilliance metric means "light return" under all of them, as it does for the ISO
//!   figure cutters know from `GemRay`. Under the tent the walls count as lit while the
//!   black cards (x0.1), the ground and the head shadow do not, so a facet reflecting a
//!   card still costs extinction. The old 4x rule counted only the tent's key cone and
//!   the dome's sun disc, which made brilliance a measure of aiming exits at one patch.
//!
//! The observer's head shadow is not part of this radiance (the observer is passed as the
//! zero vector): [`super::visibility::ray_is_visibly_returned`] applies its own cone.

use glam::Vec3;

use super::visibility::head_shadow_cone_cos;
use crate::{
    optics::{
        raytracer::{
            EnvironmentSource, LightingModel, LightingPreset, sample_studio_environment_with_rig,
        },
        studio_rig::StudioRig,
    },
    renderer::env_map::EnvironmentMap,
};

/// Wavelength the analytic rigs are probed at, the photopic luminosity peak.
const PROBE_WAVELENGTH_NM: f32 = 550.0;

/// Exposure the analytic rigs are probed at. The threshold scales with the radiance it
/// is compared against, so the classification does not depend on exposure.
const PROBE_EXPOSURE: f32 = 1.0;

/// Directions [`hemisphere_direction`] spreads over the upper hemisphere, sampled to find
/// the ambient level.
pub(super) const HEMISPHERE_DIRECTIONS: usize = 96;

/// Golden angle in radians, the azimuth step of the equal-area spiral over the hemisphere.
const GOLDEN_ANGLE_RAD: f32 = 2.399_963_2;

/// How many times brighter than the ambient level a direction must be to count as lit
/// under a `Studio` rig or an HDR map (pinpoint sources on a dim surround).
const SOURCE_OVER_AMBIENT: f32 = 4.0;

/// The fraction of the ambient level above which a direction of a lit environment (every
/// non-`Studio` model: tent, dome, ISO hemisphere) counts as lit; below it only the dark
/// cards, the ground and the soft falloff at the horizon remain.
const UNIFORM_FRACTION_OF_AMBIENT: f32 = 0.5;

/// Rec. 709 luminance weights for an HDR texel's linear RGB.
const LUMINANCE_WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Where the radiance of a direction comes from.
#[derive(Clone)]
enum RadianceSource<'a> {
    /// The analytic studio rig of a preset at a light pose, its key, fill and ring
    /// directions built once for the pose (boxed: the rig is far larger than the other
    /// variant).
    Studio {
        preset: LightingPreset,
        rig: Box<StudioRig>,
    },
    /// A loaded HDR panorama.
    Hdr(&'a EnvironmentMap),
}

impl RadianceSource<'_> {
    /// The radiance seen along unit direction `dir`.
    fn radiance(&self, dir: Vec3) -> f32 {
        match self {
            Self::Studio { preset, rig } => sample_studio_environment_with_rig(
                dir,
                PROBE_WAVELENGTH_NM,
                *preset,
                PROBE_EXPOSURE,
                rig,
                Vec3::ZERO,
            ),
            Self::Hdr(map) => {
                let [r, g, b] = map.radiance_rgb(dir);
                LUMINANCE_WEIGHTS[0]
                    .mul_add(r, LUMINANCE_WEIGHTS[1].mul_add(g, LUMINANCE_WEIGHTS[2] * b))
            }
        }
    }
}

/// The `index`-th of [`HEMISPHERE_DIRECTIONS`] directions spread evenly by solid angle
/// over the upper hemisphere (a Fibonacci spiral in height and azimuth).
pub(super) fn hemisphere_direction(index: usize) -> Vec3 {
    let y = (index as f32 + 0.5) / HEMISPHERE_DIRECTIONS as f32;
    let ring_radius = y.mul_add(-y, 1.0).max(0.0).sqrt();
    let (sin_azimuth, cos_azimuth) = (index as f32 * GOLDEN_ANGLE_RAD).sin_cos();
    Vec3::new(ring_radius * cos_azimuth, y, ring_radius * sin_azimuth)
}

/// The preset whose radiance and rule the metrics score under. The ASET contrast view is a
/// diagnostic false-colour map, not an illumination: it is scored as the grading standard
/// (`IsoHemisphere`), so switching to it never moves the numbers.
const fn scoring_preset(preset: LightingPreset) -> LightingPreset {
    match preset {
        LightingPreset::Aset => LightingPreset::IsoHemisphere,
        other => other,
    }
}

/// The median radiance over the reference directions.
fn ambient_level(source: &RadianceSource<'_>) -> f32 {
    let mut samples: [f32; HEMISPHERE_DIRECTIONS] =
        std::array::from_fn(|index| source.radiance(hemisphere_direction(index)));
    samples.sort_by(f32::total_cmp);
    samples[HEMISPHERE_DIRECTIONS / 2]
}

/// Whether an exit direction sees a light source, for one environment and light pose.
///
/// Built once per evaluation: finding the ambient level costs
/// [`HEMISPHERE_DIRECTIONS`] radiance lookups.
#[derive(Clone)]
pub(super) struct ExitLighting<'a> {
    source: RadianceSource<'a>,
    /// Radiance a direction must exceed to count as lit.
    threshold: f32,
    /// Cosine of the observer head-shadow cone, see [`head_shadow_cone_cos`].
    head_shadow_cos: f32,
}

impl<'a> ExitLighting<'a> {
    /// The lighting of `environment`. A studio environment's exposure and backdrop are
    /// ignored: neither changes which directions stand out from the ambient level. Its
    /// `head_shadow_deg` sets the metrics' head-shadow cone for the lit models.
    pub(super) fn new(environment: EnvironmentSource<'a>) -> Self {
        let head_shadow_cos = head_shadow_cone_cos(
            match environment {
                EnvironmentSource::Studio { preset, .. } => preset.model() == LightingModel::Studio,
                EnvironmentSource::HdrMap(_) => true,
            },
            environment.head_shadow_deg(),
        );
        let (source, uniform) = match environment {
            EnvironmentSource::Studio {
                preset,
                light_yaw,
                light_pitch,
                ..
            } => {
                let preset = scoring_preset(preset);
                (
                    RadianceSource::Studio {
                        preset,
                        rig: Box::new(StudioRig::new(light_yaw, light_pitch)),
                    },
                    // Only the Studio rigs have pinpoint sources; every other model (tent,
                    // dome, ISO hemisphere, and any later one) scores "light return".
                    preset.model() != LightingModel::Studio,
                )
            }
            EnvironmentSource::HdrMap(map) => (RadianceSource::Hdr(map), false),
        };
        let factor = if uniform {
            UNIFORM_FRACTION_OF_AMBIENT
        } else {
            SOURCE_OVER_AMBIENT
        };
        let threshold = (ambient_level(&source) * factor).max(f32::MIN_POSITIVE);
        Self {
            source,
            threshold,
            head_shadow_cos,
        }
    }

    /// Cosine of the observer's head-shadow cone: exits closer than this to the eye
    /// direction are lost to the observer's own shadow.
    pub(super) const fn head_shadow_cos(&self) -> f32 {
        self.head_shadow_cos
    }

    /// Whether a ray leaving the stone along unit direction `exit_dir` sees a light source.
    pub(super) fn is_illuminated(&self, exit_dir: Vec3) -> bool {
        self.source.radiance(exit_dir) > self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAW: f32 = 0.85;
    const PITCH: f32 = 0.95;

    fn lighting(preset: LightingPreset) -> ExitLighting<'static> {
        ExitLighting::new(preset.studio(1.0, YAW, PITCH))
    }

    /// A unit direction `azimuth_deg` around Y from the key's azimuth, at `elevation_deg`.
    fn direction_from_key(key: Vec3, azimuth_deg: f32, elevation_deg: f32) -> Vec3 {
        let horizontal = Vec3::new(key.x, 0.0, key.z).normalize();
        let rotated = glam::Quat::from_rotation_y(azimuth_deg.to_radians()) * horizontal;
        let elevation = elevation_deg.to_radians();
        rotated * elevation.cos() + Vec3::Y * elevation.sin()
    }

    #[test]
    fn under_the_tent_the_walls_are_lit_and_the_cards_and_ground_are_not() {
        let tent = lighting(LightingPreset::LightTent);
        let rig = StudioRig::new(YAW, PITCH);
        // A wall direction clear of the key cone, the spark and every card.
        let wall = direction_from_key(rig.key_dir, 135.0, 70.0);
        assert!(tent.is_illuminated(wall), "tent wall must count as lit");
        assert!(
            tent.is_illuminated(Vec3::Y),
            "tent zenith must count as lit"
        );
        for slot in [4, 8, 12] {
            assert!(
                !tent.is_illuminated(rig.ring_dirs[slot]),
                "black card {slot} must not count as lit"
            );
        }
        assert!(
            !tent.is_illuminated(-Vec3::Y),
            "ground must not count as lit"
        );
    }

    #[test]
    fn under_the_daylight_dome_the_sky_above_the_horizon_is_lit() {
        let dome = lighting(LightingPreset::DaylightDome);
        let rig = StudioRig::new(YAW, PITCH);
        assert!(dome.is_illuminated(Vec3::Y));
        assert!(dome.is_illuminated(direction_from_key(rig.key_dir, 180.0, 30.0)));
        assert!(dome.is_illuminated(direction_from_key(rig.key_dir, 90.0, 10.0)));
        assert!(!dome.is_illuminated(-Vec3::Y));
    }

    #[test]
    fn the_iso_hemisphere_is_lit_everywhere_above_the_horizon() {
        let iso = lighting(LightingPreset::IsoHemisphere);
        assert!(iso.is_illuminated(Vec3::Y));
        assert!(iso.is_illuminated(direction_from_key(Vec3::X, 0.0, 20.0)));
        assert!(!iso.is_illuminated(-Vec3::Y));
    }

    #[test]
    fn the_aset_view_is_scored_as_the_iso_hemisphere() {
        let aset = lighting(LightingPreset::Aset);
        let iso = lighting(LightingPreset::IsoHemisphere);
        assert_eq!(aset.threshold.to_bits(), iso.threshold.to_bits());
        assert_eq!(
            aset.head_shadow_cos.to_bits(),
            iso.head_shadow_cos.to_bits()
        );
        let rig = StudioRig::new(YAW, PITCH);
        for dir in [
            Vec3::Y,
            direction_from_key(rig.key_dir, 90.0, 10.0),
            direction_from_key(rig.key_dir, 180.0, 60.0),
            -Vec3::Y,
        ] {
            assert_eq!(aset.is_illuminated(dir), iso.is_illuminated(dir));
        }
    }

    #[test]
    fn head_shadow_cone_follows_the_scene_only_for_lit_models() {
        let cos = |preset: LightingPreset, deg: f32| {
            ExitLighting::new(preset.studio(1.0, YAW, PITCH).with_head_shadow(deg))
                .head_shadow_cos()
        };
        assert_eq!(
            cos(LightingPreset::LightTent, 16.0).to_bits(),
            0.96f32.to_bits()
        );
        assert!(cos(LightingPreset::LightTent, 0.0) > 1.0);
        assert!(cos(LightingPreset::LightTent, 30.0) < 0.9);
        assert_eq!(
            cos(LightingPreset::RingLights, 0.0).to_bits(),
            0.96f32.to_bits()
        );
    }

    /// `DaylightSun` falls under the uniform lit rule, so the sky is lit, and its
    /// 40 000-radiance sun does not break the median-based threshold: the ambient level is
    /// the median over 96 directions of equal solid angle (about 0.065 sr each), the disc is
    /// 7e-5 sr, so it could only move the median if it took half the samples. At this pose
    /// no sample even lands in it (nearest: 6.4 degrees from the key), so the threshold is
    /// exactly the sky-only dome's; the exits that do see the sun count as lit.
    #[test]
    fn under_the_daylight_sun_the_sky_is_lit_and_the_sun_does_not_move_the_threshold() {
        let dome = lighting(LightingPreset::DaylightDome);
        let sun = lighting(LightingPreset::DaylightSun);
        let rig = StudioRig::new(YAW, PITCH);
        assert!(
            (sun.threshold / dome.threshold - 1.0).abs() < 1e-2,
            "sun threshold {} vs dome threshold {}",
            sun.threshold,
            dome.threshold
        );
        // The threshold is the sky's, orders of magnitude below the sun's radiance.
        assert!(sun.threshold < 0.2, "{}", sun.threshold);
        assert!(sun.is_illuminated(rig.key_dir), "the sun itself is lit");
        assert!(sun.is_illuminated(Vec3::Y));
        assert!(sun.is_illuminated(direction_from_key(rig.key_dir, 180.0, 30.0)));
        assert!(!sun.is_illuminated(-Vec3::Y));
    }

    #[test]
    fn a_studio_rig_keeps_the_pinpoint_rule() {
        let rig = StudioRig::new(YAW, PITCH);
        let ring = lighting(LightingPreset::RingLights);
        assert!(
            ring.is_illuminated(rig.key_dir),
            "the key softbox is a source"
        );
        assert!(
            !ring.is_illuminated(-Vec3::Y),
            "the backdrop is not a source"
        );
        // The threshold is the 4x pinpoint one, not the 0.5x lit-environment one.
        let ambient = ambient_level(&ring.source);
        assert!(
            ambient.mul_add(-SOURCE_OVER_AMBIENT, ring.threshold).abs()
                <= f32::EPSILON * ring.threshold
        );
    }
}
