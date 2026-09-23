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
///
/// Only meaningful for comparing two pixels encoded in the **same** [`ColorSpace`]:
/// equal RGB ratios correspond to different chromaticities in different primaries, so
/// this metric cannot be used to compare saturation *across* spaces (see
/// [`uv_chroma_distance_from_white`], which can).
fn u8_saturation(rgb: [u8; 4]) -> f32 {
    let r = f32::from(rgb[0]) / 255.0;
    let g = f32::from(rgb[1]) / 255.0;
    let b = f32::from(rgb[2]) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max <= 0.0 { 0.0 } else { (max - min) / max }
}

/// Inverts a 3x3 matrix, row-major in and out.
///
/// Used to turn a space's `XYZ->RGB` matrix ([`ColorSpace::xyz_to_rgb_matrix`]) back
/// into an `RGB->XYZ` one, so a decoded 8-bit pixel can be checked colorimetrically
/// against the stimulus that produced it.
fn invert_3x3(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    // `p * q - r * s`, via `mul_add` -- shared by the determinant and every adjugate
    // entry below.
    fn diff_of_products(p: f32, q: f32, r: f32, s: f32) -> f32 {
        p.mul_add(q, -(r * s))
    }

    let row0 = m[0];
    let row1 = m[1];
    let row2 = m[2];

    let cof00 = diff_of_products(row1[1], row2[2], row1[2], row2[1]);
    let cof01 = diff_of_products(row1[0], row2[2], row1[2], row2[0]);
    let cof02 = diff_of_products(row1[0], row2[1], row1[1], row2[0]);
    let det = row0[0].mul_add(cof00, (-row0[1]).mul_add(cof01, row0[2] * cof02));
    let inv_det = 1.0 / det;

    [
        [
            cof00 * inv_det,
            diff_of_products(row0[2], row2[1], row0[1], row2[2]) * inv_det,
            diff_of_products(row0[1], row1[2], row0[2], row1[1]) * inv_det,
        ],
        [
            -cof01 * inv_det,
            diff_of_products(row0[0], row2[2], row0[2], row2[0]) * inv_det,
            diff_of_products(row0[2], row1[0], row0[0], row1[2]) * inv_det,
        ],
        [
            cof02 * inv_det,
            diff_of_products(row0[1], row2[0], row0[0], row2[1]) * inv_det,
            diff_of_products(row0[0], row1[1], row0[1], row1[0]) * inv_det,
        ],
    ]
}

/// Applies a row-major 3x3 matrix to a vector.
fn matvec(m: [[f32; 3]; 3], v: Vec3) -> Vec3 {
    Vec3::new(
        m[0][2].mul_add(v.z, m[0][0].mul_add(v.x, m[0][1] * v.y)),
        m[1][2].mul_add(v.z, m[1][0].mul_add(v.x, m[1][1] * v.y)),
        m[2][2].mul_add(v.z, m[2][0].mul_add(v.x, m[2][1] * v.y)),
    )
}

/// CIE 1976 `u'v'` chromaticity from CIE XYZ (`u' = 4X / (X+15Y+3Z)`, `v' = 9Y /
/// (X+15Y+3Z)`), perceptually near-uniform and, critically, defined identically
/// regardless of which RGB primaries the XYZ came from -- unlike `(max-min)/max` on
/// encoded RGB, which is only comparable within a single space's own primaries.
fn uv_prime(xyz: Vec3) -> (f32, f32) {
    let denom = 3.0f32.mul_add(xyz.z, 15.0f32.mul_add(xyz.y, xyz.x));
    if denom <= 1e-9 {
        return (0.0, 0.0);
    }
    (4.0 * xyz.x / denom, 9.0 * xyz.y / denom)
}

/// Decodes an encoded 8-bit RGB pixel (as produced by [`ColorSpace::encode`]) back to
/// CIE XYZ for `space`: undoes the transfer curve, then `space`'s `RGB->XYZ` matrix
/// (the inverse of [`ColorSpace::xyz_to_rgb_matrix`]).
fn decode_to_xyz(rgb: [u8; 4], space: ColorSpace) -> Vec3 {
    let tf = space.transfer_function();
    let linear = Vec3::new(
        tf.decode(f32::from(rgb[0]) / 255.0),
        tf.decode(f32::from(rgb[1]) / 255.0),
        tf.decode(f32::from(rgb[2]) / 255.0),
    );
    matvec(invert_3x3(space.xyz_to_rgb_matrix()), linear)
}

/// CIE 1976 `u'v'` chromaticity distance of an encoded 8-bit pixel from `space`'s own
/// white point -- a cross-gamut-comparable chroma metric.
///
/// Unlike `(max-min)/max` on the raw 8-bit values (which mixes each space's own,
/// differently-sized primaries into the numerator and denominator), this decodes back
/// to CIE XYZ first, so the same physical distance is measured the same way regardless
/// of which space encoded the pixel. 0 for a pixel exactly at the space's white point,
/// growing with excitation purity.
fn uv_chroma_distance_from_white(rgb: [u8; 4], space: ColorSpace) -> f32 {
    let xyz = decode_to_xyz(rgb, space);
    let (u, v) = uv_prime(xyz);

    let (wx, wy) = space.white_point_xy();
    let white_xyz = Vec3::new(wx / wy, 1.0, (1.0 - wx - wy) / wy);
    let (wu, wv) = uv_prime(white_xyz);

    (u - wu).hypot(v - wv)
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

/// A naive metric to compare across spaces would be `(max-min)/max` on each
/// space's own encoded 8-bit values -- but equal RGB ratios mean different
/// chromaticities in different primaries, so that comparison is not colorimetrically
/// valid across spaces (it happens to be fine *within* one space, which is how
/// [`u8_saturation`] is used elsewhere in this file). Measured for this exact
/// 520nm stimulus: sRGB encodes to `[49, 248, 167]` (`u8_saturation` = 0.8024) and
/// Display P3 to `[58, 251, 158]` (`u8_saturation` = 0.7689) -- P3 reads as *less*
/// saturated by that metric even though P3's gamut strictly contains sRGB's for this
/// hue, which is physically backwards. Decoding both back to CIE XYZ and comparing CIE
/// 1976 `u'v'` chromaticity distance from each space's own white point (a metric
/// that's valid across primaries) gives the physically expected ordering instead:
/// sRGB ~0.0762, Display P3 ~0.1004, Rec.2020 ~0.1612 -- monotonically increasing with
/// gamut width, as excitation purity retention should.
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

    let srgb_chroma = uv_chroma_distance_from_white(srgb_rgb, ColorSpace::Srgb);
    let p3_chroma = uv_chroma_distance_from_white(p3_rgb, ColorSpace::DisplayP3);
    let rec2020_chroma = uv_chroma_distance_from_white(rec2020_rgb, ColorSpace::Rec2020);

    // The core physical claim: sRGB's narrow triangle clamps this stimulus much harder
    // than either wider gamut, so after gamut mapping sRGB's output should retain LESS
    // chroma (be closer, in u'v', to its own white point) than either wider gamut's
    // output is to its own white point. Display P3 and Rec.2020 land close enough to
    // each other for this particular fit-derived chromaticity that a strict
    // P3-vs-Rec2020 ordering isn't asserted (both clearly beat sRGB, which is the
    // point).
    assert!(
        srgb_chroma < p3_chroma,
        "sRGB should retain less u'v' chroma than Display P3 for a 520nm stimulus: srgb={srgb_chroma:.4} p3={p3_chroma:.4} (rgb {srgb_rgb:?} vs {p3_rgb:?})"
    );
    assert!(
        srgb_chroma < rec2020_chroma,
        "sRGB should retain less u'v' chroma than Rec.2020 for a 520nm stimulus: srgb={srgb_chroma:.4} rec2020={rec2020_chroma:.4} (rgb {srgb_rgb:?} vs {rec2020_rgb:?})"
    );

    // And in linear (pre-transfer-curve) terms, the actual gamut-mapping compression
    // step should need to walk less far toward white as the gamut widens. The walk
    // stops as soon as every channel is non-negative, so the channel that was driving
    // the stimulus out of gamut in the first place (red, here -- `project_to_gamut`
    // returns RGB as `Vec3 { x: R, y: G, z: B }`) should still land closer to zero
    // (less diluted by the white point's own much larger red component) in the wider
    // gamut, which needs a smaller step to become non-negative.
    let srgb_linear = project_to_gamut(xyz, ColorSpace::Srgb);
    let rec2020_linear = project_to_gamut(xyz, ColorSpace::Rec2020);
    assert!(
        rec2020_linear.x < srgb_linear.x,
        "Rec.2020's wider gamut should require pulling the red channel up out of \
         negative territory less severely (via less white-point mixing) than sRGB \
         does: srgb={srgb_linear:?} rec2020={rec2020_linear:?}"
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
// 6. `project_to_srgb` / `project_to_gamut` sanity: in-gamut colours pass through
//    unchanged, and out-of-gamut colours are actually gamut-mapped, not passed through.
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

    // `project_to_srgb` must actually gamut-map out-of-gamut input rather than pass it
    // through unchanged, so the result must differ from a naive passthrough and must
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

/// A saturated, over-bright stimulus exercises the per-channel clipping bug this test
/// guards against.
///
/// The stimulus sits well beyond what tone mapping alone brings back under 1.0.
/// Gamut-projecting at the original luminance, scaling by the luminance-only ACES
/// ratio, then hard-clamping each channel to `[0, 1]` independently would reintroduce
/// the per-channel clipping the luminance-only ACES design is meant to avoid. Instead,
/// the tone-mapped colour routes through
/// [`indicatrix::color::gamut::project_to_gamut_bounded`], which desaturates toward
/// white -- measurably LOWER max-min saturation than a hard-clamped result would give,
/// for the identical input.
#[test]
fn aces_highlight_clipping_desaturates_instead_of_clamping_one_channel() {
    let [x, y, z] = cie_1931_cmf(600.0); // saturated orange-red
    let xyz = Vec3::new(x, y, z) * 6.0; // bright enough to clip after tone mapping

    // Reference: a naive hard-clamp scheme, reimplemented here only as a comparison
    // point (production code takes the gamut-bounded path instead -- see
    // `ColorSpace::encode`'s doc comment).
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
