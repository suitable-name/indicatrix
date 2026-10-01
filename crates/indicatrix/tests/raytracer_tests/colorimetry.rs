//! CIE 1931 colour-matching-function integrals and XYZ-to-sRGB gamma mapping tests.

use glam::Vec3;
use indicatrix::{
    color::ColorSpace,
    optics::raytracer::{cie_1931_cmf, xyz_to_srgb_gamma},
};

#[test]
fn test_cmf_equal_energy_white_is_neutral() {
    let mut xyz = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        xyz += cie_1931_cmf(lambda);
    }

    let sum = xyz.x + xyz.y + xyz.z;
    let x = xyz.x / sum;
    let y = xyz.y / sum;

    assert!(
        (x - 1.0 / 3.0).abs() < 0.005,
        "Equal-energy white chromaticity x should be ~1/3 (got {x})"
    );
    assert!(
        (y - 1.0 / 3.0).abs() < 0.005,
        "Equal-energy white chromaticity y should be ~1/3 (got {y})"
    );
}

#[test]
fn test_cmf_lobe_integrals() {
    let mut sum_x = 0.0f32;
    let mut sum_y = 0.0f32;
    let mut sum_z = 0.0f32;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        let cmf = cie_1931_cmf(lambda);
        sum_x += cmf.x;
        sum_y += cmf.y;
        sum_z += cmf.z;
    }

    assert!(
        (sum_x - 106.8).abs() < 2.0,
        "Integral of x-bar should be ~106.8 (got {sum_x})"
    );
    assert!(
        (sum_y - 106.8).abs() < 2.0,
        "Integral of y-bar should be ~106.8 (got {sum_y})"
    );
    assert!(
        (sum_z - 106.8).abs() < 2.0,
        "Integral of z-bar should be ~106.8 (got {sum_z})"
    );
}

#[test]
fn test_xyz_to_srgb_neutral_grey() {
    let k = 0.18f32;
    let xyz = Vec3::new(0.9505 * k, 1.0 * k, 1.0890 * k);
    let rgba = xyz_to_srgb_gamma(xyz);

    // Decode the 8-bit pixel back to linear sRGB, then to XYZ, and compare its
    // chromaticity with D65. The tolerance covers the half-code-value quantisation
    // error of a mid-grey pixel (up to ~0.003 in xy), which a max-min channel spread
    // cannot express in colorimetric terms.
    let transfer = ColorSpace::Srgb.transfer_function();
    let lin = |c: u8| transfer.decode(f32::from(c) / 255.0);
    let [red, green, blue] = [lin(rgba[0]), lin(rgba[1]), lin(rgba[2])];
    let decoded = Vec3::new(
        0.412_456_4f32.mul_add(red, 0.357_576_1f32.mul_add(green, 0.180_437_5 * blue)),
        0.212_672_9f32.mul_add(red, 0.715_152_2f32.mul_add(green, 0.072_175 * blue)),
        0.019_333_9f32.mul_add(red, 0.119_192f32.mul_add(green, 0.950_304_1 * blue)),
    );
    let sum = decoded.x + decoded.y + decoded.z;
    let [cx, cy] = [decoded.x / sum, decoded.y / sum];
    assert!(
        (cx - 0.3127).abs() < 0.004 && (cy - 0.3290).abs() < 0.004,
        "D65-neutral XYZ should map to a pixel at the D65 chromaticity (got xy = ({cx}, {cy}) from {rgba:?})"
    );
}

#[test]
fn test_xyz_to_srgb_out_of_gamut_is_finite() {
    let xyz = cie_1931_cmf(450.0) * 2.0;
    let rgba = xyz_to_srgb_gamma(xyz);

    assert!(
        rgba[0] > 0 || rgba[1] > 0 || rgba[2] > 0,
        "Strongly saturated out-of-gamut monochromatic colour must not collapse to pure black (got {rgba:?})"
    );
}
