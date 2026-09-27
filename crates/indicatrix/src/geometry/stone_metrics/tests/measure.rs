//! Basic [`measure_solid`] coverage: volume/extent figures on hand-built
//! fixtures, the axis-vs-caliper width convention, duplicate-plane handling,
//! and the determinism contract.

use glam::DVec3;

use crate::geometry::stone_metrics::measure_solid;

/// Axis-aligned box `[-1,1] x [-0.6,0.6] x [-1,1]`: volume 4.8, width 2,
/// length 2, height 1.2. The four vertical walls are girdle planes cut by
/// nothing, so the girdle band spans the full height and crown/pavilion are
/// zero.
#[test]
fn measures_a_plain_box() {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    let m = measure_solid(&planes).expect("box must measure");
    assert!((m.volume - 4.8).abs() < 1e-9, "volume {}", m.volume);
    assert!((m.width_axis - 2.0).abs() < 1e-9);
    assert!((m.length_axis - 2.0).abs() < 1e-9);
    assert!((m.width_caliper - 2.0).abs() < 1e-9);
    assert!((m.total_height - 1.2).abs() < 1e-9);
    assert_eq!(m.vertex_count, 8);
    assert!((m.crown_height.expect("girdle present")).abs() < 1e-9);
    assert!((m.pavilion_depth.expect("girdle present")).abs() < 1e-9);
    assert!((m.girdle_thickness.expect("girdle present") - 1.2).abs() < 1e-9);
}

/// A hip-roofed block: square girdle walls at `|x|,|z| <= 1`, flat floor at
/// `y = -0.5`, and four 45-degree crown planes `y <= 1 - |x|`, `y <= 1 - |z|`.
/// Hand-computed: volume `2 + 4/3`, ridge apex at `y = 1`, girdle band
/// clipped at `y = 0`, so crown height 1, pavilion depth 0, girdle 0.5.
#[test]
fn measures_crown_height_against_the_girdle_band() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
        (DVec3::NEG_Y, 0.5),
        // 45-degree crown planes: n = (+-s, s, 0) and (0, s, +-s), m = s,
        // i.e. x + y = 1 etc.
        (DVec3::new(s, s, 0.0), s),
        (DVec3::new(-s, s, 0.0), s),
        (DVec3::new(0.0, s, s), s),
        (DVec3::new(0.0, s, -s), s),
    ];
    let m = measure_solid(&planes).expect("roofed block must measure");
    assert!(
        (m.volume - (2.0 + 4.0 / 3.0)).abs() < 1e-9,
        "volume {}",
        m.volume
    );
    assert!((m.total_height - 1.5).abs() < 1e-9);
    assert!((m.crown_height.expect("girdle present") - 1.0).abs() < 1e-9);
    assert!((m.pavilion_depth.expect("girdle present")).abs() < 1e-9);
    assert!((m.girdle_thickness.expect("girdle present") - 0.5).abs() < 1e-9);
    assert!((m.width_axis - 2.0).abs() < 1e-9);
}

/// A solid the real planes never close (no floor): must report `None`, not a
/// blank-box-clipped volume.
#[test]
fn unbounded_solid_reports_none() {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    assert!(measure_solid(&planes).is_none());
}

/// A duplicated plane (same normal and offset listed twice) must not
/// double-count its face's area.
#[test]
fn duplicate_planes_do_not_double_count() {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    let m = measure_solid(&planes).expect("box must measure");
    assert!((m.volume - 4.8).abs() < 1e-9, "volume {}", m.volume);
}

/// A 45-degree-rotated square girdle: axis extents see the diagonal
/// (`2*sqrt(2)`), calipers must recover the true side length 2.
#[test]
fn caliper_width_beats_axis_width_on_a_rotated_outline() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let planes = vec![
        (DVec3::new(s, 0.0, s), 1.0),
        (DVec3::new(-s, 0.0, s), 1.0),
        (DVec3::new(s, 0.0, -s), 1.0),
        (DVec3::new(-s, 0.0, -s), 1.0),
        (DVec3::Y, 0.5),
        (DVec3::NEG_Y, 0.5),
    ];
    let m = measure_solid(&planes).expect("rotated box must measure");
    let diag = 2.0 * std::f64::consts::SQRT_2;
    assert!((m.width_axis - diag).abs() < 1e-9, "axis {}", m.width_axis);
    assert!(
        (m.width_caliper - 2.0).abs() < 1e-9,
        "caliper {}",
        m.width_caliper
    );
    // Side-2 square cross-section, height 1.
    assert!((m.volume - 4.0).abs() < 1e-9, "volume {}", m.volume);
}

/// Byte-identical determinism across repeated calls.
#[test]
fn measurement_is_deterministic() {
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
    let a = measure_solid(&planes).expect("must measure");
    let b = measure_solid(&planes).expect("must measure");
    assert_eq!(a.volume.to_bits(), b.volume.to_bits());
    assert_eq!(a.width_caliper.to_bits(), b.width_caliper.to_bits());
    assert_eq!(a.total_height.to_bits(), b.total_height.to_bits());
}
