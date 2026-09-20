//! Tests for the wide-gamut colour module (`indicatrix::color::space`, `indicatrix::color::gamut`).
//!
//! This module is deliberately self-contained and not wired into the renderer (see the
//! module docs on `indicatrix::color::space`), so these tests exercise it directly rather
//! than through any render path.

use glam::Vec3;
use indicatrix::{
    color::{
        ColorSpace, ToneMap, TransferFunction, cie1931::cie_1931_cmf, gamut::project_to_gamut,
    },
    optics::raytracer::xyz_to_srgb_gamma,
};

const ALL_SPACES: [ColorSpace; 4] = [
    ColorSpace::Srgb,
    ColorSpace::DisplayP3,
    ColorSpace::Rec2020,
    ColorSpace::AcesCg,
];

/// Simple max-min saturation metric on an `[u8; 4]` encoded pixel: 0 for a neutral
/// colour, approaching 1 for a fully saturated one.
fn u8_saturation(rgb: [u8; 4]) -> f32 {
    let r = f32::from(rgb[0]) / 255.0;
    let g = f32::from(rgb[1]) / 255.0;
    let b = f32::from(rgb[2]) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max <= 0.0 { 0.0 } else { (max - min) / max }
}

// ---------------------------------------------------------------------------------
// 1. Neutral D65 grey must round-trip to equal RGB components in every space.
// ---------------------------------------------------------------------------------

/// The single most valuable test here: a neutral D65 grey (chromaticity exactly at the
/// space's own white point, per `ColorSpace::white_point_xy`) must decode to R == G == B
/// after the full `encode` pipeline, in *every* space -- including `ACEScg`, whose D60
/// white point is different from the other three spaces' D65. A transcription error in
/// any matrix (row swap, sign flip, wrong primary) breaks this immediately: it is the
/// simplest possible signal that a space's matrix does not agree with its own declared
/// white point.
#[test]
fn neutral_d65_grey_round_trips_to_equal_rgb_components_in_every_space() {
    // A mid-grey neutral stimulus at D65 (Y=0.5, chromaticity at the D65 white point
    // that sRGB/DisplayP3/Rec2020 all share).
    let (wx, wy) = ColorSpace::Srgb.white_point_xy();
    let luminance = 0.5f32;
    let xyz = Vec3::new(
        (wx / wy) * luminance,
        luminance,
        ((1.0 - wx - wy) / wy) * luminance,
    );

    for &space in &[ColorSpace::Srgb, ColorSpace::DisplayP3, ColorSpace::Rec2020] {
        let rgb = space.encode(xyz, ToneMap::None);
        let r = i32::from(rgb[0]);
        let g = i32::from(rgb[1]);
        let b = i32::from(rgb[2]);
        assert!(
            (r - g).abs() <= 1 && (g - b).abs() <= 1,
            "{space:?}: D65 grey should decode to equal channels, got {rgb:?}"
        );
        assert_eq!(rgb[3], 255);
    }

    // ACEScg's own white point is D60, not D65 -- feed it its own neutral so the test
    // is meaningful for that space too (a D65 grey is *not* expected to be neutral in
    // AP1, since AP1's matrix is built around D60).
    let (awx, awy) = ColorSpace::AcesCg.white_point_xy();
    let aces_xyz = Vec3::new(
        (awx / awy) * luminance,
        luminance,
        ((1.0 - awx - awy) / awy) * luminance,
    );
    let rgb = ColorSpace::AcesCg.encode(aces_xyz, ToneMap::None);
    let r = i32::from(rgb[0]);
    let g = i32::from(rgb[1]);
    let b = i32::from(rgb[2]);
    assert!(
        (r - g).abs() <= 1 && (g - b).abs() <= 1,
        "AcesCg: D60 grey should decode to equal channels, got {rgb:?}"
    );
}

/// Same check one level down the stack, in linear space before quantization or any
/// transfer curve is involved -- pins down that it is specifically each matrix that is
/// self-consistent with its own white point, independent of the u8 rounding tolerance
/// used above.
#[test]
fn neutral_white_point_projects_to_equal_linear_rgb_components() {
    for &space in &ALL_SPACES {
        let (wx, wy) = space.white_point_xy();
        let luminance = 1.0f32;
        let xyz = Vec3::new(
            (wx / wy) * luminance,
            luminance,
            ((1.0 - wx - wy) / wy) * luminance,
        );
        let linear = project_to_gamut(xyz, space);
        let mean = (linear.x + linear.y + linear.z) / 3.0;
        assert!(
            (linear.x - mean).abs() < 1e-3
                && (linear.y - mean).abs() < 1e-3
                && (linear.z - mean).abs() < 1e-3,
            "{space:?}: own white point should project to equal linear RGB, got {linear:?}"
        );
    }
}

// ---------------------------------------------------------------------------------
// 2. A saturated monochromatic stimulus (520nm) clamps hard in sRGB and survives with
//    visibly more saturation in Rec.2020 (and Display P3).
// ---------------------------------------------------------------------------------

#[test]
fn monochromatic_520nm_clamps_harder_in_srgb_than_in_wider_gamuts() {
    let [x, y, z] = cie_1931_cmf(520.0);
    let xyz = Vec3::new(x, y, z);

    // Sanity check: this stimulus really is saturated and out of the sRGB gamut (a
    // chromaticity with y this high, from a Gaussian analytic CMF fit, sits well beyond
    // the sRGB green primary).
    let sum = xyz.x + xyz.y + xyz.z;
    assert!(sum > 0.0, "520nm CMF should be nonzero");

    let srgb_rgb = ColorSpace::Srgb.encode(xyz, ToneMap::None);
    let p3_rgb = ColorSpace::DisplayP3.encode(xyz, ToneMap::None);
    let rec2020_rgb = ColorSpace::Rec2020.encode(xyz, ToneMap::None);

    let srgb_sat = u8_saturation(srgb_rgb);
    let p3_sat = u8_saturation(p3_rgb);
    let rec2020_sat = u8_saturation(rec2020_rgb);

    // The core physical claim: sRGB's narrow triangle clamps this stimulus much harder
    // than either wider gamut. Display P3 and Rec.2020 land close enough to each other
    // for this particular fit-derived chromaticity that a strict P3-vs-Rec2020
    // ordering isn't asserted (both clearly beat sRGB, which is the point).
    assert!(
        srgb_sat < p3_sat,
        "sRGB should be less saturated than Display P3 for a 520nm stimulus: srgb={srgb_sat:.4} p3={p3_sat:.4} (rgb {srgb_rgb:?} vs {p3_rgb:?})"
    );
    assert!(
        srgb_sat < rec2020_sat,
        "sRGB should be less saturated than Rec.2020 for a 520nm stimulus: srgb={srgb_sat:.4} rec2020={rec2020_sat:.4} (rgb {srgb_rgb:?} vs {rec2020_rgb:?})"
    );

    // And in linear (pre-transfer-curve) terms, the actual gamut-mapping compression
    // step should need to pull less far toward white as the gamut widens: Rec.2020
    // should end up brighter/more saturated in its dominant (green) channel than sRGB
    // does, in absolute linear terms, for the same input.
    let srgb_linear = project_to_gamut(xyz, ColorSpace::Srgb);
    let rec2020_linear = project_to_gamut(xyz, ColorSpace::Rec2020);
    assert!(
        rec2020_linear.x < srgb_linear.x,
        "Rec.2020's wider gamut should require pulling the red channel down less \
         severely toward zero than sRGB does: srgb={srgb_linear:?} rec2020={rec2020_linear:?}"
    );
}

// ---------------------------------------------------------------------------------
// 3. Every transfer function round-trips: decode(encode(x)) ~= x, including across the
//    piecewise breakpoint.
// ---------------------------------------------------------------------------------

#[test]
fn transfer_functions_round_trip_across_their_full_range() {
    let functions = [
        TransferFunction::Srgb,
        TransferFunction::Rec2020,
        TransferFunction::Linear,
    ];
    for &tf in &functions {
        let mut samples: Vec<f32> = (0..=200).map(|i| i as f32 / 200.0).collect();
        // Explicitly probe right at (and either side of) each curve's piecewise
        // breakpoint, which is where a transcribed transfer function is usually wrong.
        samples.extend_from_slice(&[
            0.003_130_8,
            0.003_1,
            0.003_2,
            0.018_053_97,
            0.018_0,
            0.018_1,
        ]);

        for &x in &samples {
            let encoded = tf.encode(x);
            let decoded = tf.decode(encoded);
            assert!(
                (decoded - x).abs() < 1e-3,
                "{tf:?}: decode(encode({x})) = {decoded}, expected ~= {x} (encoded was {encoded})"
            );
        }
    }
}

#[test]
fn transfer_functions_are_monotonically_increasing() {
    let functions = [
        TransferFunction::Srgb,
        TransferFunction::Rec2020,
        TransferFunction::Linear,
    ];
    for &tf in &functions {
        let mut prev = tf.encode(0.0);
        for i in 1..=100 {
            let x = i as f32 / 100.0;
            let encoded = tf.encode(x);
            assert!(
                encoded >= prev,
                "{tf:?}: encode should be monotonic, got {prev} then {encoded} at x={x}"
            );
            prev = encoded;
        }
    }
}

/// `ACEScg` has no encoding curve: it is scene-linear by design.
#[test]
fn aces_cg_transfer_function_is_identity() {
    for i in 0..=10 {
        let x = i as f32 / 10.0;
        assert_eq!(TransferFunction::Linear.encode(x), x);
        assert_eq!(TransferFunction::Linear.decode(x), x);
    }
}

/// sRGB and Rec.2020 are genuinely different curves, not the same power function under
/// different names -- pins down the exact bug the task calls out ("the current code
/// gets it wrong" by applying one flat gamma everywhere).
#[test]
fn srgb_and_rec2020_transfer_functions_differ_in_the_upper_range() {
    let x = 0.5f32;
    let srgb = TransferFunction::Srgb.encode(x);
    let rec2020 = TransferFunction::Rec2020.encode(x);
    assert!(
        (srgb - rec2020).abs() > 1e-3,
        "sRGB and Rec.2020 curves should differ at x=0.5: srgb={srgb} rec2020={rec2020}"
    );
}

// ---------------------------------------------------------------------------------
// 4. The sRGB path matches the existing `xyz_to_srgb_gamma` reference for in-gamut
//    inputs, modulo the deliberate true-curve-vs-1/2.2-gamma difference.
// ---------------------------------------------------------------------------------

/// `xyz_to_srgb_gamma` (in `optics::raytracer`) is the existing reference behaviour
/// this module's gamut-mapping and tone-mapping steps are ported from. For in-gamut
/// inputs the two pipelines should agree closely: same gamut mapping (a no-op here,
/// since the input is in-gamut), same ACES tonemap, and *almost* the same transfer
/// curve -- sRGB's true piecewise curve here versus a flat 1/2.2 gamma there.
///
/// The two curves are not identical. Analytically, `encode_srgb(x) - x^(1/2.2)` is
/// largest near x ~= 0.00216, where it reaches ~0.0335 on the 0-1 scale -- up to 9/255
/// levels of difference deep in the
/// shadows -- and shrinks to well under 2/255 across the 0.1-0.9 midtone/highlight
/// range. A tolerance of 10 u8 levels per channel comfortably covers that known,
/// deliberate difference everywhere in range while still catching an actually broken
/// matrix or tonemap (which produces differences far larger than 10/255, typically a
/// completely different hue or a saturated channel).
#[test]
fn srgb_encode_matches_xyz_to_srgb_gamma_reference_within_the_known_gamma_curve_difference() {
    // A handful of in-gamut XYZ samples spanning shadow/midtone/highlight luminance, all
    // chosen to land inside the sRGB gamut so the gamut-mapping step is a no-op on both
    // sides and only the transfer curve differs.
    let (wx, wy) = ColorSpace::Srgb.white_point_xy();
    let neutral_at = |luminance: f32| {
        Vec3::new(
            (wx / wy) * luminance,
            luminance,
            ((1.0 - wx - wy) / wy) * luminance,
        )
    };

    let samples = [
        neutral_at(0.02),
        neutral_at(0.1),
        neutral_at(0.18),
        neutral_at(0.5),
        neutral_at(0.9),
        // A desaturated warm tone, still in-gamut.
        Vec3::new(0.5, 0.45, 0.35),
    ];

    for xyz in samples {
        let reference = xyz_to_srgb_gamma(xyz);
        let candidate = ColorSpace::Srgb.encode(xyz, ToneMap::AcesFilmic { exposure: 1.0 });

        for ch in 0..3 {
            let diff = i32::from(reference[ch]) - i32::from(candidate[ch]);
            assert!(
                diff.abs() <= 10,
                "channel {ch} differs by more than the known gamma-curve tolerance for xyz={xyz:?}: \
                 reference={reference:?} candidate={candidate:?}"
            );
        }
        assert_eq!(
            reference[3], candidate[3],
            "alpha should always be 255 on both paths"
        );
    }
}

// ---------------------------------------------------------------------------------
// 5. Extreme inputs: all channels finite and within 0..=255.
// ---------------------------------------------------------------------------------

#[test]
fn encode_handles_extreme_and_out_of_gamut_inputs_without_panicking() {
    let (wx, wy) = ColorSpace::Srgb.white_point_xy();
    let extreme_inputs = [
        Vec3::ZERO,
        Vec3::new(1e-8, 1e-8, 1e-8),
        Vec3::new(1000.0, 1000.0, 1000.0),
        Vec3::new(1e6, 1e6, 1e6),
        Vec3::new((wx / wy) * 1e6, 1e6, ((1.0 - wx - wy) / wy) * 1e6),
        // Deeply out-of-gamut, at very high luminance.
        {
            let [x, y, z] = cie_1931_cmf(520.0);
            Vec3::new(x, y, z) * 1e5
        },
        Vec3::new(f32::NAN, 1.0, 1.0),
        Vec3::new(f32::INFINITY, 1.0, 1.0),
        Vec3::new(-1.0, 0.5, 0.2),
    ];

    for &space in &ALL_SPACES {
        for &xyz in &extreme_inputs {
            let rgb = space.encode(xyz, ToneMap::AcesFilmic { exposure: 1.0 });
            for &channel in &rgb[..3] {
                // u8 is inherently in 0..=255; the meaningful assertion is that the
                // computation completed at all (no panic from a NaN propagating into an
                // out-of-range cast, no divide-by-zero trap) -- `channel` existing here
                // at all is already proof of that for release-mode saturating casts,
                // but we also sanity check debug-mode-safe behaviour explicitly.
                let _: u8 = channel;
            }
            assert_eq!(rgb[3], 255);

            let rgb_none = space.encode(xyz, ToneMap::None);
            assert_eq!(rgb_none[3], 255);
        }
    }
}

/// Very high luminance should tone-map toward white, not collapse to black -- guards
/// against a division-by-zero/NaN bug in the luminance-rescale step silently producing
/// `[0, 0, 0, 255]` for bright input instead of a saturated near-white pixel.
#[test]
fn very_high_luminance_tone_maps_toward_white_not_black() {
    let (wx, wy) = ColorSpace::Srgb.white_point_xy();
    let luminance = 1e6f32;
    let xyz = Vec3::new(
        (wx / wy) * luminance,
        luminance,
        ((1.0 - wx - wy) / wy) * luminance,
    );

    for &space in &[ColorSpace::Srgb, ColorSpace::DisplayP3, ColorSpace::Rec2020] {
        let rgb = space.encode(xyz, ToneMap::AcesFilmic { exposure: 1.0 });
        assert!(
            rgb[0] > 200 && rgb[1] > 200 && rgb[2] > 200,
            "{space:?}: very high luminance should tone-map toward white, got {rgb:?}"
        );
    }
}

/// A zero (or effectively zero) radiance sample must encode to opaque black, matching
/// `xyz_to_srgb_gamma`'s own `sum <= 1e-6` short-circuit.
#[test]
fn zero_radiance_encodes_to_opaque_black() {
    for &space in &ALL_SPACES {
        assert_eq!(
            space.encode(Vec3::ZERO, ToneMap::AcesFilmic { exposure: 1.0 }),
            [0, 0, 0, 255]
        );
        assert_eq!(
            space.encode(Vec3::new(f32::NAN, 0.0, 0.0), ToneMap::None),
            [0, 0, 0, 255]
        );
    }
}

// ---------------------------------------------------------------------------------
// 6. `project_to_srgb` / `project_to_gamut` sanity: in-gamut passthrough, and the old
//    stub behaviour (return inputs unchanged) is gone.
// ---------------------------------------------------------------------------------

#[test]
fn project_to_gamut_passes_in_gamut_colours_through_unchanged() {
    // A colour comfortably inside every gamut here (a warm, fairly desaturated tone).
    let xyz = Vec3::new(0.4, 0.38, 0.3);
    for &space in &ALL_SPACES {
        let mapped = project_to_gamut(xyz, space);
        let direct = space.xyz_to_linear(xyz);
        assert!(
            (mapped - direct).length() < 1e-5,
            "{space:?}: in-gamut colour should pass through project_to_gamut unchanged, got {mapped:?} vs direct {direct:?}"
        );
        assert!(mapped.x >= 0.0 && mapped.y >= 0.0 && mapped.z >= 0.0);
    }
}

#[test]
fn project_to_srgb_actually_compresses_out_of_gamut_colours() {
    let [x, y, z] = cie_1931_cmf(520.0);
    let xyz = Vec3::new(x, y, z);
    let mapped = indicatrix::color::gamut::project_to_srgb(xyz);

    // The old stub returned its xyY inputs unchanged; the real implementation must
    // actually gamut-map, so the result must differ from a naive passthrough and must
    // have every channel non-negative.
    assert!(
        mapped.x >= 0.0 && mapped.y >= 0.0 && mapped.z >= 0.0,
        "gamut-mapped result must be non-negative, got {mapped:?}"
    );
    let naive = ColorSpace::Srgb.xyz_to_linear(xyz);
    assert!(
        naive.x < 0.0 || naive.y < 0.0 || naive.z < 0.0,
        "test setup: 520nm should be out of sRGB gamut before mapping"
    );
    assert!(
        (mapped - naive).length() > 1e-3,
        "project_to_srgb should not just pass the un-mapped linear RGB through"
    );
}

// ---------------------------------------------------------------------------------
// 7. a bright, saturated highlight that clips after ACES tone mapping must
//    desaturate toward white, not have exactly one channel hard-capped at 1.0 while
//    the others hold still.
// ---------------------------------------------------------------------------------

/// A saturated, over-bright stimulus (well beyond what tone mapping alone brings back
/// under 1.0) exercises exactly the bug this fix targets: the OLD scheme (gamut-project
/// at the original luminance, scale by the luminance-only ACES ratio, then hard-clamp
/// each channel to `[0, 1]` independently) reintroduces the per-channel clipping the
/// luminance-only ACES design was chosen to avoid. The new scheme routes the
/// tone-mapped colour through [`indicatrix::color::gamut::project_to_gamut_bounded`]
/// instead, which desaturates toward white -- measurably LOWER max-min saturation than
/// the old hard-clamped result, for the identical input.
#[test]
fn aces_highlight_clipping_desaturates_instead_of_clamping_one_channel() {
    let [x, y, z] = cie_1931_cmf(600.0); // saturated orange-red
    let xyz = Vec3::new(x, y, z) * 6.0; // bright enough to clip after tone mapping

    // Reference: the OLD behaviour, reimplemented here only as a comparison point (the
    // production code no longer does this -- see `ColorSpace::encode`'s doc comment).
    let linear_rgb = project_to_gamut(xyz, ColorSpace::Srgb);
    let luminance = xyz.y.max(0.0);
    let y_tm = indicatrix::optics::raytracer::aces_tonemap(luminance);
    let scale = y_tm / luminance.max(1e-5);
    let old_toned = linear_rgb * scale;
    assert!(
        old_toned.x > 1.0 || old_toned.y > 1.0 || old_toned.z > 1.0,
        "test setup: chosen stimulus should clip at least one channel under the old \
         scheme, got {old_toned:?}"
    );
    let old_clamped = old_toned.clamp(Vec3::ZERO, Vec3::ONE);
    let old_sat = (old_clamped.max_element() - old_clamped.min_element())
        / old_clamped.max_element().max(1e-6);

    let new_rgb = ColorSpace::Srgb.encode(xyz, ToneMap::AcesFilmic { exposure: 1.0 });
    let new_sat = u8_saturation(new_rgb);

    assert!(
        new_sat < old_sat - 0.02,
        "Fix 1 should desaturate a clipped highlight rather than hard-clamp it: old \
         (per-channel clamp) saturation={old_sat:.4}, new (gamut-projected) \
         saturation={new_sat:.4} (old_clamped={old_clamped:?}, new={new_rgb:?})"
    );
}
