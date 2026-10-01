//! [`StandardGemCuts::index_to_azimuth`] coverage: bit-identical agreement
//! with the pre-fix plain formula at a zero reference angle, additivity of a
//! nonzero reference angle, and [`StandardGemCuts::from_asc_schedule`]'s use
//! of it end to end.

use std::f32::consts::PI;

use glam::Vec3;
use indicatrix_formats::asc::{AscSchedule, AscTier};

use crate::geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane};

/// Asserts `got` is within one unit in the last place of `want` (both non-negative, so
/// the bit patterns order like the values).
fn assert_within_one_ulp(got: f32, want: f32, what: &str) {
    let distance = got.to_bits().abs_diff(want.to_bits());
    assert!(
        distance <= 1,
        "{what}: got {got} ({:#010x}), expected {want} ({:#010x}), {distance} ULP apart",
        got.to_bits(),
        want.to_bits()
    );
}

/// `index_to_azimuth` maps tooth `index` of a `gear_teeth` wheel to the angle
/// `2*pi*(index + reference) / gear_teeth`. Expectations are written as exact turn
/// fractions, not by repeating the formula:
///
/// - `(0, 96)` is the zero turn: `0`.
/// - `(24, 96)` is `24/96 = 1/4` turn: `pi/2`.
/// - `(48, 96)` is `1/2` turn: `pi`.
/// - `(1, 8)` is `1/8` turn: `pi/4`.
///
/// All reference angles are zero. `pi/2`, `pi` and `pi/4` are the `f32` constants
/// (exact power-of-two scalings of `f32` pi); the computed value may differ from them
/// by the rounding of the multiply and divide, hence a 1-ULP tolerance.
#[test]
fn index_to_azimuth_zero_reference_angle_gives_exact_turn_fractions() {
    assert_eq!(StandardGemCuts::index_to_azimuth(0.0, 96.0, 0.0), 0.0);
    assert_within_one_ulp(
        StandardGemCuts::index_to_azimuth(24.0, 96.0, 0.0),
        std::f32::consts::FRAC_PI_2,
        "24/96 turn",
    );
    assert_within_one_ulp(
        StandardGemCuts::index_to_azimuth(48.0, 96.0, 0.0),
        PI,
        "48/96 turn",
    );
    assert_within_one_ulp(
        StandardGemCuts::index_to_azimuth(1.0, 8.0, 0.0),
        std::f32::consts::FRAC_PI_4,
        "1/8 turn",
    );
}

/// A nonzero reference angle adds to the index before the turn fraction is taken:
/// index 12 with reference 6 on a 96-tooth wheel is tooth 18, i.e. `18/96 = 3/16` turn,
/// which is `2*pi*3/16 = 3*pi/8 = 1.178_097_245...` rad (1-ULP tolerance for `f32`
/// rounding of the literal and of the computation).
#[test]
fn index_to_azimuth_nonzero_reference_angle_offsets_the_turn_fraction() {
    assert_within_one_ulp(
        StandardGemCuts::index_to_azimuth(12.0, 96.0, 6.0),
        1.178_097_2,
        "(12 + 6)/96 turn",
    );
}

/// The mapping does not reduce modulo a turn: tooth 96 of a 96-tooth wheel is the full
/// turn `2*pi` (not `0`), which points in the same direction as tooth 0 (`cos == 1`).
#[test]
fn index_to_azimuth_full_turn_is_two_pi() {
    let full = StandardGemCuts::index_to_azimuth(96.0, 96.0, 0.0);
    assert_within_one_ulp(full, 2.0 * PI, "96/96 turn");
    assert!(
        (full.cos() - 1.0).abs() < 1e-6,
        "cos of a full turn: {}",
        full.cos()
    );
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
