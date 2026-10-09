//! Tests for the camera spectral module. Written, not run, by the lane that created them.

use indicatrix::color::{cie1931::cie_1931_cmf, led::LedKind};

use super::{
    fallback::{bradford_d50_to_d65, mul_vec3},
    linalg::{cholesky, cholesky_solve, nnls_normal},
    response::trapezoid_weight,
    *,
};

const SRGB_TO_XYZ: [[f64; 3]; 3] = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192_0, 0.950_304_1],
];

fn gauss(lambda: f64, centre: f64, sigma: f64) -> f64 {
    (-0.5 * ((lambda - centre) / sigma).powi(2)).exp()
}

/// A smooth, positive "Gaussian-sum" camera: broad channels with a little cross talk.
fn true_response() -> CameraResponse {
    let s: [[f64; GRID_LEN]; 3] = [
        std::array::from_fn(|i| {
            let l = grid_wavelength_nm(i);
            0.10f64.mul_add(gauss(l, 450.0, 35.0), gauss(l, 600.0, 40.0))
        }),
        std::array::from_fn(|i| {
            let l = grid_wavelength_nm(i);
            0.05f64.mul_add(gauss(l, 620.0, 30.0), gauss(l, 540.0, 38.0))
        }),
        std::array::from_fn(|i| {
            let l = grid_wavelength_nm(i);
            0.06f64.mul_add(gauss(l, 600.0, 40.0), gauss(l, 455.0, 38.0))
        }),
    ];
    CameraResponse::from_grid(s, ResponseTier::Measured).unwrap()
}

fn led_spd(kind: LedKind) -> [f64; GRID_LEN] {
    *BacklightSpectrum::from_led(kind).unwrap().spd()
}

/// Deterministic Gaussian noise (splitmix64 + Box-Muller).
struct Noise(u64);

impl Noise {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f64 {
        let u1 = self.uniform();
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * core::f64::consts::PI * u2).cos()
    }
}

fn make_filters(
    truth: &CameraResponse,
    backlight: &[f64; GRID_LEN],
    centres: &[f64],
    mut noise: Option<(Noise, f64)>,
) -> Vec<ReferenceFilter> {
    let white = truth.camera_rgb_grid(backlight);
    centres
        .iter()
        .enumerate()
        .map(|(k, &centre)| {
            let mut t = [0.0; GRID_LEN];
            let mut lit = [0.0; GRID_LEN];
            for i in 0..GRID_LEN {
                t[i] = 0.96f64.mul_add(gauss(grid_wavelength_nm(i), centre, 15.0), 0.02);
                lit[i] = t[i] * backlight[i];
            }
            let through = truth.camera_rgb_grid(&lit);
            let mut rgb = [0.0; 3];
            for c in 0..3 {
                rgb[c] = through[c] / white[c];
                if let Some((generator, sigma)) = noise.as_mut() {
                    rgb[c] = f64::mul_add(*sigma, generator.gaussian(), rgb[c]);
                }
            }
            ReferenceFilter::new(&format!("filter {k}"), t, rgb).unwrap()
        })
        .collect()
}

/// RMS over 400 to 700 nm of `(estimate - truth_normalised) / peak(truth_normalised)`, where
/// the truth is scaled to the calibration's normalisation (backlight-weighted mean 1).
fn recovery_rms(
    estimate: &CameraResponse,
    truth: &CameraResponse,
    backlight: &[f64; GRID_LEN],
) -> f64 {
    let omega = backlight_weights(backlight).unwrap();
    let mut sum = 0.0;
    let mut count = 0.0;
    for c in 0..3 {
        let scale: f64 = (0..GRID_LEN)
            .map(|i| omega[i] * truth.sensitivity()[c][i])
            .sum();
        let peak = truth.sensitivity()[c]
            .iter()
            .fold(0.0_f64, |m, v| m.max(*v))
            / scale;
        for i in 0..GRID_LEN {
            let l = grid_wavelength_nm(i);
            if (400.0..=700.0).contains(&l) {
                let expected = truth.sensitivity()[c][i] / scale;
                sum += ((estimate.sensitivity()[c][i] - expected) / peak).powi(2);
                count += 1.0;
            }
        }
    }
    (sum / count).sqrt()
}

const CENTRES_8: [f64; 8] = [400.0, 440.0, 480.0, 520.0, 560.0, 600.0, 640.0, 680.0];
const CENTRES_6: [f64; 6] = [420.0, 472.0, 524.0, 576.0, 628.0, 680.0];

// ---------------------------------------------------------------- tier 1 and the grid

#[test]
fn grid_wavelengths_span_380_to_780() {
    assert_eq!(grid_wavelength_nm(0), 380.0);
    assert_eq!(grid_wavelength_nm(GRID_LEN - 1), 780.0);
}

#[test]
fn csv_is_resampled_linearly_onto_the_grid() {
    let csv = "wavelength,r,g,b\n400,0,1,2\n500,1,2,3\n600,2,3,4\n700,3,4,5\n";
    let response = CameraResponse::from_csv(csv).unwrap();
    assert_eq!(response.tier(), ResponseTier::Measured);
    let at = |nm: f64| ((nm - 380.0) / 5.0) as usize;
    // r(450) is halfway between r(400)=0 and r(500)=1.
    assert!((response.sensitivity()[0][at(450.0)] - 0.5).abs() < 1e-12);
    assert!((response.sensitivity()[1][at(650.0)] - 3.5).abs() < 1e-12);
    assert!((response.sensitivity()[2][at(700.0)] - 5.0).abs() < 1e-12);
    // Outside the source range: zero.
    assert_eq!(response.sensitivity()[0][at(385.0)], 0.0);
    assert_eq!(response.sensitivity()[0][at(750.0)], 0.0);
}

#[test]
fn csv_accepts_comments_blank_lines_and_other_delimiters() {
    let csv = "# camera\n\n380;0.1;0.2;0.3\n780\t0.5\t0.6\t0.7 extra\n";
    let response = CameraResponse::from_csv(csv).unwrap();
    assert!((response.sensitivity()[0][0] - 0.1).abs() < 1e-12);
    assert!((response.sensitivity()[2][GRID_LEN - 1] - 0.7).abs() < 1e-12);
    // Linear in between: r(580) is halfway.
    assert!((response.sensitivity()[0][40] - 0.3).abs() < 1e-12);
}

#[test]
fn csv_rejects_non_monotone_and_duplicate_wavelengths() {
    let backwards = "400,1,1,1\n500,1,1,1\n450,1,1,1\n";
    assert_eq!(
        CameraResponse::from_csv(backwards),
        Err(SpectralError::NotMonotone { index: 2 })
    );
    let duplicate = "400,1,1,1\n400,1,1,1\n500,1,1,1\n";
    assert_eq!(
        CameraResponse::from_csv(duplicate),
        Err(SpectralError::NotMonotone { index: 1 })
    );
}

#[test]
fn csv_rejects_large_negatives_and_clamps_tiny_ones() {
    let bad = "400,1,1,1\n500,1,-0.01,1\n";
    assert!(matches!(
        CameraResponse::from_csv(bad),
        Err(SpectralError::NegativeValue { index: 1, .. })
    ));
    let tiny = "400,0,0,0\n500,-0.0005,1,1\n";
    let response = CameraResponse::from_csv(tiny).unwrap();
    assert!(response.sensitivity()[0].iter().all(|v| *v >= 0.0));
    assert_eq!(response.sensitivity()[0][(500 - 380) / 5], 0.0);
}

#[test]
fn csv_rejects_bad_lines_and_short_input() {
    assert!(matches!(
        CameraResponse::from_csv("400,1,1,1\n500,abc,1,1\n"),
        Err(SpectralError::Parse { line: 2, .. })
    ));
    assert!(matches!(
        CameraResponse::from_csv("400,1,1,1\n"),
        Err(SpectralError::TooFewRows { found: 1 })
    ));
    assert_eq!(
        CameraResponse::from_csv("100,1,1,1\n200,1,1,1\n"),
        Err(SpectralError::NoOverlap)
    );
}

#[test]
fn camera_rgb_is_a_trapezoid_integral() {
    let mut s = [[0.0; GRID_LEN]; 3];
    s[0] = [1.0; GRID_LEN];
    s[1] = [2.0; GRID_LEN];
    let response = CameraResponse::from_grid(s, ResponseTier::Measured).unwrap();
    let rgb = response.camera_rgb(&|_| 1.0);
    assert!((rgb[0] - 400.0).abs() < 1e-9);
    assert!((rgb[1] - 800.0).abs() < 1e-9);
    assert_eq!(rgb[2], 0.0);
    // Linear spectrum: the trapezoid rule is exact. Integral of (l - 380) over 380..780 = 80000.
    let ramp = response.camera_rgb(&|l| l - 380.0);
    assert!((ramp[0] - 80_000.0).abs() < 1e-6);
    let sampled: [f64; GRID_LEN] = core::array::from_fn(|i| grid_wavelength_nm(i) - 380.0);
    assert_eq!(response.camera_rgb_grid(&sampled), ramp);
}

#[test]
fn from_grid_rejects_non_finite() {
    let mut s = [[0.0; GRID_LEN]; 3];
    s[1][3] = f64::NAN;
    assert!(CameraResponse::from_grid(s, ResponseTier::Measured).is_err());
}

// ---------------------------------------------------------------- linear algebra

#[test]
fn cholesky_solves_a_small_system() {
    // [[4, 2], [2, 3]] x = [6, 5] -> x = [1, 1].
    let a = [4.0, 2.0, 2.0, 3.0];
    let l = cholesky(&a, 2).unwrap();
    let x = cholesky_solve(&l, 2, &[6.0, 5.0]);
    assert!((x[0] - 1.0).abs() < 1e-12 && (x[1] - 1.0).abs() < 1e-12);
    assert!(cholesky(&[1.0, 2.0, 2.0, 1.0], 2).is_none());
}

#[test]
fn nnls_clamps_negative_components_and_matches_the_unconstrained_optimum_otherwise() {
    // H = I, g = (1, -1): unconstrained optimum (1, -1), constrained (1, 0).
    let s = nnls_normal(&[1.0, 0.0, 0.0, 1.0], &[1.0, -1.0], 2);
    assert!((s[0] - 1.0).abs() < 1e-12);
    assert_eq!(s[1], 0.0);
    // Coupled: H = [[2, 1], [1, 2]], g = (3, 3): optimum (1, 1), both positive.
    let s = nnls_normal(&[2.0, 1.0, 1.0, 2.0], &[3.0, 3.0], 2);
    assert!((s[0] - 1.0).abs() < 1e-10 && (s[1] - 1.0).abs() < 1e-10);
    // Coupled with a binding bound: H = [[2, 1], [1, 2]], g = (3, -1). Unconstrained
    // (7/3, -5/3) is infeasible; with s1 = 0 the optimum is s0 = 3/2.
    let s = nnls_normal(&[2.0, 1.0, 1.0, 2.0], &[3.0, -1.0], 2);
    assert!((s[0] - 1.5).abs() < 1e-10);
    assert_eq!(s[1], 0.0);
}

// ---------------------------------------------------------------- tier 2

#[test]
fn eight_filters_recover_a_gaussian_sum_response_within_3_percent() {
    let truth = true_response();
    let backlight = led_spd(LedKind::B3);
    let filters = make_filters(&truth, &backlight, &CENTRES_8, None);
    let (response, report) = calibrate_from_filters(&filters, &backlight).unwrap();
    assert_eq!(response.tier(), ResponseTier::FilterCalibrated);
    assert!(response.sensitivity().iter().flatten().all(|v| *v >= 0.0));
    let rms = recovery_rms(&response, &truth, &backlight);
    assert!(rms < 0.03, "recovery RMS {rms}, report {report:?}");
    // The fit reproduces the measured transmittances.
    let white = response.camera_rgb_grid(&backlight);
    for filter in &filters {
        let lit: [f64; GRID_LEN] = core::array::from_fn(|i| filter.transmission[i] * backlight[i]);
        let through = response.camera_rgb_grid(&lit);
        for c in 0..3 {
            assert!((through[c] / white[c] - filter.measured_rgb[c]).abs() < 5e-3);
        }
    }
}

#[test]
fn six_filters_give_a_degraded_but_bounded_recovery() {
    let truth = true_response();
    let backlight = led_spd(LedKind::B3);
    let filters = make_filters(&truth, &backlight, &CENTRES_6, None);
    let (response, _) = calibrate_from_filters(&filters, &backlight).unwrap();
    let rms = recovery_rms(&response, &truth, &backlight);
    assert!(rms.is_finite() && rms < 0.20, "recovery RMS {rms}");
}

#[test]
fn noisy_measurements_still_give_a_bounded_recovery_with_a_fixed_seed() {
    let truth = true_response();
    let backlight = led_spd(LedKind::B3);
    let filters = make_filters(
        &truth,
        &backlight,
        &CENTRES_8,
        Some((Noise(0x5EED_CAFE), 0.002)),
    );
    let (response, report) = calibrate_from_filters(&filters, &backlight).unwrap();
    let rms = recovery_rms(&response, &truth, &backlight);
    assert!(rms < 0.12, "recovery RMS {rms}, report {report:?}");
    // Deterministic: a second run is bit-identical.
    let (again, report_again) = calibrate_from_filters(&filters, &backlight).unwrap();
    assert_eq!(again, response);
    assert_eq!(report_again, report);
}

#[test]
fn calibration_report_is_sane() {
    let truth = true_response();
    let backlight = led_spd(LedKind::B3);
    let filters = make_filters(&truth, &backlight, &CENTRES_8, None);
    let (_, report) = calibrate_from_filters(&filters, &backlight).unwrap();
    assert_eq!(report.filter_count, 8);
    assert!(report.lambda_index < LAMBDA_GRID_LEN);
    assert_eq!(report.lambda, report.lambda_grid[report.lambda_index]);
    assert!(report.lambda_grid.windows(2).all(|w| w[1] > w[0]));
    assert!(report.gcv[report.lambda_index].is_finite());
    assert!(report.residual_rms < 5e-3, "{report:?}");
    assert!(report.condition_estimate.is_finite() && report.condition_estimate > 1.0);
    assert!(report.effective_dof > 0.0 && report.effective_dof <= 9.0 + 1e-6);
}

#[test]
fn calibration_rejects_too_few_filters_and_bad_inputs() {
    let truth = true_response();
    let backlight = led_spd(LedKind::B3);
    let filters = make_filters(&truth, &backlight, &CENTRES_6[..5], None);
    assert_eq!(
        calibrate_from_filters(&filters, &backlight).map(|_| ()),
        Err(SpectralError::TooFewFilters {
            found: 5,
            required: MIN_REFERENCE_FILTERS
        })
    );
    let filters = make_filters(&truth, &backlight, &CENTRES_8, None);
    assert!(calibrate_from_filters(&filters, &[0.0; GRID_LEN]).is_err());
    let mut negative = backlight;
    negative[10] = -0.1;
    assert!(calibrate_from_filters(&filters, &negative).is_err());
    assert!(ReferenceFilter::new("percent", [50.0; GRID_LEN], [0.5; 3]).is_err());
}

#[test]
fn reference_filter_csv_is_resampled() {
    let csv = "wavelength_nm,T\n380,0.0\n780,1.0\n";
    let filter = ReferenceFilter::from_csv("ramp", csv, [0.5, 0.5, 0.5]).unwrap();
    assert!((filter.transmission[40] - 0.5).abs() < 1e-12);
    assert_eq!(filter.name, "ramp");
}

#[test]
fn backlight_weights_sum_to_one() {
    let weights = backlight_weights(&led_spd(LedKind::B2)).unwrap();
    assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
}

// ---------------------------------------------------------------- tier 3

#[test]
fn srgb_fallback_reproduces_xyz_of_any_spectrum() {
    let response = CameraResponse::from_srgb().unwrap();
    assert_eq!(response.tier(), ResponseTier::MatrixFallback);
    let spectrum = |l: f64| 0.5f64.mul_add((l / 60.0).sin(), 1.0);
    let rgb = response.camera_rgb(&spectrum);
    let xyz_from_matrix = mul_vec3(&SRGB_TO_XYZ, rgb);
    let mut xyz = [0.0; 3];
    for i in 0..GRID_LEN {
        let l = grid_wavelength_nm(i);
        let cmf = cie_1931_cmf(l as f32);
        for c in 0..3 {
            xyz[c] = (trapezoid_weight(i) * f64::from(cmf[c])).mul_add(spectrum(l), xyz[c]);
        }
    }
    for c in 0..3 {
        assert!(
            (xyz_from_matrix[c] - xyz[c]).abs() < 1e-9 * xyz[c].abs().max(1.0),
            "channel {c}: {} vs {}",
            xyz_from_matrix[c],
            xyz[c]
        );
    }
}

#[test]
fn d50_forward_matrix_is_adapted_back_before_inversion() {
    let response = CameraResponse::from_camera_to_xyz(SRGB_TO_XYZ, MatrixXyzWhite::D50).unwrap();
    let rgb = response.camera_rgb(&|l| 0.3f64.mul_add((l / 50.0).cos(), 1.0));
    // camera -> M -> XYZ(D50) -> Bradford D50->D65 -> native XYZ.
    let adapted = mul_vec3(&bradford_d50_to_d65().unwrap(), mul_vec3(&SRGB_TO_XYZ, rgb));
    let mut xyz = [0.0; 3];
    for i in 0..GRID_LEN {
        let l = grid_wavelength_nm(i);
        let cmf = cie_1931_cmf(l as f32);
        for c in 0..3 {
            xyz[c] = (trapezoid_weight(i) * f64::from(cmf[c]))
                .mul_add(0.3f64.mul_add((l / 50.0).cos(), 1.0), xyz[c]);
        }
    }
    for c in 0..3 {
        assert!((adapted[c] - xyz[c]).abs() < 1e-9 * xyz[c].abs().max(1.0));
    }
}

#[test]
fn bradford_maps_the_d50_white_to_the_d65_white() {
    let adapted = mul_vec3(&bradford_d50_to_d65().unwrap(), [0.964_22, 1.0, 0.825_21]);
    for (got, want) in adapted.iter().zip([0.950_47, 1.0, 1.088_83]) {
        assert!((got - want).abs() < 1e-9, "{adapted:?}");
    }
}

#[test]
fn singular_camera_matrix_is_rejected() {
    let singular = [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 1.0]];
    assert_eq!(
        CameraResponse::from_camera_to_xyz(singular, MatrixXyzWhite::Native),
        Err(SpectralError::Singular)
    );
}

// ---------------------------------------------------------------- backlight

#[test]
fn led_backlight_is_peak_normalised_and_interpolates() {
    let backlight = BacklightSpectrum::from_led(LedKind::B3).unwrap();
    assert_eq!(backlight.source(), BacklightSource::Led(LedKind::B3));
    assert_eq!(backlight.tilt(), SpectralTilt::IDENTITY);
    let peak = backlight.spd().iter().fold(0.0_f64, |m, v| m.max(*v));
    assert!((peak - 1.0).abs() < 1e-12);
    let mid = backlight.spectral_power(502.5);
    let expected = f64::midpoint(backlight.spd()[24], backlight.spd()[25]);
    assert!((mid - expected).abs() < 1e-12);
    assert_eq!(backlight.spectral_power(379.0), 0.0);
    assert_eq!(backlight.spectral_power(781.0), 0.0);
    assert_eq!(
        BacklightSpectrum::from_cct_k(6500.0).unwrap().source(),
        BacklightSource::Led(LedKind::closest_to_cct(6500.0))
    );
}

#[test]
fn measured_backlight_csv_is_imported_and_validated() {
    let csv = "nm,power\n380,2\n580,4\n780,2\n";
    let backlight = BacklightSpectrum::from_csv(csv).unwrap();
    assert_eq!(backlight.source(), BacklightSource::Measured);
    assert!((backlight.spd()[40] - 1.0).abs() < 1e-12);
    assert!((backlight.spd()[0] - 0.5).abs() < 1e-12);
    assert!(BacklightSpectrum::from_csv("380,0\n780,0\n").is_err());
}

#[test]
fn the_correct_led_kind_is_picked_from_synthetic_data() {
    let response = true_response();
    for kind in LedKind::ALL {
        let white = response.camera_rgb_grid(&led_spd(kind));
        // Any overall exposure scale.
        let scaled = [white[0] * 0.37, white[1] * 0.37, white[2] * 0.37];
        let fit = BacklightSpectrum::closest_to_white_frame(scaled, &response).unwrap();
        assert_eq!(fit.kind, kind, "wrong kind for {}", kind.name());
        assert!(fit.chromaticity_distance < 1e-12);
        assert!(fit.tilt.slope.abs() < 1e-4 && fit.tilt.curvature.abs() < 1e-4);
    }
}

#[test]
fn a_mild_tilt_does_not_change_the_picked_kind() {
    let response = true_response();
    let tilt = SpectralTilt {
        slope: 0.03,
        curvature: -0.01,
    };
    let tilted = BacklightSpectrum::from_led(LedKind::B4)
        .unwrap()
        .with_tilt(tilt)
        .unwrap();
    let white = tilted.camera_rgb(&response);
    let fit = BacklightSpectrum::closest_to_white_frame(white, &response).unwrap();
    assert_eq!(fit.kind, LedKind::B4);
    assert!((fit.tilt.slope - 0.03).abs() < 1e-4);
    assert!((fit.tilt.curvature + 0.01).abs() < 1e-4);
}

#[test]
fn the_tilt_fit_recovers_a_known_tilt() {
    let response = true_response();
    let base = BacklightSpectrum::from_led(LedKind::B3).unwrap();
    let truth = SpectralTilt {
        slope: 0.4,
        curvature: -0.2,
    };
    let white = base.with_tilt(truth).unwrap().camera_rgb(&response);
    let (found, residual) = fit_white_point_tilt(&base, &response, white).unwrap();
    assert!(residual < 1e-9, "residual {residual}");
    assert!((found.slope - truth.slope).abs() < 1e-5, "{found:?}");
    assert!(
        (found.curvature - truth.curvature).abs() < 1e-5,
        "{found:?}"
    );
}

#[test]
fn the_white_frame_fit_matches_the_measured_chromaticity() {
    let response = true_response();
    let white = BacklightSpectrum::from_led(LedKind::B2)
        .unwrap()
        .with_tilt(SpectralTilt {
            slope: 0.25,
            curvature: -0.10,
        })
        .unwrap()
        .camera_rgb(&response);
    let fit = BacklightSpectrum::closest_to_white_frame(white, &response).unwrap();
    let predicted = fit.backlight.camera_rgb(&response);
    assert!((predicted[0] / predicted[1] - white[0] / white[1]).abs() < 1e-6);
    assert!((predicted[2] / predicted[1] - white[2] / white[1]).abs() < 1e-6);
    assert!(fit.residual < 1e-6);
}

#[test]
fn white_frame_rgb_must_be_positive() {
    let response = true_response();
    assert!(BacklightSpectrum::closest_to_white_frame([1.0, 0.0, 1.0], &response).is_err());
    assert!(BacklightSpectrum::closest_to_white_frame([1.0, f64::NAN, 1.0], &response).is_err());
}

#[test]
fn tilt_factor_has_the_documented_form() {
    assert_eq!(SpectralTilt::IDENTITY.factor(500.0), 1.0);
    let tilt = SpectralTilt {
        slope: 0.5,
        curvature: 0.0,
    };
    // u = (780 - 580) / 200 = 1.
    assert!((tilt.factor(780.0) - 0.5_f64.exp()).abs() < 1e-12);
    let combined = tilt.then(SpectralTilt {
        slope: 0.1,
        curvature: 0.2,
    });
    assert!((combined.slope - 0.6).abs() < 1e-15 && (combined.curvature - 0.2).abs() < 1e-15);
}
