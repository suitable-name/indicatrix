//! Tier 2: camera sensitivity from reference filters photographed in the rig.
//!
//! # Model
//!
//! The corrected rig image of filter `k` gives a mean transmittance `t_kc` per channel `c`,
//! relative to the empty backlit rig: `t_kc = sum_i w_i T_k(i) S_c(i) / sum_i w_i S_c(i)` with
//! `w_i` the trapezoid weight times the backlight SPD `E(i)` (see [`backlight_weights`]). The
//! scale of `S_c` is not identifiable from ratios, so it is fixed by `sum_i omega_i S_c(i) = 1`
//! with `omega = w / sum(w)`, which makes every equation linear:
//!
//! * filter row `k`: `sum_i omega_i T_k(i) S_c(i) = t_kc`
//! * white row:      `sum_i omega_i S_c(i) = 1`
//!
//! Per channel the fit is `min ||A s - b||^2 + lambda ||D s||^2 + eps ||s||^2` with `s >= 0`,
//! `D` the second difference, solved by an active-set NNLS on the normal equations. `lambda` is
//! shared by the three channels (they have the same `A`) and chosen by generalised
//! cross-validation of the unconstrained fit over [`LAMBDA_GRID_LEN`] fixed values.

use super::{
    GRID_LEN, SpectralError,
    csv::read_curve,
    linalg::{cholesky, cholesky_solve, condition_estimate, nnls_normal},
    response::{CameraResponse, ResponseTier, trapezoid_weight},
};

/// Smallest number of reference filters the calibration accepts.
pub const MIN_REFERENCE_FILTERS: usize = 6;
/// Number of candidate smoothness weights tried by generalised cross-validation.
pub const LAMBDA_GRID_LEN: usize = 12;
/// Exponent (base 10, relative to the mean diagonal of `A^T A`) of the first candidate weight;
/// the candidates are `10^(-8 + j)` for `j = 0..12`.
const LAMBDA_FIRST_EXPONENT: i32 = -8;
/// Tiny ridge (relative to the mean diagonal of `A^T A`) that keeps the system definite.
const RIDGE: f64 = 1e-10;
/// A candidate is skipped when `rows - trace(hat)` is below this fraction of `rows`
/// (the fit interpolates; the GCV ratio is numerically meaningless there).
const GCV_MIN_DOF_FRACTION: f64 = 1e-6;

/// A reference filter: transmission on the grid plus the measured mean transmittance RGB.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceFilter {
    /// Free-text name (for reports).
    pub name: String,
    /// Spectral transmission on the 380 to 780 nm grid, in `[0, 1]`.
    pub transmission: [f64; GRID_LEN],
    /// Mean transmittance in the rig per camera channel (relative to the empty backlit rig).
    pub measured_rgb: [f64; 3],
}

impl ReferenceFilter {
    /// Build from gridded data.
    ///
    /// # Errors
    /// Transmission outside `[0, 1.05]` or any non-finite number.
    pub fn new(
        name: &str,
        transmission: [f64; GRID_LEN],
        measured_rgb: [f64; 3],
    ) -> Result<Self, SpectralError> {
        if transmission
            .iter()
            .any(|t| !t.is_finite() || *t < 0.0 || *t > 1.05)
        {
            return Err(SpectralError::InvalidInput(format!(
                "filter {name}: transmission must lie in [0, 1] (fractions, not percent)"
            )));
        }
        if measured_rgb.iter().any(|v| !v.is_finite()) {
            return Err(SpectralError::InvalidInput(format!(
                "filter {name}: measured RGB is not finite"
            )));
        }
        Ok(Self {
            name: name.to_owned(),
            transmission,
            measured_rgb,
        })
    }

    /// Build from a `wavelength_nm, transmission` CSV (any spacing, fractions in `[0, 1]`).
    ///
    /// # Errors
    /// See [`SpectralError`].
    pub fn from_csv(
        name: &str,
        transmission_csv: &str,
        measured_rgb: [f64; 3],
    ) -> Result<Self, SpectralError> {
        Self::new(name, read_curve(transmission_csv)?, measured_rgb)
    }
}

/// Quality numbers of a tier 2 calibration.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationReport {
    /// Reference filters used.
    pub filter_count: usize,
    /// The candidate smoothness weights (absolute values).
    pub lambda_grid: [f64; LAMBDA_GRID_LEN],
    /// GCV score per candidate; `INFINITY` where the candidate was skipped.
    pub gcv: [f64; LAMBDA_GRID_LEN],
    /// Index of the chosen candidate.
    pub lambda_index: usize,
    /// The chosen smoothness weight.
    pub lambda: f64,
    /// Effective degrees of freedom (trace of the hat matrix) at the chosen weight, out of
    /// `filter_count + 1` data rows per channel.
    pub effective_dof: f64,
    /// RMS of the transmittance residual over all filters and channels (final, non-negative
    /// fit).
    pub residual_rms: f64,
    /// The same RMS per channel.
    pub channel_residual_rms: [f64; 3],
    /// Estimated condition number of the regularised normal matrix at the chosen weight
    /// (`lambda_max / lambda_min` by power and inverse iteration). Large means the filters
    /// constrain the response poorly; it includes the tiny ridge, so it is finite.
    pub condition_estimate: f64,
}

/// Quadrature weights `omega_i = q_i E_i / sum(q E)` of the backlight (`q` the trapezoid
/// weights), summing to 1.
///
/// Camera RGB of a transmitted spectrum relative to the backlight is
/// `sum omega T S / sum omega S`.
///
/// # Errors
/// Negative or non-finite SPD, or an all-zero SPD.
pub fn backlight_weights(backlight: &[f64; GRID_LEN]) -> Result<[f64; GRID_LEN], SpectralError> {
    if backlight.iter().any(|e| !e.is_finite() || *e < 0.0) {
        return Err(SpectralError::InvalidInput(
            "backlight SPD must be finite and non-negative".to_owned(),
        ));
    }
    let mut weights = [0.0; GRID_LEN];
    let mut total = 0.0;
    for i in 0..GRID_LEN {
        weights[i] = trapezoid_weight(i) * backlight[i];
        total += weights[i];
    }
    if total <= 0.0 {
        return Err(SpectralError::InvalidInput(
            "backlight SPD is zero everywhere".to_owned(),
        ));
    }
    for w in &mut weights {
        *w /= total;
    }
    Ok(weights)
}

/// Solve for the camera response from reference filters (tier 2).
///
/// The returned response is normalised so that `sum omega_i S_c(i) = 1` for each channel
/// (module docs). The result is a pure function of the inputs.
///
/// # Errors
/// Fewer than [`MIN_REFERENCE_FILTERS`] filters, an invalid backlight, or a numerically
/// singular system.
#[allow(
    clippy::many_single_char_names,
    clippy::too_many_lines,
    reason = "one linear-algebra derivation (A, b, H, s, n as in the module docs) read top to bottom"
)]
pub fn calibrate_from_filters(
    filters: &[ReferenceFilter],
    backlight: &[f64; GRID_LEN],
) -> Result<(CameraResponse, CalibrationReport), SpectralError> {
    if filters.len() < MIN_REFERENCE_FILTERS {
        return Err(SpectralError::TooFewFilters {
            found: filters.len(),
            required: MIN_REFERENCE_FILTERS,
        });
    }
    let n = GRID_LEN;
    let omega = backlight_weights(backlight)?;
    let rows = filters.len() + 1;

    // Design matrix (row-major rows x n) and right-hand sides per channel.
    let mut a = vec![0.0; rows * n];
    for (k, filter) in filters.iter().enumerate() {
        for i in 0..n {
            a[k * n + i] = omega[i] * filter.transmission[i];
        }
    }
    for i in 0..n {
        a[filters.len() * n + i] = omega[i];
    }
    let mut b = [vec![0.0; rows], vec![0.0; rows], vec![0.0; rows]];
    for (c, column) in b.iter_mut().enumerate() {
        for (k, filter) in filters.iter().enumerate() {
            column[k] = filter.measured_rgb[c];
        }
        column[filters.len()] = 1.0;
    }

    // Normal matrix A^T A, roughness matrix D^T D.
    let mut ata = vec![0.0; n * n];
    for k in 0..rows {
        for i in 0..n {
            let aki = a[k * n + i];
            for j in 0..n {
                ata[i * n + j] = aki.mul_add(a[k * n + j], ata[i * n + j]);
            }
        }
    }
    let scale = (0..n).map(|i| ata[i * n + i]).sum::<f64>() / n as f64;
    if scale <= 0.0 || !scale.is_finite() {
        return Err(SpectralError::Singular);
    }
    let mut dtd = vec![0.0; n * n];
    for k in 1..(n - 1) {
        let idx = [k - 1, k, k + 1];
        let coef = [1.0, -2.0, 1.0];
        for (p, &ip) in idx.iter().enumerate() {
            for (q, &iq) in idx.iter().enumerate() {
                dtd[ip * n + iq] = f64::mul_add(coef[p], coef[q], dtd[ip * n + iq]);
            }
        }
    }

    let normal_matrix = |lambda: f64| -> Vec<f64> {
        let mut h = vec![0.0; n * n];
        for i in 0..n * n {
            h[i] = lambda.mul_add(dtd[i], ata[i]);
        }
        for i in 0..n {
            h[i * n + i] = RIDGE.mul_add(scale, h[i * n + i]);
        }
        h
    };
    let at_times = |rhs: &[f64]| -> Vec<f64> {
        let mut out = vec![0.0; n];
        for k in 0..rows {
            for i in 0..n {
                out[i] = a[k * n + i].mul_add(rhs[k], out[i]);
            }
        }
        out
    };
    let a_times = |s: &[f64]| -> Vec<f64> {
        (0..rows)
            .map(|k| {
                let mut sum = 0.0;
                for i in 0..n {
                    sum = a[k * n + i].mul_add(s[i], sum);
                }
                sum
            })
            .collect()
    };

    // Generalised cross-validation of the unconstrained fit over the fixed lambda grid.
    let mut lambda_grid = [0.0; LAMBDA_GRID_LEN];
    let mut gcv = [f64::INFINITY; LAMBDA_GRID_LEN];
    let mut traces = [0.0; LAMBDA_GRID_LEN];
    let rows_f = rows as f64;
    for j in 0..LAMBDA_GRID_LEN {
        lambda_grid[j] = scale * 10f64.powi(LAMBDA_FIRST_EXPONENT + j as i32);
        let Some(factor) = cholesky(&normal_matrix(lambda_grid[j]), n) else {
            continue;
        };
        let mut rss = 0.0;
        for column in &b {
            let s = cholesky_solve(&factor, n, &at_times(column));
            let fitted = a_times(&s);
            for k in 0..rows {
                let r = column[k] - fitted[k];
                rss = f64::mul_add(r, r, rss);
            }
        }
        let mut trace = 0.0;
        for k in 0..rows {
            let row = &a[k * n..(k + 1) * n];
            let x = cholesky_solve(&factor, n, row);
            trace += row.iter().zip(&x).map(|(p, q)| p * q).sum::<f64>();
        }
        traces[j] = trace;
        let free = rows_f - trace;
        if free > GCV_MIN_DOF_FRACTION * rows_f {
            let denominator = free / rows_f;
            gcv[j] = (rss / (3.0 * rows_f)) / (denominator * denominator);
        }
    }
    let mut lambda_index = LAMBDA_GRID_LEN - 1;
    let mut best = f64::INFINITY;
    for (j, &score) in gcv.iter().enumerate() {
        if score < best {
            best = score;
            lambda_index = j;
        }
    }
    let lambda = lambda_grid[lambda_index];
    let h = normal_matrix(lambda);
    if cholesky(&h, n).is_none() {
        return Err(SpectralError::Singular);
    }

    // Final non-negative fit per channel.
    let mut sensitivity = [[0.0; GRID_LEN]; 3];
    let mut channel_rms = [0.0; 3];
    let mut square_sum = 0.0;
    for (c, column) in b.iter().enumerate() {
        let s = nnls_normal(&h, &at_times(column), n);
        let fitted = a_times(&s);
        let mut channel_sum = 0.0;
        for k in 0..filters.len() {
            let r = column[k] - fitted[k];
            channel_sum = f64::mul_add(r, r, channel_sum);
        }
        square_sum += channel_sum;
        channel_rms[c] = (channel_sum / filters.len() as f64).sqrt();
        sensitivity[c].copy_from_slice(&s);
    }
    let report = CalibrationReport {
        filter_count: filters.len(),
        lambda_grid,
        gcv,
        lambda_index,
        lambda,
        effective_dof: traces[lambda_index],
        residual_rms: (square_sum / (3.0 * filters.len() as f64)).sqrt(),
        channel_residual_rms: channel_rms,
        condition_estimate: condition_estimate(&h, n),
    };
    let response = CameraResponse::from_grid(sensitivity, ResponseTier::FilterCalibrated)?;
    Ok((response, report))
}
