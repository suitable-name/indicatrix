//! The fixed edge and corner tables of the shape editor, the default cuts a click
//! creates, and the direction maths of face cuts.
//!
//! The order of [`EDGES`] and [`CORNERS`] is part of the saved-plan and window
//! contract: the drop-down lists in the window use the same indices.

use indicatrix_cut_core::rough_plan::{BoxFace, RoughBase, RoughCut};
use slint::{ModelRc, SharedString, VecModel};

/// The twelve box edges as `[faces[0], faces[1]]`, in the order of the edge drop-down.
pub(super) const EDGES: [[BoxFace; 2]; 12] = [
    [BoxFace::Top, BoxFace::Front],
    [BoxFace::Top, BoxFace::Back],
    [BoxFace::Top, BoxFace::Left],
    [BoxFace::Top, BoxFace::Right],
    [BoxFace::Bottom, BoxFace::Front],
    [BoxFace::Bottom, BoxFace::Back],
    [BoxFace::Bottom, BoxFace::Left],
    [BoxFace::Bottom, BoxFace::Right],
    [BoxFace::Front, BoxFace::Left],
    [BoxFace::Front, BoxFace::Right],
    [BoxFace::Back, BoxFace::Left],
    [BoxFace::Back, BoxFace::Right],
];

/// The eight box corners, in the order of the corner drop-down.
pub(super) const CORNERS: [[BoxFace; 3]; 8] = [
    [BoxFace::Top, BoxFace::Front, BoxFace::Left],
    [BoxFace::Top, BoxFace::Front, BoxFace::Right],
    [BoxFace::Top, BoxFace::Back, BoxFace::Left],
    [BoxFace::Top, BoxFace::Back, BoxFace::Right],
    [BoxFace::Bottom, BoxFace::Front, BoxFace::Left],
    [BoxFace::Bottom, BoxFace::Front, BoxFace::Right],
    [BoxFace::Bottom, BoxFace::Back, BoxFace::Left],
    [BoxFace::Bottom, BoxFace::Back, BoxFace::Right],
];

/// The index in [`EDGES`] of the default edge cut (Top-Front).
pub(super) const DEFAULT_EDGE: usize = 0;

/// The index in [`CORNERS`] of the default corner cut (Top-Front-Right).
pub(super) const DEFAULT_CORNER: usize = 1;

/// The share of the shorter adjacent extent a new edge or corner cut removes.
const DEFAULT_SETBACK_SHARE: f64 = 0.15;

/// The share of the extent along the normal a new face cut removes.
const DEFAULT_DEPTH_SHARE: f64 = 0.10;

/// The smallest default setback or depth, in mm.
const DEFAULT_STEP_MM: f64 = 0.1;

/// The face's name as shown in the drop-downs and cut titles.
#[must_use]
pub(super) const fn face_label(face: BoxFace) -> &'static str {
    match face {
        BoxFace::Top => "Top",
        BoxFace::Bottom => "Bottom",
        BoxFace::Right => "Right",
        BoxFace::Left => "Left",
        BoxFace::Front => "Front",
        BoxFace::Back => "Back",
    }
}

/// The faces joined with dashes ("Top-Front-Right").
#[must_use]
pub(super) fn faces_label(faces: &[BoxFace]) -> String {
    faces
        .iter()
        .map(|&face| face_label(face))
        .collect::<Vec<_>>()
        .join("-")
}

/// The twelve edge names, in [`EDGES`] order.
#[must_use]
pub(super) fn edge_names() -> Vec<String> {
    EDGES.iter().map(|faces| faces_label(faces)).collect()
}

/// The eight corner names, in [`CORNERS`] order.
#[must_use]
pub(super) fn corner_names() -> Vec<String> {
    CORNERS.iter().map(|faces| faces_label(faces)).collect()
}

/// `names` as the string model a Slint drop-down binds to.
#[must_use]
pub(super) fn options_model(names: Vec<String>) -> ModelRc<SharedString> {
    let items: Vec<SharedString> = names.into_iter().map(SharedString::from).collect();
    ModelRc::new(VecModel::from(items))
}

/// Whether `a` and `b` name the same set of faces, in any order.
fn same_faces(a: &[BoxFace], b: &[BoxFace]) -> bool {
    a.len() == b.len() && a.iter().all(|f| b.contains(f)) && b.iter().all(|f| a.contains(f))
}

/// The index in [`EDGES`] of the edge between `faces` (either order).
#[must_use]
pub(super) fn edge_index(faces: [BoxFace; 2]) -> Option<usize> {
    EDGES.iter().position(|edge| same_faces(edge, &faces))
}

/// The index in [`CORNERS`] of the corner between `faces` (any order).
#[must_use]
pub(super) fn corner_index(faces: [BoxFace; 3]) -> Option<usize> {
    CORNERS.iter().position(|corner| same_faces(corner, &faces))
}

/// Rounds to 0.1 mm.
fn round_tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// 15 % of `shortest_mm`, rounded to 0.1 mm, at least 0.1 mm and never more than
/// `shortest_mm` itself.
#[must_use]
pub(super) fn default_setback(shortest_mm: f64) -> f64 {
    round_tenth(DEFAULT_SETBACK_SHARE * shortest_mm)
        .max(DEFAULT_STEP_MM)
        .min(shortest_mm)
}

/// The setbacks of an edge cut on `faces`, clamped to what the faces can carry:
/// `setbacks[0]` runs along `faces[0]`, so it cannot exceed the extent across `faces[1]`.
#[must_use]
pub(super) fn clamp_edge_setbacks(
    faces: [BoxFace; 2],
    setbacks: [f64; 2],
    extents: [f64; 3],
) -> [f64; 2] {
    let axis = faces.map(|face| face.axis_and_side().0);
    [
        setbacks[0].min(extents[axis[1]]),
        setbacks[1].min(extents[axis[0]]),
    ]
}

/// The setbacks of a corner cut on `faces`, clamped to the extent across each face.
#[must_use]
pub(super) fn clamp_corner_setbacks(
    faces: [BoxFace; 3],
    setbacks: [f64; 3],
    extents: [f64; 3],
) -> [f64; 3] {
    let mut clamped = setbacks;
    for (value, face) in clamped.iter_mut().zip(faces) {
        *value = value.min(extents[face.axis_and_side().0]);
    }
    clamped
}

/// A new edge cut on `faces` with the default setbacks (15 % of the shorter of the
/// two extents the setbacks run along).
#[must_use]
pub(super) fn default_edge(faces: [BoxFace; 2], extents: [f64; 3]) -> RoughCut {
    let axis = faces.map(|face| face.axis_and_side().0);
    let setback = default_setback(extents[axis[0]].min(extents[axis[1]]));
    RoughCut::Edge {
        faces,
        setbacks_mm: clamp_edge_setbacks(faces, [setback; 2], extents),
    }
}

/// A new corner cut on `faces` with the default setbacks (15 % of the shortest extent).
#[must_use]
pub(super) fn default_corner(faces: [BoxFace; 3], extents: [f64; 3]) -> RoughCut {
    let setback = default_setback(extents[0].min(extents[1]).min(extents[2]));
    RoughCut::Corner {
        faces,
        setbacks_mm: clamp_corner_setbacks(faces, [setback; 3], extents),
    }
}

/// How far the base's bounding box reaches along the unit `normal`.
#[must_use]
pub(super) fn extent_along(base: &RoughBase, normal: [f64; 3]) -> f64 {
    let extents = base.bounding_box_extents();
    normal
        .iter()
        .zip(extents)
        .fold(0.0, |sum, (n, e)| n.abs().mul_add(e, sum))
}

/// The default depth of a face cut with `normal`: 10 % of the extent along it, rounded
/// to 0.1 mm, at least 0.1 mm and less than the extent itself.
#[must_use]
pub(super) fn default_face_depth(base: &RoughBase, normal: [f64; 3]) -> f64 {
    let along = extent_along(base, normal);
    let depth = round_tenth(DEFAULT_DEPTH_SHARE * along).max(DEFAULT_STEP_MM);
    if depth < along { depth } else { along * 0.5 }
}

/// A new face cut with the given `normal` and the default depth for `base`.
#[must_use]
pub(super) fn default_face(base: &RoughBase, normal: [f64; 3]) -> RoughCut {
    RoughCut::Face {
        normal,
        depth_mm: default_face_depth(base, normal),
    }
}

/// Snaps float noise of a direction component to exactly zero.
fn snap(component: f64) -> f64 {
    if component.abs() < 1e-12 {
        0.0
    } else {
        component
    }
}

/// The grid the components of a face normal are rounded to before they enter the model.
const NORMAL_GRID: f64 = 1e12;

/// A direction as the model stores it: every component rounded to 1e-12 and the vector
/// scaled back to unit length with nothing but `sqrt` (correctly rounded on every
/// platform). The platform's `sin` and `cos` differ in the last bits; rounding to a grid
/// far coarser than that keeps those bits out of the model, so the planner sees the same
/// normal for the same typed angles everywhere. A zero or non-finite direction becomes
/// +Y.
#[must_use]
pub(super) fn canonical_normal(raw: [f64; 3]) -> [f64; 3] {
    let gridded = raw.map(|c| snap((c * NORMAL_GRID).round() / NORMAL_GRID));
    let length = gridded
        .iter()
        .fold(0.0_f64, |sum, c| c.mul_add(*c, sum))
        .sqrt();
    if length > 0.0 {
        gridded.map(|c| snap(c / length))
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// The unit outward normal for a direction given in degrees: elevation 90 is +Y,
/// azimuth 0 is +X and azimuth 90 turns toward +Z. The typed angle pair is the input;
/// the normal is its [`canonical_normal`], and the angles shown for it come back from
/// [`angles_from_normal`] (the azimuth of a vertical normal is not recoverable, which is
/// why the window remembers the typed one).
#[must_use]
pub(super) fn normal_from_angles(azimuth_deg: f64, elevation_deg: f64) -> [f64; 3] {
    let (sin_az, cos_az) = azimuth_deg.to_radians().sin_cos();
    let (sin_el, cos_el) = elevation_deg.to_radians().sin_cos();
    canonical_normal([cos_el * cos_az, sin_el, cos_el * sin_az])
}

/// Whether `normal` points straight up or down, where the azimuth has no effect (the
/// same test [`angles_from_normal`] uses to report azimuth 0).
#[must_use]
pub(super) fn is_vertical(normal: [f64; 3]) -> bool {
    let length = normal
        .iter()
        .fold(0.0_f64, |sum, c| c.mul_add(*c, sum))
        .sqrt();
    if length <= 0.0 {
        return true;
    }
    let [x, _, z] = normal.map(|c| c / length);
    snap(x) == 0.0 && snap(z) == 0.0
}

/// The `(azimuth, elevation)` in degrees of `normal` (the inverse of
/// [`normal_from_angles`]); the azimuth of a vertical normal is 0.
#[must_use]
pub(super) fn angles_from_normal(normal: [f64; 3]) -> (f64, f64) {
    let length = normal
        .iter()
        .fold(0.0_f64, |sum, c| c.mul_add(*c, sum))
        .sqrt();
    if length <= 0.0 {
        return (0.0, 90.0);
    }
    let [x, y, z] = normal.map(|c| c / length);
    let elevation = y.clamp(-1.0, 1.0).asin().to_degrees();
    let azimuth = if snap(x) == 0.0 && snap(z) == 0.0 {
        0.0
    } else {
        z.atan2(x).to_degrees()
    };
    (azimuth, elevation)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES_EDGES: [&str; 12] = [
        "Top-Front",
        "Top-Back",
        "Top-Left",
        "Top-Right",
        "Bottom-Front",
        "Bottom-Back",
        "Bottom-Left",
        "Bottom-Right",
        "Front-Left",
        "Front-Right",
        "Back-Left",
        "Back-Right",
    ];

    #[test]
    fn the_option_names_follow_the_contract_order() {
        assert_eq!(edge_names(), NAMES_EDGES);
        let corners = corner_names();
        assert_eq!(corners.len(), 8);
        assert_eq!(corners[0], "Top-Front-Left");
        assert_eq!(corners[1], "Top-Front-Right");
        assert_eq!(corners[7], "Bottom-Back-Right");
    }

    #[test]
    fn every_table_entry_maps_back_to_its_own_index() {
        for (i, faces) in EDGES.iter().enumerate() {
            assert_eq!(edge_index(*faces), Some(i));
            // The same edge named the other way round is still that edge.
            assert_eq!(edge_index([faces[1], faces[0]]), Some(i));
        }
        for (i, faces) in CORNERS.iter().enumerate() {
            assert_eq!(corner_index(*faces), Some(i));
            assert_eq!(corner_index([faces[2], faces[0], faces[1]]), Some(i));
        }
        assert_eq!(edge_index([BoxFace::Top, BoxFace::Bottom]), None);
        assert_eq!(edge_index([BoxFace::Top, BoxFace::Top]), None);
        assert_eq!(
            corner_index([BoxFace::Top, BoxFace::Bottom, BoxFace::Left]),
            None
        );
    }

    #[test]
    fn every_table_entry_is_a_valid_cut_for_the_core() {
        let base = RoughBase::Block {
            x_mm: 10.0,
            y_mm: 8.0,
            z_mm: 6.0,
        };
        let extents = base.bounding_box_extents();
        for faces in EDGES {
            let cut = default_edge(faces, extents);
            assert!(cut.to_halfspace(0, &base, &[]).is_ok(), "{faces:?}");
        }
        for faces in CORNERS {
            let cut = default_corner(faces, extents);
            assert!(cut.to_halfspace(0, &base, &[]).is_ok(), "{faces:?}");
        }
    }

    #[test]
    fn default_setbacks_are_fifteen_percent_of_the_shorter_extent() {
        // Top-Front runs across Y (8) and Z (6): 15 % of 6 is 0.9.
        let cut = default_edge([BoxFace::Top, BoxFace::Front], [10.0, 8.0, 6.0]);
        assert_eq!(
            cut,
            RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [0.9, 0.9],
            }
        );
        // A corner uses the shortest of all three: 15 % of 6.
        let corner = default_corner(CORNERS[DEFAULT_CORNER], [10.0, 8.0, 6.0]);
        let RoughCut::Corner { setbacks_mm, .. } = corner else {
            panic!("a corner default must be a corner cut");
        };
        assert_eq!(setbacks_mm, [0.9; 3]);
        // A tiny rough still gets a usable, in-range setback.
        assert!((default_setback(0.4) - 0.1).abs() < 1e-12);
        assert!((default_setback(0.05) - 0.05).abs() < 1e-12);
    }

    #[test]
    fn changing_the_faces_keeps_the_setbacks_clamped_to_the_new_extents() {
        // Faces Top-Front: setbacks[0] is limited by Z (2), setbacks[1] by Y (9).
        let clamped = clamp_edge_setbacks(
            [BoxFace::Top, BoxFace::Front],
            [5.0, 12.0],
            [20.0, 9.0, 2.0],
        );
        assert_eq!(clamped, [2.0, 9.0]);
        let corner = clamp_corner_setbacks(
            [BoxFace::Top, BoxFace::Front, BoxFace::Right],
            [30.0, 30.0, 30.0],
            [20.0, 9.0, 2.0],
        );
        // Top is Y, Front is Z, Right is X.
        assert_eq!(corner, [9.0, 2.0, 20.0]);
    }

    #[test]
    fn the_default_face_depth_is_ten_percent_of_the_extent_along_the_normal() {
        let base = RoughBase::Block {
            x_mm: 10.0,
            y_mm: 8.0,
            z_mm: 6.0,
        };
        assert!((default_face_depth(&base, [0.0, 1.0, 0.0]) - 0.8).abs() < 1e-12);
        assert!((default_face_depth(&base, [1.0, 0.0, 0.0]) - 1.0).abs() < 1e-12);
        let cut = default_face(&base, [0.0, 1.0, 0.0]);
        let base_verts = [
            glam::DVec3::new(0.0, 0.0, 0.0),
            glam::DVec3::new(10.0, 8.0, 6.0),
        ];
        assert!(cut.to_halfspace(0, &base, &base_verts).is_ok());
    }

    #[test]
    fn azimuth_and_elevation_give_the_documented_directions() {
        let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-12);
        assert!(close(normal_from_angles(0.0, 90.0), [0.0, 1.0, 0.0]));
        assert!(close(normal_from_angles(0.0, 0.0), [1.0, 0.0, 0.0]));
        assert!(close(normal_from_angles(90.0, 0.0), [0.0, 0.0, 1.0]));
        assert!(close(normal_from_angles(180.0, 0.0), [-1.0, 0.0, 0.0]));
        assert!(close(normal_from_angles(0.0, -90.0), [0.0, -1.0, 0.0]));
        let slanted = normal_from_angles(30.0, 45.0);
        let length = slanted.iter().map(|c| c * c).sum::<f64>().sqrt();
        assert!((length - 1.0).abs() < 1e-12, "the normal is a unit vector");
        assert!(slanted[1] > 0.0 && slanted[2] > 0.0);
    }

    #[test]
    fn a_canonical_normal_carries_no_trigonometric_noise() {
        // Noise far below the 1e-12 grid vanishes: the result is exactly +Y.
        assert_eq!(canonical_normal([3e-14, 1.0, -3e-14]), [0.0, 1.0, 0.0]);
        // 3-4-5: the components divide exactly by the length 5, and IEEE division is
        // correctly rounded, so the results are the doubles nearest 0.6 and 0.8.
        assert_eq!(canonical_normal([3.0, 4.0, 0.0]), [0.6, 0.8, 0.0]);
        // Zero and non-finite directions fall back to +Y.
        assert_eq!(canonical_normal([0.0; 3]), [0.0, 1.0, 0.0]);
        assert_eq!(canonical_normal([f64::NAN, 0.0, 0.0]), [0.0, 1.0, 0.0]);
        // The axis directions are exact whatever the platform's sin and cos return.
        assert_eq!(normal_from_angles(90.0, 0.0), [0.0, 0.0, 1.0]);
        assert_eq!(normal_from_angles(0.0, 90.0), [0.0, 1.0, 0.0]);
        assert_eq!(normal_from_angles(0.0, -90.0), [0.0, -1.0, 0.0]);
        assert_eq!(normal_from_angles(123.0, 90.0), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn only_straight_up_or_down_is_vertical() {
        assert!(is_vertical([0.0, 1.0, 0.0]));
        assert!(is_vertical([0.0, -1.0, 0.0]));
        assert!(is_vertical([0.0, 0.0, 0.0]));
        assert!(is_vertical([1e-14, 1.0, 0.0]));
        assert!(!is_vertical([1.0, 0.0, 0.0]));
        assert!(!is_vertical(normal_from_angles(30.0, 89.0)));
    }

    #[test]
    fn angles_and_normals_round_trip() {
        for (az, el) in [
            (0.0, 90.0),
            (30.0, 45.0),
            (-120.0, -10.0),
            (90.0, 0.0),
            (179.0, 60.0),
        ] {
            let n = normal_from_angles(az, el);
            let (az2, el2) = angles_from_normal(n);
            assert!((el - el2).abs() < 1e-9, "elevation {el} came back as {el2}");
            if el.abs() < 89.0 {
                assert!((az - az2).abs() < 1e-9, "azimuth {az} came back as {az2}");
            }
        }
        let (az, el) = angles_from_normal([0.0, 1.0, 0.0]);
        assert!(az.abs() < 1e-12 && (el - 90.0).abs() < 1e-9);
    }
}
