//! [`SceneState`]: the fully-resolved description of one frame that a remote render
//! worker needs to reproduce it -- and nothing else.
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
}
