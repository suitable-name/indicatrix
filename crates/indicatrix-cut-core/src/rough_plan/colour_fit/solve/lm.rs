//! Levenberg-Marquardt with dense normal equations, box bounds and a fixed iteration order.
//!
//! One iteration solves `(H + mu diag(H)) delta = -g` on the free parameters (those not pinned
//! at a bound by the gradient), clamps the step to the bounds, and accepts it when the objective
//! does not rise (`mu` shrinks by 3, floor 1e-9); otherwise `mu` grows by 4 and the step is tried
//! again (at most 16 tries). The iteration stops when an accepted step lowers the objective by
//! less than `rel_tol` (relative), when the step moves nothing, when no step helps (`mu` beyond
//! 1e12), or at the iteration limit.

use std::sync::atomic::{AtomicBool, Ordering};

use super::{
    FitError,
    linalg::solve_spd,
    models::{FitModel, PriorContext},
    problem::{GAIN_LOG_LIMIT, Normal, Problem},
};

/// Limits of one run.
#[derive(Debug, Clone, Copy)]
pub(super) struct LmSettings {
    pub max_iterations: usize,
    pub rel_tol: f64,
    /// Hold every log-gain at its starting value (stage 1 of the staged fit, round 3, D1).
    pub freeze_gains: bool,
    /// Pin the common mode of the log-gains by the numerical sigma (stage 2).
    pub pin_common_gain: bool,
}

impl LmSettings {
    /// All parameters free, the config's gain prior.
    pub(super) const fn new(max_iterations: usize, rel_tol: f64) -> Self {
        Self {
            max_iterations,
            rel_tol,
            freeze_gains: false,
            pin_common_gain: false,
        }
    }
}

/// The end point of a run.
pub(super) struct LmOutcome {
    pub x: Vec<f64>,
    /// The normal equations at `x` (the Hessian for the covariance).
    pub normal: Normal,
    pub iterations: usize,
    pub converged: bool,
}

/// The bounds of `[model parameters, log-gains]`.
pub(super) fn bounds(model: &dyn FitModel, views: usize) -> (Vec<f64>, Vec<f64>) {
    let (mut lo, mut hi) = model.bounds();
    lo.extend(std::iter::repeat_n(-GAIN_LOG_LIMIT, views));
    hi.extend(std::iter::repeat_n(GAIN_LOG_LIMIT, views));
    (lo, hi)
}

/// Runs Levenberg-Marquardt from `x0` over the views marked `active`.
///
/// # Errors
///
/// [`FitError::Cancelled`] when `cancel` is raised, and the forward evaluator's errors.
pub(super) fn run_lm(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    x0: &[f64],
    active: &[bool],
    ctx: &PriorContext<'_>,
    settings: &LmSettings,
    cancel: &AtomicBool,
) -> Result<LmOutcome, FitError> {
    let n = x0.len();
    let np = model.n_params();
    let pinned_ctx = PriorContext {
        config: ctx.config,
        l1_weights: ctx.l1_weights,
        pin_common_gain: ctx.pin_common_gain || settings.pin_common_gain,
    };
    let ctx = &pinned_ctx;
    let (lo, hi) = bounds(model, problem.view_count());
    let mut x: Vec<f64> = x0
        .iter()
        .enumerate()
        .map(|(i, v)| {
            if v.is_finite() {
                v.clamp(lo[i], hi[i])
            } else {
                f64::midpoint(lo[i], hi[i])
            }
        })
        .collect();
    let mut normal = problem.normal(model, &x, active, ctx)?;
    let mut mu = 1e-3_f64;
    let mut converged = false;
    let mut iterations = 0;
    while iterations < settings.max_iterations {
        if cancel.load(Ordering::Relaxed) {
            return Err(FitError::Cancelled);
        }
        iterations += 1;
        let free: Vec<usize> = (0..n)
            .filter(|&i| {
                if settings.freeze_gains && i >= np {
                    return false;
                }
                let pinned_low = x[i] <= lo[i] + 1e-12 && normal.g[i] > 0.0;
                let pinned_high = x[i] >= hi[i] - 1e-12 && normal.g[i] < 0.0;
                !(pinned_low || pinned_high)
            })
            .collect();
        if free.is_empty() {
            converged = true;
            break;
        }
        let m = free.len();
        let mut accepted = false;
        for _try in 0..16 {
            let mut a = vec![0.0; m * m];
            let mut b = vec![0.0; m];
            for (ri, &i) in free.iter().enumerate() {
                b[ri] = -normal.g[i];
                for (ci, &j) in free.iter().enumerate() {
                    a[ri * m + ci] = normal.h[i * n + j];
                }
                a[ri * m + ri] += mu * normal.h[i * n + i].max(1e-12) + 1e-12;
            }
            let Some(delta) = solve_spd(&a, m, &b) else {
                mu *= 10.0;
                if mu > 1e12 {
                    break;
                }
                continue;
            };
            let mut candidate = x.clone();
            for (ri, &i) in free.iter().enumerate() {
                candidate[i] = (x[i] + delta[ri]).clamp(lo[i], hi[i]);
            }
            let moved = candidate
                .iter()
                .zip(&x)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f64, f64::max);
            let trial = problem.normal(model, &candidate, active, ctx)?;
            if trial.cost <= normal.cost {
                let rel = (normal.cost - trial.cost) / normal.cost.abs().max(1e-300);
                x = candidate;
                normal = trial;
                mu = (mu / 3.0).max(1e-9);
                accepted = true;
                if rel < settings.rel_tol || moved < 1e-12 {
                    converged = true;
                }
                break;
            }
            mu *= 4.0;
            if mu > 1e12 {
                break;
            }
        }
        if !accepted {
            // No step lowers the objective: a stationary point (or the numerical floor).
            converged = true;
            break;
        }
        if converged {
            break;
        }
    }
    Ok(LmOutcome {
        x,
        normal,
        iterations,
        converged,
    })
}
