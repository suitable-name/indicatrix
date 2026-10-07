//! The cut list as window rows: one row per cut, and the reverse mapping of an edited
//! field back onto a cut.

use super::cut_faces::{
    angles_from_normal, corner_index, edge_index, faces_label, is_vertical, normal_from_angles,
};
use crate::RoughCutRow;
use indicatrix_cut_core::rough_plan::{RoughCut, RoughModel};
use slint::{ModelRc, SharedString, VecModel};
use std::collections::BTreeMap;

/// The hint a face row shows while its direction is vertical, where the azimuth does
/// not matter.
pub(super) const POLE_HINT: &str = "azimuth has no effect at elevation \u{b1}90";

/// The azimuth range a face row accepts, in degrees.
const AZIMUTH_LIMIT_DEG: f64 = 180.0;

/// The elevation range a face row accepts, in degrees.
const ELEVATION_LIMIT_DEG: f64 = 90.0;

/// `value` with at most `decimals` decimals and no trailing zeros ("3.5", "12").
#[must_use]
pub(super) fn fmt_num(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    let trimmed = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    };
    if trimmed == "-0" {
        "0".to_string()
    } else {
        trimmed
    }
}

/// The window row of one cut, without any status text. `typed_azimuth` is the azimuth
/// the user typed for a face cut: it is shown while the face is vertical, where the
/// normal itself cannot tell the azimuth (and the row's hint says it has no effect).
#[must_use]
pub(super) fn cut_row(cut: &RoughCut, typed_azimuth: Option<f64>) -> RoughCutRow {
    let mut row = RoughCutRow::default();
    match cut {
        RoughCut::Edge { faces, setbacks_mm } => {
            row.kind = 0;
            row.title = format!("Edge {}", faces_label(faces)).into();
            row.a = fmt_num(setbacks_mm[0], 2).into();
            row.b = fmt_num(setbacks_mm[1], 2).into();
            row.face_index = edge_index(*faces).map_or(-1, super::format::to_i32);
        }
        RoughCut::Corner { faces, setbacks_mm } => {
            row.kind = 1;
            row.title = format!("Corner {}", faces_label(faces)).into();
            row.a = fmt_num(setbacks_mm[0], 2).into();
            row.b = fmt_num(setbacks_mm[1], 2).into();
            row.c = fmt_num(setbacks_mm[2], 2).into();
            row.face_index = corner_index(*faces).map_or(-1, super::format::to_i32);
        }
        RoughCut::Face { normal, depth_mm } => {
            let (derived, elevation) = angles_from_normal(*normal);
            let vertical = is_vertical(*normal);
            let azimuth = if vertical {
                typed_azimuth.unwrap_or(derived)
            } else {
                derived
            };
            row.kind = 2;
            row.title = "Face".into();
            if vertical {
                row.hint = POLE_HINT.into();
            }
            row.azimuth = fmt_num(azimuth, 1).into();
            row.elevation = fmt_num(elevation, 1).into();
            row.depth = fmt_num(*depth_mm, 2).into();
            row.face_index = -1;
        }
    }
    row
}

/// A new row model for the whole cut list (the components are re-created).
/// `typed_azimuths` holds the azimuth typed for a face row, by cut index.
#[must_use]
pub(super) fn cut_rows(
    model: &RoughModel,
    typed_azimuths: &BTreeMap<usize, f64>,
) -> ModelRc<RoughCutRow> {
    let rows: Vec<RoughCutRow> = model
        .cuts
        .iter()
        .enumerate()
        .map(|(index, cut)| cut_row(cut, typed_azimuths.get(&index).copied()))
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Which value of a cut row was edited (the field numbers of the window callback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CutField {
    /// The first setback.
    A,
    /// The second setback.
    B,
    /// The third setback (corners).
    C,
    /// The face direction's azimuth in degrees.
    Azimuth,
    /// The face direction's elevation in degrees.
    Elevation,
    /// The face depth in mm.
    Depth,
}

impl CutField {
    /// The field for the window's number (0 a, 1 b, 2 c, 3 azimuth, 4 elevation, 5 depth).
    #[must_use]
    pub(super) const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::A),
            1 => Some(Self::B),
            2 => Some(Self::C),
            3 => Some(Self::Azimuth),
            4 => Some(Self::Elevation),
            5 => Some(Self::Depth),
            _ => None,
        }
    }

    /// The window's number for the field (the inverse of [`CutField::from_index`]).
    #[must_use]
    pub(super) const fn number(self) -> i32 {
        match self {
            Self::A => 0,
            Self::B => 1,
            Self::C => 2,
            Self::Azimuth => 3,
            Self::Elevation => 4,
            Self::Depth => 5,
        }
    }

    /// The name used in "X must be a number" messages.
    #[must_use]
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::A => "Setback A",
            Self::B => "Setback B",
            Self::C => "Setback C",
            Self::Azimuth => "Azimuth",
            Self::Elevation => "Elevation",
            Self::Depth => "Depth",
        }
    }

    /// The unit the field is measured in, as the "must be a number in ..." message names
    /// it: degrees for the face direction's angles, millimetres for every length.
    #[must_use]
    pub(super) const fn unit(self) -> &'static str {
        match self {
            Self::Azimuth | Self::Elevation => "degrees",
            Self::A | Self::B | Self::C | Self::Depth => "mm",
        }
    }
}

/// The message for a field the cut's kind does not have.
fn no_such_field(field: CutField) -> String {
    format!("This cut has no {} field.", field.label())
}

/// Writes an azimuth or elevation of `value` degrees into the face cut `cut` and returns
/// the azimuth the face is now defined by.
///
/// A vertical face cannot tell its azimuth, so `typed_azimuth` (what the user typed
/// earlier) stands in for it: editing the elevation of a vertical face keeps that
/// azimuth instead of falling back to 0, and editing the azimuth of a vertical face
/// leaves the normal as it is while the returned azimuth is remembered by the caller.
fn apply_angle(
    cut: &mut RoughCut,
    field: CutField,
    value: f64,
    typed_azimuth: Option<f64>,
) -> Result<f64, String> {
    let RoughCut::Face { normal, .. } = cut else {
        return Err(no_such_field(field));
    };
    let limit = if field == CutField::Azimuth {
        AZIMUTH_LIMIT_DEG
    } else {
        ELEVATION_LIMIT_DEG
    };
    if !(-limit..=limit).contains(&value) {
        return Err(format!(
            "{} must be between -{limit} and {limit} degrees.",
            field.label()
        ));
    }
    let (derived, current_elevation) = angles_from_normal(*normal);
    let current_azimuth = if is_vertical(*normal) {
        typed_azimuth.unwrap_or(derived)
    } else {
        derived
    };
    let (azimuth, elevation) = if field == CutField::Azimuth {
        (value, current_elevation)
    } else {
        (current_azimuth, value)
    };
    *normal = normal_from_angles(azimuth, elevation);
    Ok(azimuth)
}

/// Writes `value` into `field` of `cut`. For an azimuth or elevation the answer is the
/// azimuth the face is defined by now (see [`apply_angle`]); for any other field `None`.
///
/// # Errors
///
/// Returns a message when `cut` is of a kind that does not have `field`, or when an
/// angle is outside -180..=180 (azimuth) or -90..=90 (elevation) degrees; the cut is
/// left as it was.
pub(super) fn apply_field(
    cut: &mut RoughCut,
    field: CutField,
    value: f64,
    typed_azimuth: Option<f64>,
) -> Result<Option<f64>, String> {
    match field {
        CutField::A | CutField::B | CutField::C => {
            let slot = match field {
                CutField::A => 0,
                CutField::B => 1,
                _ => 2,
            };
            let setback = setbacks_mut(cut)
                .and_then(|setbacks| setbacks.get_mut(slot))
                .ok_or_else(|| no_such_field(field))?;
            *setback = value;
            Ok(None)
        }
        CutField::Azimuth | CutField::Elevation => {
            apply_angle(cut, field, value, typed_azimuth).map(Some)
        }
        CutField::Depth => {
            if let RoughCut::Face { depth_mm, .. } = cut {
                *depth_mm = value;
                Ok(None)
            } else {
                Err(no_such_field(field))
            }
        }
    }
}

/// The text `row` shows for `field`.
#[must_use]
pub(super) fn field_text(row: &RoughCutRow, field: CutField) -> SharedString {
    match field {
        CutField::A => row.a.clone(),
        CutField::B => row.b.clone(),
        CutField::C => row.c.clone(),
        CutField::Azimuth => row.azimuth.clone(),
        CutField::Elevation => row.elevation.clone(),
        CutField::Depth => row.depth.clone(),
    }
}

/// Puts `text` into the `field` of `row`.
pub(super) fn set_field_text(row: &mut RoughCutRow, field: CutField, text: &str) {
    let slot = match field {
        CutField::A => &mut row.a,
        CutField::B => &mut row.b,
        CutField::C => &mut row.c,
        CutField::Azimuth => &mut row.azimuth,
        CutField::Elevation => &mut row.elevation,
        CutField::Depth => &mut row.depth,
    };
    *slot = text.into();
}

/// The setbacks of an edge or corner cut.
const fn setbacks_mut(cut: &mut RoughCut) -> Option<&mut [f64]> {
    match cut {
        RoughCut::Edge { setbacks_mm, .. } => Some(setbacks_mm.as_mut_slice()),
        RoughCut::Corner { setbacks_mm, .. } => Some(setbacks_mm),
        RoughCut::Face { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::{BoxFace, RoughBase};
    use slint::Model;

    fn model() -> RoughModel {
        RoughModel::new(
            RoughBase::Block {
                x_mm: 10.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            vec![
                RoughCut::Edge {
                    faces: [BoxFace::Front, BoxFace::Top],
                    setbacks_mm: [1.5, 2.0],
                },
                RoughCut::Corner {
                    faces: [BoxFace::Top, BoxFace::Front, BoxFace::Right],
                    setbacks_mm: [0.9, 1.0, 1.25],
                },
                RoughCut::Face {
                    normal: [0.0, 1.0, 0.0],
                    depth_mm: 0.8,
                },
            ],
        )
    }

    #[test]
    fn numbers_drop_trailing_zeros() {
        assert_eq!(fmt_num(3.5, 2), "3.5");
        assert_eq!(fmt_num(12.0, 2), "12");
        assert_eq!(fmt_num(0.126, 2), "0.13");
        assert_eq!(fmt_num(-0.001, 1), "0");
        assert_eq!(fmt_num(90.0, 1), "90");
        assert_eq!(fmt_num(-30.04, 1), "-30");
    }

    #[test]
    fn rows_show_the_titles_values_and_drop_down_indices() {
        let model = model();
        let edge = cut_row(&model.cuts[0], None);
        assert_eq!(edge.kind, 0);
        assert_eq!(edge.title.as_str(), "Edge Front-Top");
        assert_eq!((edge.a.as_str(), edge.b.as_str()), ("1.5", "2"));
        // Front-Top is the Top-Front edge whichever way round it is named.
        assert_eq!(edge.face_index, 0);

        let corner = cut_row(&model.cuts[1], None);
        assert_eq!(corner.kind, 1);
        assert_eq!(corner.title.as_str(), "Corner Top-Front-Right");
        assert_eq!(corner.c.as_str(), "1.25");
        assert_eq!(corner.face_index, 1);

        // The model's face cut points straight up, so its row carries the hint while the
        // title stays the plain cut name.
        let face = cut_row(&model.cuts[2], None);
        assert_eq!(face.kind, 2);
        assert_eq!(face.title.as_str(), "Face");
        assert_eq!(
            face.hint.as_str(),
            "azimuth has no effect at elevation \u{b1}90"
        );
        assert_eq!((edge.hint.as_str(), corner.hint.as_str()), ("", ""));
        assert_eq!(face.face_index, -1);
        assert_eq!(face.azimuth.as_str(), "0");
        assert_eq!(face.elevation.as_str(), "90");
        assert_eq!(face.depth.as_str(), "0.8");
    }

    #[test]
    fn an_edited_field_lands_in_the_right_place_and_only_there() {
        let mut model = model();
        apply_field(&mut model.cuts[0], CutField::B, 3.0, None)
            .expect("edges have a second setback");
        assert_eq!(
            model.cuts[0],
            RoughCut::Edge {
                faces: [BoxFace::Front, BoxFace::Top],
                setbacks_mm: [1.5, 3.0],
            }
        );
        apply_field(&mut model.cuts[1], CutField::C, 2.5, None)
            .expect("corners have a third setback");
        let RoughCut::Corner { setbacks_mm, .. } = &model.cuts[1] else {
            panic!("still a corner");
        };
        assert_eq!(*setbacks_mm, [0.9, 1.0, 2.5]);
        assert!(apply_field(&mut model.cuts[0], CutField::C, 1.0, None).is_err());
        assert!(apply_field(&mut model.cuts[2], CutField::A, 1.0, None).is_err());
        assert!(apply_field(&mut model.cuts[0], CutField::Depth, 1.0, None).is_err());
        assert!(apply_field(&mut model.cuts[0], CutField::Azimuth, 10.0, None).is_err());
    }

    fn slanted_face() -> RoughCut {
        RoughCut::Face {
            normal: normal_from_angles(30.0, 45.0),
            depth_mm: 1.0,
        }
    }

    fn face_normal(cut: &RoughCut) -> [f64; 3] {
        let RoughCut::Face { normal, .. } = cut else {
            panic!("a face cut");
        };
        *normal
    }

    #[test]
    fn editing_one_angle_keeps_the_other() {
        let mut cut = slanted_face();
        let used =
            apply_field(&mut cut, CutField::Azimuth, 100.0, None).expect("faces have an azimuth");
        assert_eq!(used, Some(100.0), "an azimuth edit defines the azimuth");
        let (azimuth, elevation) = angles_from_normal(face_normal(&cut));
        assert!((azimuth - 100.0).abs() < 1e-9);
        assert!((elevation - 45.0).abs() < 1e-9);
    }

    #[test]
    fn angles_outside_their_range_are_refused_and_change_nothing() {
        // Azimuth takes -180..=180 and elevation -90..=90; the limits themselves are fine.
        for (field, value) in [
            (CutField::Azimuth, 180.5),
            (CutField::Azimuth, -181.0),
            (CutField::Azimuth, 1e300),
            (CutField::Elevation, 90.5),
            (CutField::Elevation, -91.0),
            (CutField::Elevation, 1e300),
        ] {
            let mut cut = slanted_face();
            let error = apply_field(&mut cut, field, value, None).unwrap_err();
            assert!(error.contains(field.label()), "{error}");
            assert_eq!(cut, slanted_face(), "{field:?} {value}");
        }
        let mut cut = slanted_face();
        assert!(apply_field(&mut cut, CutField::Azimuth, -180.0, None).is_ok());
        assert!(apply_field(&mut cut, CutField::Azimuth, 180.0, None).is_ok());
        assert!(apply_field(&mut cut, CutField::Elevation, -90.0, None).is_ok());
        assert!(apply_field(&mut cut, CutField::Elevation, 90.0, None).is_ok());
        assert_eq!(
            apply_field(&mut slanted_face(), CutField::Elevation, 90.5, None).unwrap_err(),
            "Elevation must be between -90 and 90 degrees."
        );
    }

    #[test]
    fn the_azimuth_survives_a_visit_to_the_pole() {
        let mut cut = slanted_face();
        // Up to the pole: the azimuth in force is the face's own, about 30 degrees.
        let at_pole = apply_field(&mut cut, CutField::Elevation, 90.0, None)
            .expect("faces have an elevation")
            .expect("an elevation edit names the azimuth");
        assert!((at_pole - 30.0).abs() < 1e-9);
        assert_eq!(face_normal(&cut), [0.0, 1.0, 0.0]);
        // Typing an azimuth there changes nothing in the normal ...
        let typed = apply_field(&mut cut, CutField::Azimuth, 45.0, Some(at_pole))
            .expect("faces have an azimuth");
        assert_eq!(typed, Some(45.0));
        assert_eq!(face_normal(&cut), [0.0, 1.0, 0.0]);
        // ... but it is what the face turns to when it leaves the pole again.
        apply_field(&mut cut, CutField::Elevation, 45.0, typed).expect("faces have an elevation");
        let (azimuth, elevation) = angles_from_normal(face_normal(&cut));
        assert!((azimuth - 45.0).abs() < 1e-9, "azimuth {azimuth}");
        assert!((elevation - 45.0).abs() < 1e-9, "elevation {elevation}");
    }

    #[test]
    fn a_vertical_face_row_shows_the_typed_azimuth_and_the_hint() {
        let up = RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 1.0,
        };
        let row = cut_row(&up, Some(45.0));
        assert_eq!(row.azimuth.as_str(), "45");
        assert_eq!(row.elevation.as_str(), "90");
        assert_eq!(row.hint.as_str(), POLE_HINT);
        assert_eq!(row.title.as_str(), "Face");
        // Off the pole the normal decides and there is no hint.
        let row = cut_row(&slanted_face(), Some(99.0));
        assert_eq!(row.azimuth.as_str(), "30");
        assert_eq!(row.title.as_str(), "Face");
        assert_eq!(row.hint.as_str(), "");
        // The rows of a whole model look the remembered azimuth up by cut index.
        let typed = BTreeMap::from([(2, 45.0)]);
        let rows = cut_rows(&model(), &typed);
        let face = rows.row_data(2).expect("the model has three cuts");
        assert_eq!(face.azimuth.as_str(), "45");
        let edge = rows.row_data(0).expect("the model has three cuts");
        assert_eq!(edge.title.as_str(), "Edge Front-Top");
    }

    #[test]
    fn a_row_field_is_read_and_written_by_its_field() {
        let mut row = cut_row(&model().cuts[1], None);
        assert_eq!(field_text(&row, CutField::C).as_str(), "1.25");
        set_field_text(&mut row, CutField::C, "x");
        assert_eq!(row.c.as_str(), "x");
        assert_eq!(row.a.as_str(), "0.9", "the other fields stay");
        let mut face = cut_row(&model().cuts[2], None);
        set_field_text(&mut face, CutField::Depth, "2");
        assert_eq!(field_text(&face, CutField::Depth).as_str(), "2");
    }

    #[test]
    fn a_field_that_is_not_a_number_names_its_own_unit() {
        use crate::gui::rough_plan::inputs::parse_with_unit;
        let message = |field: CutField| {
            parse_with_unit("abc", field.label(), field.unit()).expect_err("not a number")
        };
        assert_eq!(
            message(CutField::Azimuth),
            "Azimuth must be a number in degrees."
        );
        assert_eq!(
            message(CutField::Elevation),
            "Elevation must be a number in degrees."
        );
        for (field, text) in [
            (CutField::A, "Setback A must be a number in mm."),
            (CutField::B, "Setback B must be a number in mm."),
            (CutField::C, "Setback C must be a number in mm."),
            (CutField::Depth, "Depth must be a number in mm."),
        ] {
            assert_eq!(message(field), text);
        }
        // The decimal comma and the finite check do not depend on the unit.
        assert_eq!(parse_with_unit("-12,5", "Azimuth", "degrees"), Ok(-12.5));
        assert_eq!(
            parse_with_unit("inf", "Elevation", "degrees"),
            Err("Elevation must be a finite number.".to_string())
        );
    }

    #[test]
    fn the_window_field_numbers_map_to_fields() {
        assert_eq!(CutField::from_index(0), Some(CutField::A));
        assert_eq!(CutField::from_index(5), Some(CutField::Depth));
        assert_eq!(CutField::from_index(6), None);
        assert_eq!(CutField::from_index(-1), None);
        for number in 0..=5 {
            let field = CutField::from_index(number).expect("0 to 5 are fields");
            assert_eq!(field.number(), number);
        }
    }
}
