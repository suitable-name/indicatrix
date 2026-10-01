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
//! a fixed set of directions spread evenly over the upper hemisphere. A direction is lit
//! when its radiance exceeds [`SOURCE_OVER_AMBIENT`] times that level, which separates the
//! studio's softboxes and ring pinpoints, the tent's overhead softbox and spark light, the
//! dome's sun and an HDR map's bright regions from the charcoal backdrop, tent walls and
//! open sky behind them. The ISO hemisphere has no source standing out from its ambient,
//! being uniformly radiant: every direction it radiates into counts as lit
//! ([`UNIFORM_FRACTION_OF_AMBIENT`]). An HDR map with no region brighter than
//! [`SOURCE_OVER_AMBIENT`] times its median luminance has no source, so nothing in it counts
//! as lit.
//!
//! The observer's head shadow is not part of this radiance (the observer is passed as the
//! zero vector): [`super::visibility::ray_is_visibly_returned`] applies its own cone.

use glam::Vec3;

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

/// How many times brighter than the ambient level a direction must be to count as lit.
const SOURCE_OVER_AMBIENT: f32 = 4.0;

/// The fraction of the ambient level above which a direction of a uniformly radiant
/// environment counts as lit; below it only the soft falloff at the horizon remains.
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
}

impl<'a> ExitLighting<'a> {
    /// The lighting of `environment`. A studio environment's exposure and backdrop are
    /// ignored: neither changes which directions stand out from the ambient level.
    pub(super) fn new(environment: EnvironmentSource<'a>) -> Self {
        let (source, uniform) = match environment {
            EnvironmentSource::Studio {
                preset,
                light_yaw,
                light_pitch,
                ..
            } => (
                RadianceSource::Studio {
                    preset,
                    rig: Box::new(StudioRig::new(light_yaw, light_pitch)),
                },
                preset.model() == LightingModel::IsoHemisphere,
            ),
            EnvironmentSource::HdrMap(map) => (RadianceSource::Hdr(map), false),
        };
        let factor = if uniform {
            UNIFORM_FRACTION_OF_AMBIENT
        } else {
            SOURCE_OVER_AMBIENT
        };
        let threshold = (ambient_level(&source) * factor).max(f32::MIN_POSITIVE);
        Self { source, threshold }
    }

    /// Whether a ray leaving the stone along unit direction `exit_dir` sees a light source.
    pub(super) fn is_illuminated(&self, exit_dir: Vec3) -> bool {
        self.source.radiance(exit_dir) > self.threshold
    }
}
