//! Tests for the tier form's slider specs and the Typical menu rows.

use super::*;
use indicatrix_editor::slider_ranges::{angle_presets, preform_spec_data};
use slint::Model;

#[test]
fn a_hidden_slider_maps_to_an_invisible_spec() {
    let spec = slider_spec_from(angle_spec_data("Girdle", "30", "", "1.5440"));
    assert!(!spec.visible && !spec.usable);
    assert!(spec.band_to <= spec.band_from);
    assert!(spec.note.is_empty());
}

#[test]
fn a_pavilion_slider_maps_number_for_number() {
    let spec = slider_spec_from(angle_spec_data("Pavilion", "41.25", "P1", "1.5440"));
    assert!(spec.visible && spec.usable);
    assert_eq!(spec.side.as_str(), "Pavilion");
    assert_eq!(spec.value, 41.25);
    assert_eq!((spec.minimum, spec.maximum), (30.0, 55.0));
    assert_eq!((spec.step, spec.fine_step, spec.mark), (0.05, 0.01, 0.5));
    assert!(spec.band_from > 40.0 && spec.band_to > spec.band_from);
    assert_eq!(spec.value_text.as_str(), "41.25 degrees");
    assert!(spec.note.starts_with("Green band"));
}

#[test]
fn a_crown_slider_has_an_empty_band() {
    let spec = slider_spec_from(angle_spec_data("Crown", "34.5", "C1", "1.7620"));
    assert_eq!(spec.side.as_str(), "Crown");
    assert!(spec.band_to <= spec.band_from);
    assert_eq!((spec.minimum, spec.maximum), (5.0, 55.0));
}

#[test]
fn a_preform_slider_maps_like_an_angle_one() {
    let spec = slider_spec_from(preform_spec_data("depth", "1.50", "1.50", ""));
    assert!(spec.visible && spec.usable);
    assert_eq!(spec.side.as_str(), "");
    assert_eq!((spec.minimum, spec.maximum), (1.0, 3.5));
    assert!(spec.band_to <= spec.band_from);
    let y_offset = slider_spec_from(preform_spec_data("y_offset", "0.00", "1.50", ""));
    assert!(y_offset.visible && !y_offset.usable);
    assert!(!y_offset.note.is_empty());
}

#[test]
fn the_typical_menu_rows_carry_label_value_and_reason() {
    let presets = angle_presets(AngleSide::Pavilion, 2.417, "Diamond");
    let count = presets.len();
    let rows = preset_rows(presets);
    assert_eq!(rows.row_count(), count);
    let first = rows.row_data(0).expect("a first row");
    assert_eq!(first.label.as_str(), "Standard for Diamond");
    assert_eq!(first.value_text.as_str(), "41.20\u{b0}");
    assert!((first.value - 41.2).abs() < 1e-4);
    assert!(!first.reason.is_empty());
}
