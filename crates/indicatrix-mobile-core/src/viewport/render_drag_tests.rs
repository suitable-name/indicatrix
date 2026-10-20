use super::*;
use crate::viewport::{ViewMode, ViewportState};

const POSE: CameraPose = CameraPose {
    yaw: 0.5,
    pitch: 0.25,
    distance: 6.0,
};

fn at(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn nothing_owed_means_the_normal_check_applies() {
    let live = LiveDrag::default();
    assert_eq!(live.picture(), Picture::Trace);
    assert!(!live.solid() && !live.owes_restart());
    assert_eq!(live.gate(at(1_000), true), Gate::Idle);
    assert_eq!(live.gate(at(1_000), false), Gate::Idle);
}

#[test]
fn a_camera_move_shows_the_solid_raster_and_holds_while_a_finger_is_down() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.stamp(at(1_000));
    assert!(live.solid() && live.owes_restart());
    // However long the finger stays down, and however many moves arrive, nothing starts.
    for ms in (1_000..4_000).step_by(16) {
        live.camera_moved();
        live.stamp(at(ms));
        assert_eq!(live.gate(at(ms), true), Gate::Hold, "at {ms} ms");
    }
    assert!(live.solid());
}

#[test]
fn the_restart_waits_for_the_pose_to_rest_after_the_finger_lifts() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.stamp(at(2_000));
    // The finger lifts 30 ms after the last move: 90 ms of the settle time are left.
    assert_eq!(live.gate(at(2_030), false), Gate::Wait(at(90)));
    assert_eq!(live.gate(at(2_119), false), Gate::Wait(at(1)));
    assert_eq!(live.gate(at(2_120), false), Gate::Go);
    // A finger held still for a while and then lifted starts at once.
    assert_eq!(live.gate(at(9_000), false), Gate::Go);
}

#[test]
fn a_move_after_the_wait_began_pushes_the_restart_back() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.stamp(at(2_000));
    assert_eq!(live.gate(at(2_100), false), Gate::Wait(at(20)));
    // A pinch end followed by one more orbit event.
    live.camera_moved();
    live.stamp(at(2_110));
    assert_eq!(live.gate(at(2_110), false), Gate::Wait(SETTLE));
}

#[test]
fn a_move_is_timed_once_at_the_first_look() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.stamp(at(100));
    // Looking again later without a new move does not move the stamp.
    live.stamp(at(500));
    assert_eq!(live.gate(at(220), false), Gate::Go);
}

#[test]
fn the_solid_raster_stays_until_the_new_poses_first_traced_frame() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.stamp(at(0));
    live.started(7, POSE);
    assert!(live.solid(), "the start alone does not switch back");
    assert!(!live.owes_restart() && !live.is_stopped());
    // A frame of an older epoch does not switch.
    assert!(!live.frame_landed(6, POSE));
    assert!(live.solid());
    // The first frame of the new epoch does, once.
    assert!(live.frame_landed(7, POSE));
    assert_eq!(live.picture(), Picture::Trace);
    assert!(!live.frame_landed(7, POSE), "later frames change nothing");
}

#[test]
fn a_frame_that_lands_after_the_camera_moved_again_does_not_switch() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.started(3, POSE);
    // The finger is back down before epoch 3's first pass is ready.
    live.camera_moved();
    assert!(!live.frame_landed(3, POSE));
    assert!(live.solid());
    // Even if the pose came back, the owed trace has not started yet.
    live.stamp(at(10));
    live.started(4, POSE);
    assert!(
        !live.frame_landed(3, POSE),
        "epoch 3 is not the running one"
    );
    assert!(live.frame_landed(4, POSE));
}

#[test]
fn a_frame_for_another_pose_does_not_switch() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.started(5, POSE);
    let other = CameraPose {
        yaw: POSE.yaw + 0.1,
        ..POSE
    };
    assert!(!live.frame_landed(5, other));
    assert!(live.solid());
}

#[test]
fn a_light_move_keeps_the_traced_picture_and_holds_the_same_way() {
    let mut live = LiveDrag::default();
    live.light_moved();
    live.stamp(at(300));
    assert_eq!(
        live.picture(),
        Picture::Trace,
        "a raster cannot show a light"
    );
    assert!(live.owes_restart());
    assert_eq!(live.gate(at(320), true), Gate::Hold);
    assert_eq!(live.gate(at(330), false), Gate::Wait(at(90)));
    assert_eq!(live.gate(at(420), false), Gate::Go);
    live.started(2, POSE);
    assert_eq!(live.gate(at(430), false), Gate::Idle);
}

#[test]
fn a_cancelled_trace_is_cancelled_once_per_drag() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    assert!(!live.is_stopped());
    live.mark_stopped();
    assert!(live.is_stopped());
    // The next start re-arms it for the following drag.
    live.started(1, POSE);
    assert!(!live.is_stopped());
}

#[test]
fn a_failed_start_gives_the_picture_area_back_to_the_status_text() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.mark_stopped();
    live.abandoned();
    assert_eq!(live.picture(), Picture::Trace);
    assert!(!live.owes_restart() && !live.is_stopped());
    assert_eq!(live.gate(at(5_000), false), Gate::Idle);
}

#[test]
fn reset_forgets_the_drag() {
    let mut live = LiveDrag::default();
    live.camera_moved();
    live.mark_stopped();
    live.started(9, POSE);
    live.camera_moved();
    live.reset();
    assert_eq!(live, LiveDrag::default());
}

#[test]
fn the_viewport_marks_a_moved_camera_only_in_the_render_mode() {
    let mut solid = ViewportState::default();
    solid.orbit(40.0, 10.0);
    assert!(!solid.render_live.solid() && !solid.render_live.owes_restart());

    let mut render = ViewportState::default();
    assert!(render.set_mode(ViewMode::Render));
    render.orbit(40.0, 10.0);
    assert!(render.render_live.solid() && render.render_live.owes_restart());
}

#[test]
fn a_camera_call_that_changes_nothing_owes_nothing() {
    let mut render = ViewportState::default();
    render.set_mode(ViewMode::Render);
    // A non-finite zoom factor leaves the distance alone.
    render.zoom_camera(f32::NAN);
    assert!(!render.render_live.solid() && !render.render_live.owes_restart());
}

#[test]
fn camera_pills_and_zoom_count_as_moves_in_the_render_mode() {
    let mut render = ViewportState::default();
    render.set_mode(ViewMode::Render);
    render.set_standard_view(1);
    assert!(render.render_live.solid());

    let mut zoomed = ViewportState::default();
    zoomed.set_mode(ViewMode::Render);
    zoomed.zoom_camera(1.5);
    assert!(zoomed.render_live.owes_restart());
}

#[test]
fn leaving_or_entering_the_render_mode_clears_the_drag() {
    let mut render = ViewportState::default();
    render.set_mode(ViewMode::Render);
    render.orbit(40.0, 10.0);
    assert!(render.render_live.solid());
    render.set_mode(ViewMode::Solid);
    assert_eq!(render.render_live, LiveDrag::default());
}
