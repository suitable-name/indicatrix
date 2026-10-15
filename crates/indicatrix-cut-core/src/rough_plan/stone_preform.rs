//! The piece of rough a planned stone comes from, as the editor's preform: the sawn piece box,
//! clipped by the rough's own outline, as half-spaces in MODEL UNITS in the design's frame.
//!
//! The planner places a stone as a sawn piece box (`PlacedStone::piece_origin_mm` and
//! `piece_size_mm`, rough frame, mm) plus a [`StonePose`]. The editor's preform is a convex plane
//! set `n . x <= m` in the design's own model coordinates, so everything here is the half-space
//! form of the frames explained in [`crate::rough_plan::placement`]:
//!
//! ```text
//! rough = center + s * A * (Rc * model - c)
//! ```
//!
//! A rough-frame plane `(n, m)` becomes `(Rc^T A^T n, (m - n . center) / s + (A^T n) . c)`.
//! Pure `glam` geometry, deterministic and wasm-clean.

use crate::rough_plan::{
    PlacedStone,
    fit::StonePose,
    placement::{DesignPlacement, axes_matrix},
};
use glam::{DMat3, DVec3};

/// Relative tolerance of "this rough plane reaches into the piece box": the largest signed excess
/// over the box's eight corners must exceed this fraction of `1 + |m|` (mm).
const CUT_TOLERANCE: f64 = 1e-9;

/// What [`stone_preform_planes`] produced.
#[derive(Debug, Clone, PartialEq)]
pub struct StonePreform {
    /// The half-spaces `n . x <= m`, model units, the design's frame: the six faces of the piece
    /// box, then every rough plane that cuts the box.
    pub planes: Vec<(DVec3, f64)>,
    /// How many of the rough's planes cut the piece box (and are therefore in `planes`).
    pub clipped_by_rough: usize,
}

/// The piece box of `stone` as its low and high corners (rough frame, mm), or `None` when an
/// origin is not finite or a size is not finite and positive.
fn piece_bounds(stone: &PlacedStone) -> Option<([f64; 3], [f64; 3])> {
    let origin = stone.piece_origin_mm;
    let size = stone.piece_size_mm;
    let usable =
        origin.iter().all(|v| v.is_finite()) && size.iter().all(|&s| s.is_finite() && s > 0.0);
    if !usable {
        return None;
    }
    let high = [
        origin[0] + size[0],
        origin[1] + size[1],
        origin[2] + size[2],
    ];
    let proper = origin
        .iter()
        .zip(&high)
        .all(|(o, h)| h.is_finite() && h > o);
    proper.then_some((origin, high))
}

/// The six half-spaces of the box `low .. high`, in the order `-x, +x, -y, +y, -z, +z`.
const fn box_faces(low: [f64; 3], high: [f64; 3]) -> [(DVec3, f64); 6] {
    [
        (DVec3::NEG_X, -low[0]),
        (DVec3::X, high[0]),
        (DVec3::NEG_Y, -low[1]),
        (DVec3::Y, high[1]),
        (DVec3::NEG_Z, -low[2]),
        (DVec3::Z, high[2]),
    ]
}

/// The six half-spaces of the sawn piece of `stone` in the rough frame (mm).
///
/// They are `x_i >= origin_i` and `x_i <= origin_i + size_i`, in the order `-x, +x, -y, +y, -z, +z` (so the normals are
/// `-X, +X, -Y, +Y, -Z, +Z` and the offsets `-origin_x`, `origin_x + size_x`, and so on).
///
/// `None` when a size is not finite and positive (or an origin is not finite).
#[must_use]
pub fn piece_box_halfspaces(stone: &PlacedStone) -> Option<[(DVec3, f64); 6]> {
    let (low, high) = piece_bounds(stone)?;
    Some(box_faces(low, high))
}

/// Whether the rough plane `n . x <= m` (rough frame, mm) cuts the box `low .. high`: the
/// largest `n . corner - m` over the eight corners exceeds `1e-9 * (1 + |m|)`.
///
/// A plane with a non-finite normal or offset cuts nothing. The maximum over the corners is
/// taken per axis (`n . corner` is a sum of one term per axis, and the sum is monotone in each),
/// which gives the very value the corner-by-corner maximum would.
fn reaches_into_box(normal: DVec3, offset: f64, low: [f64; 3], high: [f64; 3]) -> bool {
    if !(normal.is_finite() && offset.is_finite()) {
        return false;
    }
    let furthest = (normal.x * low[0]).max(normal.x * high[0])
        + (normal.y * low[1]).max(normal.y * high[1])
        + (normal.z * low[2]).max(normal.z * high[2]);
    furthest - offset > CUT_TOLERANCE * (1.0 + offset.abs())
}

/// The pieces of the rough-to-design map of one stone, computed once and applied to many planes.
struct ToDesign {
    /// `A^T`: rough-frame directions into the caliper frame.
    axes_t: DMat3,
    /// `Rc^T`: caliper-frame directions into the design's own frame.
    turn_t: DMat3,
    /// The pose's `center_mm` (rough frame, mm).
    center: DVec3,
    /// The design's centring offset `c` (model units).
    centre_units: DVec3,
    /// Millimetres per model unit, `s`.
    scale: f64,
}

impl ToDesign {
    /// `None` when the pose is not a proper rotation with a positive scale, or `design` is not
    /// valid.
    fn new(pose: &StonePose, design: &DesignPlacement) -> Option<Self> {
        if !design.is_valid() {
            return None;
        }
        Some(Self {
            axes_t: axes_matrix(pose)?.transpose(),
            turn_t: design.caliper_turn().transpose(),
            center: DVec3::from_array(pose.center_mm),
            centre_units: DVec3::from_array(design.centre_units),
            scale: pose.mm_per_unit,
        })
    }

    /// The plane `(n, m)` of the rough frame (mm) in the design's frame (model units); `None`
    /// when the result is not finite.
    fn plane(&self, (normal, offset): (DVec3, f64)) -> Option<(DVec3, f64)> {
        let in_caliper = self.axes_t * normal;
        let design_normal = self.turn_t * in_caliper;
        let design_offset =
            (offset - normal.dot(self.center)) / self.scale + in_caliper.dot(self.centre_units);
        (design_normal.is_finite() && design_offset.is_finite())
            .then_some((design_normal, design_offset))
    }
}

/// A half-space of the rough frame (mm) in the design's frame (model units), for a stone placed
/// with `pose` and a design placed by `design`:
/// `n' = Rc^T A^T n`, `m' = (m - n . center) / s + (A^T n) . c`.
///
/// Derivation: with `rough = center + s A (Rc model - c)` (see [`crate::rough_plan::placement`]),
/// `n . rough <= m` becomes `(A^T n) . (Rc model) <= (m - n . center) / s + (A^T n) . c`, i.e.
/// `(Rc^T A^T n) . model <= (m - n . center) / s + (A^T n) . c`. `A` and `Rc` are rotations, so
/// a unit `n` gives a unit `n'`. Here `s = pose.mm_per_unit` and `c = design.centre_units`.
///
/// `pose` is passed explicitly (not read from a `PlacedStone`) so the caller can hand in the
/// cutter's chosen pose variant (`pose_variant`).
///
/// `None` when the pose is not a proper rotation, the scale is not positive, `design` is not
/// valid, or the result is not finite.
#[must_use]
pub fn halfspace_to_design_frame(
    plane: (DVec3, f64),
    pose: &StonePose,
    design: &DesignPlacement,
) -> Option<(DVec3, f64)> {
    ToDesign::new(pose, design)?.plane(plane)
}

/// The preform of `stone`: its piece box clipped by every plane of `rough_halfspaces`.
///
/// Those are the rough's own `RoughModel::halfspaces` (rough frame, mm) that reach into the
/// box, all moved into the design's frame in model units.
///
/// The box's own six planes come first (`-x, +x, -y, +y, -z, +z`), then the rough planes that
/// cut the box, in the order given. A rough plane that leaves the whole box on its inside is
/// left out (it would be redundant), and so is a plane with a non-finite normal or offset. A
/// plane cuts the box when the largest `n . corner - m` over the eight box corners exceeds
/// `1e-9 * (1 + |m|)`.
///
/// `pose` is passed explicitly, not read from `stone.pose`, so the caller can hand in the
/// cutter's chosen pose variant (`pose_variant`): the preform follows the pose it is given.
///
/// `None` when the pose, the placement or the box is unusable.
#[must_use]
pub fn stone_preform_planes(
    rough_halfspaces: &[(DVec3, f64)],
    stone: &PlacedStone,
    pose: &StonePose,
    design: &DesignPlacement,
) -> Option<StonePreform> {
    let (low, high) = piece_bounds(stone)?;
    let to_design = ToDesign::new(pose, design)?;
    let mut planes = Vec::with_capacity(6 + rough_halfspaces.len());
    for face in box_faces(low, high) {
        planes.push(to_design.plane(face)?);
    }
    let mut clipped_by_rough = 0;
    for &(normal, offset) in rough_halfspaces {
        if reaches_into_box(normal, offset, low, high) {
            planes.push(to_design.plane((normal, offset))?);
            clipped_by_rough += 1;
        }
    }
    Some(StonePreform {
        planes,
        clipped_by_rough,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rough_plan::{Axis, placement::pose_variant};

    const TOL: f64 = 1e-12;

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < TOL
    }

    fn pose_at(center_mm: [f64; 3], mm_per_unit: f64) -> StonePose {
        StonePose {
            center_mm,
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            mm_per_unit,
        }
    }

    fn stone(origin: [f64; 3], size: [f64; 3], pose: StonePose) -> PlacedStone {
        PlacedStone {
            entry_id: 1,
            piece_origin_mm: origin,
            piece_size_mm: size,
            stone_size_mm: size,
            table_axis: Axis::Y,
            carat: 1.0,
            volume_mm3: 1.0,
            pose,
        }
    }

    /// The box `[-2, -3, -2] .. [2, 3, 2]` of test 1, placed with the identity pose at scale 2.
    fn identity_stone() -> PlacedStone {
        stone([-2.0, -3.0, -2.0], [4.0, 6.0, 4.0], pose_at([0.0; 3], 2.0))
    }

    const FACE_NORMALS: [DVec3; 6] = [
        DVec3::NEG_X,
        DVec3::X,
        DVec3::NEG_Y,
        DVec3::Y,
        DVec3::NEG_Z,
        DVec3::Z,
    ];

    #[test]
    fn the_identity_placement_scales_the_box_into_model_units() {
        let placed = identity_stone();
        let preform = stone_preform_planes(&[], &placed, &placed.pose, &DesignPlacement::IDENTITY)
            .expect("usable inputs");
        assert_eq!(preform.clipped_by_rough, 0);
        assert_eq!(preform.planes.len(), 6);
        let offsets = [1.0, 1.0, 1.5, 1.5, 1.0, 1.0];
        for ((plane, normal), offset) in preform.planes.iter().zip(FACE_NORMALS).zip(offsets) {
            assert!(close(plane.0, normal), "{plane:?} against {normal:?}");
            assert!((plane.1 - offset).abs() < TOL, "{plane:?} against {offset}");
        }
    }

    /// The independent inverse of the module's own formula:
    /// `model = Rc^T (A^T (rough - center) / s + c)`, with `Rc` and `A` spelled out column by
    /// column.
    fn rough_to_model(
        rough: DVec3,
        center: DVec3,
        scale: f64,
        centre_units: DVec3,
        width_dir: [f64; 2],
    ) -> DVec3 {
        let [wx, wz] = width_dir;
        let caliper_columns = [DVec3::new(wx, 0.0, -wz), DVec3::Y, DVec3::new(wz, 0.0, wx)];
        let axes_columns = [
            DVec3::new(-1.0, 0.0, 0.0),
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::new(0.0, -1.0, 0.0),
        ];
        let w = rough - center;
        let in_caliper = DVec3::new(
            axes_columns[0].dot(w),
            axes_columns[1].dot(w),
            axes_columns[2].dot(w),
        ) / scale
            + centre_units;
        DVec3::new(
            caliper_columns[0].dot(in_caliper),
            caliper_columns[1].dot(in_caliper),
            caliper_columns[2].dot(in_caliper),
        )
    }

    /// Points of the box `low .. low + size` (rough frame, mm): twelve inside (its centre, the
    /// six face centres moved 1e-3 inwards, and five others) and, in the order of the box's
    /// planes `-x, +x, -y, +y, -z, +z`, the six face centres moved 1e-3 outwards.
    fn probe_points(low: DVec3, size: DVec3) -> (Vec<DVec3>, Vec<DVec3>) {
        let high = low + size;
        let middle = low + size * 0.5;
        let mut inside = vec![middle];
        let mut outside = Vec::new();
        for axis in 0..3 {
            // Moving inwards is up from the low face and down from the high face.
            for (inwards, face) in [(1e-3, low[axis]), (-1e-3, high[axis])] {
                let mut inner = middle;
                inner[axis] = face + inwards;
                inside.push(inner);
                let mut outer = middle;
                outer[axis] = face - inwards;
                outside.push(outer);
            }
        }
        for fraction in [
            [0.1, 0.1, 0.1],
            [0.9, 0.9, 0.9],
            [0.2, 0.7, 0.4],
            [0.8, 0.3, 0.6],
            [0.05, 0.95, 0.5],
        ] {
            inside.push(low + size * DVec3::from_array(fraction));
        }
        (inside, outside)
    }

    #[test]
    fn a_turned_and_shifted_placement_keeps_the_box_points_inside_and_the_outside_out() {
        // A half turn about y composed with a quarter turn about x: columns (-1,0,0), (0,0,-1),
        // (0,-1,0), right handed.
        let axes = [[-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, -1.0, 0.0]];
        let [x, y, z] = axes.map(DVec3::from_array);
        assert!((x.dot(y.cross(z)) - 1.0).abs() < TOL, "det must be +1");
        let center = [10.0, 5.0, -3.0];
        let scale = 0.5;
        let pose = StonePose {
            center_mm: center,
            axes,
            mm_per_unit: scale,
        };
        let angle = 30.0_f64.to_radians();
        let width_dir = [angle.cos(), angle.sin()];
        let centre_units = [0.3, -0.2, 0.1];
        let design = DesignPlacement {
            width_dir,
            centre_units,
        };
        let low = DVec3::new(7.0, 3.0, -6.0);
        let size = DVec3::new(6.0, 4.0, 5.0);
        let placed = stone(low.to_array(), size.to_array(), pose);
        let preform = stone_preform_planes(&[], &placed, &pose, &design).expect("usable inputs");
        assert_eq!(preform.planes.len(), 6);
        for (normal, _) in &preform.planes {
            assert!(
                (normal.length() - 1.0).abs() < TOL,
                "{normal:?} is not unit"
            );
        }
        let to_model = |rough: DVec3| {
            rough_to_model(
                rough,
                DVec3::from_array(center),
                scale,
                DVec3::from_array(centre_units),
                width_dir,
            )
        };

        let (inside, outside) = probe_points(low, size);
        assert_eq!(inside.len(), 12);
        for point in &inside {
            let model = to_model(*point);
            for (normal, offset) in &preform.planes {
                assert!(
                    normal.dot(model) <= offset + 1e-9,
                    "{point:?} escapes {normal:?} <= {offset}"
                );
            }
        }
        // A point just outside face k violates exactly plane k.
        for (face, point) in outside.iter().enumerate() {
            let model = to_model(*point);
            for (index, (normal, offset)) in preform.planes.iter().enumerate() {
                let excess = normal.dot(model) - offset;
                if index == face {
                    // 1e-3 mm is 2e-3 model units at 0.5 mm per unit.
                    assert!(excess > 1e-3, "face {face} not violated: {excess}");
                } else {
                    assert!(
                        excess <= 1e-9,
                        "face {face} violates plane {index}: {excess}"
                    );
                }
            }
        }
    }

    #[test]
    fn only_the_rough_planes_that_reach_into_the_box_are_kept() {
        let placed = identity_stone();
        let slanted = (DVec3::new(1.0, 1.0, 0.0).normalize(), 3.0);
        let far = (DVec3::X, 10.0);
        let preform = stone_preform_planes(
            &[slanted, far],
            &placed,
            &placed.pose,
            &DesignPlacement::IDENTITY,
        )
        .expect("usable inputs");
        assert_eq!(preform.clipped_by_rough, 1);
        assert_eq!(preform.planes.len(), 7);
        let (normal, offset) = preform.planes[6];
        assert!(close(normal, slanted.0), "{normal:?}");
        assert!((offset - 1.5).abs() < TOL, "{offset}");
        // The box's own planes are untouched by the clipping.
        let alone = stone_preform_planes(&[], &placed, &placed.pose, &DesignPlacement::IDENTITY)
            .expect("usable inputs");
        assert_eq!(&preform.planes[..6], &alone.planes[..]);
    }

    #[test]
    fn a_plane_touching_the_box_or_not_finite_is_left_out() {
        let placed = identity_stone();
        let touching = (DVec3::X, 2.0);
        let broken = (DVec3::new(f64::NAN, 0.0, 0.0), 1.0);
        let infinite = (DVec3::Y, f64::INFINITY);
        let preform = stone_preform_planes(
            &[touching, broken, infinite],
            &placed,
            &placed.pose,
            &DesignPlacement::IDENTITY,
        )
        .expect("usable inputs");
        assert_eq!(preform.clipped_by_rough, 0);
        assert_eq!(preform.planes.len(), 6);
    }

    #[test]
    fn the_pose_argument_decides_not_the_stones_own_pose() {
        let placed = identity_stone();
        let turned = pose_variant(&placed.pose, 1).expect("pose 1 exists");
        let preform = stone_preform_planes(&[], &placed, &turned, &DesignPlacement::IDENTITY)
            .expect("usable inputs");
        // A half turn about the caliper x flips y and z: the -y face becomes the +y plane.
        let normals = [
            DVec3::NEG_X,
            DVec3::X,
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::NEG_Z,
        ];
        let offsets = [1.0, 1.0, 1.5, 1.5, 1.0, 1.0];
        for ((plane, normal), offset) in preform.planes.iter().zip(normals).zip(offsets) {
            assert!(close(plane.0, normal), "{plane:?} against {normal:?}");
            assert!((plane.1 - offset).abs() < TOL, "{plane:?} against {offset}");
        }
    }

    #[test]
    fn unusable_inputs_give_nothing() {
        let placed = identity_stone();
        let design = DesignPlacement::IDENTITY;
        let good = stone_preform_planes(&[], &placed, &placed.pose, &design);
        assert!(good.is_some());

        let skewed = StonePose {
            axes: [[1.0, 0.1, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            ..placed.pose
        };
        assert!(stone_preform_planes(&[], &placed, &skewed, &design).is_none());
        assert!(halfspace_to_design_frame((DVec3::X, 1.0), &skewed, &design).is_none());

        let no_scale = pose_at([0.0; 3], 0.0);
        assert!(stone_preform_planes(&[], &placed, &no_scale, &design).is_none());
        assert!(halfspace_to_design_frame((DVec3::X, 1.0), &no_scale, &design).is_none());

        let flat = stone([-2.0, -3.0, -2.0], [4.0, 0.0, 4.0], placed.pose);
        assert!(piece_box_halfspaces(&flat).is_none());
        assert!(stone_preform_planes(&[], &flat, &flat.pose, &design).is_none());

        let bad_design = DesignPlacement {
            width_dir: [2.0, 0.0],
            ..DesignPlacement::IDENTITY
        };
        assert!(stone_preform_planes(&[], &placed, &placed.pose, &bad_design).is_none());
        assert!(halfspace_to_design_frame((DVec3::X, 1.0), &placed.pose, &bad_design).is_none());
    }

    #[test]
    fn the_piece_box_halfspaces_come_in_the_documented_order() {
        let placed = stone([1.0, 2.0, 3.0], [2.0, 4.0, 6.0], pose_at([0.0; 3], 1.0));
        let faces = piece_box_halfspaces(&placed).expect("a usable box");
        let offsets = [-1.0, 3.0, -2.0, 6.0, -3.0, 9.0];
        for ((face, normal), offset) in faces.iter().zip(FACE_NORMALS).zip(offsets) {
            assert!(close(face.0, normal), "{face:?} against {normal:?}");
            assert!((face.1 - offset).abs() < TOL, "{face:?} against {offset}");
        }
    }
}
