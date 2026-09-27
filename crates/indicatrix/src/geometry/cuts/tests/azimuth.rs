//! [`StandardGemCuts::index_to_azimuth`] coverage: bit-identical agreement
//! with the pre-fix plain formula at a zero reference angle, additivity of a
//! nonzero reference angle, and [`StandardGemCuts::from_asc_schedule`]'s use
//! of it end to end.

use std::f32::consts::PI;

use glam::Vec3;
use indicatrix_formats::asc::{AscSchedule, AscTier};

use crate::geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane};

/// `index_to_azimuth` with `gear_reference_angle == 0.0` must be bit-for-bit
/// identical to the plain `2*pi*index/gear_teeth` formula it replaces --
/// every schedule that never set a reference angle (the default, and
/// 65.1% of the real corpus, see the function's own doc comment) must
/// render exactly as it did before this fix.
#[test]
fn index_to_azimuth_zero_reference_angle_is_bit_for_bit_identical_to_the_plain_formula() {
    for &gear_teeth in &[16.0f32, 32.0, 64.0, 80.0, 96.0, 120.0] {
        for idx in 0..gear_teeth as i32 {
            let index = idx as f32;
            let old = 2.0 * PI * index / gear_teeth;
            let new = StandardGemCuts::index_to_azimuth(index, gear_teeth, 0.0);
            assert_eq!(
                old.to_bits(),
                new.to_bits(),
                "index {index}, gear {gear_teeth}: old={old}, new={new}"
            );
        }
    }
}

/// A nonzero reference angle is a pure additive offset on the raw index,
/// not a rescale or a sign flip: shifting every index in a set by `k` and
/// leaving the reference angle at zero must land on the same azimuth as
/// leaving the index alone and setting the reference angle to `k` (both
/// in tooth units, per the function's doc comment).
#[test]
fn index_to_azimuth_reference_angle_is_additive_on_the_index() {
    let gear_teeth = 96.0f32;
    for &(index, reference_angle) in &[(0.0, 48.0), (5.0, 20.0), (84.0, -6.0)] {
        let via_reference_angle =
            StandardGemCuts::index_to_azimuth(index, gear_teeth, reference_angle);
        let via_shifted_index =
            StandardGemCuts::index_to_azimuth(index + reference_angle, gear_teeth, 0.0);
        assert!(
            (via_reference_angle - via_shifted_index).abs() < 1e-6,
            "index {index}, reference angle {reference_angle}"
        );
    }
}

/// `from_asc_schedule` at `gear_reference_angle == 0.0` reproduces exactly
/// what the pre-fix `2*pi*index/gear_teeth_abs()` formula produced --
/// existing scenes reconstructed from schedules that never set a
/// reference angle must not move a single bit.
#[test]
fn from_asc_schedule_zero_reference_angle_reproduces_the_previous_mapping_bit_for_bit() {
    let gear_teeth = 96.0f32;
    let mast = 0.6f64;
    let indices = vec![3.0, 17.0, 55.0, 90.0];
    let schedule = AscSchedule {
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 1,
        refractive_index: 1.0,
        tiers: vec![AscTier {
            angle_deg: -41.0,
            mast,
            indices: indices.clone(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let planes = StandardGemCuts::from_asc_schedule(&schedule);
    assert_eq!(planes.len(), indices.len());

    let theta = 41.0f32.to_radians();
    let d = -(mast.abs() as f32);
    for (&idx, plane) in indices.iter().zip(&planes) {
        let phi = 2.0 * PI * (idx as f32) / gear_teeth; // the pre-fix formula, verbatim
        let expected = GpuFacetPlane::new(
            Vec3::new(
                theta.sin() * phi.cos(),
                -theta.cos(),
                theta.sin() * phi.sin(),
            ),
            d,
        );
        assert_eq!(*plane, expected, "index {idx}");
    }
}

/// An asymmetric index set (not symmetric under `i -> gear - i`, so a
/// mirror-transform bug would be visible) with a nonzero, corpus-typical
/// reference angle (`gear_teeth / 2`, the most common nonzero value in
/// the real corpus) maps to exactly the expected azimuths.
#[test]
fn from_asc_schedule_asymmetric_indices_map_to_expected_azimuths_with_reference_angle() {
    let gear_teeth = 96.0f32;
    let reference_angle = 48.0f64; // gear_teeth / 2, the dominant nonzero corpus value
    let mast = 0.6f64;
    // Asymmetric under i -> 96 - i: 96-5=91, 96-20=76, 96-47=49 -- none of
    // those are in the set, so this is a real corpus-shaped asymmetric tier,
    // not a rotationally clean one.
    let indices = vec![5.0, 20.0, 47.0];
    let schedule = AscSchedule {
        gear_teeth: 96,
        gear_reference_angle: reference_angle,
        symmetry_order: 1,
        refractive_index: 1.0,
        tiers: vec![AscTier {
            angle_deg: 40.0,
            mast,
            indices: indices.clone(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let planes = StandardGemCuts::from_asc_schedule(&schedule);
    assert_eq!(planes.len(), indices.len());

    let theta = 40.0f32.to_radians();
    for (&idx, plane) in indices.iter().zip(&planes) {
        let phi = 2.0 * PI * (idx as f32 + reference_angle as f32) / gear_teeth;
        let expected = [
            theta.sin() * phi.cos(),
            theta.cos(),
            theta.sin() * phi.sin(),
        ];
        for (component, &value) in plane.normal.iter().zip(expected.iter()) {
            assert!(
                (component - value).abs() < 1e-5,
                "index {idx}: got {component}, expected {value}"
            );
        }
    }
}

/// A tier whose indices are spaced evenly around the whole wheel (an
/// 8-fold symmetric star, `.asc`'s own `clean_fold` shape -- see the
/// corpus measurement cited in `from_asc_schedule`'s doc comment) has its
/// entire plane set rotated as one rigid whole by a nonzero reference
/// angle -- never distorted per-facet.
#[test]
fn from_asc_schedule_symmetric_design_is_globally_rotated_by_reference_angle() {
    let gear_teeth = 96.0f32;
    let mast = 0.59f64;
    let indices = vec![0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    // Deliberately not a multiple of gear/8 (the set's own symmetry step),
    // so the rotated plane set is genuinely distinct from the base one
    // rather than mapping onto itself.
    let reference_angle = 6.0f64;

    let base = AscSchedule {
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        refractive_index: 1.0,
        tiers: vec![AscTier {
            angle_deg: 34.5,
            mast,
            indices: indices.clone(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut rotated = base.clone();
    rotated.gear_reference_angle = reference_angle;

    let planes_base = StandardGemCuts::from_asc_schedule(&base);
    let planes_rotated = StandardGemCuts::from_asc_schedule(&rotated);
    assert_eq!(planes_base.len(), planes_rotated.len());
    assert_eq!(planes_base.len(), indices.len());

    let delta = 2.0 * PI * reference_angle as f32 / gear_teeth;
    let (sin_delta, cos_delta) = (delta.sin(), delta.cos());
    for base_plane in &planes_base {
        let n = base_plane.normal;
        let rotated_normal = [
            n[0].mul_add(cos_delta, -(n[2] * sin_delta)),
            n[1],
            n[0].mul_add(sin_delta, n[2] * cos_delta),
        ];
        let matches_one = planes_rotated.iter().any(|p| {
            (p.normal[0] - rotated_normal[0]).abs() < 1e-5
                && (p.normal[1] - rotated_normal[1]).abs() < 1e-5
                && (p.normal[2] - rotated_normal[2]).abs() < 1e-5
                && (p.d - base_plane.d).abs() < 1e-5
        });
        assert!(
            matches_one,
            "base normal {n:?} rotated by {delta} rad must reappear in the \
             reference-angle-shifted schedule's plane set"
        );
    }
}
