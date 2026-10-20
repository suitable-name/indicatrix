use super::{
    GpuFacetPlane, PLANE_REPUBLISH_EPSILON, frame_files_masts, frame_is_superseded,
    planes_equal_within,
};
use glam::Vec3;

/// A few dozen planes with unit normals spread over yaw/pitch angles.
fn sample_planes() -> Vec<GpuFacetPlane> {
    let mut planes = Vec::new();
    for yaw_step in 0..8_u8 {
        for pitch_step in 0..6_u8 {
            let yaw = f32::from(yaw_step) * 0.7;
            let pitch = f32::from(pitch_step).mul_add(0.4, -1.0);
            let normal = Vec3::new(
                pitch.cos() * yaw.cos(),
                pitch.sin(),
                pitch.cos() * yaw.sin(),
            );
            planes.push(GpuFacetPlane::new(normal, -0.5));
        }
    }
    planes
}

#[test]
fn identical_planes_are_equal() {
    let planes = sample_planes();
    assert!(planes_equal_within(
        &planes,
        &planes.clone(),
        PLANE_REPUBLISH_EPSILON
    ));
}

#[test]
fn a_one_ulp_normal_difference_does_not_republish() {
    let planes = sample_planes();
    let mut perturbed = planes.clone();
    perturbed[3].normal[1] = f32::from_bits(perturbed[3].normal[1].to_bits() + 1);
    assert!(planes_equal_within(
        &planes,
        &perturbed,
        PLANE_REPUBLISH_EPSILON
    ));
}

#[test]
fn a_real_geometry_change_republishes() {
    let planes = sample_planes();
    let mut changed = planes.clone();
    changed[3].normal[1] += 0.01;
    assert!(!planes_equal_within(
        &planes,
        &changed,
        PLANE_REPUBLISH_EPSILON
    ));
}

#[test]
fn different_plane_counts_never_match() {
    let planes = sample_planes();
    assert!(!planes_equal_within(
        &planes,
        &planes[..planes.len() - 1],
        PLANE_REPUBLISH_EPSILON
    ));
}

/// Pins the behaviour behind the flicker: re-normalising an already published
/// plane through `GpuFacetPlane::new` moves a normal by an ULP at most, which the
/// republish tolerance must absorb (twice, since the wobble has period 2).
#[test]
fn gpu_facet_plane_new_round_trip_stays_within_epsilon() {
    let planes = sample_planes();
    let once: Vec<GpuFacetPlane> = planes
        .iter()
        .map(|p| GpuFacetPlane::new(Vec3::from(p.normal), p.d))
        .collect();
    let twice: Vec<GpuFacetPlane> = once
        .iter()
        .map(|p| GpuFacetPlane::new(Vec3::from(p.normal), p.d))
        .collect();
    assert!(planes_equal_within(&planes, &once, PLANE_REPUBLISH_EPSILON));
    assert!(planes_equal_within(
        &planes,
        &twice,
        PLANE_REPUBLISH_EPSILON
    ));
}

#[test]
fn a_camera_follow_frame_behind_the_cache_watermark_is_never_superseded() {
    assert!(!frame_is_superseded(false, 3, 9));
}

#[test]
fn a_planned_frame_behind_the_cache_watermark_is_superseded() {
    assert!(frame_is_superseded(true, 3, 9));
}

#[test]
fn a_planned_frame_at_or_past_the_watermark_is_not_superseded() {
    assert!(!frame_is_superseded(true, 9, 9));
    assert!(!frame_is_superseded(true, 10, 9));
}

/// F4-8: masts are filed from planned frames that are not behind the watermark. A
/// camera-follow frame repeats the worker's last masts under its last planned
/// generation, which after an unsolvable plan is a generation those masts do not
/// describe.
#[test]
fn only_a_current_planned_frame_files_its_masts() {
    assert!(frame_files_masts(true, false));
    assert!(!frame_files_masts(true, true), "behind the watermark");
    assert!(!frame_files_masts(false, false), "a camera-follow frame");
    assert!(!frame_files_masts(false, true));
}
