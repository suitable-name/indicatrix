//! The render scene for the app as it stands: the solved planes, the material, the
//! lighting, the camera of the Solid view and the HDR map, in the shape the shared
//! `indicatrix_web_core` scene builder takes.
//!
//! The scene is assembled exactly like the web app's live view (`render_material`,
//! `scene_spec`), with the phone's settings (`design_props::render_settings`) and the
//! Solid view's orbit camera.

use std::sync::Arc;

use indicatrix::geometry::tool::ToolPrimitive;
use indicatrix::renderer::env_map::EnvironmentMap;
use indicatrix_web_core::{
    scene::{CameraSpec, CustomMaterialSpec, SceneSpec},
    settings::{SceneInputs, render_material, scene_spec_with_frosting},
};

use super::RenderView;
use crate::{app_state::AppState, design_props::render_settings, layout::WindowClass};

/// The id the scene names its HDR map with; the pool holds at most one.
const HDR_ID: u64 = 1;

/// What the user is told when the design has no current solve.
pub(super) const NOT_SOLVED: &str = "Solve the design to render it";

/// Everything one render epoch depends on. Two keys that compare equal trace the same
/// picture, so an unchanged key needs no restart.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SceneKey {
    pub(super) engine: indicatrix::renderer::engine::EngineSpec,
    pub(super) document: u64,
    pub(super) generation: u64,
    pub(super) backend: crate::settings::RenderBackend,
    pub(super) remote: Option<crate::settings::RemoteSettings>,
    /// The scene description (planes, material, camera, lighting, size).
    pub(super) spec: SceneSpec,
    /// Exact resolved subtracted tool volumes; changing any tool restarts tracing.
    pub(super) tools: Vec<ToolPrimitive>,
    /// The traced material's physics recipe when it is a custom material that has one: the
    /// scene builds its fluorescence from it (`GeometrySceneSpec::build_with_customs`). Empty
    /// for a catalogue material. Part of the key so a recipe edit restarts the trace.
    pub(super) customs: Vec<CustomMaterialSpec>,
    /// Which HDR map lights the scene (by identity), if one does.
    pub(super) hdr: Option<usize>,
    /// The sample target of the epoch.
    pub(super) target: u32,
}

/// A scene ready to be built, with the HDR map it shares.
pub(super) struct Prepared {
    /// The description and what it depends on.
    pub(super) key: SceneKey,
    /// The HDR map for `key.spec.hdr_id`.
    pub(super) hdr: Option<Arc<EnvironmentMap>>,
}

/// Describes the scene of `state` at `view`'s size.
///
/// # Errors
///
/// A sentence for the user when there is nothing to render yet: the design has no current
/// solve, the solve failed, the solved design has no facets, or the render material does
/// not resolve.
pub(super) fn prepare(state: &AppState, view: RenderView) -> Result<Prepared, String> {
    let generation = state.session.current_generation();
    let Some(planes) = state.solve.current_planes(generation) else {
        return Err(state.solve.failure(generation).map_or_else(
            || NOT_SOLVED.to_owned(),
            |message| {
                format!("The design does not solve, so there is nothing to render: {message}")
            },
        ));
    };
    if planes.is_empty() {
        return Err("Nothing to render: the design has no facets yet.".to_owned());
    }
    let settings = render_settings(state, state.window_class == WindowClass::Compact);
    let tiers = state
        .solve
        .current_tiers(generation)
        .ok_or_else(|| NOT_SOLVED.to_owned())?;
    let (tools, _) = state
        .session
        .design
        .concave_tools_from_solved(tiers)
        .map_err(|error| format!("Cannot render concave geometry: {error}"))?;
    let custom = &state.props.custom_materials;
    let material = render_material(&settings, Some(&state.session.design), custom)?;
    let hdr = state.hdr.active();
    // The planes the design frosts (empty for a design without frosting, which is then the
    // scene it always was).
    let frosted = state.session.design.frosted_planes_from_solved(tiers);
    let mut spec = scene_spec_with_frosting(
        &settings,
        &SceneInputs {
            planes,
            material: &material,
            custom_materials: custom,
            width: view.width,
            height: view.height,
            hdr_id: hdr.as_ref().map(|_| HDR_ID),
        },
        &frosted,
    );
    // The render shares the Solid view's orbit, unclamped by the web settings' ranges.
    let pose = state.viewport.camera;
    spec.camera = CameraSpec {
        yaw: pose.yaw,
        pitch: pose.pitch,
        distance: pose.distance,
    };
    // Only the traced material can fluoresce, so only its recipe is carried.
    let customs: Vec<CustomMaterialSpec> = state
        .props
        .snapshot_for(&material.name)
        .map(|snapshot| CustomMaterialSpec::from_snapshot(material.name.clone(), snapshot))
        .into_iter()
        .collect();
    let key = SceneKey {
        engine: state.settings.render.engine_spec(),
        document: state.operations.document_id(),
        generation,
        backend: state.settings.render.backend,
        remote: (state.settings.render.backend == crate::settings::RenderBackend::Remote)
            .then(|| state.settings.remote.clone()),
        spec,
        tools,
        customs,
        hdr: hdr.as_ref().map(|map| Arc::as_ptr(map).addr()),
        target: super::live_target_spp(state),
    };
    Ok(Prepared { key, hdr })
}
