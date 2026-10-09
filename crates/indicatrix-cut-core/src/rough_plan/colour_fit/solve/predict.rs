//! Predicted face-up colours and their uncertainty (plan section 6.5).
//!
//! The colour of a stone of width `W` mm made of one zone is the body colour
//! (`indicatrix::color::body_color::body_color`) of that zone's absorption over the face-up
//! path `MODEL_UNIT_FACE_UP_PATH * W / model_width_units` mm, the representative light path of a
//! face-up round brilliant (`render_setup::MODEL_UNIT_FACE_UP_PATH`, 2.52 model units, measured
//! by the tone metric). `model_width_units` is the girdle width of the design in model units
//! (1 for the standard designs, which are normalised to a unit girdle width).
//!
//! The uncertainty is the posterior covariance of the model parameters propagated through the
//! colour with a central-difference Jacobian (`Lab` changes with the parameters of its own zone
//! only), `C_lab = J C J^T`. The CIEDE2000 radius is the root mean square CIEDE2000 distance of
//! the posterior from the prediction: `sqrt(trace(R C_lab R^T))`, where `R` is the local linear
//! map from a Lab offset to the CIEDE2000 residual vector
//! (`delta_e_2000_residual`, whose squared norm is the squared CIEDE2000).
//!
//! The radius is the posterior WITH the smoothness and magnitude priors, which fill the null
//! space of the data and so make it overconfident. The **metamer spread**
//! ([`ZonePrediction::metamer_spread`]) is the honest counterpart: the largest face-up CIEDE2000
//! from the fitted colour among parameter vectors within `Delta chi2 <= 1` of the optimum under
//! the data alone (see `metamer_steps`, verified with the nonlinear chi-square in the fitter).

use indicatrix::{
    color::body_color::{Illuminant, body_color, delta_e_2000, delta_e_2000_residual},
    render_setup::MODEL_UNIT_FACE_UP_PATH,
};

use super::{
    FitConfig,
    linalg::{sandwich3, symmetric_eigen},
    models::FitModel,
    output::ZonePrediction,
};
use crate::rough_plan::colour_fit::forward::SpectralModel;

/// The step of the local linear map of the CIEDE2000 residual, in Lab units.
const DELTA_E_PROBE: f64 = 0.01;

/// The mean face-up light path of a stone `size_mm` wide, mm.
pub(super) fn face_up_path_mm(size_mm: f64, model_width_units: f64) -> f64 {
    f64::from(MODEL_UNIT_FACE_UP_PATH) * size_mm / model_width_units
}

/// The Lab colour of `zone` over `path_mm` under `illuminant`.
pub(super) fn zone_lab(
    model: &dyn SpectralModel,
    zone: usize,
    params: &[f64],
    path_mm: f64,
    illuminant: Illuminant,
) -> [f64; 3] {
    body_color(
        |lambda| model.alpha(zone, lambda, params).max(0.0),
        path_mm,
        illuminant,
    )
    .lab
}

/// The map from a Lab offset at `lab` to the CIEDE2000 residual vector.
fn delta_e_map(lab: [f64; 3]) -> [[f64; 3]; 3] {
    let mut map = [[0.0; 3]; 3];
    for b in 0..3 {
        let mut other = lab;
        other[b] += DELTA_E_PROBE;
        let residual = delta_e_2000_residual(lab, other);
        for a in 0..3 {
            map[a][b] = residual[a] / DELTA_E_PROBE;
        }
    }
    map
}

/// How many of the weakest eigen directions of the metamer search are combined (round 2, C3).
pub const METAMER_DIRECTIONS: usize = 6;
/// The largest change of one model parameter (a log-amount or log-amplitude, so a factor of
/// `e^2`) a metamer candidate may make.
///
/// Directions the data does not constrain at all would
/// otherwise run away; the cap makes the spread a statement about a plausible range of spectra.
pub const METAMER_STEP_CAP: f64 = 2.0;

/// The deterministic candidates of the metamer search: parameter vectors within `Delta chi2 <= 1`
/// of `params` under the quadratic form `metric` (the Gauss-Newton matrix of the DATA term plus
/// the gain prior; the model's smoothness, magnitude, L1 and zone-pull priors are left out on
/// purpose, they are what hides the null space).
///
/// The eigenvectors of `metric` with the [`METAMER_DIRECTIONS`] smallest eigenvalues `l_i` give
/// the semi-axes `v_i / sqrt(l_i)` of the `Delta chi2 = 1` ellipsoid. The candidates are `+-` each
/// semi-axis and, for each pair of the directions, the four points `(+-a_i +- a_j) / sqrt 2` of
/// the ellipse in their plane (two-parameter combinations). A candidate whose largest parameter
/// change exceeds [`METAMER_STEP_CAP`] is scaled down to it (inside the ellipsoid), and the
/// result is clamped to the model bounds. The quadratic form is exact only for small steps, so
/// the figure is an estimate, not a bound; it is meant to show whether the data can tell
/// colours apart at all.
///
/// Round 3, D3: this returns the full-length STEPS (parameters then gains) at the full ellipsoid,
/// already capped; [`candidate_point`] applies a fraction of a step, and the fitter verifies
/// every candidate with the nonlinear chi-square and shrinks it by bisection until it passes.
/// `threshold` is the `Delta chi2` of the ellipsoid (1, or the Birge ratio above 1.2).
pub(super) fn metamer_steps(metric: &[f64], n: usize, np: usize, threshold: f64) -> Vec<Vec<f64>> {
    if metric.len() != n * n || n == 0 || np == 0 {
        return Vec::new();
    }
    let (values, vectors) = symmetric_eigen(metric, n);
    let top = values.last().copied().unwrap_or(0.0).max(0.0);
    let floor = (1e-10 * top).max(1e-300);
    let k = METAMER_DIRECTIONS.min(n);
    // The semi-axes, full length (parameters and gains).
    let axes: Vec<Vec<f64>> = (0..k)
        .map(|i| {
            let step = (threshold.max(1.0) / values[i].max(floor)).sqrt();
            vectors[i * n..(i + 1) * n]
                .iter()
                .map(|v| v * step)
                .collect()
        })
        .collect();
    let mut raw: Vec<Vec<f64>> = Vec::new();
    for axis in &axes {
        raw.push(axis.clone());
        raw.push(axis.iter().map(|v| -v).collect());
    }
    let half_root = std::f64::consts::FRAC_1_SQRT_2;
    for i in 0..k {
        for j in (i + 1)..k {
            for (si, sj) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                raw.push(
                    axes[i]
                        .iter()
                        .zip(&axes[j])
                        .map(|(a, b)| half_root * (si * a + sj * b))
                        .collect(),
                );
            }
        }
    }
    raw.into_iter()
        .filter_map(|delta| {
            let largest = delta[..np].iter().fold(0.0_f64, |m, v| m.max(v.abs()));
            if largest <= 0.0 || !largest.is_finite() {
                return None;
            }
            let scale = (METAMER_STEP_CAP / largest).min(1.0);
            Some(delta.iter().map(|v| scale * v).collect())
        })
        .collect()
}

/// The point `x + fraction * step`, clamped to the bounds `lo`/`hi` of `[parameters, gains]`.
pub(super) fn candidate_point(
    x: &[f64],
    step: &[f64],
    fraction: f64,
    lo: &[f64],
    hi: &[f64],
) -> Vec<f64> {
    (0..x.len())
        .map(|i| (x[i] + fraction * step[i]).clamp(lo[i], hi[i]))
        .collect()
}

/// The predictions of every zone, size and illuminant of `config.prediction` at `params`, with the
/// uncertainty from `cov` (row stride `stride`; its leading `np x np` block is the covariance of
/// the model parameters) and the metamer spread from the candidate points `verified` (each at
/// least `np` long; the model parameters lead), see [`metamer_steps`] and the verification in the
/// fitter. `unverified` are the same candidates before the verification (the candidates at the
/// full step), for the comparison.
pub(super) fn predictions(
    model: &dyn FitModel,
    params: &[f64],
    cov: &[f64],
    stride: usize,
    config: &FitConfig,
    verified: &[Vec<f64>],
    unverified: &[Vec<f64>],
) -> Vec<ZonePrediction> {
    let np = model.n_params();
    let per = model.per_zone();
    let step = model.fd_step();
    let (lo, hi) = model.bounds();
    let sizes = config.prediction.sizes();
    let mut out = Vec::new();
    for zone in 0..model.zone_count() {
        for &size_mm in &sizes {
            let path_mm = face_up_path_mm(size_mm, config.prediction.model_width_units);
            for &illuminant in &config.prediction.illuminants {
                let lab = zone_lab(model.as_spectral(), zone, params, path_mm, illuminant);
                let mut jac = vec![[0.0_f64; 3]; np];
                let mut moved = params.to_vec();
                for p in zone * per..(zone + 1) * per {
                    let up = (params[p] + step).min(hi[p]);
                    let down = (params[p] - step).max(lo[p]);
                    if up - down < 1e-12 {
                        continue;
                    }
                    moved[p] = up;
                    let lab_up = zone_lab(model.as_spectral(), zone, &moved, path_mm, illuminant);
                    moved[p] = down;
                    let lab_down = zone_lab(model.as_spectral(), zone, &moved, path_mm, illuminant);
                    moved[p] = params[p];
                    for c in 0..3 {
                        jac[p][c] = (lab_up[c] - lab_down[c]) / (up - down);
                    }
                }
                let c_lab = sandwich3(&jac, cov, np, stride);
                let lab_sigma = [
                    c_lab[0][0].max(0.0).sqrt(),
                    c_lab[1][1].max(0.0).sqrt(),
                    c_lab[2][2].max(0.0).sqrt(),
                ];
                let map = delta_e_map(lab);
                let mut radius2 = 0.0;
                for a in 0..3 {
                    for b in 0..3 {
                        for c in 0..3 {
                            radius2 += map[a][b] * c_lab[b][c] * map[a][c];
                        }
                    }
                }
                let spread_of = |candidates: &[Vec<f64>]| -> f64 {
                    let mut spread = 0.0_f64;
                    for candidate in candidates {
                        let other = zone_lab(
                            model.as_spectral(),
                            zone,
                            &candidate[..np],
                            path_mm,
                            illuminant,
                        );
                        let de = delta_e_2000(lab, other);
                        if de.is_finite() {
                            spread = spread.max(de);
                        }
                    }
                    spread
                };
                let metamer_spread = spread_of(verified);
                let metamer_spread_unverified = spread_of(unverified);
                out.push(ZonePrediction {
                    zone,
                    size_mm,
                    path_mm,
                    illuminant,
                    lab,
                    lab_sigma,
                    delta_e_radius: radius2.max(0.0).sqrt(),
                    metamer_spread,
                    metamer_spread_unverified,
                });
            }
        }
    }
    out
}
