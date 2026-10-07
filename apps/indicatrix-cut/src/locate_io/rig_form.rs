//! The text form of a camera rig: what the "Edit rig" window shows and parses.
//!
//! A rig is typed by hand (hand-entered values work without any calibration), so every number
//! is a text field. [`RigForm`] holds the texts, [`RigForm::to_profile`] turns them back into a
//! [`RigProfile`] with a message naming the field when one is wrong, and
//! [`layout_views`] fills the eight views of the default layout from five numbers.

use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::rough_plan::locate::{CubeSpec, Projection, RigProfile, ViewPose};

/// The labels of the fifteen fields of one view, in the order of [`ViewForm::fields`].
pub const FIELD_LABELS: [&str; 15] = [
    "Name", "Pos X", "Pos Y", "Pos Z", "Look X", "Look Y", "Look Z", "Up X", "Up Y", "Up Z",
    "Scale", "Pt u", "Pt v", "Width", "Height",
];

/// Field positions inside [`ViewForm::fields`].
const NAME: usize = 0;
const POSITION: usize = 1;
const LOOK: usize = 4;
const UP: usize = 7;
const SCALE: usize = 10;
const PRINCIPAL: usize = 11;
const SIZE: usize = 13;

/// The stone index used for a material the built-in table does not know (a catalogue
/// material): a typical gem value, to be overtyped.
const FALLBACK_STONE_N: f64 = 1.54;

/// Defaults of the layout generator: camera distance in mm, elevation in degrees, focal length
/// in pixels, image width and height in pixels.
pub const DEFAULT_LAYOUT: [&str; 5] = ["150", "30", "4000", "4000", "3000"];

/// A decimal number typed by the user: a decimal comma is accepted, surrounding space is not
/// part of it, and it must be finite. `what` names the field in the message.
pub fn parse_number(text: &str, what: &str) -> Result<f64, String> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{what} is not a number."))
}

/// A number for a text field: at most four decimals, shortest form.
#[must_use]
pub fn format_number(value: f64) -> String {
    let rounded = (value * 1.0e4).round() / 1.0e4;
    // A value that rounds to zero is "0", never "-0".
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    format!("{rounded}")
}

/// The refractive index `n_d` of the built-in material called `name`, or a typical gem value
/// when it is not one of them. For a birefringent material this is the ordinary index.
#[must_use]
pub fn default_stone_n(name: &str) -> f64 {
    GemMaterial::all_materials()
        .into_iter()
        .find(|material| material.name.eq_ignore_ascii_case(name))
        .map_or(FALLBACK_STONE_N, |material| {
            f64::from(material.dispersion.n_d())
        })
}

/// One view as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewForm {
    /// The fifteen fields, in the order of [`FIELD_LABELS`]. "Scale" is the focal length in
    /// pixels, or pixels per millimetre when the view is orthographic.
    pub fields: Vec<String>,
    /// Whether the view is orthographic (a telecentric or macro lens).
    pub orthographic: bool,
}

impl ViewForm {
    /// The text form of `pose`.
    #[must_use]
    pub fn from_pose(pose: &ViewPose) -> Self {
        let mut fields = Vec::with_capacity(FIELD_LABELS.len());
        fields.push(pose.name.clone());
        for value in pose.position.iter().chain(&pose.forward).chain(&pose.up) {
            fields.push(format_number(*value));
        }
        fields.push(format_number(pose.projection.scale_px()));
        for value in pose.principal_point {
            fields.push(format_number(value));
        }
        for side in pose.image_size {
            fields.push(side.to_string());
        }
        Self {
            fields,
            orthographic: matches!(pose.projection, Projection::Orthographic { .. }),
        }
    }

    /// The pose this text describes. `index` is the 0-based view, for the messages.
    pub fn to_pose(&self, index: usize) -> Result<ViewPose, String> {
        let number = |at: usize| {
            let text = self.fields.get(at).map_or("", String::as_str);
            parse_number(text, &format!("View {}: {}", index + 1, FIELD_LABELS[at]))
        };
        let triple = |first: usize| -> Result<[f64; 3], String> {
            Ok([number(first)?, number(first + 1)?, number(first + 2)?])
        };
        let scale = number(SCALE)?;
        if scale <= 0.0 {
            return Err(format!("View {}: the scale must be positive.", index + 1));
        }
        let side = |at: usize| -> Result<u32, String> {
            whole_pixels(number(at)?).ok_or_else(|| {
                format!(
                    "View {}: {} must be a whole number of pixels.",
                    index + 1,
                    FIELD_LABELS[at]
                )
            })
        };
        let projection = if self.orthographic {
            Projection::Orthographic { px_per_mm: scale }
        } else {
            Projection::Pinhole { focal_px: scale }
        };
        let name = self.fields.get(NAME).map_or("", |text| text.trim());
        Ok(ViewPose {
            name: if name.is_empty() {
                format!("View {}", index + 1)
            } else {
                name.to_owned()
            },
            position: triple(POSITION)?,
            forward: triple(LOOK)?,
            up: triple(UP)?,
            projection,
            principal_point: [number(PRINCIPAL)?, number(PRINCIPAL + 1)?],
            image_size: [side(SIZE)?, side(SIZE + 1)?],
        })
    }
}

/// A whole rig as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigForm {
    /// The rig's name.
    pub name: String,
    /// The stone's refractive index (the ordinary index for a birefringent stone).
    pub stone_n: String,
    /// The refractive index around the stone: 1 for air, the liquid's in immersion.
    pub surround_n: String,
    /// The views.
    pub views: Vec<ViewForm>,
}

impl RigForm {
    /// The text form of `profile`.
    #[must_use]
    pub fn from_profile(profile: &RigProfile) -> Self {
        Self {
            name: profile.name.clone(),
            stone_n: format_number(profile.stone_n),
            surround_n: format_number(profile.surround_n),
            views: profile.views.iter().map(ViewForm::from_pose).collect(),
        }
    }

    /// A new rig in air with the default eight-view layout, for a stone of index `stone_n`.
    ///
    /// # Panics
    ///
    /// Never: the default layout texts are valid numbers.
    #[must_use]
    pub fn blank(name: &str, stone_n: f64) -> Self {
        let views = layout_views(
            DEFAULT_LAYOUT[0],
            DEFAULT_LAYOUT[1],
            DEFAULT_LAYOUT[2],
            DEFAULT_LAYOUT[3],
            DEFAULT_LAYOUT[4],
            false,
        )
        .expect("the default layout is valid");
        Self {
            name: name.to_owned(),
            stone_n: format_number(stone_n),
            surround_n: "1".to_owned(),
            views,
        }
    }

    /// The profile this text describes.
    ///
    /// `original` is the stored profile the form was opened from: its calibration is kept
    /// when the views are unchanged and dropped when they were edited, because a calibration
    /// describes the poses it measured.
    pub fn to_profile(&self, original: Option<&RigProfile>) -> Result<RigProfile, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Give the rig a name.".to_owned());
        }
        let views = self
            .views
            .iter()
            .enumerate()
            .map(|(index, view)| view.to_pose(index))
            .collect::<Result<Vec<_>, _>>()?;
        let mut profile = RigProfile::new(
            name,
            views,
            parse_number(&self.stone_n, "The stone's refractive index")?,
        );
        profile.surround_n = parse_number(&self.surround_n, "The surrounding refractive index")?;
        profile.validate().map_err(|error| error.to_string())?;
        if let Some(stored) = original
            && same_views(&stored.views, &profile.views)
        {
            // The poses were not edited (the text only keeps four decimals): keep the stored
            // ones at full precision, and their calibration.
            profile.views.clone_from(&stored.views);
            profile.calibration.clone_from(&stored.calibration);
        }
        Ok(profile)
    }
}

/// How far a number may differ from its stored value and still count as unedited: the text
/// keeps four decimals.
const TEXT_TOLERANCE: f64 = 1.0e-3;

/// Whether the views are the same poses, up to the rounding of their text.
fn same_views(stored: &[ViewPose], edited: &[ViewPose]) -> bool {
    let close = |left: &[f64], right: &[f64]| {
        left.iter()
            .zip(right)
            .all(|(a, b)| (a - b).abs() <= TEXT_TOLERANCE)
    };
    stored.len() == edited.len()
        && stored.iter().zip(edited).all(|(a, b)| {
            a.name == b.name
                && a.image_size == b.image_size
                && std::mem::discriminant(&a.projection) == std::mem::discriminant(&b.projection)
                && close(&[a.projection.scale_px()], &[b.projection.scale_px()])
                && close(&a.position, &b.position)
                && close(&a.forward, &b.forward)
                && close(&a.up, &b.up)
                && close(&a.principal_point, &b.principal_point)
        })
}

/// The eight views of the default layout (+X, -X, +Y and -Y, each from an upper and a lower
/// camera) from the five numbers of the layout generator: distance in mm, elevation in
/// degrees, scale (focal length in pixels, or pixels per millimetre), image width and height.
pub fn layout_views(
    distance: &str,
    elevation: &str,
    scale: &str,
    width: &str,
    height: &str,
    orthographic: bool,
) -> Result<Vec<ViewForm>, String> {
    let distance = parse_number(distance, "The distance")?;
    let elevation = parse_number(elevation, "The elevation")?;
    let scale = parse_number(scale, "The scale")?;
    if distance <= 0.0 || scale <= 0.0 {
        return Err("The distance and the scale must be positive.".to_owned());
    }
    let size = [
        pixel_count(width, "The image width")?,
        pixel_count(height, "The image height")?,
    ];
    let projection = if orthographic {
        Projection::Orthographic { px_per_mm: scale }
    } else {
        Projection::Pinhole { focal_px: scale }
    };
    Ok(
        RigProfile::side_layout(distance, elevation, projection, size)
            .iter()
            .map(ViewForm::from_pose)
            .collect(),
    )
}

/// `value` as a count of pixels: a whole number of at least 1.
fn whole_pixels(value: f64) -> Option<u32> {
    (value >= 1.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX)).then_some(value as u32)
}

/// The calibration cube from the window's fields: datasheet edge and tolerance in mm, the
/// glass's `n_d`, the rig axis (0, 1 or 2) the coated diagonal is parallel to, and whether it
/// runs the other way round.
pub fn cube_from_texts(
    edge: &str,
    tolerance: &str,
    n_d: &str,
    diagonal_axis: i32,
    mirrored: bool,
) -> Result<CubeSpec, String> {
    let cube = CubeSpec {
        edge_mm: parse_number(edge, "The cube's edge")?,
        tolerance_mm: parse_number(tolerance, "The cube's tolerance")?,
        n_d: parse_number(n_d, "The glass index")?,
        diagonal_axis: u8::try_from(diagonal_axis)
            .ok()
            .filter(|axis| *axis < 3)
            .ok_or_else(|| "Pick the axis the coated diagonal is parallel to.".to_owned())?,
        mirrored,
    };
    if cube.edge_mm <= 0.0 {
        return Err("The cube's edge must be positive.".to_owned());
    }
    if cube.tolerance_mm < 0.0 {
        return Err("The cube's tolerance cannot be negative.".to_owned());
    }
    if cube.n_d < 1.0 {
        return Err("The glass index must be at least 1.".to_owned());
    }
    Ok(cube)
}

/// How well the focal lengths are known, as the fraction the calibration takes, from the
/// window's percent field.
pub fn focal_sigma_from_percent(percent: &str) -> Result<f64, String> {
    let value = parse_number(percent, "The focal length's uncertainty")?;
    if (0.01..=100.0).contains(&value) {
        Ok(value / 100.0)
    } else {
        Err("The focal length's uncertainty must be between 0.01 and 100 percent.".to_owned())
    }
}

/// A whole number of pixels typed in a field, at least 1.
fn pixel_count(text: &str, what: &str) -> Result<u32, String> {
    whole_pixels(parse_number(text, what)?)
        .ok_or_else(|| format!("{what} must be a whole number of pixels."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::locate::{CalibrationResult, RigProfile};

    fn blank_profile() -> RigProfile {
        RigForm::blank("Bench", 1.76)
            .to_profile(None)
            .expect("valid")
    }

    #[test]
    fn numbers_accept_a_decimal_comma_and_reject_junk() {
        assert_eq!(parse_number(" 1,5 ", "n"), Ok(1.5));
        assert_eq!(parse_number("2e2", "n"), Ok(200.0));
        assert!(parse_number("", "n").is_err());
        assert!(parse_number("abc", "n").unwrap_err().contains('n'));
        assert!(parse_number("inf", "n").is_err());
        assert!(parse_number("NaN", "n").is_err());
    }

    #[test]
    fn numbers_are_written_short() {
        assert_eq!(format_number(3500.0), "3500");
        assert_eq!(format_number(0.123_456_789), "0.1235");
        assert_eq!(format_number(-0.000_001), "0");
    }

    #[test]
    fn the_default_layout_has_eight_views_and_round_trips_through_text() {
        let form = RigForm::blank("Bench", 1.76);
        assert_eq!(form.views.len(), 8);
        assert!(
            form.views
                .iter()
                .all(|v| v.fields.len() == FIELD_LABELS.len())
        );
        let profile = form.to_profile(None).expect("valid");
        assert_eq!(profile.views.len(), 8);
        assert_eq!(profile.views[0].name, "+X upper");
        assert_eq!(profile.views[7].name, "-Y lower");
        assert_eq!(profile.surround_n, 1.0);
        assert_eq!(profile.stone_n, 1.76);
        // The text of a profile is the profile again, to the four decimals the text keeps.
        let again = RigForm::from_profile(&profile)
            .to_profile(None)
            .expect("valid");
        for (left, right) in profile.views.iter().zip(&again.views) {
            for axis in 0..3 {
                assert!((left.position[axis] - right.position[axis]).abs() < 1e-3);
            }
            assert_eq!(left.image_size, right.image_size);
        }
    }

    #[test]
    fn a_bad_field_is_named_with_its_view() {
        let mut form = RigForm::blank("Bench", 1.76);
        form.views[2].fields[1] = "far".to_owned();
        let message = form.to_profile(None).unwrap_err();
        assert!(message.contains("View 3"), "{message}");
        assert!(message.contains("Pos X"), "{message}");

        let mut form = RigForm::blank("Bench", 1.76);
        form.views[0].fields[13] = "4000.5".to_owned();
        assert!(form.to_profile(None).unwrap_err().contains("Width"));

        let mut form = RigForm::blank("Bench", 1.76);
        form.stone_n = "0.5".to_owned();
        assert!(
            form.to_profile(None).is_err(),
            "an index below 1 is refused"
        );

        let mut form = RigForm::blank("  ", 1.76);
        assert!(form.to_profile(None).unwrap_err().contains("name"));
        form.name = "ok".to_owned();
        assert!(form.to_profile(None).is_ok());
    }

    #[test]
    fn an_orthographic_view_reads_its_scale_as_pixels_per_millimetre() {
        let mut form = RigForm::blank("Macro", 1.54);
        form.views[0].orthographic = true;
        form.views[0].fields[10] = "20".to_owned();
        let profile = form.to_profile(None).expect("valid");
        assert_eq!(
            profile.views[0].projection,
            Projection::Orthographic { px_per_mm: 20.0 }
        );
        assert!(matches!(
            profile.views[1].projection,
            Projection::Pinhole { .. }
        ));
    }

    #[test]
    fn a_calibration_is_kept_for_unchanged_views_and_dropped_for_edited_ones() {
        let mut stored = blank_profile();
        stored.calibration = Some(CalibrationResult {
            datasheet_edge_mm: 25.4,
            tolerance_mm: 0.1,
            cube_n_d: 1.5168,
            fitted_edge_mm: 25.41,
            scale_ratio: 1.0004,
            scale_within_tolerance: true,
            edge_rms_px: 0.5,
            lines_used: 30,
            diagonal_rms_mm: Some(0.1),
            diagonal_plane_rms_mm: Some(0.05),
        });
        let mut form = RigForm::from_profile(&stored);
        let kept = form.to_profile(Some(&stored)).expect("valid");
        assert_eq!(kept.calibration, stored.calibration);

        form.views[0].fields[1] = "151".to_owned();
        let edited = form.to_profile(Some(&stored)).expect("valid");
        assert_eq!(edited.calibration, None);

        // A calibrated rig has poses at full precision, which the text only shows to four
        // decimals: saving it unedited keeps both the calibration and the precise poses.
        let mut precise = stored.clone();
        precise.views[0].position[0] += 0.000_012_3;
        let again = RigForm::from_profile(&precise)
            .to_profile(Some(&precise))
            .expect("valid");
        assert_eq!(again.views, precise.views);
        assert_eq!(again.calibration, precise.calibration);

        // Renaming the rig or changing the stone does not touch the poses.
        let mut renamed = RigForm::from_profile(&stored);
        renamed.name = "Bench 2".to_owned();
        renamed.stone_n = "1.54".to_owned();
        assert!(
            renamed
                .to_profile(Some(&stored))
                .unwrap()
                .calibration
                .is_some()
        );
    }

    #[test]
    fn the_layout_generator_checks_its_numbers() {
        assert!(layout_views("150", "30", "4000", "4000", "3000", false).is_ok());
        assert!(layout_views("0", "30", "4000", "4000", "3000", false).is_err());
        assert!(layout_views("150", "x", "4000", "4000", "3000", false).is_err());
        assert!(layout_views("150", "30", "4000", "0", "3000", false).is_err());
        let ortho = layout_views("150", "30", "20", "4000", "3000", true).expect("valid");
        assert!(ortho.iter().all(|view| view.orthographic));
    }

    #[test]
    fn the_cube_fields_are_checked() {
        let cube = cube_from_texts("25.4", "0,1", "1.5168", 2, false).expect("valid");
        assert_eq!(cube, CubeSpec::default());
        let mirrored = cube_from_texts("25.4", "0.1", "1.5168", 0, true).expect("valid");
        assert!(mirrored.mirrored);
        assert_eq!(mirrored.diagonal_axis, 0);
        assert!(cube_from_texts("0", "0.1", "1.5", 2, false).is_err());
        assert!(cube_from_texts("25", "-1", "1.5", 2, false).is_err());
        assert!(cube_from_texts("25", "0.1", "0.9", 2, false).is_err());
        assert!(cube_from_texts("25", "0.1", "1.5", 3, false).is_err());
        assert!(cube_from_texts("25", "0.1", "1.5", -1, false).is_err());
        assert!(
            cube_from_texts("big", "0.1", "1.5", 2, false)
                .unwrap_err()
                .contains("edge")
        );
    }

    #[test]
    fn the_focal_uncertainty_is_a_percent_between_a_hundredth_and_a_hundred() {
        assert_eq!(focal_sigma_from_percent("10"), Ok(0.1));
        assert_eq!(focal_sigma_from_percent("0,5"), Ok(0.005));
        assert!(focal_sigma_from_percent("0").is_err());
        assert!(focal_sigma_from_percent("101").is_err());
        assert!(focal_sigma_from_percent("x").is_err());
    }

    #[test]
    fn a_known_material_gives_its_index_and_an_unknown_one_a_typical_value() {
        let diamond = default_stone_n("diamond");
        assert!((2.40..2.43).contains(&diamond), "{diamond}");
        assert_eq!(default_stone_n("No Such Gem"), FALLBACK_STONE_N);
    }
}
