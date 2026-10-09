//! Frames: where a planned stone sits in the rough, and the rigid map that carries the zones of
//! the rough into the stone's own frame.
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
//! are in mm), which is what `ZonedAbsorption::transformed` composes.

use crate::rough_plan::fit::StonePose;
use glam::{DMat3, DQuat, DVec3};
use indicatrix::optics::zoning::{ZoneFrame, ZonedAbsorption};

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
    const fn caliper_turn(&self) -> DMat3 {
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
fn axes_matrix(pose: &StonePose) -> Option<DMat3> {
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

/// The rigid map from the rough frame (mm) to the stone frame (mm) of a stone placed with
/// `pose`, for a design placed by `design`.
///
/// `None` when the pose is not a proper rotation, the scale is not positive, or `design` is
/// not valid.
#[must_use]
pub fn rough_to_stone_frame(pose: &StonePose, design: &DesignPlacement) -> Option<ZoneFrame> {
    if !design.is_valid() {
        return None;
    }
    let a = axes_matrix(pose)?;
    let turn_t = design.caliper_turn().transpose();
    let scale = pose.mm_per_unit;
    let centre = DVec3::from_array(design.centre_units);
    let rotation_matrix = turn_t * a.transpose();
    let translation = turn_t * (centre * scale - a.transpose() * DVec3::from_array(pose.center_mm));
    let rotation = DQuat::from_mat3(&rotation_matrix).normalize();
    rotation.is_finite().then_some(ZoneFrame {
        rotation,
        translation,
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
