//! [`SceneState`]: the fully-resolved description of one frame.
//!
//! Everything a remote render worker needs to reproduce it -- and nothing else.
//!
//! The viewer stores a gem material as `material_name: String` and a diagram as an id
//! looked up against `indicatrix-vault`, but a remote worker has neither the database
//! connection nor the catalog: sending the name/id instead of the resolved value would
//! compile fine, then silently render the wrong stone if the worker's copy of the data
//! disagrees. So [`SceneState`] carries the resolved [`GemMaterial`] and facet-plane
//! geometry directly, never a name or id.
//!
//! Deliberately excludes local UI/session bookkeeping (`dirty`/`running`/`paused`/
//! `tab_visible`/`quality_preset`): none of it changes what a worker computes -- the
//! viewer already resolves `quality_preset` to a concrete sample count before asking for
//! work (the `samples` field on the `RENDER` message in [`crate::messages`]).

use indicatrix::{
    geometry::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};

/// Everything a remote render worker needs to trace samples for one frame, fully
/// resolved -- see the module docs for why every field is a value, never a name or id.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SceneState {
    /// Output image width, in pixels.
    pub width: u32,
    /// Output image height, in pixels.
    pub height: u32,
    /// Camera orbit yaw, radians.
    pub yaw: f32,
    /// Camera orbit pitch, radians.
    pub pitch: f32,
    /// Camera orbit distance from the origin.
    pub distance: f32,
    /// Key light yaw, radians -- see `indicatrix::optics::studio_rig::StudioRig`.
    pub light_yaw: f32,
    /// Key light pitch, radians.
    pub light_pitch: f32,
    /// Tone-mapping exposure multiplier.
    pub exposure: f32,
    /// Maximum ray bounce depth.
    pub max_bounces: u32,
    /// Which analytic studio lighting rig to sample when a ray misses the gem.
    pub lighting_preset: LightingPreset,
    /// The fully-resolved gem material -- never a `material_name`. See the module docs.
    pub material: GemMaterial,
    /// The fully-resolved facet-plane geometry -- never a diagram id. See the module
    /// docs.
    pub planes: Vec<GpuFacetPlane>,
    /// Whether the girdle band renders with a frosted (diffusely-scattering) finish
    /// rather than the default polished one.
    ///
    /// A single `bool`, not a `Vec<optics::raytracer::FacetFinish>` parallel to
    /// `planes`: `indicatrix::geometry::girdle_facet_finishes` is a pure function of
    /// `planes` alone, so a worker re-derives the per-facet finish list from `planes`
    /// plus this one bit. `#[serde(default)]` so an on-disk `scene.json` predating this
    /// field still deserializes, defaulting to `false` (all-polished).
    #[serde(default)]
    pub girdle_frosted: bool,
    /// Radiance of the backdrop card the camera sees where it misses the stone, `0.0`
    /// for none -- `EnvironmentSource::Studio::backdrop`. `#[serde(default)]` for the
    /// same on-disk `scene.json` reason as `girdle_frosted`.
    #[serde(default)]
    pub backdrop: f32,
    /// What lights the stone where a ray misses it (v14): the analytic studio rig
    /// described by the fields above, or a loaded HDR panorama named by content hash.
    /// `#[serde(default)]` (the studio rig) for the same on-disk `scene.json` reason as
    /// `girdle_frosted`.
    #[serde(default)]
    pub environment: SceneEnvironment,
    /// Scale of the first-surface specular reflection of the analytic presets (v18),
    /// `0.0..=1.0`; `1.0` is the unscaled render --
    /// `EnvironmentSource::Studio::surface_glare`. Ignored for an HDR environment.
    /// `#[serde(default = "default_surface_glare")]` for the same on-disk `scene.json`
    /// reason as `girdle_frosted`.
    #[serde(default = "default_surface_glare")]
    pub surface_glare: f32,
}

/// The unscaled surface-glare value (`1.0`), the serde default of
/// [`SceneState::surface_glare`].
#[must_use]
pub const fn default_surface_glare() -> f32 {
    1.0
}

impl SceneState {
    /// The HDR panorama this scene is lit by, if any.
    #[must_use]
    pub const fn hdr(&self) -> Option<&HdrEnvironment> {
        match &self.environment {
            SceneEnvironment::Studio => None,
            SceneEnvironment::Hdr(hdr) => Some(hdr),
        }
    }
}

/// A scene's environment (v14). Variant order is wire-load-bearing; append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SceneEnvironment {
    /// The analytic studio rig: [`SceneState::lighting_preset`] at
    /// [`SceneState::light_yaw`]/[`SceneState::light_pitch`] with
    /// [`SceneState::backdrop`].
    #[default]
    Studio,
    /// An equirectangular HDR panorama, lit exactly as the viewer lights it
    /// (`indicatrix::optics::raytracer::EnvironmentSource::HdrMap`): the lighting preset,
    /// light angles and backdrop do not apply.
    Hdr(HdrEnvironment),
}

/// An HDR panorama named by content, never by path: a worker that lacks the bytes asks
/// for them (`StreamEvent::NeedAsset`, see `crate::messages::asset`).
///
/// Carries no rotation or intensity: the renderer's `EnvironmentSource::HdrMap` applies
/// neither (the map is used as decoded, in its own orientation, and the scene's
/// [`SceneState::exposure`] is the only exposure). What the worker must reproduce bit for
/// bit is the decoded map itself, which `indicatrix::renderer::env_map::
/// environment_from_hdr_bytes` builds identically on every node from the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HdrEnvironment {
    /// SHA-256 of the exact `.hdr` file bytes (`crate::messages::content_hash`).
    pub content_hash: [u8; 32],
    /// The decoded map's width in texels -- lets a server refuse an over-limit map
    /// before asking for its bytes, and cross-check the decode.
    pub width: u32,
    /// The decoded map's height in texels.
    pub height: u32,
}
