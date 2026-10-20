//! Golden capture scenes: fixed list of 11 scenes (10 without the `zoning` feature) used for GPU golden image capture
//! and equivalence verification against baseline megakernel runs.
//!
//! Every scene returns a [`SceneBundle`] specifying:
//! - 64x64 resolution
//! - 64 samples per pixel (spp)
//! - 12 maximum bounces (except Scene 10, which forces 3 max bounces to test survivors)
//! - Scene 11 is the only scene that combines an HDR environment with scattering: Studio
//!   scenes have next-event estimation off, so scenes 5 and 6 cannot see NEE or MIS bugs.
//! - Sample offset 0
//! - Standard round brilliant cut (57 facets)
//! - Standard camera pose: `Camera::new(0.35, 0.28, 5.0, 18.0)`

use indicatrix::{
    geometry::{GpuFacetPlane, StandardGemCuts, girdle_facet_finishes},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish, LightingPreset},
    },
    renderer::{env_map::EnvironmentMap, frame_scene::FrameScene},
};

/// Default image width for golden capture scenes.
pub const DEFAULT_WIDTH: u32 = 64;

/// Default image height for golden capture scenes.
pub const DEFAULT_HEIGHT: u32 = 64;

/// Default samples per pixel for golden capture scenes.
pub const DEFAULT_SPP: u32 = 64;

/// Default maximum internal bounces for golden capture scenes.
pub const DEFAULT_MAX_BOUNCES: u32 = 12;

/// Default starting sample offset.
pub const DEFAULT_SAMPLE_OFFSET: u32 = 0;

/// Standard camera pose shared by all capture scenes.
#[must_use]
pub fn default_camera() -> Camera {
    Camera::new(0.35, 0.28, 5.0, 18.0)
}

/// Environment kind for a capture scene: either an analytic studio rig preset or an HDR map.
pub enum SceneEnvironment {
    Studio(LightingPreset),
    Hdr(EnvironmentMap),
}

/// An owned scene bundle returning everything needed to render a frame with `accumulate`.
pub struct SceneBundle {
    /// Short identifier used for filenames (e.g. `<dir>/<name>.f32`).
    pub name: &'static str,
    /// Human-readable Title Case description.
    pub description: &'static str,
    /// Camera the frame is rendered from.
    pub camera: Camera,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Facet planes of the stone.
    pub planes: Vec<GpuFacetPlane>,
    /// Per-plane finish. Empty slice defaults to all polished.
    pub facet_finishes: Vec<FacetFinish>,
    /// Gem material.
    pub material: GemMaterial,
    /// Maximum number of internal bounces per path.
    pub max_bounces: u32,
    /// Samples per pixel to accumulate.
    pub spp: u32,
    /// Starting sample offset index.
    pub sample_offset: u32,
    /// Lighting environment source.
    pub environment: SceneEnvironment,
}

impl SceneBundle {
    /// Borrows the bundle fields into a [`FrameScene`] suitable for `GpuFrameRenderer::accumulate`.
    #[must_use]
    pub fn as_frame_scene(&self) -> FrameScene<'_> {
        let environment = match &self.environment {
            SceneEnvironment::Studio(preset) => preset.studio(1.0, 0.4, 0.35),
            SceneEnvironment::Hdr(hdr) => EnvironmentSource::HdrMap(hdr),
        };
        FrameScene {
            camera: &self.camera,
            width: self.width,
            height: self.height,
            planes: &self.planes,
            facet_finishes: &self.facet_finishes,
            material: &self.material,
            max_bounces: self.max_bounces,
            environment,
        }
    }
}

/// Builds the synthetic 16x8 HDR environment map used by the equivalence harness.
#[must_use]
pub fn synthetic_harness_hdr_map() -> EnvironmentMap {
    let width = 16;
    let height = 8;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let r = (((x * 3 + y * 5) % 7) as f32).mul_add(0.4, 0.1);
            let g = (((x * 2 + y * 3) % 5) as f32).mul_add(0.3, 0.1);
            let b = (((x * 5 + y * 2) % 6) as f32).mul_add(0.5, 0.1);
            pixels.push([r, g, b]);
        }
    }
    EnvironmentMap::from_rgb(width, height, pixels).expect("synthetic harness HDR map is valid")
}

/// Scene 1: Diamond, studio daylight.
#[must_use]
pub fn scene_01_diamond_daylight() -> SceneBundle {
    SceneBundle {
        name: "scene01_diamond_daylight",
        description: "Diamond, Studio Daylight",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Diamond").expect("Diamond is a built-in material"),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Studio(LightingPreset::Daylight),
    }
}

/// Scene 2: Spinel, studio lit preset 4 (`WindowDaylight`).
#[must_use]
pub fn scene_02_spinel_preset4() -> SceneBundle {
    SceneBundle {
        name: "scene02_spinel_preset4",
        description: "Spinel, Studio Preset 4 (Window Daylight)",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Spinel").expect("Spinel is a built-in material"),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Studio(LightingPreset::from_index(4)),
    }
}

/// Scene 3: Zircon (uniaxial material).
#[must_use]
pub fn scene_03_zircon() -> SceneBundle {
    SceneBundle {
        name: "scene03_zircon",
        description: "Zircon (Uniaxial)",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Zircon").expect("Zircon is a built-in material"),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Studio(LightingPreset::Daylight),
    }
}

/// Scene 4: Biaxial material (Topaz).
#[must_use]
pub fn scene_04_biaxial() -> SceneBundle {
    SceneBundle {
        name: "scene04_topaz_biaxial",
        description: "Topaz (Biaxial)",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Topaz").expect("Topaz is a built-in material"),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Studio(LightingPreset::Daylight),
    }
}

/// Scene 5: One scattering material (Ruby with `scattering_sigma_s` = 1.0, g = 0.3).
#[must_use]
pub fn scene_05_scattering() -> SceneBundle {
    SceneBundle {
        name: "scene05_ruby_scattering",
        description: "Ruby with Scattering (sigma_s = 1.0, g = 0.3)",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Ruby")
            .expect("Ruby is a built-in material")
            .with_scattering(1.0, 0.3),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Studio(LightingPreset::Daylight),
    }
}

/// Scene 6: Spinel with a frosted girdle.
#[must_use]
pub fn scene_06_spinel_frosted_girdle() -> SceneBundle {
    let planes = StandardGemCuts::standard_round_brilliant();
    let facet_finishes = girdle_facet_finishes(&planes);
    SceneBundle {
        name: "scene06_spinel_frosted_girdle",
        description: "Spinel with Frosted Girdle",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes,
        facet_finishes,
        material: GemMaterial::by_name("Spinel").expect("Spinel is a built-in material"),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Studio(LightingPreset::Daylight),
    }
}

/// Scene 7: Diamond under the harness's synthetic HDR map.
#[must_use]
pub fn scene_07_diamond_hdr() -> SceneBundle {
    SceneBundle {
        name: "scene07_diamond_hdr",
        description: "Diamond under Synthetic HDR Map",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Diamond").expect("Diamond is a built-in material"),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Hdr(synthetic_harness_hdr_map()),
    }
}

/// Scene 8: Synthetic Moissanite (strong dispersion, exit splits occur).
#[must_use]
pub fn scene_08_moissanite_dispersion() -> SceneBundle {
    SceneBundle {
        name: "scene08_moissanite_dispersion",
        description: "Synthetic Moissanite (Dispersion and Exit Splits)",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Synthetic Moissanite")
            .expect("Synthetic Moissanite is a built-in material"),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Studio(LightingPreset::Daylight),
    }
}

/// Scene 9: Zoned stone only if built with `cfg(feature = "zoning")` (skip otherwise or return None).
#[must_use]
#[cfg_attr(
    feature = "zoning",
    expect(
        clippy::unnecessary_wraps,
        reason = "the scene exists only with the `zoning` feature; the caller's signature is the same either way"
    )
)]
#[cfg_attr(
    not(feature = "zoning"),
    expect(
        clippy::missing_const_for_fn,
        reason = "without `zoning` the body is a bare `None`; with it the scene is built at run time"
    )
)]
pub fn scene_09_zoned() -> Option<SceneBundle> {
    #[cfg(feature = "zoning")]
    {
        Some(SceneBundle {
            name: "scene09_zoned_bicolor",
            description: "Zoned Stone (Bicolor)",
            camera: default_camera(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            planes: StandardGemCuts::standard_round_brilliant(),
            facet_finishes: Vec::new(),
            material: indicatrix::renderer::gpu::zoned_cases::ZonedImageCase::Bicolour.material(),
            max_bounces: DEFAULT_MAX_BOUNCES,
            spp: DEFAULT_SPP,
            sample_offset: DEFAULT_SAMPLE_OFFSET,
            environment: SceneEnvironment::Studio(LightingPreset::Daylight),
        })
    }
    #[cfg(not(feature = "zoning"))]
    {
        None
    }
}

/// Scene 10: Diamond with `max_bounces = 3` (forces survivor rays).
#[must_use]
pub fn scene_10_diamond_max_bounces_3() -> SceneBundle {
    SceneBundle {
        name: "scene10_diamond_max_bounces_3",
        description: "Diamond with Max Bounces 3 (Forces Survivors)",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Diamond").expect("Diamond is a built-in material"),
        max_bounces: 3,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Studio(LightingPreset::Daylight),
    }
}

/// Scene 11: Ruby with scattering under the synthetic HDR map. HDR turns next-event
/// estimation on, so this is the only scene that exercises the scattering NEE deposit
/// and its MIS carry (Studio scenes have NEE off).
#[must_use]
pub fn scene_11_scattering_hdr() -> SceneBundle {
    SceneBundle {
        name: "scene11_ruby_scattering_hdr",
        description: "Ruby with Scattering under Synthetic HDR Map (sigma_s = 1.0, g = 0.3)",
        camera: default_camera(),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        material: GemMaterial::by_name("Ruby")
            .expect("Ruby is a built-in material")
            .with_scattering(1.0, 0.3),
        max_bounces: DEFAULT_MAX_BOUNCES,
        spp: DEFAULT_SPP,
        sample_offset: DEFAULT_SAMPLE_OFFSET,
        environment: SceneEnvironment::Hdr(synthetic_harness_hdr_map()),
    }
}

/// Returns the fixed list of all available scenes (scenes 1-8, scene 9 if enabled,
/// scene 10 and scene 11): 11 scenes with the `zoning` feature, 10 without.
#[must_use]
pub fn all_scenes() -> Vec<SceneBundle> {
    let mut scenes = Vec::with_capacity(11);
    scenes.push(scene_01_diamond_daylight());
    scenes.push(scene_02_spinel_preset4());
    scenes.push(scene_03_zircon());
    scenes.push(scene_04_biaxial());
    scenes.push(scene_05_scattering());
    scenes.push(scene_06_spinel_frosted_girdle());
    scenes.push(scene_07_diamond_hdr());
    scenes.push(scene_08_moissanite_dispersion());
    if let Some(scene_09) = scene_09_zoned() {
        scenes.push(scene_09);
    }
    scenes.push(scene_10_diamond_max_bounces_3());
    scenes.push(scene_11_scattering_hdr());
    scenes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_names_are_unique_and_count_matches_the_documentation() {
        let scenes = all_scenes();
        let expected = if cfg!(feature = "zoning") { 11 } else { 10 };
        assert_eq!(scenes.len(), expected);
        let mut names: Vec<_> = scenes.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), scenes.len(), "scene names must be unique");
    }

    #[test]
    fn hdr_scattering_scene_really_combines_hdr_and_scattering() {
        let s = scene_11_scattering_hdr();
        assert!(matches!(s.environment, SceneEnvironment::Hdr(_)));
        assert!(s.material.scattering_sigma_s > 0.0);
    }

    #[test]
    fn event_comparison_covers_frosted_and_uniaxial_scenes() {
        let scenes = all_scenes();
        assert!(scenes.iter().any(|s| !s.facet_finishes.is_empty()));
        assert!(scenes.iter().any(|s| s.name == "scene03_zircon"));
    }
}
