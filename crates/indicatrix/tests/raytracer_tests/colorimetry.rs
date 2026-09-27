//! CIE 1931 colour-matching-function integrals and XYZ-to-sRGB gamma mapping tests.

use glam::Vec3;
use indicatrix::optics::raytracer::{cie_1931_cmf, xyz_to_srgb_gamma};

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

    let max_c = rgba[0].max(rgba[1]).max(rgba[2]);
    let min_c = rgba[0].min(rgba[1]).min(rgba[2]);
    assert!(
        max_c - min_c <= 4,
        "D65-neutral XYZ should map to a genuinely grey pixel (got {rgba:?})"
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
