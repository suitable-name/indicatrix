//! The render scene for the app as it stands: the solved planes, the material, the
//! lighting, the camera of the Solid view and the HDR map, in the shape the shared
//! `indicatrix_web_core` scene builder takes.
//!
//! The scene is assembled exactly like the web app's live view (`render_material`,
//! `scene_spec`), with the phone's settings (`design_props::render_settings`) and the
//! Solid view's orbit camera.

use std::sync::Arc;

use indicatrix::renderer::env_map::EnvironmentMap;
use indicatrix_web_core::{
    scene::{CameraSpec, SceneSpec},
    settings::{SceneInputs, render_material, scene_spec},
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
    /// The scene description (planes, material, camera, lighting, size).
    pub(super) spec: SceneSpec,
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
    let custom = &state.props.custom_materials;
    let material = render_material(&settings, Some(&state.session.design), custom)?;
    let hdr = state.hdr.active();
    let mut spec = scene_spec(
        &settings,
        &SceneInputs {
            planes,
            material: &material,
            custom_materials: custom,
            width: view.width,
            height: view.height,
            hdr_id: hdr.as_ref().map(|_| HDR_ID),
        },
    );
    // The render shares the Solid view's orbit, unclamped by the web settings' ranges.
    let pose = state.viewport.camera;
    spec.camera = CameraSpec {
        yaw: pose.yaw,
        pitch: pose.pitch,
        distance: pose.distance,
    };
    let key = SceneKey {
        spec,
        hdr: hdr.as_ref().map(|map| Arc::as_ptr(map).addr()),
        target: super::live_target_spp(state),
    };
    Ok(Prepared { key, hdr })
}
