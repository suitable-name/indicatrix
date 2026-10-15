//! Where a planned stone sits in the rough, and the rigid map between the rough and the stone.
//!
//! Pure geometry on `glam` alone (no renderer types), so the editor's
//! preform ([`crate::rough_plan::stone_preform`]) and the rough-colour zones (`zoned_plan::frame`) share one definition of the frames.
//!
//! # The three frames
//!
//! - the **rough frame** (mm): the frame of [`StonePose::center_mm`] and of the zones a rough
//!   colour fit produces;
//! - the **caliper frame** (model units): the planner's frame of a design, in which the stone's
//!   pose is given. A design's own model coordinates are first turned about the vertical by its
//!   caliper width direction (`to_caliper_frame` in the application's planner code) and the
//!   centre of the bounding box of the result is then subtracted;
//! - the **stone frame** (mm): the frame the renderer's zones live in
//!   (`indicatrix::render_setup`'s chain: model units, times `absorption_path_scale`, gives
//!   stone mm with the design's own model origin and axes), so the zones of an adopted stone are
//!   expressed relative to the design's model origin.
//!
//! With `s = StonePose::mm_per_unit`, the pose's axes `A` (columns, caliper to rough), the
//! caliper turn `Rc` (model to caliper, before centring) and the centring offset `c`:
//!
//! ```text
//! rough  = center + s * A * (Rc * model - c)
//! stone  = s * model = Rc^T * (A^T * (rough - center) + s * c)
//! ```
//!
//! The map `rough -> stone` is therefore a pure rotation plus translation (no scale, both frames
//! are in mm): a [`RigidMap`], which `ZonedAbsorption::transformed` composes in the zoning
//! build. The map `rough -> model` is the same one divided by `s`.
//!
//! # The poses of a stone
//!
//! A box-model stone is its bounding box, so its pose is defined up to the box's symmetries:
//! [`pose_variant`] and [`candidate_poses`] enumerate the four proper ones.

use crate::rough_plan::fit::StonePose;
use glam::{DMat3, DVec3};

/// How many box-symmetric poses a stone has: the identity and the three half-turns about its
/// own axes.
pub const POSE_COUNT: u8 = 4;

/// The sign each caliper axis keeps in pose `index` (a half turn about the axis whose sign is
/// `+1`; pose `0` is the planner's own).
const POSE_SIGNS: [[f64; 3]; POSE_COUNT as usize] = [
    [1.0, 1.0, 1.0],
    [1.0, -1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
];

/// Orthonormality tolerance of a pose's axes.
const AXES_TOLERANCE: f64 = 1e-6;

/// How a design's model frame relates to the planner's caliper frame.
///
/// The two numbers the planner derives from the design itself (its hull is turned into the
/// caliper frame and centred), which a [`StonePose`] does not carry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DesignPlacement {
    /// The caliper width direction in the design's `x`/`z` plane (a unit 2-vector), the
    /// `width_dir` of `indicatrix::geometry::stone_metrics::CaliperFrame`.
    pub width_dir: [f64; 2],
    /// The centre of the bounding box of the design in the turned (caliper) frame, in model
    /// units: what is subtracted to centre the design.
    pub centre_units: [f64; 3],
}

impl DesignPlacement {
    /// A design whose model frame already is its centred caliper frame.
    pub const IDENTITY: Self = Self {
        width_dir: [1.0, 0.0],
        centre_units: [0.0; 3],
    };

    /// Whether the numbers are usable: finite, with a unit width direction.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let [wx, wz] = self.width_dir;
        wx.is_finite()
            && wz.is_finite()
            && (wx.mul_add(wx, wz * wz) - 1.0).abs() < 1e-6
            && self.centre_units.iter().all(|c| c.is_finite())
    }

    /// The model-to-caliper turn `Rc` (about the vertical axis).
    pub(crate) const fn caliper_turn(&self) -> DMat3 {
        let [wx, wz] = self.width_dir;
        DMat3::from_cols(DVec3::new(wx, 0.0, -wz), DVec3::Y, DVec3::new(wz, 0.0, wx))
    }
}

impl Default for DesignPlacement {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// The pose axes as the matrix `A` (columns: caliper `x`, `y`, `z` in the rough frame), or
/// `None` when the pose is not a finite proper rotation with a positive scale.
pub(crate) fn axes_matrix(pose: &StonePose) -> Option<DMat3> {
    let columns = pose.axes.map(DVec3::from_array);
    let finite = columns.iter().all(|c| c.is_finite())
        && pose.center_mm.iter().all(|c| c.is_finite())
        && pose.mm_per_unit.is_finite()
        && pose.mm_per_unit > 0.0;
    if !finite {
        return None;
    }
    let a = DMat3::from_cols(columns[0], columns[1], columns[2]);
    let should_be_identity = a.transpose() * a;
    let orthonormal = (should_be_identity - DMat3::IDENTITY)
        .to_cols_array()
        .iter()
        .all(|v| v.abs() < AXES_TOLERANCE);
    (orthonormal && a.determinant() > 0.0).then_some(a)
}

/// A rigid map `p -> rotation * p + translation` between two mm frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigidMap {
    /// The rotation part (a proper rotation: orthonormal, determinant `+1`).
    pub rotation: DMat3,
    /// The translation part, applied after the rotation, in mm.
    pub translation: DVec3,
}

/// The rigid map from the rough frame (mm) to the stone frame (mm, `s * model`) of a stone placed
/// with `pose`, for a design placed by `design`: `stone = Rc^T (A^T (rough - center) + s c)`.
///
/// `None` when the pose is not a proper rotation, the scale is not positive, or `design` is
/// not valid.
#[must_use]
pub fn rough_to_stone_map(pose: &StonePose, design: &DesignPlacement) -> Option<RigidMap> {
    if !design.is_valid() {
        return None;
    }
    let a = axes_matrix(pose)?;
    let turn_t = design.caliper_turn().transpose();
    let scale = pose.mm_per_unit;
    let centre = DVec3::from_array(design.centre_units);
    let rotation = turn_t * a.transpose();
    let translation = turn_t * (centre * scale - a.transpose() * DVec3::from_array(pose.center_mm));
    Some(RigidMap {
        rotation,
        translation,
    })
}

/// Pose `index` of the stone: the planner's pose (`0`) turned by a half turn about one of its
/// own axes (`1`: about the caliper `x`, `2`: about `y`, `3`: about `z`).
///
/// The bounding box of
/// the stone is unchanged, so each is as valid as the planner's pose for a box-model stone.
///
/// `None` for an index of [`POSE_COUNT`] or more.
#[must_use]
pub fn pose_variant(pose: &StonePose, index: u8) -> Option<StonePose> {
    let signs = POSE_SIGNS.get(usize::from(index))?;
    let mut axes = pose.axes;
    for (axis, sign) in axes.iter_mut().zip(signs) {
        for c in &mut *axis {
            *c *= sign;
        }
    }
    Some(StonePose { axes, ..*pose })
}

/// The poses a stone may take: just pose `0` for an exact single-stone fit (its pose is unique,
/// see `PlacedStone::pose`), the four box-symmetric poses otherwise. Pairs of `(index, pose)`.
#[must_use]
pub fn candidate_poses(pose: &StonePose, exact_fit: bool) -> Vec<(u8, StonePose)> {
    let count = if exact_fit { 1 } else { POSE_COUNT };
    (0..count)
        .filter_map(|index| pose_variant(pose, index).map(|p| (index, p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < EPS
    }

    fn identity_pose(centre: [f64; 3], mm_per_unit: f64) -> StonePose {
        StonePose {
            center_mm: centre,
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            mm_per_unit,
        }
    }

    #[test]
    fn the_four_poses_are_proper_rotations_of_the_same_box() {
        let pose = StonePose {
            center_mm: [1.0, 2.0, 3.0],
            axes: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
            mm_per_unit: 1.0,
        };
        let all = candidate_poses(&pose, false);
        assert_eq!(all.len(), usize::from(POSE_COUNT));
        assert_eq!(all[0], (0, pose));
        for (index, variant) in &all {
            assert_eq!(variant.center_mm, pose.center_mm);
            assert!((variant.mm_per_unit - pose.mm_per_unit).abs() < EPS);
            let [x, y, z] = variant.axes.map(DVec3::from_array);
            assert!(close(x.cross(y), z), "pose {index} is not right handed");
            // The axes are the planner's, up to sign: the same lines in space.
            for (a, b) in variant.axes.iter().zip(&pose.axes) {
                let dot = DVec3::from_array(*a).dot(DVec3::from_array(*b));
                assert!((dot.abs() - 1.0).abs() < EPS);
            }
        }
        // Each variant turns the table normal differently: poses 1 and 3 turn the stone over.
        let ups: Vec<f64> = all
            .iter()
            .map(|(_, p)| DVec3::from_array(p.axes[1]).dot(DVec3::from_array(pose.axes[1])))
            .collect();
        assert_eq!(ups, vec![1.0, -1.0, 1.0, -1.0]);
        assert_eq!(pose_variant(&pose, POSE_COUNT), None);
        // An exact single-stone fit has one pose.
        assert_eq!(candidate_poses(&pose, true).len(), 1);
    }

    #[test]
    fn the_rough_to_stone_map_carries_a_known_point() {
        // axes x = +z, y = +y, z = -x (right handed), scale 2, centre (10, 0, 0).
        let pose = StonePose {
            center_mm: [10.0, 0.0, 0.0],
            axes: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
            mm_per_unit: 2.0,
        };
        let design = DesignPlacement {
            width_dir: [0.0, 1.0],
            centre_units: [0.5, 0.25, -0.5],
        };
        let map = rough_to_stone_map(&pose, &design).expect("a proper pose");
        // The model point (1, 2, 3) sits at (11, 3.5, 5) in the rough and at 2 * (1, 2, 3) mm
        // in the stone frame (the same worked example as the zoning frame test).
        let stone = map.rotation * DVec3::new(11.0, 3.5, 5.0) + map.translation;
        assert!(close(stone, DVec3::new(2.0, 4.0, 6.0)), "{stone:?}");
        // A rotation: orthonormal with determinant +1.
        let gram = map.rotation.transpose() * map.rotation;
        assert!(
            (gram - DMat3::IDENTITY)
                .to_cols_array()
                .iter()
                .all(|v| v.abs() < EPS)
        );
        assert!((map.rotation.determinant() - 1.0).abs() < EPS);
    }

    #[test]
    fn an_unusable_pose_or_placement_gives_no_map() {
        let design = DesignPlacement::IDENTITY;
        assert!(rough_to_stone_map(&identity_pose([0.0; 3], 1.0), &design).is_some());
        let mut mirrored = identity_pose([0.0; 3], 1.0);
        mirrored.axes[2] = [0.0, 0.0, -1.0];
        assert!(rough_to_stone_map(&mirrored, &design).is_none());
        assert!(rough_to_stone_map(&identity_pose([0.0; 3], 0.0), &design).is_none());
        assert!(rough_to_stone_map(&identity_pose([f64::NAN, 0.0, 0.0], 1.0), &design).is_none());
        let skewed = StonePose {
            axes: [[1.0, 0.1, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            ..identity_pose([0.0; 3], 1.0)
        };
        assert!(rough_to_stone_map(&skewed, &design).is_none());
        let bad_design = DesignPlacement {
            width_dir: [2.0, 0.0],
            ..DesignPlacement::IDENTITY
        };
        assert!(rough_to_stone_map(&identity_pose([0.0; 3], 1.0), &bad_design).is_none());
    }
}
