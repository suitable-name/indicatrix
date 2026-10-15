//! Printed proportions round-tripping through the design file's `[source]` table.

use super::fixtures::{reopen, simple_design};
use crate::native::{DesignExtras, design_from_str, design_to_file, design_to_string};
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
    let text = design_to_string(&design, Some(&props), &DesignExtras::default())
        .expect("must save with printed proportions");
    assert!(
        text.contains("[source]"),
        "a non-empty ExternalProportions must produce a [source] table"
    );

    let loaded = design_from_str(&text).expect("must load back");
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
    let file = design_to_file(&design, Some(&empty_props), &DesignExtras::default());
    assert!(
        file.source.is_none(),
        "an all-None ExternalProportions should not grow a [source] table nothing can use"
    );
}

#[test]
fn a_file_saved_without_source_loads_with_no_printed_proportions() {
    // No `printed_proportions` at all -- the shape of a file saved before this
    // table existed.
    let loaded = reopen(&simple_design());
    assert!(loaded.printed_proportions.is_none());
}
