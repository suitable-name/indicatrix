//! Printed proportions round-tripping through the sidecar's `[source]` table,
//! and `format_version` being written but never checked on load.

use super::fixtures::simple_design;
use crate::native::{FORMAT_VERSION, load_paired, save_paired, to_toml_string};
use indicatrix::geometry::stone_metrics::ExternalProportions;

#[test]
fn printed_proportions_round_trip_through_save_and_open() {
    let design = simple_design();
    let props = ExternalProportions {
        vol_w3: Some(0.42),
        lw: Some(1.05),
        cw: Some(0.17),
        pw: Some(0.44),
        hw: Some(0.61),
    };
    let saved = save_paired(&design, "design.asc", None, None, Some(&props))
        .expect("must save with printed proportions");
    assert!(
        saved.native.source.is_some(),
        "a non-empty ExternalProportions must produce a [source] table"
    );

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    // `ExternalProportions` derives no `PartialEq`, so each field is checked
    // individually.
    let restored = loaded
        .printed_proportions
        .expect("printed proportions must survive the round trip");
    assert_eq!(restored.vol_w3, props.vol_w3);
    assert_eq!(restored.lw, props.lw);
    assert_eq!(restored.cw, props.cw);
    assert_eq!(restored.pw, props.pw);
    assert_eq!(restored.hw, props.hw);
}

#[test]
fn an_all_none_external_proportions_writes_no_source_table_at_all() {
    let design = simple_design();
    let empty_props = ExternalProportions::default();
    let saved =
        save_paired(&design, "design.asc", None, None, Some(&empty_props)).expect("must save");
    assert!(
        saved.native.source.is_none(),
        "an all-None ExternalProportions should not grow a [source] table nothing can use"
    );
}

#[test]
fn a_sidecar_saved_before_source_existed_loads_with_no_printed_proportions() {
    let design = simple_design();
    // No `printed_proportions` at all -- the shape of a sidecar saved before this
    // field existed.
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert!(loaded.printed_proportions.is_none());
}

#[test]
fn a_sidecar_saved_by_this_build_never_reports_a_newer_format_version() {
    let design = simple_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert!(!loaded.written_by_newer_version);
}

#[test]
fn a_sidecar_with_a_higher_format_version_is_flagged_as_written_by_a_newer_build() {
    let design = simple_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    let mut bumped = saved.native;
    bumped.format_version = FORMAT_VERSION + 1;
    let bumped_toml = to_toml_string(&bumped).expect("must serialize");
    let loaded = load_paired(&saved.asc_text, &bumped_toml, false)
        .expect("a version bump alone must not refuse to load");
    assert!(loaded.written_by_newer_version);
}
