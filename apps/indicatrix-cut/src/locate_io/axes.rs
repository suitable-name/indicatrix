//! The coarse start of the mesh-to-rig alignment.
//!
//! The scan's frame is not the rig's. The user says which mesh axis points along rig +Z (up)
//! and which along rig +X, and nudges the result by turns about the rig axes and a shift; the
//! alignment then refines it from the stone's outline in each photo. The mesh's bounding-box
//! centre is put at the rig origin (plus the shift), because the cameras look at the origin.

use glam::{DQuat, DVec3};
use indicatrix_cut_core::rough_plan::locate::Rigid;

use super::rig_form::parse_number;

/// The six mesh axes, in the order of the two dropdowns.
pub const AXIS_NAMES: [&str; 6] = ["+X", "-X", "+Y", "-Y", "+Z", "-Z"];

/// The labels of the six nudge fields, in the order of [`Nudge::from_texts`].
pub const NUDGE_LABELS: [&str; 6] = [
    "Turn about rig X (deg)",
    "Turn about rig Y (deg)",
    "Turn about rig Z (deg)",
    "Shift along rig X (mm)",
    "Shift along rig Y (mm)",
    "Shift along rig Z (mm)",
];

/// The default dropdown picks: mesh +Z along rig +Z, mesh +X along rig +X.
pub const DEFAULT_AXES: (usize, usize) = (4, 0);

/// The unit vector of axis `index` of [`AXIS_NAMES`].
#[must_use]
pub const fn axis_vector(index: usize) -> Option<DVec3> {
    match index {
        0 => Some(DVec3::X),
        1 => Some(DVec3::NEG_X),
        2 => Some(DVec3::Y),
        3 => Some(DVec3::NEG_Y),
        4 => Some(DVec3::Z),
        5 => Some(DVec3::NEG_Z),
        _ => None,
    }
}

/// The user's fine adjustment of the coarse start.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Nudge {
    /// Turns about the rig's X, Y and Z axes, in degrees.
    pub turn_deg: [f64; 3],
    /// A shift along the rig's X, Y and Z axes, in millimetres.
    pub shift_mm: [f64; 3],
}

impl Nudge {
    /// The nudge typed in the six fields (empty means 0).
    pub fn from_texts(texts: &[String]) -> Result<Self, String> {
        let mut values = [0.0; 6];
        for (index, label) in NUDGE_LABELS.iter().enumerate() {
            let text = texts.get(index).map_or("", |t| t.trim());
            if !text.is_empty() {
                values[index] = parse_number(text, label)?;
            }
        }
        Ok(Self {
            turn_deg: [values[0], values[1], values[2]],
            shift_mm: [values[3], values[4], values[5]],
        })
    }
}

/// The starting transform from the mesh frame to the rig frame: mesh axis `z_axis` along rig
/// +Z, mesh axis `x_axis` (its part across the first) along rig +X, the mesh centre
/// `mesh_centre` at the rig origin, then `nudge` applied about the rig's axes.
pub fn start_transform(
    z_axis: usize,
    x_axis: usize,
    mesh_centre: DVec3,
    nudge: &Nudge,
) -> Result<Rigid, String> {
    let (Some(z), Some(x)) = (axis_vector(z_axis), axis_vector(x_axis)) else {
        return Err("Pick one of the six axes for each direction.".to_owned());
    };
    let base = Rigid::from_axes(z, x, DVec3::ZERO)
        .ok_or_else(|| "The two axes must lie along different mesh axes.".to_owned())?;
    let [turn_x, turn_y, turn_z] = nudge.turn_deg.map(f64::to_radians);
    let turn = DQuat::from_rotation_z(turn_z)
        * DQuat::from_rotation_y(turn_y)
        * DQuat::from_rotation_x(turn_x);
    let rotation = turn * base.quat();
    let translation = DVec3::from_array(nudge.shift_mm) - rotation * mesh_centre;
    Ok(Rigid::from_parts(rotation, translation))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTRE: DVec3 = DVec3::new(3.0, -2.0, 5.0);

    fn close(left: DVec3, right: DVec3) -> bool {
        (left - right).length() < 1e-9
    }

    #[test]
    fn the_default_axes_keep_the_mesh_upright_and_put_its_centre_at_the_origin() {
        let rigid = start_transform(4, 0, CENTRE, &Nudge::default()).expect("valid");
        assert!(close(rigid.to_rig(CENTRE), DVec3::ZERO));
        assert!(close(rigid.dir_to_rig(DVec3::Z), DVec3::Z));
        assert!(close(rigid.dir_to_rig(DVec3::X), DVec3::X));
    }

    #[test]
    fn a_mesh_that_lies_on_its_side_is_stood_up_by_the_axis_choice() {
        // The mesh's long axis is X: it should point up in the rig.
        let rigid = start_transform(0, 2, CENTRE, &Nudge::default()).expect("valid");
        assert!(close(rigid.dir_to_rig(DVec3::X), DVec3::Z));
        assert!(close(rigid.dir_to_rig(DVec3::Y), DVec3::X));
        // Right-handed: the third axis follows.
        assert!(close(rigid.dir_to_rig(DVec3::Z), DVec3::Y));
        assert!(close(rigid.to_rig(CENTRE), DVec3::ZERO));
    }

    #[test]
    fn the_same_axis_twice_or_opposite_axes_are_refused() {
        assert!(start_transform(4, 4, CENTRE, &Nudge::default()).is_err());
        assert!(start_transform(4, 5, CENTRE, &Nudge::default()).is_err());
        assert!(start_transform(9, 0, CENTRE, &Nudge::default()).is_err());
    }

    #[test]
    fn the_nudge_shifts_and_turns_about_the_rig_axes() {
        let nudge = Nudge {
            turn_deg: [0.0, 0.0, 90.0],
            shift_mm: [1.0, 2.0, 3.0],
        };
        let rigid = start_transform(4, 0, CENTRE, &nudge).expect("valid");
        // The centre sits at the shift, and rig +X is turned to rig +Y.
        assert!(close(rigid.to_rig(CENTRE), DVec3::new(1.0, 2.0, 3.0)));
        assert!(close(rigid.dir_to_rig(DVec3::X), DVec3::Y));
    }

    #[test]
    fn nudge_fields_may_be_empty_and_must_be_numbers() {
        let texts: Vec<String> = ["", "5", " 1,5 ", "", "", "-2"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let nudge = Nudge::from_texts(&texts).expect("valid");
        assert_eq!(nudge.turn_deg, [0.0, 5.0, 1.5]);
        assert_eq!(nudge.shift_mm, [0.0, 0.0, -2.0]);
        let mut bad = texts;
        bad[3] = "x".to_owned();
        assert!(
            Nudge::from_texts(&bad)
                .unwrap_err()
                .contains("Shift along rig X")
        );
        assert_eq!(Nudge::from_texts(&[]).unwrap(), Nudge::default());
    }
}
