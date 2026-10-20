use super::camera_pose_for_hover;
use indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG;

/// Positive tilt stays on the axis's own (positive) azimuth, pitch equal to
/// `90 - tilt` (tilt is measured away from table-up, not camera elevation).
#[test]
fn positive_tilt_uses_the_axis_own_azimuth() {
    let (yaw, pitch) = camera_pose_for_hover(1, 30.0);
    assert!((yaw - PROFILE_AZIMUTHS_DEG[1].to_radians()).abs() < 1e-6);
    assert!((pitch - (90.0 - 30.0f32).to_radians()).abs() < 1e-6);
}

/// Negative tilt flips to the opposite (+180 deg) azimuth, pitch equal to
/// `90 - |tilt|`.
#[test]
fn negative_tilt_uses_the_opposite_azimuth_and_positive_pitch() {
    let (yaw, pitch) = camera_pose_for_hover(2, -25.0);
    let expected_yaw = (PROFILE_AZIMUTHS_DEG[2] + 180.0).to_radians();
    assert!((yaw - expected_yaw).abs() < 1e-6);
    assert!((pitch - (90.0 - 25.0f32).to_radians()).abs() < 1e-6);
}

/// The shared table-up point: `tilt_deg == 0` must resolve to pitch 90°
/// (table-up), on the axis's own positive azimuth.
#[test]
fn zero_tilt_is_table_up_on_the_positive_azimuth() {
    let (yaw, pitch) = camera_pose_for_hover(0, 0.0);
    assert!((yaw - PROFILE_AZIMUTHS_DEG[0].to_radians()).abs() < 1e-6);
    assert!((pitch - 90.0f32.to_radians()).abs() < 1e-6);
}

/// The two edge-on extremes: `tilt_deg == +-90` must resolve to pitch 0°
/// (edge-on/profile), on the positive/negative azimuth respectively.
#[test]
fn extreme_tilt_is_edge_on() {
    let (positive_yaw, positive_pitch) = camera_pose_for_hover(3, 90.0);
    assert!((positive_yaw - PROFILE_AZIMUTHS_DEG[3].to_radians()).abs() < 1e-6);
    assert!(positive_pitch.abs() < 1e-6);

    let (negative_yaw, negative_pitch) = camera_pose_for_hover(3, -90.0);
    assert!((negative_yaw - (PROFILE_AZIMUTHS_DEG[3] + 180.0).to_radians()).abs() < 1e-6);
    assert!(negative_pitch.abs() < 1e-6);
}

/// An out-of-range axis index falls back to azimuth 0 rather than panicking --
/// `PROFILE_AZIMUTHS_DEG.get(axis_index)` is `None` past index 3.
#[test]
fn out_of_range_axis_index_falls_back_to_azimuth_zero() {
    let (yaw, _pitch) = camera_pose_for_hover(99, 10.0);
    assert!((yaw - 0.0f32.to_radians()).abs() < 1e-6);
}
