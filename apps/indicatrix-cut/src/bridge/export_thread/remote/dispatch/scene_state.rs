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
        // The loaded HDR map by content hash, else the studio rig.
        environment: crate::bridge::remote::hdr_asset::scene_environment(snapshot.env_map.as_ref()),
    }
}
