//! Tests of what a plan records about a design's shape: the ratio fingerprint, the
//! width the ratios cannot tell, the measuring rule version, and how a file from before
//! those fields loads and re-saves.

use super::{
    dto::DesignShape,
    fixtures::{designs, plain_block, sample_layout, write_with},
    format::{
        RankedLayout, compute_fingerprint, design_shape, parse_and_validate_plan, round_significant,
    },
};
use indicatrix_vault::model::solid_extents::{SOLID_EXTENTS_VERSION, SolidExtents};

#[test]
fn fingerprints_keep_nine_significant_digits() {
    assert_eq!(round_significant(1.234_567_891_234, 9), 1.234_567_89);
    assert_eq!(round_significant(123_456.789_123_456, 9), 123_456.789);
    assert_eq!(
        round_significant(-0.000_123_456_789_123, 9),
        -0.000_123_456_789
    );
    assert!(round_significant(f64::NAN, 9).is_nan());
    assert!(round_significant(f64::INFINITY, 9).is_infinite());

    let extents = SolidExtents {
        width_caliper: 3.0,
        length_caliper: 4.17,
        width_axis: 3.0,
        length_axis: 4.17,
        height: 2.0,
        volume: 10.0,
    };
    let fingerprint = compute_fingerprint(&extents);
    assert_eq!(fingerprint, [1.39, 0.666_666_667, 0.370_370_370]);
    let swapped = SolidExtents {
        width_caliper: 4.17,
        length_caliper: 3.0,
        ..extents
    };
    assert_eq!(compute_fingerprint(&swapped), fingerprint);
    let flat = SolidExtents {
        width_caliper: 0.0,
        ..extents
    };
    assert_eq!(compute_fingerprint(&flat), [0.0; 3]);
}

#[test]
fn a_design_shape_records_the_width_the_ratios_cannot_tell() {
    let extents = SolidExtents {
        width_caliper: 3.0,
        length_caliper: 4.17,
        width_axis: 3.0,
        length_axis: 4.17,
        height: 2.0,
        volume: 10.0,
    };
    let shape = design_shape(&extents);
    assert_eq!(shape.fingerprint, compute_fingerprint(&extents));
    assert_eq!(shape.width_caliper, Some(3.0));
    assert_eq!(shape.extents_version, SOLID_EXTENTS_VERSION);

    // The same design drawn twice as large: equal ratios, twice the width.
    let doubled = SolidExtents {
        width_caliper: 6.0,
        length_caliper: 8.34,
        width_axis: 6.0,
        length_axis: 8.34,
        height: 4.0,
        volume: 80.0,
    };
    let twice = design_shape(&doubled);
    assert_eq!(twice.fingerprint, shape.fingerprint);
    assert_eq!(twice.width_caliper, Some(6.0));

    // The width is the smaller caliper extent whichever way round they are stored.
    let swapped = SolidExtents {
        width_caliper: 4.17,
        length_caliper: 3.0,
        ..extents
    };
    assert_eq!(design_shape(&swapped).width_caliper, Some(3.0));

    // Nothing usable: nothing is recorded, not even a version.
    let flat = SolidExtents {
        width_caliper: 0.0,
        ..extents
    };
    assert_eq!(design_shape(&flat), DesignShape::default());
}

#[test]
fn a_file_without_the_width_or_the_rule_version_still_loads_and_a_resave_keeps_them() {
    let layout = sample_layout();
    let ranked = [RankedLayout {
        rank: 1,
        layout: &layout,
    }];
    let text = write_with(&plain_block(), &ranked, &designs());
    assert!(text.contains("width_caliper = 1.25"), "{text}");
    assert!(
        text.contains(&format!("extents_version = {SOLID_EXTENTS_VERSION}")),
        "{text}"
    );

    // An older file carries neither line.
    let older: String = text
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !line.starts_with("width_caliper =") && !line.starts_with("extents_version =")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let loaded = parse_and_validate_plan(&older).expect("an older file loads");
    assert!(loaded.designs.iter().all(|d| d.width_caliper.is_none()));
    assert!(loaded.designs.iter().all(|d| d.extents_version == 0));

    // A plan that has the fields writes them back as they were.
    let current = parse_and_validate_plan(&text).expect("the current file loads");
    let again = write_with(&plain_block(), &ranked, &current.designs);
    assert_eq!(again, text);
}
