//! Basic [`measure_solid`] coverage: volume/extent figures on hand-built
//! fixtures, the axis-vs-caliper width convention, duplicate-plane handling,
//! and the determinism contract.

use glam::DVec3;

use crate::geometry::{
    cuts::StandardGemCuts,
    plane::GpuFacetPlane,
    stone_metrics::{caliper_frame, measure_solid, measure_solid_with_vertices},
};

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

/// Every figure `measure_solid` reports must scale exactly with the
/// arrangement's own absolute scale -- `volume` as `k^3`, every length as
/// `k` -- across many orders of magnitude, not just near `k = 1`. Before
/// `measure_solid`'s internal normalisation, this real RBC arrangement's
/// `volume / k^3` drifted (+81% at k=1e-3) and `measure_solid` returned
/// `None` above k=~65 (a real vertex escaping to the absolute-`64`-unit
/// blank box, `BLANK_HALF_EXTENT`, once the design's own masts approached
/// it) -- because every internal epsilon here is an absolute constant tuned
/// for masts of order 1. Regression coverage for that measured divergence.
#[test]
fn measure_solid_scales_every_figure_with_the_arrangements_own_scale() {
    let base: Vec<(DVec3, f64)> = StandardGemCuts::standard_round_brilliant()
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();
    let baseline = measure_solid(&base).expect("k=1 must measure");
    for &k in &[1e-4, 1e-3, 0.01, 0.1, 10.0, 30.0, 60.0, 100.0] {
        let scaled: Vec<(DVec3, f64)> = base.iter().map(|&(n, m)| (n, m * k)).collect();
        let m = measure_solid(&scaled).unwrap_or_else(|| panic!("k={k} must still measure"));
        assert_eq!(
            m.vertex_count, baseline.vertex_count,
            "k={k}: vertex count changed"
        );
        let rel =
            |actual: f64, expected: f64| (actual - expected).abs() / expected.abs().max(1e-12);
        assert!(
            rel(m.volume, baseline.volume * k.powi(3)) < 1e-6,
            "k={k}: volume/k^3 = {} vs baseline {}",
            m.volume / k.powi(3),
            baseline.volume
        );
        assert!(
            rel(m.width_axis, baseline.width_axis * k) < 1e-6,
            "k={k}: width_axis/k = {} vs baseline {}",
            m.width_axis / k,
            baseline.width_axis
        );
        assert!(
            rel(m.total_height, baseline.total_height * k) < 1e-6,
            "k={k}: total_height/k = {} vs baseline {}",
            m.total_height / k,
            baseline.total_height
        );
    }
}

/// The fixtures whose figures are pinned bit-for-bit by
/// [`measure_solid_figures_are_pinned_bit_for_bit`].
fn golden_fixtures() -> Vec<(&'static str, Vec<(DVec3, f64)>)> {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let brilliant = StandardGemCuts::standard_round_brilliant()
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();
    let box_planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    let roof_planes = vec![
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
    let rotated_box = vec![
        (DVec3::new(s, 0.0, s), 1.0),
        (DVec3::new(-s, 0.0, s), 1.0),
        (DVec3::new(s, 0.0, -s), 1.0),
        (DVec3::new(-s, 0.0, -s), 1.0),
        (DVec3::Y, 0.5),
        (DVec3::NEG_Y, 0.5),
    ];
    vec![
        ("brilliant", brilliant),
        ("box", box_planes),
        ("roof", roof_planes),
        ("rotated", rotated_box),
    ]
}

/// Pins `volume`, `width_caliper` and `total_height` of the round brilliant and
/// the three hand-built fixtures to literal bit patterns. Unlike a comparison
/// between two calls of the same code path, this fails when the measuring rule
/// (and with it the cached `SOLID_EXTENTS_VERSION` contract) changes.
// Must be re-pinned after the caliper hypot/tie-break and pow2_scale_norm
// exponent-extraction change (last-bit movement of width_caliper and volume).
// Brilliant volume re-pinned for glam 0.34.1 (DMat3 determinant reorder, 1 ULP).
#[test]
fn measure_solid_figures_are_pinned_bit_for_bit() {
    let golden: [(&str, u64, u64, u64); 4] = [
        (
            "brilliant",
            0x3ffb_0a48_00da_e156,
            0x3fff_ffff_e989_4f9e,
            0x3ff3_3333_3000_0000,
        ),
        (
            "box",
            0x4013_3333_3333_3333,
            0x4000_0000_0000_0000,
            0x3ff3_3333_3333_3333,
        ),
        (
            "roof",
            0x400a_aaaa_aaaa_aaac,
            0x4000_0000_0000_0000,
            0x3ff8_0000_0000_0000,
        ),
        (
            "rotated",
            0x400f_ffff_ffff_ffff,
            0x4000_0000_0000_0000,
            0x3ff0_0000_0000_0000,
        ),
    ];
    let mut mismatches: Vec<String> = Vec::new();
    for ((label, planes), (golden_label, volume, width, height)) in
        golden_fixtures().into_iter().zip(golden)
    {
        assert_eq!(label, golden_label, "fixture order changed");
        let m = measure_solid(&planes).expect("fixture must measure");
        for (field, value, pinned) in [
            ("volume", m.volume, volume),
            ("width_caliper", m.width_caliper, width),
            ("total_height", m.total_height, height),
        ] {
            let actual = value.to_bits();
            if actual != pinned {
                mismatches.push(format!(
                    "{label}: {field} = {value:?}; actual bits {actual:#018x} ({actual}); pinned bits {pinned:#018x} ({pinned})"
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} pinned value(s) drifted:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// Extents of a vertex set along each axis.
fn bbox_extents(verts: &[DVec3]) -> DVec3 {
    let lo = verts
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |a, v| a.min(*v));
    let hi = verts
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |a, v| a.max(*v));
    hi - lo
}

/// The vertices come back in `planes`' own units even when the internal
/// power-of-two normalisation scale is not 1: the bounding box of the returned
/// vertices must equal the reported extents, for a box at 100x and 0.01x and
/// for the round brilliant scaled by 0.3.
#[test]
fn returned_vertices_are_scaled_back_by_the_normalisation_scale() {
    let base_box = [
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    for k in [100.0_f64, 0.01] {
        let planes: Vec<(DVec3, f64)> = base_box.iter().map(|&(n, m)| (n, m * k)).collect();
        let (m, verts) = measure_solid_with_vertices(&planes).expect("scaled box must measure");
        let ext = bbox_extents(&verts);
        assert!(
            2.0_f64.mul_add(-k, ext.x).abs() < 1e-9 * k,
            "k={k}: x {}",
            ext.x
        );
        assert!(
            1.2_f64.mul_add(-k, ext.y).abs() < 1e-9 * k,
            "k={k}: y {}",
            ext.y
        );
        assert!(
            2.0_f64.mul_add(-k, ext.z).abs() < 1e-9 * k,
            "k={k}: z {}",
            ext.z
        );
        assert!((ext.y - m.total_height).abs() < 1e-9 * k);
        for v in &verts {
            assert!(v.x.abs() <= k * (1.0 + 1e-9) && v.z.abs() <= k * (1.0 + 1e-9));
        }
    }

    let k = 30.0 / 100.0;
    let planes: Vec<(DVec3, f64)> = StandardGemCuts::standard_round_brilliant()
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .map(|(n, m)| (n, m * k))
        .collect();
    let (m, verts) = measure_solid_with_vertices(&planes).expect("scaled RBC must measure");
    let ext = bbox_extents(&verts);
    let (lo, hi) = if ext.x <= ext.z {
        (ext.x, ext.z)
    } else {
        (ext.z, ext.x)
    };
    assert!((ext.y - m.total_height).abs() < 1e-9 * k, "y {}", ext.y);
    assert!((lo - m.width_axis).abs() < 1e-9 * k, "width {lo}");
    assert!((hi - m.length_axis).abs() < 1e-9 * k, "length {hi}");
}

/// `measure_solid_with_vertices` returns figures bit-identical to `measure_solid`,
/// and returns the exact number of vertices reported in `vertex_count`.
#[test]
fn measure_solid_with_vertices_matches_measure_solid_on_fixtures() {
    for (label, planes) in golden_fixtures() {
        let expected = measure_solid(&planes).expect("fixture must measure");
        let (actual, verts) =
            measure_solid_with_vertices(&planes).expect("fixture with vertices must measure");
        assert_eq!(
            expected, actual,
            "{label}: measure_solid and measure_solid_with_vertices metrics must be bit-identical"
        );
        assert_eq!(
            verts.len(),
            actual.vertex_count,
            "{label}: vertex count must equal vertex_count metric"
        );
    }

    // Also verify unbounded returns None
    let unbounded = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    assert!(measure_solid_with_vertices(&unbounded).is_none());
}

/// Caliper frame extents match `caliper_extents` on the rotated outline fixture,
/// and `width_dir` is a unit vector.
#[test]
fn caliper_frame_matches_caliper_extents_on_rotated_box() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let planes = vec![
        (DVec3::new(s, 0.0, s), 1.0),
        (DVec3::new(-s, 0.0, s), 1.0),
        (DVec3::new(s, 0.0, -s), 1.0),
        (DVec3::new(-s, 0.0, -s), 1.0),
        (DVec3::Y, 0.5),
        (DVec3::NEG_Y, 0.5),
    ];
    let (_, verts) = measure_solid_with_vertices(&planes).expect("must measure");
    let outline: Vec<(f64, f64)> = verts.iter().map(|v| (v.x, v.z)).collect();
    let frame = caliper_frame(&outline).expect("frame must measure");
    assert!((frame.width - 2.0).abs() < 1e-12, "width {}", frame.width);
    assert!(
        (frame.length - 2.0).abs() < 1e-12,
        "length {}",
        frame.length
    );
    let dir_len = frame.width_dir[0].hypot(frame.width_dir[1]);
    assert!(
        (dir_len - 1.0).abs() < 1e-12,
        "width_dir must be unit vector"
    );
}
