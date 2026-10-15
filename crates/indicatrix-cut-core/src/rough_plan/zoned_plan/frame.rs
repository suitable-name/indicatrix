//! Frames: the rigid map that carries the zones of the rough into a planned stone's own frame.
//!
//! The three frames (rough, caliper, stone), the pose axes `A`, the caliper turn `Rc` and the
//! exact map `stone = Rc^T * (A^T * (rough - center) + s * c)` are explained in
//! [`crate::rough_plan::placement`], together with [`DesignPlacement`] and the box-symmetric
//! poses ([`pose_variant`], [`candidate_poses`], [`POSE_COUNT`]). Those live there so the
//! editor's preform can use them without the zoned planner; they are re-exported here, so
//! the `zoned_plan` paths keep working.
//!
//! This module only turns that map into the renderer's [`ZoneFrame`] and applies it to a
//! [`ZonedAbsorption`].

pub use crate::rough_plan::placement::{
    DesignPlacement, POSE_COUNT, candidate_poses, pose_variant,
};
use crate::rough_plan::{fit::StonePose, placement::rough_to_stone_map};
use glam::DQuat;
use indicatrix::optics::zoning::{ZoneFrame, ZonedAbsorption};

/// The rigid map from the rough frame (mm) to the stone frame (mm) of a stone placed with
/// `pose`, for a design placed by `design`.
///
/// `None` when the pose is not a proper rotation, the scale is not positive, or `design` is
/// not valid.
#[must_use]
pub fn rough_to_stone_frame(pose: &StonePose, design: &DesignPlacement) -> Option<ZoneFrame> {
    let map = rough_to_stone_map(pose, design)?;
    let rotation = DQuat::from_mat3(&map.rotation).normalize();
    rotation.is_finite().then_some(ZoneFrame {
        rotation,
        translation: map.translation,
    })
}

/// The zones of the rough, expressed in the frame of a stone placed with `pose`: the geometry
/// of `rough_zoned` (mm, rough frame) composed with [`rough_to_stone_frame`].
///
/// The absorptions, the number of zones and the softness are untouched, so the per-segment
/// lengths of a segment moved with the stone are exactly those of the same segment in the
/// rough.
#[must_use]
pub fn zones_in_stone_frame(
    rough_zoned: &ZonedAbsorption,
    pose: &StonePose,
    design: &DesignPlacement,
) -> Option<ZonedAbsorption> {
    let frame = rough_to_stone_frame(pose, design)?;
    Some(rough_zoned.transformed(&frame))
}
