//! The rough base in the window: reading the size fields into a [`RoughBase`], switching
//! the base kind and writing a base back into the fields.

use super::{cut_rows::fmt_num, inputs::parse_mm};
use crate::{RoughPlanModel, RoughPlannerWindow};
use indicatrix_cut_core::rough_plan::{Axis, RoughBase, RoughModel};
use slint::{ComponentHandle, SharedString};

/// The window's base kind numbers.
const KIND_BLOCK: i32 = 0;
const KIND_CYLINDER: i32 = 1;
const KIND_PEBBLE: i32 = 2;

/// The base fields exactly as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct BaseFields {
    /// 0 block, 1 cylinder, 2 pebble.
    pub(super) kind: i32,
    /// Block and pebble X.
    pub(super) x: String,
    /// Block and pebble Y.
    pub(super) y: String,
    /// Block and pebble Z.
    pub(super) z: String,
    /// Cylinder diameter.
    pub(super) diameter: String,
    /// Cylinder length.
    pub(super) length: String,
    /// Cylinder axis: 0 x, 1 y, 2 z.
    pub(super) axis: i32,
}

impl BaseFields {
    /// Reads the fields off the window.
    #[must_use]
    pub(super) fn read(window: &RoughPlannerWindow) -> Self {
        let model = window.global::<RoughPlanModel>();
        Self {
            kind: model.get_base_kind(),
            x: model.get_rough_x().to_string(),
            y: model.get_rough_y().to_string(),
            z: model.get_rough_z().to_string(),
            diameter: model.get_cyl_diameter().to_string(),
            length: model.get_cyl_length().to_string(),
            axis: model.get_cyl_axis(),
        }
    }
}

/// One size field: an empty field is 0 (the size is not entered yet).
fn size(text: &str, label: &str) -> Result<f64, String> {
    if text.trim().is_empty() {
        Ok(0.0)
    } else {
        parse_mm(text, label)
    }
}

/// The axis for the window's number (anything unknown is y).
#[must_use]
pub(super) const fn axis_from_number(number: i32) -> Axis {
    match number {
        0 => Axis::X,
        2 => Axis::Z,
        _ => Axis::Y,
    }
}

/// The window's number of `axis`.
#[must_use]
const fn axis_number(axis: Axis) -> i32 {
    match axis {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    }
}

/// The base the typed fields describe. Empty fields count as 0 (see [`is_blank`]).
///
/// # Errors
///
/// Returns the message for the field that is not a number.
pub(super) fn parse_base(fields: &BaseFields) -> Result<RoughBase, String> {
    if fields.kind == KIND_CYLINDER {
        return Ok(RoughBase::Cylinder {
            diameter_mm: size(&fields.diameter, "Diameter")?,
            length_mm: size(&fields.length, "Length")?,
            axis: axis_from_number(fields.axis),
        });
    }
    let (x_mm, y_mm, z_mm) = (
        size(&fields.x, "Rough X")?,
        size(&fields.y, "Rough Y")?,
        size(&fields.z, "Rough Z")?,
    );
    if fields.kind == KIND_PEBBLE {
        Ok(RoughBase::Pebble { x_mm, y_mm, z_mm })
    } else {
        Ok(RoughBase::Block { x_mm, y_mm, z_mm })
    }
}

/// The base kind number of `base`.
#[must_use]
pub(super) const fn kind_of(base: &RoughBase) -> i32 {
    match base {
        RoughBase::Block { .. } => KIND_BLOCK,
        RoughBase::Cylinder { .. } => KIND_CYLINDER,
        RoughBase::Pebble { .. } => KIND_PEBBLE,
    }
}

/// The base of kind `kind` that keeps the size of `current` as far as the shapes agree:
/// block and pebble share their box, a cylinder becomes the box of its extents and a
/// box becomes an upright cylinder of its narrower horizontal side.
#[must_use]
pub(super) const fn switch_base_kind(current: &RoughBase, kind: i32) -> RoughBase {
    let [x_mm, y_mm, z_mm] = current.bounding_box_extents();
    if kind == KIND_CYLINDER {
        if matches!(current, RoughBase::Cylinder { .. }) {
            *current
        } else {
            RoughBase::Cylinder {
                diameter_mm: x_mm.min(z_mm),
                length_mm: y_mm,
                axis: Axis::Y,
            }
        }
    } else if kind == KIND_PEBBLE {
        RoughBase::Pebble { x_mm, y_mm, z_mm }
    } else {
        RoughBase::Block { x_mm, y_mm, z_mm }
    }
}

/// Whether the model has no usable size yet (some dimension is not entered). Such a
/// model is not evaluated and shows no error.
#[must_use]
pub(super) fn is_blank(model: &RoughModel) -> bool {
    model
        .base
        .bounding_box_extents()
        .iter()
        .any(|extent| extent.abs() < f64::EPSILON)
}

/// The empty model: a block without a size and no cuts.
#[must_use]
pub(super) const fn blank_model() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 0.0,
            y_mm: 0.0,
            z_mm: 0.0,
        },
        Vec::new(),
    )
}

/// A size field's text: empty for 0, otherwise at most two decimals when that reads back
/// as exactly `value`, else the shortest text that does. A shown size therefore always
/// parses to the size the model holds, so committing an untouched field is never an
/// edit, and any other typed value is one.
fn size_string(value: f64) -> String {
    if value.abs() < f64::EPSILON {
        return String::new();
    }
    let short = fmt_num(value, 2);
    if short
        .parse::<f64>()
        .is_ok_and(|parsed| parsed.to_bits() == value.to_bits())
    {
        short
    } else {
        format!("{value}")
    }
}

/// A size field's text as a Slint string.
fn size_text(value: f64) -> SharedString {
    size_string(value).into()
}

/// The base the typed `fields` describe when it differs from `current`, `None` when the
/// typed sizes are exactly the sizes `current` holds (whatever the spelling). The window
/// shows a size in full precision (see `size_string`), so a value typed over a shown one
/// is an edit even when it only differs beyond the second decimal.
///
/// # Errors
///
/// Returns the message for the field that is not a number.
pub(super) fn pending_base(
    fields: &BaseFields,
    current: &RoughBase,
) -> Result<Option<RoughBase>, String> {
    let typed = parse_base(fields)?;
    if typed == *current {
        Ok(None)
    } else {
        Ok(Some(typed))
    }
}

/// Writes `base` into the window: the kind and its size fields.
pub(super) fn push_base(window: &RoughPlannerWindow, base: &RoughBase) {
    let model = window.global::<RoughPlanModel>();
    model.set_base_kind(kind_of(base));
    match *base {
        RoughBase::Block { x_mm, y_mm, z_mm } | RoughBase::Pebble { x_mm, y_mm, z_mm } => {
            model.set_rough_x(size_text(x_mm));
            model.set_rough_y(size_text(y_mm));
            model.set_rough_z(size_text(z_mm));
        }
        RoughBase::Cylinder {
            diameter_mm,
            length_mm,
            axis,
        } => {
            model.set_cyl_diameter(size_text(diameter_mm));
            model.set_cyl_length(size_text(length_mm));
            model.set_cyl_axis(axis_number(axis));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fields as [`push_base`] writes `base` into the window.
    fn fields_of(base: &RoughBase) -> BaseFields {
        let mut fields = BaseFields {
            kind: kind_of(base),
            ..BaseFields::default()
        };
        match *base {
            RoughBase::Block { x_mm, y_mm, z_mm } | RoughBase::Pebble { x_mm, y_mm, z_mm } => {
                fields.x = size_string(x_mm);
                fields.y = size_string(y_mm);
                fields.z = size_string(z_mm);
            }
            RoughBase::Cylinder {
                diameter_mm,
                length_mm,
                axis,
            } => {
                fields.diameter = size_string(diameter_mm);
                fields.length = size_string(length_mm);
                fields.axis = axis_number(axis);
            }
        }
        fields
    }

    fn block_fields(x: &str, y: &str, z: &str) -> BaseFields {
        BaseFields {
            kind: KIND_BLOCK,
            x: x.into(),
            y: y.into(),
            z: z.into(),
            ..BaseFields::default()
        }
    }

    #[test]
    fn typed_sizes_become_the_base_and_empty_ones_count_as_blank() {
        let base = parse_base(&block_fields("12,5", " 9 ", "8.25")).expect("valid sizes");
        assert_eq!(
            base,
            RoughBase::Block {
                x_mm: 12.5,
                y_mm: 9.0,
                z_mm: 8.25,
            }
        );
        let partial = parse_base(&block_fields("12", "", "")).expect("empty fields are allowed");
        assert!(is_blank(&RoughModel::new(partial, Vec::new())));
        assert!(!is_blank(&RoughModel::new(base, Vec::new())));
    }

    #[test]
    fn a_field_that_is_not_a_number_names_itself() {
        let error = parse_base(&block_fields("12", "abc", "8")).unwrap_err();
        assert!(error.starts_with("Rough Y"), "{error}");
        let cylinder = BaseFields {
            kind: KIND_CYLINDER,
            diameter: "x".into(),
            ..BaseFields::default()
        };
        assert!(parse_base(&cylinder).unwrap_err().starts_with("Diameter"));
    }

    #[test]
    fn the_cylinder_fields_and_axis_are_read() {
        let fields = BaseFields {
            kind: KIND_CYLINDER,
            diameter: "10".into(),
            length: "24".into(),
            axis: 2,
            ..BaseFields::default()
        };
        assert_eq!(
            parse_base(&fields),
            Ok(RoughBase::Cylinder {
                diameter_mm: 10.0,
                length_mm: 24.0,
                axis: Axis::Z,
            })
        );
    }

    fn block(x: f64, y: f64, z: f64) -> RoughBase {
        RoughBase::Block {
            x_mm: x,
            y_mm: y,
            z_mm: z,
        }
    }

    #[test]
    fn the_fields_of_a_base_are_what_the_window_shows_for_it() {
        let fields = fields_of(&block(12.5, 9.0, 0.0));
        assert_eq!(
            (fields.x.as_str(), fields.y.as_str(), fields.z.as_str()),
            ("12.5", "9", ""),
            "no trailing zeros, empty for 0"
        );
        assert_eq!(fields.kind, KIND_BLOCK);
        let cylinder = fields_of(&RoughBase::Cylinder {
            diameter_mm: 10.0,
            length_mm: 24.25,
            axis: Axis::Z,
        });
        assert_eq!(cylinder.kind, KIND_CYLINDER);
        assert_eq!(
            (cylinder.diameter.as_str(), cylinder.length.as_str()),
            ("10", "24.25")
        );
        assert_eq!(cylinder.axis, 2);
    }

    #[test]
    fn typed_sizes_that_change_the_base_are_pending() {
        let current = block(10.0, 8.0, 6.0);
        // Nothing typed differently: no edit.
        assert_eq!(pending_base(&fields_of(&current), &current), Ok(None));
        // The same values in another spelling: no edit either.
        assert_eq!(
            pending_base(&block_fields("10.00", " 8,0", "6"), &current),
            Ok(None)
        );
        // A changed size is an edit.
        assert_eq!(
            pending_base(&block_fields("12", "8", "6"), &current),
            Ok(Some(block(12.0, 8.0, 6.0)))
        );
        // Sizes typed into a blank rough are an edit too.
        assert_eq!(
            pending_base(&block_fields("12", "9", "8"), &block(0.0, 0.0, 0.0)),
            Ok(Some(block(12.0, 9.0, 8.0)))
        );
        // Text that is not a number is an error, not an edit.
        assert!(
            pending_base(&block_fields("12", "x", "6"), &current)
                .unwrap_err()
                .starts_with("Rough Y")
        );
    }

    #[test]
    fn a_size_is_shown_in_full_so_committing_it_untouched_is_no_edit() {
        // 12.345 mm is shown as 12.345, not rounded to 12.35: the text reads back as the
        // very size the base holds.
        let current = block(12.345, 8.0, 6.0);
        let shown = fields_of(&current);
        assert_eq!(shown.x, "12.345");
        assert_eq!(pending_base(&shown, &current), Ok(None));
        // A size that two decimals spell exactly stays short.
        assert_eq!(fields_of(&block(12.5, 8.0, 6.0)).x, "12.5");
    }

    #[test]
    fn a_value_typed_over_a_shown_one_is_an_edit_even_beyond_the_second_decimal() {
        let current = block(12.345, 8.0, 6.0);
        // 12.35 is the rounded display of 12.345 but not the same number: an edit.
        assert_eq!(
            pending_base(&block_fields("12.35", "8", "6"), &current),
            Ok(Some(block(12.35, 8.0, 6.0)))
        );
        assert_eq!(
            pending_base(&block_fields("12.4", "8", "6"), &current),
            Ok(Some(block(12.4, 8.0, 6.0)))
        );
        // The same number spelled differently is not.
        assert_eq!(
            pending_base(&block_fields("12,3450", "8.0", "6"), &current),
            Ok(None)
        );
    }

    #[test]
    fn a_cylinder_axis_change_is_pending() {
        let current = RoughBase::Cylinder {
            diameter_mm: 10.0,
            length_mm: 24.0,
            axis: Axis::Y,
        };
        let mut fields = fields_of(&current);
        assert_eq!(pending_base(&fields, &current), Ok(None));
        fields.axis = 0;
        assert_eq!(
            pending_base(&fields, &current),
            Ok(Some(RoughBase::Cylinder {
                diameter_mm: 10.0,
                length_mm: 24.0,
                axis: Axis::X,
            }))
        );
    }

    #[test]
    fn switching_the_kind_keeps_the_size() {
        let block = RoughBase::Block {
            x_mm: 12.0,
            y_mm: 9.0,
            z_mm: 8.0,
        };
        assert_eq!(
            switch_base_kind(&block, KIND_PEBBLE),
            RoughBase::Pebble {
                x_mm: 12.0,
                y_mm: 9.0,
                z_mm: 8.0,
            }
        );
        let cylinder = switch_base_kind(&block, KIND_CYLINDER);
        assert_eq!(
            cylinder,
            RoughBase::Cylinder {
                diameter_mm: 8.0,
                length_mm: 9.0,
                axis: Axis::Y,
            }
        );
        // Back to a box: the cylinder's bounding box.
        assert_eq!(
            switch_base_kind(&cylinder, KIND_BLOCK),
            RoughBase::Block {
                x_mm: 8.0,
                y_mm: 9.0,
                z_mm: 8.0,
            }
        );
        assert_eq!(switch_base_kind(&cylinder, KIND_CYLINDER), cylinder);
    }
}
