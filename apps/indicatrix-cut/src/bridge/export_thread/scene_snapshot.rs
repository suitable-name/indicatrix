//! [`SceneSnapshot`]: a read-only capture of everything a render needs, taken out of
//! the live `RenderContext` under one short lock.
//!
//! Split out of `bridge::export_thread` purely to keep that module (already sizeable)
//! from growing further.

use crate::bridge::{
    frame_cache::stone_width::StoneWidthCache,
    render_thread::{
        MaterialOverrides, RenderContext, apply_material_overrides, resolve_material_with_override,
    },
};
use indicatrix::{
    geometry::{girdle_facet_finishes, plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{FacetFinish, LightingPreset},
    },
    renderer::env_map::EnvironmentMap,
};
use std::sync::{Arc, Mutex};

/// A read-only snapshot of everything a render needs, captured out of the live
/// `RenderContext` under one short lock. Deliberately excludes `width`/`height` and
/// the accumulation buffer -- those belong solely to the interactive viewport; the
/// export worker sizes its own buffer from the user's requested export dimensions.
///
/// `Clone`: a preset-fan-out export clones the current-view capture once per selected
/// preset and overlays that preset's own light/camera/env-map fields on top
/// (`gui::render_export::apply_preset_to_scene`) rather than re-capturing the live
/// viewport per render -- every fanned-out render must share the exact same material,
/// geometry, and bounce cap, which only holds if they all descend from ONE capture.
#[derive(Clone)]
pub struct SceneSnapshot {
    /// Camera yaw.
    pub yaw: f32,
    /// Camera pitch.
    pub pitch: f32,
    /// Camera distance from the stone.
    pub distance: f32,
    /// Light yaw.
    pub light_yaw: f32,
    /// Light pitch.
    pub light_pitch: f32,
    /// Gem material the stone is rendered with.
    pub material: GemMaterial,
    /// Lighting preset the render uses.
    pub lighting_preset: LightingPreset,
    /// Maximum number of ray bounces per path.
    pub max_bounces: u32,
    /// Exposure multiplier applied when tone-mapping.
    pub exposure: f32,
    /// Backdrop radiance -- `RenderContext::backdrop` resolved through `Backdrop::level`.
    pub backdrop: f32,
    /// Surface-glare scale (`RenderContext::surface_glare`, `0.0..=1.0`): exports follow
    /// the viewport's value the way they follow its lighting preset. Analytic presets
    /// only; an HDR map ignores it.
    pub surface_glare: f32,
    /// Facet planes of the active design.
    pub active_planes: Vec<GpuFacetPlane>,
    /// The concave tools cut out of `active_planes` (`RenderContext::active_tools`);
    /// empty for a planar design, which is then exported exactly as before. A tool-bearing
    /// scene is traced on the CPU alone (`scene_routes_to_gpu`) and sent to a remote
    /// worker as `SceneState::tools`.
    pub tools: Vec<ToolPrimitive>,
    /// The material's fluorescent emitters (`RenderContext::active_fluorescence`, resolved
    /// from the physics recipe), empty for every non-fluorescent material, which then exports
    /// exactly as before. A fluorescent scene is traced on the CPU alone and sent to a remote
    /// worker as `SceneState::fluorescence`. An `Arc` so a fan-out's clones share the sampling
    /// tables the first trace builds.
    pub fluorescence: Arc<Fluorescence>,
    /// Frosted girdle: `girdle_facet_finishes(&active_planes)` when
    /// `RenderContext::girdle_frosted` was on at capture time, empty otherwise --
    /// already resolved here (rather than a bare `bool` re-classified per batch) since
    /// `active_planes` never changes mid-export.
    pub facet_finishes: Vec<FacetFinish>,
    /// A loaded HDR environment map, captured from `RenderContext::env_map` exactly
    /// like the live viewport reads it. `None` means the export uses the analytic
    /// studio rig. `run_export`'s `environment`
    /// binding reads this via the same `as_deref().map_or_else(studio,
    /// EnvironmentSource::HdrMap)` `render_thread::mod` uses, so an export renders an
    /// HDR map exactly like the live viewport does: the GPU megakernel has its own
    /// `env_mode` for `HdrMap` and renders it directly, falling back to
    /// the CPU tracer only on the same generic per-frame decline every other scene
    /// gets, not an HDR-specific one.
    ///
    /// `Arc`, not a bare `EnvironmentMap`: a decoded panorama can be tens of
    /// megabytes, and this snapshot is cloned once per fanned-out preset render.
    pub env_map: Option<Arc<EnvironmentMap>>,
}

impl SceneSnapshot {
    /// # Errors
    ///
    /// Refuses (returning the cutter-facing reason) rather than capturing a scene
    /// that would trace as the wrong stone: when `RenderContext::material_unresolved`
    /// is already set, or when -- despite that check -- neither `material_override`
    /// nor `material_name` resolves to a real material. Every caller must handle
    /// this by aborting the export/dispatch/tilt-video render, not by substituting
    /// a default material (see `resolve_material`'s own doc comment for why).
    pub fn capture(ctx: &Mutex<RenderContext>) -> Result<Self, String> {
        let guard = ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(reason) = &guard.material_unresolved {
            return Err(reason.clone());
        }
        let materials = GemMaterial::all_materials();
        // A high-resolution export must honour the same RI/custom
        // material override the live viewport, tilt sweep and hover preview already
        // resolve through -- otherwise the export silently reverts to the by-name
        // lookup for the one surface that matters most (the delivered image).
        let Some(material) = resolve_material_with_override(
            &materials,
            &guard.custom_materials,
            guard.material_override.as_ref(),
            &guard.material_name,
        ) else {
            return Err(format!(
                "'{}' is not a built-in preset or a saved custom material -- nothing to \
                 export.",
                guard.material_name
            ));
        };
        // Every one of these sliders/toggles is a property of what the user is looking
        // at, so an export has to carry it or the file silently differs from the
        // viewport. Applied here (not inside `resolve_material`, which `render_thread`
        // shares) via the same `MaterialOverrides`/`apply_material_overrides` path, so
        // an export with nothing dialled in stays bit-identical to a plain by-name
        // resolve. A fresh `StoneWidthCache` since this runs once per export,
        // not once per frame like the live loop's persistent cache.
        let material = apply_material_overrides(
            material,
            &MaterialOverrides {
                inclusion_sigma_s: guard.inclusion_sigma_s,
                c_axis_override: guard.c_axis_override,
                edge_rounding_radius: guard.edge_rounding_radius,
                stone_width_mm: guard.stone_width_mm,
            },
            &guard.active_planes,
            &mut StoneWidthCache::new(),
            guard.physics_color(),
        );
        // Frosted girdle: an empty `Vec` at the off position is
        // `trace_spectral_ray_with_finish`'s documented equivalent of
        // `trace_spectral_ray` (every facet reads `FacetFinish::default() == Polished`).
        let facet_finishes = if guard.girdle_frosted {
            girdle_facet_finishes(&guard.active_planes)
        } else {
            Vec::new()
        };
        Ok(Self {
            yaw: guard.yaw,
            pitch: guard.pitch,
            distance: guard.distance,
            light_yaw: guard.light_yaw,
            light_pitch: guard.light_pitch,
            material,
            lighting_preset: guard.lighting_preset,
            max_bounces: guard.max_bounces,
            exposure: guard.exposure,
            backdrop: guard.backdrop.level(),
            surface_glare: guard.surface_glare,
            // `SceneSnapshot::active_planes` is a plain `Vec` (a one-shot export
            // capture, not `RenderContext`'s hot-path per-frame snapshot), so this is
            // the one actual deep copy `capture` makes -- `.to_vec()` off the `Arc<Vec<..>>`
            // (via its `Deref<Target = [GpuFacetPlane]>`).
            active_planes: guard.active_planes.to_vec(),
            tools: guard.active_tools.to_vec(),
            fluorescence: guard.active_fluorescence().unwrap_or_default(),
            facet_finishes,
            // `Arc::clone`, not a deep copy of the decoded panorama.
            env_map: guard.env_map.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// A high-resolution export must resolve the design's real
    /// effective material (RI override / unlisted custom material) exactly as the
    /// live viewport, tilt sweep and hover preview do, rather than falling back to
    /// a plain by-name lookup that cannot represent an override. Guards
    /// `SceneSnapshot::capture` against regressing to a bare `resolve_material` call.
    #[test]
    fn capture_prefers_the_material_override_over_the_by_name_lookup() {
        let override_dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
            a: 1.62,
            b: 0.0,
            c: 0.0,
        };
        let mut overridden = GemMaterial::diamond();
        overridden.name = "Custom RI 1.62".to_string();
        overridden.dispersion = override_dispersion;

        let with_override = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Diamond".to_string(),
            material_override: Some(overridden),
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert_eq!(
            with_override.material.dispersion, override_dispersion,
            "the override's flattened dispersion (its RI) must reach the exported \
             scene, not the by-name material's own dispersion curve"
        );

        let without_override = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Diamond".to_string(),
            material_override: None,
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert_eq!(
            without_override.material.dispersion,
            GemMaterial::diamond().dispersion,
            "with no override, the export keeps resolving by name exactly as before"
        );
    }

    /// The inclusion slider is a property of what the user is looking at, so an
    /// export must carry it. Guards `SceneSnapshot::capture`'s override against the
    /// regression of "simplifying" it back into a bare `resolve_material` call.
    #[test]
    fn capture_carries_the_inclusion_setting_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            inclusion_sigma_s: 0.0,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            off.material.scattering_sigma_s, 0.0,
            "the off position must leave the material untouched"
        );

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            inclusion_sigma_s: 1.25,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            on.material.scattering_sigma_s, 1.25,
            "a dialled-in inclusion amount must reach the exported scene"
        );
        assert_eq!(
            on.material.scattering_g,
            GemMaterial::DEFAULT_SCATTERING_G,
            "anisotropy comes from the crate's default, matching the live path"
        );
    }

    /// Crystal-axis override must reach the export, but must leave an isotropic
    /// material's `c_axis` alone even when the override is on
    /// (`RenderContext::default().material_name` is "Diamond", isotropic).
    #[test]
    fn capture_carries_the_c_axis_override_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            c_axis_override: None,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            off.material.c_axis,
            GemMaterial::diamond().c_axis,
            "the off (\"as cut\") position must leave the material's own c_axis untouched"
        );

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Sapphire".to_string(),
            c_axis_override: Some(Vec3::X),
            ..Default::default()
        }))
        .expect("Sapphire resolves");
        assert_eq!(
            on.material.c_axis,
            Vec3::X,
            "a dialled-in override on an anisotropic material must reach the exported scene"
        );

        let isotropic_guarded = SceneSnapshot::capture(&Mutex::new(RenderContext {
            material_name: "Diamond".to_string(),
            c_axis_override: Some(Vec3::X),
            ..Default::default()
        }))
        .expect("Diamond resolves");
        assert_eq!(
            isotropic_guarded.material.c_axis,
            GemMaterial::diamond().c_axis,
            "an override dialled in for an isotropic material must be ignored, matching \
             apply_material_overrides's own guard"
        );
    }

    /// Edge-rounding's own seam guard, same shape as the inclusion test above.
    #[test]
    fn capture_carries_the_edge_rounding_setting_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            edge_rounding_radius: 0.0,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            off.material.edge_rounding_radius, 0.0,
            "the off position must leave the material untouched"
        );

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            edge_rounding_radius: 0.02,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            on.material.edge_rounding_radius, 0.02,
            "a dialled-in edge-rounding radius must reach the exported scene"
        );
    }

    /// The off position must leave `absorption_path_scale` at the base material's
    /// default (`1.0`), and a dialled-in width must scale it by the ratio to the
    /// design's measured model-unit girdle width, matching
    /// `apply_material_overrides`'s computation exactly.
    #[test]
    fn capture_carries_the_stone_width_setting_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            stone_width_mm: 0.0,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            off.material.absorption_path_scale, 1.0,
            "the off position must leave the material's absorption_path_scale untouched"
        );

        let default_ctx = RenderContext::default();
        let model_width = indicatrix::geometry::stone_metrics::measure_solid(
            &default_ctx
                .active_planes
                .iter()
                .map(|p| {
                    (
                        glam::DVec3::new(
                            f64::from(p.normal[0]),
                            f64::from(p.normal[1]),
                            f64::from(p.normal[2]),
                        ),
                        -f64::from(p.d),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .expect("default active_planes must measure")
        .width_axis;

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            stone_width_mm: 6.5,
            ..Default::default()
        }))
        .expect("default resolves");
        let expected_scale = (6.5 / model_width) as f32;
        assert!(
            (on.material.absorption_path_scale - expected_scale).abs() < 1e-4,
            "a dialled-in stone width must reach the exported scene as the expected \
             absorption_path_scale: got {}, expected {expected_scale}",
            on.material.absorption_path_scale
        );
    }

    /// The viewport's surface glare reaches the export snapshot, and the default
    /// (`1.0`) is what an untouched context captures.
    #[test]
    fn capture_carries_the_surface_glare_into_the_exported_scene() {
        let default = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("default resolves");
        assert_eq!(default.surface_glare.to_bits(), 1.0f32.to_bits());

        let dimmed = SceneSnapshot::capture(&Mutex::new(RenderContext {
            surface_glare: 0.25,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(dimmed.surface_glare.to_bits(), 0.25f32.to_bits());
    }

    /// The girdle-frosted toggle is captured as a resolved per-facet finish list, not
    /// a bare `bool`, so `run_export`/`render_batch` need no further classification.
    #[test]
    fn capture_carries_the_girdle_frosted_setting_into_the_exported_scene() {
        let off = SceneSnapshot::capture(&Mutex::new(RenderContext {
            girdle_frosted: false,
            ..Default::default()
        }))
        .expect("default resolves");
        assert!(
            off.facet_finishes.is_empty(),
            "the off position must carry no per-facet finish data"
        );

        let on = SceneSnapshot::capture(&Mutex::new(RenderContext {
            girdle_frosted: true,
            ..Default::default()
        }))
        .expect("default resolves");
        assert_eq!(
            on.facet_finishes,
            girdle_facet_finishes(&RenderContext::default().active_planes),
            "the on position must carry the same classification the live viewport uses"
        );
    }
}
