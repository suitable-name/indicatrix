//! Tests for the angle-side classification, row-count filter and plane-fallback
//! logic shared across the detail module's local/remote load paths.

use super::{
    planes::planes_gear_and_reference_angle,
    shared::{count_angles_for_mode, parse_catalogue_angle_deg, sides_from_angle_sequence},
};
use crate::AngleItem;
use indicatrix::geometry::plane::GpuFacetPlane;
use slint::{ModelRc, VecModel};

// --- parse_catalogue_angle_deg / sides_from_angle_sequence ---

/// The real catalogue's own shape: 50,809 of 50,817 stored `angle_settings.angle`
/// values end in `\u{b0}`, which a bare `str::parse` chokes on.
#[test]
fn parse_catalogue_angle_deg_strips_the_degree_sign() {
    assert_eq!(parse_catalogue_angle_deg("42.70\u{b0}"), Some(42.70));
}

#[test]
fn parse_catalogue_angle_deg_also_accepts_plain_text_with_no_degree_sign() {
    assert_eq!(parse_catalogue_angle_deg("42.70"), Some(42.70));
}

#[test]
fn parse_catalogue_angle_deg_is_none_for_unparsable_text() {
    assert_eq!(parse_catalogue_angle_deg("not a number"), None);
}

/// A girdle-ordered schedule -- crown facets, then the girdle row(s) at ~90
/// degrees, then pavilion facets -- exactly the real catalogue's own row order and
/// unsigned, degree-sign-suffixed angle text.
#[test]
fn sides_from_angle_sequence_classifies_a_girdle_ordered_schedule() {
    let angles = [
        "34.50\u{b0}",
        "42.70\u{b0}",
        "90.00\u{b0}",
        "41.00\u{b0}",
        "43.10\u{b0}",
    ];
    let sides = sides_from_angle_sequence(angles.iter().copied());
    assert_eq!(sides, vec![1, 1, 0, -1, -1]);
}

/// More than one row can sit at/above the girdle threshold (e.g. two girdle
/// facets listed back to back) -- every one of them reports `0`, not just the
/// first.
#[test]
fn sides_from_angle_sequence_reports_zero_for_every_girdle_row_not_only_the_first() {
    let angles = ["40.00\u{b0}", "89.80\u{b0}", "90.00\u{b0}", "41.00\u{b0}"];
    let sides = sides_from_angle_sequence(angles.iter().copied());
    assert_eq!(sides, vec![1, 0, 0, -1]);
}

#[test]
fn sides_from_angle_sequence_reports_zero_for_unparsable_text() {
    let angles = ["not a number", "41.00\u{b0}"];
    let sides = sides_from_angle_sequence(angles.iter().copied());
    assert_eq!(sides[0], 0);
}

/// A schedule with no girdle-magnitude row at all (nothing >= the threshold) has
/// no boundary to cross, so every row stays crown -- never guesses a pavilion
/// split with no marker to place it at.
#[test]
fn sides_from_angle_sequence_treats_every_row_as_crown_with_no_girdle_marker() {
    let angles = ["34.50\u{b0}", "42.70\u{b0}"];
    let sides = sides_from_angle_sequence(angles.iter().copied());
    assert_eq!(sides, vec![1, 1]);
}

/// With no `real_design` resolved (the remote route, or a
/// local resolution failure), the catalogue view must still fall back to the
/// existing placeholder `reconstruct_planes` path -- gear read from the text
/// column, reference angle `0.0` (library records carry none).
#[test]
fn falls_back_to_the_placeholder_reconstruction_with_no_real_design() {
    let angle_items = [AngleItem {
        order_idx: 0,
        side: -1,
        facet: "P1".into(),
        angle: "-41.0".into(),
        index_val: "0, 24, 48, 72".into(),
        notes: "".into(),
    }];
    let (planes, gear_teeth, reference_angle) =
        planes_gear_and_reference_angle(None, Some("Round"), Some("96"), &angle_items);
    assert_ne!(planes.len(), 0);
    assert_eq!(gear_teeth, 96);
    assert_eq!(reference_angle, 0.0);
}

/// With a resolved `real_design` tuple, the
/// catalogue view must use exactly that (already-converted) planes/gear/reference-
/// angle, not the placeholder guess or a hardcoded `0.0` -- regardless of what the
/// (deliberately wrong, "999") shape/gear text arguments would otherwise have
/// produced.
#[test]
fn prefers_the_real_designs_own_planes_and_reference_angle_when_resolved() {
    let resolved_planes = vec![GpuFacetPlane::new(glam::Vec3::Y, -1.0)];
    let (planes, gear_teeth, reference_angle) = planes_gear_and_reference_angle(
        Some((resolved_planes.clone(), 12, 2.5)),
        Some("Round"),
        Some("999"),
        &[],
    );

    assert_eq!(planes.len(), resolved_planes.len());
    assert_eq!(gear_teeth, 12);
    assert_eq!(reference_angle, 2.5);
}

// --- count_angles_for_mode ---

fn angle_item(side: i32) -> AngleItem {
    AngleItem {
        order_idx: 0,
        side,
        facet: "P1".into(),
        angle: "0.0".into(),
        index_val: "".into(),
        notes: "".into(),
    }
}

/// Mode 0 (All) counts every row regardless of side, including girdle/unparsed
/// rows (`side == 0`).
#[test]
fn mode_all_counts_every_row() {
    let angles = ModelRc::new(VecModel::from(vec![
        angle_item(-1),
        angle_item(1),
        angle_item(0),
    ]));
    assert_eq!(count_angles_for_mode(&angles, 0), 3);
}

/// Mode 1 (Pavilion) counts only `side < 0` rows -- exactly
/// `cutting_table.slint`'s own `row_shown` predicate.
#[test]
fn mode_pavilion_counts_only_negative_side_rows() {
    let angles = ModelRc::new(VecModel::from(vec![
        angle_item(-1),
        angle_item(-1),
        angle_item(1),
        angle_item(0),
    ]));
    assert_eq!(count_angles_for_mode(&angles, 1), 2);
}

/// Mode 2 (Crown) counts only `side > 0` rows.
#[test]
fn mode_crown_counts_only_positive_side_rows() {
    let angles = ModelRc::new(VecModel::from(vec![
        angle_item(-1),
        angle_item(1),
        angle_item(1),
        angle_item(0),
    ]));
    assert_eq!(count_angles_for_mode(&angles, 2), 2);
}

/// An empty angle list counts to zero under every mode, rather than panicking.
#[test]
fn empty_angle_list_counts_to_zero_under_every_mode() {
    let angles: ModelRc<AngleItem> = ModelRc::new(VecModel::from(Vec::<AngleItem>::new()));
    for mode in [0, 1, 2] {
        assert_eq!(count_angles_for_mode(&angles, mode), 0);
    }
}
