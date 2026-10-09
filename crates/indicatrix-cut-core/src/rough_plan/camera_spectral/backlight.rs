//! The backlight spectrum (plan section 4.4): measured SPD, a CIE 15:2018 LED illuminant, or the
//! LED closest to the white frame plus a two-parameter smooth white point correction.

use indicatrix::color::led::LedKind;

use super::{
    GRID_FIRST_NM, GRID_LEN, GRID_STEP_NM, SpectralError, csv::read_curve, grid_wavelength_nm,
    response::CameraResponse,
};

/// Where a backlight spectrum came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacklightSource {
    /// Imported or supplied measurement.
    Measured,
    /// A bundled CIE LED illuminant (possibly tilted, see [`BacklightSpectrum::tilt`]).
    Led(LedKind),
}

/// A smooth multiplicative white point correction `exp(slope * u + curvature * (u^2 - 1/3))`
/// with `u = (lambda - 580 nm) / 200 nm`. Two parameters, matching the two degrees of freedom
/// of a white point.
///
/// `slope` tilts blue against red, `curvature` lifts the middle against the
/// ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectralTilt {
    /// Linear term in `u`.
    pub slope: f64,
    /// Quadratic term in `u` (centred so its mean over `[-1, 1]` is zero).
    pub curvature: f64,
}

impl SpectralTilt {
    /// No correction.
    pub const IDENTITY: Self = Self {
        slope: 0.0,
        curvature: 0.0,
    };

    /// The multiplicative factor at `lambda_nm`.
    #[must_use]
    pub fn factor(self, lambda_nm: f64) -> f64 {
        let u = (lambda_nm - 580.0) / 200.0;
        self.slope
            .mul_add(u, self.curvature * u.mul_add(u, -1.0 / 3.0))
            .exp()
    }

    /// The tilt equal to applying `self` and then `other` (parameters add).
    #[must_use]
    pub const fn then(self, other: Self) -> Self {
        Self {
            slope: self.slope + other.slope,
            curvature: self.curvature + other.curvature,
        }
    }
}

/// A backlight SPD on the 380 to 780 nm, 5 nm grid, normalised to peak 1.
#[derive(Debug, Clone, PartialEq)]
pub struct BacklightSpectrum {
    spd: [f64; GRID_LEN],
    source: BacklightSource,
    tilt: SpectralTilt,
}

fn normalise_peak(spd: &[f64; GRID_LEN]) -> Result<[f64; GRID_LEN], SpectralError> {
    if spd.iter().any(|e| !e.is_finite() || *e < 0.0) {
        return Err(SpectralError::InvalidInput(
            "backlight SPD must be finite and non-negative".to_owned(),
        ));
    }
    let peak = spd.iter().fold(0.0_f64, |m, v| m.max(*v));
    if peak <= 0.0 {
        return Err(SpectralError::InvalidInput(
            "backlight SPD is zero everywhere".to_owned(),
        ));
    }
    let mut out = *spd;
    for v in &mut out {
        *v /= peak;
    }
    Ok(out)
}

impl BacklightSpectrum {
    /// A measured SPD already on the grid.
    ///
    /// # Errors
    /// Negative, non-finite or all-zero data.
    pub fn from_grid(spd: &[f64; GRID_LEN]) -> Result<Self, SpectralError> {
        Ok(Self {
            spd: normalise_peak(spd)?,
            source: BacklightSource::Measured,
            tilt: SpectralTilt::IDENTITY,
        })
    }

    /// A measured SPD from a `wavelength_nm, power` CSV (any spacing; resampled linearly,
    /// 0 outside the source range).
    ///
    /// # Errors
    /// See [`SpectralError`].
    pub fn from_csv(text: &str) -> Result<Self, SpectralError> {
        Self::from_grid(&read_curve(text)?)
    }

    /// A bundled CIE LED illuminant.
    ///
    /// # Errors
    /// Never in practice (the table is non-empty); the `Result` keeps the signature uniform.
    pub fn from_led(kind: LedKind) -> Result<Self, SpectralError> {
        Ok(Self {
            spd: normalise_peak(kind.samples())?,
            source: BacklightSource::Led(kind),
            tilt: SpectralTilt::IDENTITY,
        })
    }

    /// The CIE LED illuminant whose published CCT is nearest to `cct_k`.
    ///
    /// # Errors
    /// As [`Self::from_led`].
    pub fn from_cct_k(cct_k: f64) -> Result<Self, SpectralError> {
        Self::from_led(LedKind::closest_to_cct(cct_k))
    }

    /// The SPD on the grid, peak 1.
    #[must_use]
    pub const fn spd(&self) -> &[f64; GRID_LEN] {
        &self.spd
    }

    /// Where the spectrum came from.
    #[must_use]
    pub const fn source(&self) -> BacklightSource {
        self.source
    }

    /// The white point correction applied so far (identity for an untouched spectrum).
    #[must_use]
    pub const fn tilt(&self) -> SpectralTilt {
        self.tilt
    }

    /// Linear interpolation of the SPD; 0 outside 380 to 780 nm.
    #[must_use]
    pub fn spectral_power(&self, lambda_nm: f64) -> f64 {
        let position = (lambda_nm - GRID_FIRST_NM) / GRID_STEP_NM;
        if !(0.0..=(GRID_LEN - 1) as f64).contains(&position) {
            return 0.0;
        }
        let lo = (position.floor() as usize).min(GRID_LEN - 2);
        let t = position - lo as f64;
        (self.spd[lo + 1] - self.spd[lo]).mul_add(t, self.spd[lo])
    }

    /// This spectrum multiplied by `tilt` (peak renormalised); tilts accumulate.
    ///
    /// # Errors
    /// Cannot fail for a valid spectrum and finite tilt; kept for uniformity.
    pub fn with_tilt(&self, tilt: SpectralTilt) -> Result<Self, SpectralError> {
        let mut spd = self.spd;
        for (i, v) in spd.iter_mut().enumerate() {
            *v *= tilt.factor(grid_wavelength_nm(i));
        }
        Ok(Self {
            spd: normalise_peak(&spd)?,
            source: self.source,
            tilt: self.tilt.then(tilt),
        })
    }

    /// Camera RGB of this backlight.
    #[must_use]
    pub fn camera_rgb(&self, response: &CameraResponse) -> [f64; 3] {
        response.camera_rgb_grid(&self.spd)
    }

    /// "Closest to the white frame": picks the nearest CIE LED kind and tilts it onto the white.
    ///
    /// Given the camera RGB of the empty backlit rig (any scale) and
    /// the camera response, pick the CIE LED kind whose predicted camera chromaticity
    /// `(r, g) / (r + g + b)` is nearest (ties go to the earlier entry of [`LedKind::ALL`]), then
    /// fit a [`SpectralTilt`] so that the predicted chromaticity matches the white frame.
    ///
    /// # Errors
    /// Non-positive or non-finite `white_rgb`, no LED with a positive predicted RGB under this
    /// response, or a tilt fit that cannot start.
    pub fn closest_to_white_frame(
        white_rgb: [f64; 3],
        response: &CameraResponse,
    ) -> Result<BacklightFit, SpectralError> {
        if white_rgb.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err(SpectralError::InvalidInput(
                "white frame RGB must be positive and finite".to_owned(),
            ));
        }
        let chromaticity = |rgb: [f64; 3]| {
            let sum = rgb[0] + rgb[1] + rgb[2];
            [rgb[0] / sum, rgb[1] / sum]
        };
        let target = chromaticity(white_rgb);
        let mut best: Option<(LedKind, f64)> = None;
        for kind in LedKind::ALL {
            let led = Self::from_led(kind)?;
            let rgb = led.camera_rgb(response);
            if rgb.iter().any(|v| !v.is_finite() || *v <= 0.0) {
                continue;
            }
            let c = chromaticity(rgb);
            let distance = (c[0] - target[0]).hypot(c[1] - target[1]);
            if best.is_none_or(|(_, d)| distance < d) {
                best = Some((kind, distance));
            }
        }
        let Some((kind, chromaticity_distance)) = best else {
            return Err(SpectralError::InvalidInput(
                "no LED illuminant gives a positive camera RGB under this response".to_owned(),
            ));
        };
        let base = Self::from_led(kind)?;
        let (tilt, residual) = fit_white_point_tilt(&base, response, white_rgb)?;
        Ok(BacklightFit {
            backlight: base.with_tilt(tilt)?,
            kind,
            tilt,
            chromaticity_distance,
            residual,
        })
    }
}

/// Result of [`BacklightSpectrum::closest_to_white_frame`].
#[derive(Debug, Clone, PartialEq)]
pub struct BacklightFit {
    /// The chosen LED with the fitted tilt applied.
    pub backlight: BacklightSpectrum,
    /// The chosen LED kind.
    pub kind: LedKind,
    /// The fitted correction.
    pub tilt: SpectralTilt,
    /// Camera chromaticity distance between the chosen untilted LED and the white frame.
    pub chromaticity_distance: f64,
    /// Remaining norm of the two log channel ratios (`ln r/g`, `ln b/g`) after the tilt.
    pub residual: f64,
}

/// The two log channel ratios of `camera_rgb(base * tilt(p))` minus the `target` ratios, or
/// `None` when the tilted backlight gives a non-positive camera RGB.
fn tilt_residual(
    base: &BacklightSpectrum,
    response: &CameraResponse,
    target: [f64; 2],
    p: [f64; 2],
) -> Option<[f64; 2]> {
    let tilt = SpectralTilt {
        slope: p[0],
        curvature: p[1],
    };
    let mut spd = base.spd;
    for (i, v) in spd.iter_mut().enumerate() {
        *v *= tilt.factor(grid_wavelength_nm(i));
    }
    let rgb = response.camera_rgb_grid(&spd);
    if rgb.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    Some([
        (rgb[0] / rgb[1]).ln() - target[0],
        (rgb[2] / rgb[1]).ln() - target[1],
    ])
}

/// Fit the two tilt parameters so that `camera_rgb(base * tilt)` has the channel ratios
/// `r/g` and `b/g` of `white_rgb`.
///
/// Levenberg-Marquardt on the two log ratios with central
/// differences; starts at the identity, parameters limited to +-4.
///
/// Returns the tilt (relative to `base`) and the remaining residual norm.
///
/// # Errors
/// `white_rgb` not positive, or `base` gives a non-positive camera RGB under `response`.
pub fn fit_white_point_tilt(
    base: &BacklightSpectrum,
    response: &CameraResponse,
    white_rgb: [f64; 3],
) -> Result<(SpectralTilt, f64), SpectralError> {
    if white_rgb.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(SpectralError::InvalidInput(
            "white frame RGB must be positive and finite".to_owned(),
        ));
    }
    let target = [
        (white_rgb[0] / white_rgb[1]).ln(),
        (white_rgb[2] / white_rgb[1]).ln(),
    ];
    let evaluate = |p: [f64; 2]| tilt_residual(base, response, target, p);
    let mut p = [0.0; 2];
    let Some(mut r) = evaluate(p) else {
        return Err(SpectralError::InvalidInput(
            "the base backlight gives a non-positive camera RGB".to_owned(),
        ));
    };
    let mut cost = f64::mul_add(r[1], r[1], r[0] * r[0]);
    let mut mu = 1e-6;
    let step = 1e-5;
    'outer: for _ in 0..200 {
        if cost < 1e-26 {
            break;
        }
        let mut jac = [[0.0; 2]; 2];
        for col in 0..2 {
            let mut plus = p;
            plus[col] += step;
            let mut minus = p;
            minus[col] -= step;
            let (Some(rp), Some(rm)) = (evaluate(plus), evaluate(minus)) else {
                break 'outer;
            };
            for row in 0..2 {
                jac[row][col] = (rp[row] - rm[row]) / (2.0 * step);
            }
        }
        let mut jtj = [[0.0; 2]; 2];
        let mut grad = [0.0; 2];
        for a in 0..2 {
            for b in 0..2 {
                jtj[a][b] = jac[0][a].mul_add(jac[0][b], jac[1][a] * jac[1][b]);
            }
            grad[a] = jac[0][a].mul_add(r[0], jac[1][a] * r[1]);
        }
        let mut accepted = false;
        for _ in 0..30 {
            let a00 = jtj[0][0].mul_add(mu, jtj[0][0]);
            let a11 = jtj[1][1].mul_add(mu, jtj[1][1]);
            let det = a00.mul_add(a11, -(jtj[0][1] * jtj[1][0]));
            if det.abs() < 1e-300 || !det.is_finite() {
                mu *= 10.0;
                continue;
            }
            let dp = [
                -(a11.mul_add(grad[0], -(jtj[0][1] * grad[1]))) / det,
                -(a00.mul_add(grad[1], -(jtj[1][0] * grad[0]))) / det,
            ];
            let trial = [
                (p[0] + dp[0]).clamp(-4.0, 4.0),
                (p[1] + dp[1]).clamp(-4.0, 4.0),
            ];
            if let Some(rt) = evaluate(trial) {
                let trial_cost = f64::mul_add(rt[1], rt[1], rt[0] * rt[0]);
                if trial_cost < cost {
                    let moved = (trial[0] - p[0]).abs().max((trial[1] - p[1]).abs());
                    p = trial;
                    r = rt;
                    cost = trial_cost;
                    mu = (mu * 0.3).max(1e-12);
                    accepted = true;
                    if moved < 1e-14 {
                        break 'outer;
                    }
                    break;
                }
            }
            mu *= 10.0;
        }
        if !accepted {
            break;
        }
    }
    Ok((
        SpectralTilt {
            slope: p[0],
            curvature: p[1],
        },
        cost.sqrt(),
    ))
}
