//! [`SceneIdentity`]: a content-derived scene generation counter for `RenderContext`.
//!
//! Every backend contributing to one live image must render the IDENTICAL scene
//! (invariant 2 of the hybrid guide). A settle's remote epoch is dispatched from a
//! scene captured on the UI thread, while the render thread reads the scene again on
//! its next frame; a scene change landing in between must never be merged into that
//! epoch. `RenderContext::scene_generation` turns "the scene the render thread / the
//! dispatch is looking at right now" into a number: it bumps whenever any
//! scene-defining field differs from the last time it was asked, so two readers that
//! get the same generation saw the same scene. The live epoch carries the generation
//! it was dispatched for; the render loop only claims from and merges with an epoch
//! whose generation matches the scene it is about to trace.
//!
//! No call site has to remember to bump anything: the key compares field CONTENT (and
//! `Arc` identity for the large shared buffers), so every writer of a scene field is
//! covered automatically.

use super::RenderContext;
use crate::settings::model::Backdrop;
use glam::Vec3;
use indicatrix::{
    geometry::plane::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
    renderer::env_map::EnvironmentMap,
};
use std::sync::Arc;

/// Everything a remote `SceneState` (and the local tracer) is built from, captured for
/// comparison. Floats are compared by bit pattern so a `NaN` can never make a key
/// unequal to itself; the big shared buffers by `Arc` identity (a writer always
/// replaces the `Arc` when their content changes).
struct SceneKey {
    dims: [u32; 4],
    floats: [u32; 9],
    c_axis: Option<[u32; 3]>,
    girdle_frosted: bool,
    lighting_preset: LightingPreset,
    backdrop: Backdrop,
    material_name: String,
    material_override: Option<GemMaterial>,
    active_planes: Arc<Vec<GpuFacetPlane>>,
    custom_materials: Arc<Vec<GemMaterial>>,
    env_map: Option<Arc<EnvironmentMap>>,
}

const fn dims(ctx: &RenderContext) -> [u32; 4] {
    [ctx.width, ctx.height, ctx.target_samples, ctx.max_bounces]
}

const fn floats(ctx: &RenderContext) -> [u32; 9] {
    [
        ctx.yaw.to_bits(),
        ctx.pitch.to_bits(),
        ctx.distance.to_bits(),
        ctx.light_yaw.to_bits(),
        ctx.light_pitch.to_bits(),
        ctx.exposure.to_bits(),
        ctx.inclusion_sigma_s.to_bits(),
        ctx.edge_rounding_radius.to_bits(),
        ctx.stone_width_mm.to_bits(),
    ]
}

fn c_axis(ctx: &RenderContext) -> Option<[u32; 3]> {
    ctx.c_axis_override
        .map(|v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
}

fn same_env(a: Option<&Arc<EnvironmentMap>>, b: Option<&Arc<EnvironmentMap>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

impl SceneKey {
    fn of(ctx: &RenderContext) -> Self {
        Self {
            dims: dims(ctx),
            floats: floats(ctx),
            c_axis: c_axis(ctx),
            girdle_frosted: ctx.girdle_frosted,
            lighting_preset: ctx.lighting_preset,
            backdrop: ctx.backdrop,
            material_name: ctx.material_name.clone(),
            material_override: ctx.material_override.clone(),
            active_planes: Arc::clone(&ctx.active_planes),
            custom_materials: Arc::clone(&ctx.custom_materials),
            env_map: ctx.env_map.clone(),
        }
    }

    /// Whether `ctx`'s scene still equals this key -- compared in place, with no
    /// allocation, since the render loop asks every frame.
    fn matches(&self, ctx: &RenderContext) -> bool {
        self.dims == dims(ctx)
            && self.floats == floats(ctx)
            && self.c_axis == c_axis(ctx)
            && self.girdle_frosted == ctx.girdle_frosted
            && self.lighting_preset == ctx.lighting_preset
            && self.backdrop == ctx.backdrop
            && self.material_name == ctx.material_name
            && self.material_override == ctx.material_override
            && Arc::ptr_eq(&self.active_planes, &ctx.active_planes)
            && Arc::ptr_eq(&self.custom_materials, &ctx.custom_materials)
            && same_env(self.env_map.as_ref(), ctx.env_map.as_ref())
    }
}

/// The last scene key `RenderContext::scene_generation` saw and its generation number
/// -- see the module doc comment. `Default` starts at generation 0 with no key, so the
/// very first query always bumps to 1.
#[derive(Default)]
pub struct SceneIdentity {
    key: Option<SceneKey>,
    generation: u64,
}

impl RenderContext {
    /// The current scene's generation: unchanged while every scene-defining field is
    /// unchanged since the previous call, bumped otherwise. Called under the context
    /// lock by the render loop (every frame, via `snapshot_frame_inputs`), by the
    /// remote dispatch (stamped into the live epoch) and by the orchestrator's poll
    /// tick (scene-stability debounce for re-dispatch).
    pub fn scene_generation(&mut self) -> u64 {
        let unchanged = self
            .scene_identity
            .key
            .as_ref()
            .is_some_and(|key| key.matches(self));
        if !unchanged {
            self.scene_identity.key = Some(SceneKey::of(self));
            self.scene_identity.generation += 1;
        }
        self.scene_identity.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_scene_keeps_its_generation() {
        let mut ctx = RenderContext::default();
        let first = ctx.scene_generation();
        assert_eq!(ctx.scene_generation(), first);
        // Non-scene fields (visibility, pause, compute targets) never bump it.
        ctx.tab_visible = false;
        ctx.paused = true;
        ctx.dirty = true;
        assert_eq!(ctx.scene_generation(), first);
    }

    #[test]
    fn every_kind_of_scene_change_bumps_the_generation() {
        let mut ctx = RenderContext::default();
        let mut last = ctx.scene_generation();
        let changes: [fn(&mut RenderContext); 8] = [
            |c| c.yaw += 0.1,
            |c| c.exposure = 2.0,
            |c| c.material_name = "Spinel".to_string(),
            |c| c.material_override = Some(GemMaterial::diamond()),
            |c| c.active_planes = Arc::new(c.active_planes.as_ref().clone()),
            |c| c.env_map = Some(Arc::new(EnvironmentMap::uniform(2, 2, [1.0, 1.0, 1.0]))),
            |c| c.girdle_frosted = true,
            |c| c.width = 640,
        ];
        for change in changes {
            change(&mut ctx);
            let now = ctx.scene_generation();
            assert!(now > last, "a scene change must bump the generation");
            last = now;
        }
    }
}
