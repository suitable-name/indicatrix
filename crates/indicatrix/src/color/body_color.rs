//! Physically based body color calculation from spectral absorption, illuminant curves,
//! and standard observer color matching functions.
//!
//! Provides [`body_color`], [`body_colors`], and [`delta_e_2000`] (CIEDE2000).

#![expect(
    clippy::suboptimal_flops,
    clippy::imprecise_flops,
    clippy::many_single_char_names,
    clippy::unreadable_literal,
    reason = "CIE and CIEDE2000 formulas are written as published, with their published constants"
)]

use glam::Vec3;

use super::{
    cie1931::cie_1931_cmf,
    gamut::project_to_gamut_bounded,
    space::{ColorSpace, TransferFunction},
};
use crate::optics::{
    absorption::AbsorptionTensor,
    raytracer::{
        apply_von_kries_white_balance, compute_illuminant_white_balance,
        environment::{blackbody_spectrum, d65_relative_spectral_power},
    },
};

/// Standard illuminant for body color calculation.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Illuminant {
    /// CIE Standard Illuminant D65 (daylight reference).
    D65,
    /// Planckian blackbody radiator at color temperature `T` in Kelvin (e.g. 3200.0 K for Incandescent).
    Planckian(f32),
    /// Long-wave ultraviolet lamp (peak at 365.0 nm, LW UV / blacklight).
    UvLongWave365,
    /// Short-wave ultraviolet lamp (peak at 254.0 nm, SW UV / mineralogical).
    UvShortWave254,
    /// One of the CIE 15:2018 LED illuminants (tabulated at 5 nm, linearly interpolated, peak
    /// normalised). Appended last so the earlier variants keep their serialised indices; only
    /// with the `zoning` feature. See [`LedKind`](super::led::LedKind).
    #[cfg(feature = "zoning")]
    Led(super::led::LedKind),
}

impl Illuminant {
    /// Relative spectral power at wavelength `lambda_nm`.
    #[must_use]
    pub fn spectral_power(self, lambda_nm: f64) -> f64 {
        match self {
            Self::D65 => f64::from(d65_relative_spectral_power(lambda_nm as f32)),
            Self::Planckian(t) => f64::from(blackbody_spectrum(lambda_nm as f32, t)),
            Self::UvLongWave365 => {
                // Gaussian peak centered at 365 nm, FWHM 15 nm
                const SIGMA: f64 = 15.0 / 2.354_82;
                let diff = (lambda_nm - 365.0) / SIGMA;
                (-0.5 * diff * diff).exp()
            }
            Self::UvShortWave254 => {
                // Gaussian peak centered at 254 nm, FWHM 10 nm
                const SIGMA: f64 = 10.0 / 2.354_82;
                let diff = (lambda_nm - 254.0) / SIGMA;
                (-0.5 * diff * diff).exp()
            }
            #[cfg(feature = "zoning")]
            Self::Led(kind) => kind.spectral_power(lambda_nm),
        }
    }
}

/// A computed body color in CIE XYZ, CIELAB, and sRGB, with out-of-gamut detection.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BodyColor {
    /// CIE 1931 XYZ tristimulus values normalized so that ideal transmitter (T ≡ 1) has Y = 1.0.
    pub xyz: [f64; 3],
    /// CIE L*a*b* coordinates under the chosen illuminant.
    pub lab: [f64; 3],
    /// 8-bit per channel display-referred sRGB triple.
    pub srgb: [u8; 3],
    /// Whether the color fell outside the sRGB gamut before gamut mapping.
    pub out_of_gamut: bool,
}

/// Body colors for all eigenmodes of an [`AbsorptionTensor`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BodyColors {
    /// Unpolarised transmitted color (mean of eigenmode transmittances).
    pub unpolarised: BodyColor,
    /// Ordinary ray / alpha-axis color.
    pub o_ray: BodyColor,
    /// Extraordinary ray / gamma-axis color.
    pub e_ray: BodyColor,
    /// Optional beta-ray color for trichroic biaxial materials.
    pub beta_ray: Option<BodyColor>,
}

/// Evaluates body color for a given spectral absorption function `alpha_per_mm(lambda)`
/// over path length `path_mm` under illuminant `ill` at 1 nm resolution (380 to 780 nm).
#[must_use]
pub fn body_color(alpha_per_mm: impl Fn(f64) -> f64, path_mm: f64, ill: Illuminant) -> BodyColor {
    body_color_transmittance(
        |lambda| (-alpha_per_mm(lambda).max(0.0) * path_mm).exp(),
        ill,
    )
}

/// Evaluates body colors for an [`AbsorptionTensor`].
#[must_use]
#[expect(
    clippy::option_if_let_else,
    reason = "three-way split: biaxial, uniaxial dichroic, isotropic"
)]
pub fn body_colors(tensor: &AbsorptionTensor, path_mm: f64, ill: Illuminant) -> BodyColors {
    let eval_o = |lambda: f64| -> f64 {
        f64::from(
            tensor
                .o_ray
                .iter()
                .map(|b| b.evaluate(lambda as f32))
                .sum::<f32>(),
        )
    };
    let eval_e = |lambda: f64| -> f64 {
        f64::from(
            tensor
                .e_ray
                .iter()
                .map(|b| b.evaluate(lambda as f32))
                .sum::<f32>(),
        )
    };

    let o_col = body_color(eval_o, path_mm, ill);
    let e_col = body_color(eval_e, path_mm, ill);

    let (beta_col, unpol_col) = if let Some(ref beta_bands) = tensor.beta_ray {
        let eval_beta = |lambda: f64| -> f64 {
            f64::from(
                beta_bands
                    .iter()
                    .map(|b| b.evaluate(lambda as f32))
                    .sum::<f32>(),
            )
        };
        let b_col = body_color(eval_beta, path_mm, ill);

        // Biaxial unpolarised: mean of 3 eigenmode transmittances: T_unpol = (T_alpha + T_beta + T_gamma) / 3
        let unpol = body_color_transmittance(
            |lambda| {
                let to = (-eval_o(lambda).max(0.0) * path_mm).exp();
                let tb = (-eval_beta(lambda).max(0.0) * path_mm).exp();
                let te = (-eval_e(lambda).max(0.0) * path_mm).exp();
                (to + tb + te) / 3.0
            },
            ill,
        );
        (Some(b_col), unpol)
    } else if tensor.is_pleochroic {
        // Uniaxial dichroic: (2*T_o + T_e) / 3
        let unpol = body_color_transmittance(
            |lambda| {
                let to = (-eval_o(lambda).max(0.0) * path_mm).exp();
                let te = (-eval_e(lambda).max(0.0) * path_mm).exp();
                (2.0 * to + te) / 3.0
            },
            ill,
        );
        (None, unpol)
    } else {
        // Isotropic: T_o
        (None, o_col)
    };

    BodyColors {
        unpolarised: unpol_col,
        o_ray: o_col,
        e_ray: e_col,
        beta_ray: beta_col,
    }
}

/// Evaluates body color from an explicit transmittance spectrum `t_fn(lambda)` (clamped to
/// `[0, 1]`).
///
/// `xyz` and `lab` are colorimetric under the illuminant itself (Lab is taken relative to the
/// illuminant's own white, which is what the color-change readout `delta_E_cc` compares).
/// Only the display `srgb` is chromatically adapted: a Planckian illuminant is mapped to D65
/// exactly like the renderer's Incandescent preset white-balances (Bradford von Kries,
/// `compute_illuminant_white_balance`), so a colorless stone shows as white, not orange.
fn body_color_transmittance(t_fn: impl Fn(f64) -> f64, ill: Illuminant) -> BodyColor {
    let display_adaptation = match ill {
        Illuminant::Planckian(temp_k) => Some(compute_illuminant_white_balance(temp_k)),
        Illuminant::D65 | Illuminant::UvLongWave365 | Illuminant::UvShortWave254 => None,
        // The LED lamps are shown as they are, like D65 and the UV lines: no adaptation. A
        // caller that wants the swatch adapted to the screen uses `body_color_from_spectra`
        // with its own `display_adaptation`.
        #[cfg(feature = "zoning")]
        Illuminant::Led(_) => None,
    };
    body_color_from_spectra(
        t_fn,
        |lambda| ill.spectral_power(lambda),
        display_adaptation,
    )
}

/// Evaluates body color from an explicit transmittance spectrum `t_fn(lambda)` and an
/// explicit illuminant curve `spd(lambda)` (relative spectral power), at 1 nm from 380 to
/// 780 nm.
///
/// The entry for a caller that already has both curves in hand (the optical metrics'
/// face-up tone, whose transmittance is a mixture over path lengths and whose illuminant is
/// a lighting preset's own spectrum). `xyz` and `lab` are relative to the SPD's own white.
/// `display_adaptation` is the per-cone von Kries scale (see
/// `compute_illuminant_white_balance`) applied to the XYZ before the sRGB conversion:
/// `Some(scale)` adapts the swatch to the screen's D65, `None` shows the XYZ as it is (the
/// D65 presets and the UV lines, which have no white point to adapt from).
#[must_use]
pub fn body_color_from_spectra(
    t_fn: impl Fn(f64) -> f64,
    spd: impl Fn(f64) -> f64,
    display_adaptation: Option<Vec3>,
) -> BodyColor {
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_z = 0.0;
    let mut white_x = 0.0;
    let mut white_y = 0.0;
    let mut white_z = 0.0;

    for wavelength_i in 380..=780 {
        let lambda = f64::from(wavelength_i);
        let cmf = cie_1931_cmf(lambda as f32);
        let s = spd(lambda);

        let t = t_fn(lambda).clamp(0.0, 1.0);

        let s_x = s * f64::from(cmf[0]);
        let s_y = s * f64::from(cmf[1]);
        let s_z = s * f64::from(cmf[2]);

        sum_x += t * s_x;
        sum_y += t * s_y;
        sum_z += t * s_z;

        white_x += s_x;
        white_y += s_y;
        white_z += s_z;
    }

    let norm_y = if white_y > 1e-12 { white_y } else { 1.0 };
    let xyz = [sum_x / norm_y, sum_y / norm_y, sum_z / norm_y];
    let white_xyz = [white_x / norm_y, 1.0, white_z / norm_y];

    let lab = xyz_to_lab(xyz, white_xyz);
    let (srgb, out_of_gamut) = xyz_to_srgb(display_xyz(xyz, display_adaptation));

    BodyColor {
        xyz,
        lab,
        srgb,
        out_of_gamut,
    }
}

/// XYZ as displayed on an sRGB (D65) screen: with an adaptation scale the renderer's own
/// Bradford von Kries white balance is applied (the Incandescent preset for 3200 K); without
/// one (D65, the UV lines: no white point to adapt from) the XYZ is shown as it is.
fn display_xyz(xyz: [f64; 3], adaptation: Option<Vec3>) -> [f64; 3] {
    let Some(scale) = adaptation else {
        return xyz;
    };
    let v = apply_von_kries_white_balance(
        Vec3::new(xyz[0] as f32, xyz[1] as f32, xyz[2] as f32),
        scale,
    );
    [f64::from(v.x), f64::from(v.y), f64::from(v.z)]
}

/// Converts CIE XYZ to CIE L*a*b* using standard white point.
#[must_use]
pub fn xyz_to_lab(xyz: [f64; 3], white: [f64; 3]) -> [f64; 3] {
    fn f(t: f64) -> f64 {
        const EPSILON: f64 = 216.0 / 24389.0; // (6/29)^3 ≈ 0.008856
        const KAPPA: f64 = 24389.0 / 27.0; // (29/3)^3 ≈ 903.3
        if t > EPSILON {
            t.cbrt()
        } else {
            (KAPPA * t + 16.0) / 116.0
        }
    }

    let xr = f(xyz[0] / white[0].max(1e-9));
    let yr = f(xyz[1] / white[1].max(1e-9));
    let zr = f(xyz[2] / white[2].max(1e-9));

    let l = 116.0 * yr - 16.0;
    let a = 500.0 * (xr - yr);
    let b = 200.0 * (yr - zr);

    [l, a, b]
}

/// Converts sRGB [0..1] to CIE L*a*b* under D65 illuminant.
#[must_use]
pub fn srgb_to_lab(srgb: [f64; 3]) -> [f64; 3] {
    fn srgb_to_linear(c: f64) -> f64 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    let r = srgb_to_linear(srgb[0].clamp(0.0, 1.0));
    let g = srgb_to_linear(srgb[1].clamp(0.0, 1.0));
    let b = srgb_to_linear(srgb[2].clamp(0.0, 1.0));

    let x = 0.4124564 * r + 0.3575761 * g + 0.1804375 * b;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = 0.0193339 * r + 0.1191920 * g + 0.9503041 * b;

    let d65_white = [0.95047, 1.0, 1.08883];
    xyz_to_lab([x, y, z], d65_white)
}

/// Converts CIE XYZ to sRGB and checks whether it fell out of gamut.
fn xyz_to_srgb(xyz: [f64; 3]) -> ([u8; 3], bool) {
    const GAMUT_EPS: f32 = 1e-3;
    let v = Vec3::new(xyz[0] as f32, xyz[1] as f32, xyz[2] as f32);
    let raw = ColorSpace::Srgb.xyz_to_linear(v);
    let out_of_gamut = raw.x < -GAMUT_EPS
        || raw.y < -GAMUT_EPS
        || raw.z < -GAMUT_EPS
        || raw.x > 1.0 + GAMUT_EPS
        || raw.y > 1.0 + GAMUT_EPS
        || raw.z > 1.0 + GAMUT_EPS;

    let bounded = project_to_gamut_bounded(v, ColorSpace::Srgb, 1.0);
    let r = (TransferFunction::Srgb.encode(bounded.x).clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (TransferFunction::Srgb.encode(bounded.y).clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (TransferFunction::Srgb.encode(bounded.z).clamp(0.0, 1.0) * 255.0).round() as u8;

    ([r, g, b], out_of_gamut)
}

/// CIEDE2000 total color difference formula between two L*a*b* colors.
///
/// Implements standard CIEDE2000 (Sharma, Wu, Dalal 2005) with parametric weights `k_L = k_C = k_H = 1.0`.
#[must_use]
pub fn delta_e_2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l, c, h, r_t] = delta_e_2000_terms(lab1, lab2);
    (l * l + c * c + h * h + r_t * c * h).sqrt()
}

/// The CIEDE2000 terms `[dL'/S_L, dC'/S_C, dH'/S_H, R_T]` (k = 1) with
/// `dE00^2 = l^2 + c^2 + h^2 + R_T*c*h`.
///
/// Used by the solver to build a three-component least-squares residual whose squared norm is
/// exactly `dE00^2` (see [`delta_e_2000_residual`]).
#[must_use]
pub fn delta_e_2000_terms(lab1: [f64; 3], lab2: [f64; 3]) -> [f64; 4] {
    let l1 = lab1[0];
    let a1 = lab1[1];
    let b1 = lab1[2];

    let l2 = lab2[0];
    let a2 = lab2[1];
    let b2 = lab2[2];

    let c1_ab = (a1 * a1 + b1 * b1).sqrt();
    let c2_ab = (a2 * a2 + b2 * b2).sqrt();
    let c_bar_ab = f64::midpoint(c1_ab, c2_ab);

    let c_bar_ab_7 = c_bar_ab.powi(7);
    let g = 0.5 * (1.0 - (c_bar_ab_7 / (c_bar_ab_7 + 25.0_f64.powi(7))).sqrt());

    let a1_prime = (1.0 + g) * a1;
    let a2_prime = (1.0 + g) * a2;

    let c1_prime = (a1_prime * a1_prime + b1 * b1).sqrt();
    let c2_prime = (a2_prime * a2_prime + b2 * b2).sqrt();

    let h1_prime = if a1_prime.abs() < 1e-12 && b1.abs() < 1e-12 {
        0.0
    } else {
        let deg = b1.atan2(a1_prime).to_degrees();
        if deg < 0.0 { deg + 360.0 } else { deg }
    };

    let h2_prime = if a2_prime.abs() < 1e-12 && b2.abs() < 1e-12 {
        0.0
    } else {
        let deg = b2.atan2(a2_prime).to_degrees();
        if deg < 0.0 { deg + 360.0 } else { deg }
    };

    let delta_l_prime = l2 - l1;
    let delta_c_prime = c2_prime - c1_prime;

    let h_diff = (h2_prime - h1_prime).abs();
    let delta_h_deg = if c1_prime * c2_prime < 1e-12 {
        0.0
    } else if h_diff <= 180.0 {
        h2_prime - h1_prime
    } else if h2_prime - h1_prime > 180.0 {
        h2_prime - h1_prime - 360.0
    } else {
        h2_prime - h1_prime + 360.0
    };

    let delta_h_prime = 2.0 * (c1_prime * c2_prime).sqrt() * (delta_h_deg.to_radians() * 0.5).sin();

    let l_bar_prime = f64::midpoint(l1, l2);
    let c_bar_prime = f64::midpoint(c1_prime, c2_prime);

    let h_bar_prime = if c1_prime * c2_prime < 1e-12 {
        h1_prime + h2_prime
    } else if h_diff <= 180.0 {
        f64::midpoint(h1_prime, h2_prime)
    } else if h1_prime + h2_prime < 360.0 {
        0.5 * (h1_prime + h2_prime + 360.0)
    } else {
        0.5 * (h1_prime + h2_prime - 360.0)
    };

    let t = 1.0 - 0.17 * (h_bar_prime - 30.0).to_radians().cos()
        + 0.24 * (2.0 * h_bar_prime).to_radians().cos()
        + 0.32 * (3.0 * h_bar_prime + 6.0).to_radians().cos()
        - 0.20 * (4.0 * h_bar_prime - 63.0).to_radians().cos();

    let delta_theta = 30.0 * (-(((h_bar_prime - 275.0) / 25.0).powi(2))).exp();
    let c_bar_prime_7 = c_bar_prime.powi(7);
    let r_c = 2.0 * (c_bar_prime_7 / (c_bar_prime_7 + 25.0_f64.powi(7))).sqrt();

    let l_bar_minus_50_sq = (l_bar_prime - 50.0).powi(2);
    let s_l = 1.0 + (0.015 * l_bar_minus_50_sq) / (20.0 + l_bar_minus_50_sq).sqrt();
    let s_c = 1.0 + 0.045 * c_bar_prime;
    let s_h = 1.0 + 0.015 * c_bar_prime * t;

    let r_t = -(2.0 * delta_theta.to_radians()).sin() * r_c;

    let term_l = delta_l_prime / s_l;
    let term_c = delta_c_prime / s_c;
    let term_h = delta_h_prime / s_h;

    [term_l, term_c, term_h, r_t]
}

/// A 3-vector `r` with `|r|^2 = dE00(lab1, lab2)^2`, for Levenberg-Marquardt: the CIEDE2000
/// terms with the `R_T` cross term completed to a square.
#[must_use]
pub fn delta_e_2000_residual(lab1: [f64; 3], lab2: [f64; 3]) -> [f64; 3] {
    let [l, c, h, r_t] = delta_e_2000_terms(lab1, lab2);
    [
        l,
        0.5f64.mul_add(r_t * h, c),
        h * (1.0 - 0.25 * r_t * r_t).max(0.0).sqrt(),
    ]
}

/// CIE L*C*h(ab) to L*a*b*: `h` in degrees.
#[must_use]
pub fn lch_to_lab(lch: [f64; 3]) -> [f64; 3] {
    let h = lch[2].to_radians();
    [lch[0], lch[1] * h.cos(), lch[1] * h.sin()]
}

/// CIE L*a*b* to L*C*h(ab): `h` in degrees in `[0, 360)` (0 for a neutral colour).
#[must_use]
pub fn lab_to_lch(lab: [f64; 3]) -> [f64; 3] {
    let c = lab[1].hypot(lab[2]);
    let mut h = lab[2].atan2(lab[1]).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }
    if c < 1e-9 {
        h = 0.0;
    }
    [lab[0], c, h]
}

/// The sRGB swatches of the seven-band body colour at each path length.
///
/// Each channel is in `[0, 1]` (D65); `amplitudes` are per millimetre (see
/// `optics::absorption::body_color_bands`) and `paths_mm` are the path lengths: the
/// editor's "at 3 / 5 / 10 mm" swatches.
#[must_use]
pub fn body_color_swatches(amplitudes: &[f32; 7], paths_mm: &[f64]) -> Vec<[f32; 3]> {
    let bands = crate::optics::absorption::body_color_bands(*amplitudes);
    let alpha = |lambda: f64| -> f64 {
        f64::from(bands.iter().map(|b| b.evaluate(lambda as f32)).sum::<f32>())
    };
    paths_mm
        .iter()
        .map(|&path| {
            let c = body_color(alpha, path, Illuminant::D65);
            c.srgb.map(|v| f32::from(v) / 255.0)
        })
        .collect()
}

/// color change metric between D65 daylight and Incandescent 3200K.
#[must_use]
pub fn color_change_delta_e(d65: &BodyColors, incandescent_3200k: &BodyColors) -> f64 {
    delta_e_2000(d65.unpolarised.lab, incandescent_3200k.unpolarised.lab)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciede2000_identical_is_zero() {
        let lab = [50.0, 10.0, -20.0];
        assert_eq!(delta_e_2000(lab, lab), 0.0);
    }

    #[test]
    fn ciede2000_sharma_2005_full_test_set() {
        // (L1, a1, b1, L2, a2, b2, dE00): Sharma, Wu, Dalal (2005), Table 1, pairs 1-34.
        const SHARMA: [[f64; 7]; 34] = [
            [50.0, 2.6772, -79.7751, 50.0, 0.0, -82.7485, 2.0425],
            [50.0, 3.1571, -77.2803, 50.0, 0.0, -82.7485, 2.8615],
            [50.0, 2.8361, -74.0200, 50.0, 0.0, -82.7485, 3.4412],
            [50.0, -1.3802, -84.2814, 50.0, 0.0, -82.7485, 1.0000],
            [50.0, -1.1848, -84.8006, 50.0, 0.0, -82.7485, 1.0000],
            [50.0, -0.9009, -85.5211, 50.0, 0.0, -82.7485, 1.0000],
            [50.0, 0.0, 0.0, 50.0, -1.0, 2.0, 2.3669],
            [50.0, -1.0, 2.0, 50.0, 0.0, 0.0, 2.3669],
            [50.0, 2.49, -0.0010, 50.0, -2.49, 0.0009, 7.1792],
            [50.0, 2.49, -0.0010, 50.0, -2.49, 0.0010, 7.1792],
            [50.0, 2.49, -0.0010, 50.0, -2.49, 0.0011, 7.2195],
            [50.0, 2.49, -0.0010, 50.0, -2.49, 0.0012, 7.2195],
            [50.0, -0.0010, 2.49, 50.0, 0.0009, -2.49, 4.8045],
            [50.0, -0.0010, 2.49, 50.0, 0.0010, -2.49, 4.8045],
            [50.0, -0.0010, 2.49, 50.0, 0.0011, -2.49, 4.7461],
            [50.0, 2.5, 0.0, 50.0, 0.0, -2.5, 4.3065],
            [50.0, 2.5, 0.0, 73.0, 25.0, -18.0, 27.1492],
            [50.0, 2.5, 0.0, 61.0, -5.0, 29.0, 22.8977],
            [50.0, 2.5, 0.0, 56.0, -27.0, -3.0, 31.9030],
            [50.0, 2.5, 0.0, 58.0, 24.0, 15.0, 19.4535],
            [50.0, 2.5, 0.0, 50.0, 3.1736, 0.5854, 1.0000],
            [50.0, 2.5, 0.0, 50.0, 3.2972, 0.0, 1.0000],
            [50.0, 2.5, 0.0, 50.0, 1.8634, 0.5757, 1.0000],
            [50.0, 2.5, 0.0, 50.0, 3.2592, 0.3350, 1.0000],
            [
                60.2574, -34.0099, 36.2677, 60.4626, -34.1751, 39.4387, 1.2644,
            ],
            [
                63.0109, -31.0961, -5.8663, 62.8187, -29.7946, -4.0864, 1.2630,
            ],
            [61.2901, 3.7196, -5.3901, 61.4292, 2.2480, -4.9620, 1.8731],
            [35.0831, -44.1164, 3.7933, 35.0232, -40.0716, 1.5901, 1.8645],
            [
                22.7233, 20.0904, -46.6940, 23.0331, 14.9730, -42.5619, 2.0373,
            ],
            [36.4612, 47.8580, 18.3852, 36.2715, 50.5065, 21.2231, 1.4146],
            [90.8027, -2.0831, 1.4410, 91.1528, -1.6435, 0.0447, 1.4441],
            [90.9257, -0.5406, -0.9208, 88.6381, -0.8985, -0.7239, 1.5381],
            [6.7747, -0.2908, -2.4247, 5.8714, -0.0985, -2.2286, 0.6377],
            [2.0776, 0.0795, -1.1350, 0.9033, -0.0636, -0.5514, 0.9082],
        ];
        for (i, r) in SHARMA.iter().enumerate() {
            let de = delta_e_2000([r[0], r[1], r[2]], [r[3], r[4], r[5]]);
            let de_swapped = delta_e_2000([r[3], r[4], r[5]], [r[0], r[1], r[2]]);
            assert!(
                (de - r[6]).abs() < 1e-4,
                "pair {}: expected {}, got {de}",
                i + 1,
                r[6]
            );
            assert!(
                (de - de_swapped).abs() < 1e-9,
                "pair {} not symmetric",
                i + 1
            );
            // The least-squares residual used by the solver squares to the same value.
            let res = delta_e_2000_residual([r[0], r[1], r[2]], [r[3], r[4], r[5]]);
            let n = res.iter().map(|x| x * x).sum::<f64>().sqrt();
            assert!(
                (n - de).abs() < 1e-9,
                "pair {}: residual norm {n} vs {de}",
                i + 1
            );
        }
    }

    #[test]
    fn planckian_swatch_is_adapted_to_d65_for_display_but_lab_is_under_own_white() {
        // A colorless stone under 3200 K: Lab relative to its own white is neutral and the
        // displayed sRGB is white (previously the un-adapted XYZ rendered orange).
        let col = body_color(|_| 0.0, 5.0, Illuminant::Planckian(3200.0));
        assert!(
            col.lab[1].abs() < 0.5 && col.lab[2].abs() < 0.5,
            "{:?}",
            col.lab
        );
        assert!(
            col.srgb.iter().all(|&c| c >= 253),
            "colorless stone under 3200 K must display white, got {:?}",
            col.srgb
        );
        // The raw XYZ stays colorimetric (a 3200 K white is not the D65 white).
        assert!(
            col.xyz[0] > col.xyz[2],
            "3200 K white is orange in XYZ: {:?}",
            col.xyz
        );
        // D65 is untouched.
        assert_eq!(
            body_color(|_| 0.0, 5.0, Illuminant::D65).srgb,
            [255, 255, 255]
        );
    }

    #[test]
    fn planckian_display_matches_the_renderers_incandescent_white_balance() {
        use crate::optics::raytracer::{
            apply_von_kries_white_balance, compute_illuminant_white_balance,
        };
        let alpha = |l: f64| {
            if (560.0..620.0).contains(&l) {
                0.4
            } else {
                0.02
            }
        };
        let col = body_color(alpha, 5.0, Illuminant::Planckian(3200.0));
        let xyz = Vec3::new(col.xyz[0] as f32, col.xyz[1] as f32, col.xyz[2] as f32);
        let adapted = apply_von_kries_white_balance(xyz, compute_illuminant_white_balance(3200.0));
        let (srgb, _) = xyz_to_srgb([
            f64::from(adapted.x),
            f64::from(adapted.y),
            f64::from(adapted.z),
        ]);
        assert_eq!(col.srgb, srgb);
    }

    fn colour_bits(c: &BodyColor) -> ([u64; 3], [u64; 3], [u8; 3], bool) {
        (
            c.xyz.map(f64::to_bits),
            c.lab.map(f64::to_bits),
            c.srgb,
            c.out_of_gamut,
        )
    }

    #[test]
    fn body_color_from_spectra_is_the_old_function() {
        let alpha = |l: f64| -> f64 {
            if (560.0..620.0).contains(&l) {
                0.4
            } else {
                0.02
            }
        };
        let t = |l: f64| (-alpha(l) * 5.0f64).exp();

        let planck = Illuminant::Planckian(3200.0);
        let old = body_color(alpha, 5.0, planck);
        let new = body_color_from_spectra(
            t,
            |l| planck.spectral_power(l),
            Some(compute_illuminant_white_balance(3200.0)),
        );
        assert_eq!(colour_bits(&old), colour_bits(&new));

        let old_d65 = body_color(alpha, 5.0, Illuminant::D65);
        let new_d65 = body_color_from_spectra(t, |l| Illuminant::D65.spectral_power(l), None);
        assert_eq!(colour_bits(&old_d65), colour_bits(&new_d65));
    }

    #[test]
    fn ideal_transmitter_is_white() {
        let col = body_color(|_| 0.0, 5.0, Illuminant::D65);
        assert!((col.lab[0] - 100.0).abs() < 0.1);
        assert!(col.lab[1].abs() < 0.5);
        assert!(col.lab[2].abs() < 0.5);
        assert_eq!(col.srgb, [255, 255, 255]);
        assert!(!col.out_of_gamut);
    }

    #[test]
    fn total_absorber_is_black() {
        let col = body_color(|_| 1000.0, 5.0, Illuminant::D65);
        assert!(col.lab[0] < 0.1);
        assert_eq!(col.srgb, [0, 0, 0]);
    }

    #[test]
    fn planckian_illuminant_runs() {
        let col = body_color(|_| 0.0, 5.0, Illuminant::Planckian(3200.0));
        assert!((col.lab[0] - 100.0).abs() < 0.1);
    }

    #[test]
    fn lch_lab_round_trip_and_hue_range() {
        for lab in [[50.0, 20.0, -30.0], [70.0, -40.0, 10.0], [30.0, 0.0, 0.0]] {
            let back = lch_to_lab(lab_to_lch(lab));
            for k in 0..3 {
                assert!((back[k] - lab[k]).abs() < 1e-9, "{lab:?} -> {back:?}");
            }
        }
        let lch = lab_to_lch([50.0, 0.0, -10.0]);
        assert!((lch[2] - 270.0).abs() < 1e-9 && (lch[1] - 10.0).abs() < 1e-9);
        assert_eq!(lab_to_lch([50.0, 0.0, 0.0])[2], 0.0);
    }

    #[test]
    fn swatches_darken_with_path_and_clear_is_white() {
        let s = body_color_swatches(&[0.0, 0.0, 0.0, 0.0, 0.1, 0.3, 0.2], &[3.0, 5.0, 10.0]);
        assert_eq!(s.len(), 3);
        let lum = |c: &[f32; 3]| c[0] + c[1] + c[2];
        assert!(lum(&s[0]) > lum(&s[1]) && lum(&s[1]) > lum(&s[2]));
        assert_eq!(
            body_color_swatches(&[0.0; 7], &[5.0]),
            vec![[1.0, 1.0, 1.0]]
        );
    }

    #[test]
    fn srgb_to_lab_pure_white() {
        let lab = srgb_to_lab([1.0, 1.0, 1.0]);
        assert!((lab[0] - 100.0).abs() < 0.1);
        assert!(lab[1].abs() < 0.5);
        assert!(lab[2].abs() < 0.5);
    }
}
