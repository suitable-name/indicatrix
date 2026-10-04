//! Building the `indicatrix_net::SceneState` a remote worker needs from an export's own
//! scene snapshot.

use crate::bridge::export_thread::scene_snapshot::SceneSnapshot;
use indicatrix_net::SceneState;

/// Builds the `indicatrix_net::SceneState` a remote worker needs from an export's own
/// [`SceneSnapshot`] plus its `width x height` -- the export-side equivalent of
/// `gui::remote::orchestrator::tick::scene_state_from_snapshot`, duplicated since that
/// function is private to a different module tree.
///
/// `cam_yaw`/`cam_pitch` are threaded in explicitly rather than read off
/// `snapshot.yaw`/`snapshot.pitch` so a caller sweeping the camera across many renders
/// of the SAME static snapshot (the tilt performance video, one frame per swept angle)
/// can pass that frame's own pose -- the still-image export passes `snapshot.yaw`/
/// `snapshot.pitch` unchanged, reproducing today's behaviour exactly. This is the ONE
/// place the remote `SceneState`'s pose comes from, so it can never drift from the
/// `Camera` the local CPU/GPU engines trace with for the same frame.
#[must_use]
pub(in crate::bridge::export_thread) fn scene_state_from_snapshot(
    snapshot: &SceneSnapshot,
    width: u32,
    height: u32,
    cam_yaw: f32,
    cam_pitch: f32,
) -> SceneState {
    SceneState {
        width,
        height,
        yaw: cam_yaw,
        pitch: cam_pitch,
        distance: snapshot.distance,
        light_yaw: snapshot.light_yaw,
        light_pitch: snapshot.light_pitch,
        exposure: snapshot.exposure,
        max_bounces: snapshot.max_bounces,
        lighting_preset: snapshot.lighting_preset,
        material: snapshot.material.clone(),
        planes: snapshot.active_planes.clone(),
        girdle_frosted: !snapshot.facet_finishes.is_empty(),
        backdrop: snapshot.backdrop,
        surface_glare: snapshot.surface_glare,
        // The concave tools, so the worker traces the same stone as the local tracer; empty
        // (and so absent from the wire's meaning) for a planar design.
        tools: snapshot.tools.clone(),
        // The loaded HDR map by content hash, else the studio rig.
        environment: crate::bridge::remote::hdr_asset::scene_environment(snapshot.env_map.as_ref()),
        // The material's emitters, so the worker traces the same glow as the local tracer.
        fluorescence: snapshot.fluorescence.as_ref().clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use indicatrix::geometry::tool::ToolPrimitive;
    use std::sync::{Arc, Mutex};

    fn state_for(tools: Vec<ToolPrimitive>) -> SceneState {
        let ctx = Mutex::new(RenderContext {
            active_tools: Arc::new(tools),
            ..RenderContext::default()
        });
        let snapshot = SceneSnapshot::capture(&ctx).expect("the default material resolves");
        scene_state_from_snapshot(&snapshot, 32, 24, snapshot.yaw, snapshot.pitch)
    }

    #[test]
    fn a_remote_scene_carries_the_designs_concave_tools_so_the_worker_traces_the_same_stone() {
        let tools = vec![ToolPrimitive::ball(glam::Vec3::new(0.0, 0.3, 0.0), 0.1)];
        assert_eq!(state_for(tools.clone()).tools, tools);
    }

    #[test]
    fn a_planar_remote_scene_sends_no_tools() {
        assert!(state_for(Vec::new()).tools.is_empty());
    }
}
