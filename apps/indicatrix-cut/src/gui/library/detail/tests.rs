//! Tests for the angle-side classification, row-count filter and plane-fallback
//! logic shared across the detail module's local/remote load paths.

use super::{
    planes::planes_gear_and_reference_angle,
    shared::{count_angles_for_mode, parse_catalogue_angle_deg, sides_from_rows},
};
use crate::AngleItem;
use indicatrix::geometry::plane::GpuFacetPlane;
use slint::{ModelRc, VecModel};

// --- parse_catalogue_angle_deg / sides_from_rows ---

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

/// 6 of the 50,817 real rows (detail 3282's `P`/`G`/`C` rows) store a mangled
/// degree sign as `\u{fffd}` (the Unicode replacement character) instead of
/// `\u{b0}` -- this must still parse rather than silently reporting `0`/neither
/// side for those rows.
#[test]
fn parse_catalogue_angle_deg_strips_a_mangled_degree_sign_too() {
    assert_eq!(parse_catalogue_angle_deg("43.00\u{fffd}"), Some(43.00));
}

#[test]
fn parse_catalogue_angle_deg_is_none_for_unparsable_text() {
    assert_eq!(parse_catalogue_angle_deg("not a number"), None);
}

/// A tiny helper so each test can write `row("A", "50.00\u{b0}", "...")` instead of
/// a 3-tuple literal.
fn row<'a>(facet: &'a str, angle: &'a str, index_val: &'a str) -> (&'a str, &'a str, &'a str) {
    (facet, angle, index_val)
}

/// Real catalogue detail 16 (`G(90) 1(47) 2(43) A(50) B(35) C(15) T(0,'Table')`):
/// digit facets are pavilion, letter facets are crown, `G` is the girdle, and the
/// `T`/`'Table'` row is crown even though it's listed last.
#[test]
fn classifies_real_detail_16() {
    let rows = [
        row("G", "90.00\u{b0}", "04-12-20"),
        row("1", "47.00\u{b0}", "04-12-20"),
        row("2", "43.00\u{b0}", "08-24-40"),
        row("A", "50.00\u{b0}", "04-12-20"),
        row("B", "35.00\u{b0}", "96-16-32"),
        row("C", "15.00\u{b0}", "08-24-40"),
        row("T", "0.00\u{b0}", "Table"),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![0, -1, -1, 1, 1, 1, 1]);
}

/// Real catalogue detail 47 (`''(90) 1(43) 2(41) A(36) B(34.16) T(0)`): an EMPTY
/// facet label at the girdle angle is still caught by the angle-threshold rule
/// (`label_side`'s rule (a) doesn't need a facet label at all).
#[test]
fn classifies_real_detail_47_with_an_empty_girdle_label() {
    let rows = [
        row("", "90.00\u{b0}", "96-08-16"),
        row("1", "43.00\u{b0}", "96-08-16"),
        row("2", "41.00\u{b0}", "02-06-10"),
        row("A", "36.00\u{b0}", "96-08-16"),
        row("B", "34.16\u{b0}", "02-06-10"),
        row("T", "0.00\u{b0}", "Table"),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![0, -1, -1, 1, 1, 1]);
}

/// Real catalogue detail 1001 (`a..f`(crown letters) `g`(Table) `1..5`(pavilion
/// digits) `G1 G2`(girdle)): lowercase letters are crown same as uppercase, and
/// TWO girdle-labelled rows (`G1`/`G2`) both report `0`.
#[test]
fn classifies_real_detail_1001() {
    let rows = [
        row("a", "44.00\u{b0}", "02-22-26"),
        row("b", "44.00\u{b0}", "08-16-32"),
        row("c", "34.00\u{b0}", "02-22-26"),
        row("d", "34.00\u{b0}", "08-16-32"),
        row("e", "24.00\u{b0}", "02-22-26"),
        row("f", "24.00\u{b0}", "08-16-32"),
        row("g", "0.00\u{b0}", "Table"),
        row("1", "41.00\u{b0}", "96-24-48"),
        row("2", "42.74\u{b0}", "03-21-27"),
        row("3", "43.33\u{b0}", "06-18-30"),
        row("4", "43.00\u{b0}", "08-16-32"),
        row("5", "60.00\u{b0}", "02-22-26"),
        row("G1", "90.00\u{b0}", "02-22-26"),
        row("G2", "90.00\u{b0}", "08-16-32"),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, 0, 0]);
}

/// Real catalogue detail 2001 (`a..e`(crown) `f`(Table) `1 2`(pavilion) `G1`
/// (girdle)) -- a shorter facet count than 1001, same conventions.
#[test]
fn classifies_real_detail_2001() {
    let rows = [
        row("a", "37.00\u{b0}", "64-09-18"),
        row("b", "45.00\u{b0}", "02-07-11"),
        row("c", "27.00\u{b0}", "02-07-11"),
        row("d", "24.48\u{b0}", "29-35"),
        row("e", "27.00\u{b0}", "39-44-48"),
        row("f", "0.00\u{b0}", "Table"),
        row("1", "43.00\u{b0}", "64-09-18"),
        row("2", "44.00\u{b0}", "02-07-11"),
        row("G1", "90.00\u{b0}", "02-07-11"),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![1, 1, 1, 1, 1, 1, -1, -1, 0]);
}

/// The `P<n>`/`C<n>` facet convention (1,059 + 790 rows in the real catalogue):
/// `P`/`PF` prefixed labels are pavilion, `C` prefixed labels are crown, regardless
/// of angle or row order -- including the odd real labels `P2(G)`, `C1A`, `pf3`.
#[test]
fn classifies_the_p_n_and_c_n_facet_convention() {
    let rows = [
        row("P1", "43.00\u{b0}", ""),
        row("P2(G)", "44.00\u{b0}", ""),
        row("pf3", "45.00\u{b0}", ""),
        row("PF1", "46.00\u{b0}", ""),
        row("C1", "15.00\u{b0}", ""),
        row("C1A", "16.00\u{b0}", ""),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![-1, -1, -1, -1, 1, 1]);
}

/// `1G` is the catalogue's own way of tagging a facet as girdle-adjacent by label
/// rather than by reaching the angle threshold -- it must report girdle (`0`), not
/// fall into the more general "digits with an optional trailing letter" pavilion
/// rule that would otherwise also match its shape.
#[test]
fn classifies_digit_plus_trailing_g_as_girdle_not_pavilion() {
    let rows = [row("1G", "80.00\u{b0}", "")];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![0]);
}

/// A design whose facet labels are all EMPTY (no digit/letter/prefix convention to
/// read at all), with the girdle row FIRST and the table row LAST -- exactly the
/// shape 1,403 of the 3,021 real designs use for the girdle position, which is what
/// defeats a purely positional before/after-girdle rule. This is the case that
/// exercises [`positional_fallback`]'s table-anchored branch (a digits-only design
/// wouldn't reach it: `label_side`'s digit rule already classifies every digit
/// facet as pavilion regardless of position, see
/// `classifies_the_p_n_and_c_n_facet_convention` above).
///
/// DECISION: the rows between the girdle and the table are reported crown (`1`),
/// not pavilion -- since the table sits on the far side of the girdle from row 0,
/// [`positional_fallback`] treats that whole post-girdle block as the table's
/// (crown) side. This is the fix for the bug: the OLD before/after-girdle rule
/// would have reported every one of these rows pavilion.
#[test]
fn positional_fallback_treats_the_tables_side_of_the_girdle_as_crown() {
    let rows = [
        row("", "90.00\u{b0}", ""), // girdle by angle threshold
        row("", "47.00\u{b0}", ""), // unlabelled, between girdle and table
        row("", "43.00\u{b0}", ""), // unlabelled, between girdle and table
        row("", "50.00\u{b0}", ""), // unlabelled, between girdle and table
        row("", "0.00\u{b0}", "Table"),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![0, 1, 1, 1, 1]);
}

/// With no table row to anchor on, an empty-labelled design falls back to the OLD
/// before/after-girdle rule -- but only when the girdle sits in the MIDDLE of the
/// row list (there's at least one row on each side to trust).
#[test]
fn positional_fallback_without_a_table_row_uses_before_after_girdle_when_girdle_is_in_the_middle() {
    let rows = [
        row("", "40.00\u{b0}", ""),
        row("", "90.00\u{b0}", ""), // girdle, in the middle
        row("", "41.00\u{b0}", ""),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![1, 0, -1]);
}

/// Same as above but the girdle is FIRST -- there is no "before" side at all, so
/// the old before/after rule cannot be trusted; every unlabelled row reports `0`
/// (unclassified) rather than guessing.
#[test]
fn positional_fallback_without_a_table_row_reports_zero_when_girdle_is_first() {
    let rows = [
        row("", "90.00\u{b0}", ""), // girdle, first
        row("", "41.00\u{b0}", ""),
        row("", "42.00\u{b0}", ""),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![0, 0, 0]);
}

/// Same again but the girdle is LAST -- no "after" side to trust either.
#[test]
fn positional_fallback_without_a_table_row_reports_zero_when_girdle_is_last() {
    let rows = [
        row("", "41.00\u{b0}", ""),
        row("", "42.00\u{b0}", ""),
        row("", "90.00\u{b0}", ""), // girdle, last
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![0, 0, 0]);
}

/// More than one row can sit at exactly the girdle angle (e.g. two girdle facets
/// listed back to back, as real detail 1001's `G1`/`G2` do) -- every one of them
/// reports `0`, not just the first. Uses labelled crown/pavilion rows either side
/// (rather than empty labels) so this test only exercises the girdle-angle check
/// itself, not the positional fallback.
#[test]
fn sides_from_rows_reports_zero_for_every_girdle_row_not_only_the_first() {
    let rows = [
        row("A", "40.00\u{b0}", ""),
        row("", "90.00\u{b0}", ""),
        row("", "90.00\u{b0}", ""),
        row("1", "41.00\u{b0}", ""),
    ];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![1, 0, 0, -1]);
}

/// A row at 89.6\u{b0} is a real, if steep, facet -- NOT the girdle -- since the
/// real catalogue's own girdle angles cluster tightly at exactly `90.00\u{b0}` with
/// nothing between `88.73\u{b0}` and `90.0\u{b0}`. A "steep enough" threshold would
/// have wrongly swallowed it; the equality-with-tolerance rule must not.
#[test]
fn a_steep_but_not_exactly_90_degree_facet_is_not_the_girdle() {
    let rows = [row("A", "89.60\u{b0}", "")];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![1]);
}

#[test]
fn sides_from_rows_reports_zero_for_unparsable_angle_text_with_no_usable_label() {
    let rows = [row("", "not a number", ""), row("", "41.00\u{b0}", "")];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides[0], 0);
}

/// With NEITHER a girdle row nor a table row at all, there is nothing to anchor
/// the positional fallback on -- every unlabelled row reports `0` rather than
/// defaulting to crown (a change from the old order-only rule, which had no way to
/// tell "no girdle marker" apart from "every row before the girdle").
#[test]
fn sides_from_rows_reports_zero_with_no_girdle_marker_and_no_usable_label() {
    let rows = [row("", "34.50\u{b0}", ""), row("", "42.70\u{b0}", "")];
    let sides = sides_from_rows(rows.into_iter());
    assert_eq!(sides, vec![0, 0]);
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
        second_line: "".into(),
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
        second_line: "".into(),
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

/// End-to-end: [`sides_from_rows`]'s output, loaded into real `AngleItem`s the way
/// `local_load.rs`/`remote_load.rs` do, must agree with [`count_angles_for_mode`] --
/// which is in turn exactly `cutting_table.slint`'s own `row_shown` predicate
/// (`side < 0` pavilion, `side > 0` crown). Uses real detail 16's own schedule
/// (`G 1 2 A B C T`): 1 girdle row, 2 pavilion digit facets, 4 crown rows (3
/// lettered facets plus the table row).
#[test]
fn filtered_row_count_agrees_with_sides_from_rows_on_a_real_schedule() {
    let rows = [
        row("G", "90.00\u{b0}", ""),
        row("1", "47.00\u{b0}", ""),
        row("2", "43.00\u{b0}", ""),
        row("A", "50.00\u{b0}", ""),
        row("B", "35.00\u{b0}", ""),
        row("C", "15.00\u{b0}", ""),
        row("T", "0.00\u{b0}", "Table"),
    ];
    let sides = sides_from_rows(rows.into_iter());
    let angles = ModelRc::new(VecModel::from(
        sides.into_iter().map(angle_item).collect::<Vec<_>>(),
    ));
    assert_eq!(count_angles_for_mode(&angles, 0), 7); // All
    assert_eq!(count_angles_for_mode(&angles, 1), 2); // Pavilion: 1, 2
    assert_eq!(count_angles_for_mode(&angles, 2), 4); // Crown: A, B, C, T
}
