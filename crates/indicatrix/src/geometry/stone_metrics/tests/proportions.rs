//! [`StoneProportions`] coverage: the table-percent readout and its `None`
//! case, plus the `to_mm` scale conversion.

use glam::DVec3;

use crate::geometry::stone_metrics::{
    SolidStatus, StoneProportions, build_solid_mesh, measure_solid,
};

/// The plain box's flat top IS its table (normal exactly `+Y`), spanning
/// the whole width -- `table_percent` must read 100%, and
/// `length_to_width` must be 1.0 (a square footprint).
#[test]
fn stone_proportions_reads_100_percent_table_on_a_plain_box() {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    let metrics = measure_solid(&planes).expect("box must measure");
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("box must close");
    };
    let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
    assert!((proportions.table_percent.expect("box top is a table") - 100.0).abs() < 1e-6);
    assert!((proportions.length_to_width.expect("positive width") - 1.0).abs() < 1e-9);
    assert!((proportions.total_depth - metrics.total_height).abs() < 1e-9);
    // The box's four side planes are all girdle (normal.y == 0), spanning the
    // whole height, so crown/pavilion are both zero and the girdle band is the
    // box's whole 1.2-unit height over a width of 2.0 -- 60%.
    assert!((proportions.crown_to_width_percent.expect("girdle present")).abs() < 1e-9);
    assert!(
        (proportions
            .pavilion_to_width_percent
            .expect("girdle present"))
        .abs()
            < 1e-9
    );
    assert!((proportions.girdle_to_width_percent.expect("girdle present") - 60.0).abs() < 1e-6);

    let mm = proportions.to_mm(2.0);
    assert!((proportions.total_depth.mul_add(-2.0, mm.total_depth)).abs() < 1e-9);
    assert_eq!(mm.table_percent, proportions.table_percent);
    assert!(
        (proportions
            .girdle_thickness
            .expect("girdle present")
            .mul_add(-2.0, mm.girdle_thickness.expect("girdle present")))
        .abs()
            < 1e-9
    );
    // Percentages are scale-invariant: `to_mm` must leave them untouched.
    assert_eq!(
        mm.crown_to_width_percent,
        proportions.crown_to_width_percent
    );
    assert_eq!(
        mm.pavilion_to_width_percent,
        proportions.pavilion_to_width_percent
    );
    assert_eq!(
        mm.girdle_to_width_percent,
        proportions.girdle_to_width_percent
    );
}

/// The hip-roofed block has no facet with an outward normal near
/// straight up -- its top is a ridge, not a table -- so `table_percent`
/// must be `None`.
#[test]
fn stone_proportions_reports_no_table_on_a_hip_roofed_block() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
        (DVec3::NEG_Y, 0.5),
        (DVec3::new(s, s, 0.0), s),
        (DVec3::new(-s, s, 0.0), s),
        (DVec3::new(0.0, s, s), s),
        (DVec3::new(0.0, s, -s), s),
    ];
    let metrics = measure_solid(&planes).expect("roofed block must measure");
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("roofed block must close");
    };
    let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
    assert!(proportions.table_percent.is_none());
}
