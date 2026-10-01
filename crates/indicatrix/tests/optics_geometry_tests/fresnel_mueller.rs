//! `fresnel_transmission` Mueller-matrix regression -- at normal incidence
//! there is no polarization effect, so element (1,1) and element (3,3)/(4,4) of the
//! transmission Mueller matrix must be equal (no spurious `sqrt()`).

use indicatrix::optics::polarization::MuellerMatrix;

// ---------------------------------------------------------------------------
// fresnel_transmission must not apply a spurious sqrt() to the (3,3)/(4,4)
// element. At normal incidence there is no polarization effect, so element
// (1,1) [top-left, "m11"] and element (3,3) ["m33"] of the Mueller matrix must
// be equal.
// ---------------------------------------------------------------------------
#[test]
fn fresnel_transmission_normal_incidence_m11_equals_m33() {
    let n1 = 1.0f32;
    let n2 = 2.4178f32;
    let cos_i = 1.0f32;
    let cos_t = 1.0f32;

    // Standard Fresnel amplitude transmission coefficients.
    let t_s = (2.0 * n1 * cos_i) / n2.mul_add(cos_t, n1 * cos_i);
    let t_p = (2.0 * n1 * cos_i) / n1.mul_add(cos_t, n2 * cos_i);

    let m = MuellerMatrix::fresnel_transmission(n1, n2, cos_i, cos_t, t_s, t_p);

    // glam::Mat4 is column-major and MuellerMatrix::fresnel_transmission is built via
    // Mat4::from_cols_array(&[a, b, 0, 0,  b, a, 0, 0,  0, 0, c, 0,  0, 0, 0, c]).
    // Column 0 is [a, b, 0, 0] and column 2 is [0, 0, c, 0], so:
    //   m11 (row0, col0) = m.x_axis.x
    //   m33 (row2, col2) = m.z_axis.z
    let m11 = m.x_axis.x;
    let m33 = m.z_axis.z;

    // Independently cross-check against the value quoted in the bug report.
    assert!(
        (m11 - 0.8279).abs() < 1e-3,
        "expected m11 ~= 0.8279, got {m11}"
    );

    assert!(
        (m11 - m33).abs() < 1e-5,
        "at normal incidence m11 and m33 must agree (no polarization effect): m11={m11}, m33={m33}"
    );

    // The buggy version (with .sqrt() on m33) produced ~0.9099, a 9.9% deviation --
    // make sure we are nowhere near that.
    assert!(
        (m33 - 0.9099).abs() > 0.01,
        "m33 looks like the old, buggy sqrt() value: {m33}"
    );
}
