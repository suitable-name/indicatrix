//! Fitting a model to the records.
//!
//! Deterministic multi-start, the reweighted L1 rounds of the chromophore model, the posterior
//! covariance, and [`fit_records`], which fits every available model, compares them and
//! validates the chosen one.
//!
//! # Multi-start and stages (round 3, D1)
//!
//! `config.seeds` (8) deterministic starts run Levenberg-Marquardt for `stage1_iterations` (12)
//! each with ALL gains held at 1, in parallel when `config.threads` allows (every start is
//! independent, so the result does not depend on the thread count). The best `finalists` (2) by
//! objective, ties by seed order, continue through three stages ([`run_staged`]): the
//! absorption with the gains at 1, then the per-view deviations, then the common mode. Two
//! structured starts from the best finalist (the flat component removed, one band at the most
//! absorbed channel) are staged too, and the lowest total objective wins. A warm start is one
//! plain run with every parameter free.

use std::sync::atomic::AtomicBool;

use indicatrix::optics::absorption::BODY_COLOR_BASIS_NM;

use super::{
    FitConfig, FitError, FitProgress, FitStage, ObservedView,
    diagnostics::{SUGGEST_MORAN, SUGGEST_RMS, lovo, structure},
    linalg::{inverse_spd_ridged, trace_product},
    lm::{LmOutcome, LmSettings, bounds, run_lm},
    models::{ChromophoreModel, FitModel, PriorContext, SmoothBasisModel},
    output::{
        BIRGE_THRESHOLD, ColourFit, FIT_VERSION, FitWarning, ModelFit, ModelKind, WarningKind,
    },
    par::par_map,
    predict::{candidate_point, metamer_steps, predictions},
    problem::{Normal, Problem},
    select::{Candidate, compare},
};
use crate::rough_plan::colour_fit::forward::{ForwardRecords, status};

/// A converged fit of one model.
pub(super) struct Fitted {
    pub x: Vec<f64>,
    pub normal: Normal,
    pub iterations: usize,
    pub converged: bool,
    pub seed_costs: Vec<f64>,
    pub l1: Vec<f64>,
}

fn collect_ok<T>(results: Vec<Option<Result<T, FitError>>>) -> Result<Vec<T>, FitError> {
    let mut out = Vec::with_capacity(results.len());
    for result in results {
        match result {
            None => return Err(FitError::Cancelled),
            Some(Err(e)) => return Err(e),
            Some(Ok(value)) => out.push(value),
        }
    }
    Ok(out)
}

/// Runs the three stages of round 3, D1 from `x0` (its gains are reset to 1):
///
/// 1. the absorption with ALL gains held at 1 (`stage_one_budget` iterations);
/// 2. the per-view deviations freed, the common mode pinned;
/// 3. the common mode freed too (skipped when `config.fix_common_gain` already pins it).
///
/// Each stage starts from the previous result and gets `config.max_iterations` (the first one
/// `stage_one_budget`). The returned outcome is the last stage's, with the iterations summed.
fn run_staged(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    x0: &[f64],
    ctx: &PriorContext<'_>,
    config: &FitConfig,
    cancel: &AtomicBool,
    stage_one_budget: usize,
) -> Result<LmOutcome, FitError> {
    let np = model.n_params();
    let active = vec![true; problem.view_count()];
    let settings = |max_iterations: usize, freeze_gains: bool, pin_common_gain: bool| LmSettings {
        max_iterations,
        rel_tol: config.rel_cost_tol,
        freeze_gains,
        pin_common_gain,
    };
    let mut start = x0.to_vec();
    for gain in &mut start[np..] {
        *gain = 0.0;
    }
    let mut out = run_lm(
        problem,
        model,
        &start,
        &active,
        ctx,
        &settings(stage_one_budget.max(1), true, false),
        cancel,
    )?;
    let mut iterations = out.iterations;
    out = run_lm(
        problem,
        model,
        &out.x,
        &active,
        ctx,
        &settings(config.max_iterations, false, true),
        cancel,
    )?;
    iterations += out.iterations;
    if !config.fix_common_gain {
        out = run_lm(
            problem,
            model,
            &out.x,
            &active,
            ctx,
            &settings(config.max_iterations, false, false),
            cancel,
        )?;
        iterations += out.iterations;
    }
    out.iterations = iterations;
    Ok(out)
}

/// The deterministic multi-start of a cold fit (round 3, D1):
///
/// * `config.seeds` starts run the first stage (gains held at 1, `stage1_iterations`), in
///   parallel; the best `finalists` by objective (ties by seed order) go through the three
///   stages of [`run_staged`];
/// * two structured starts from the best finalist (`FitModel::structured_starts`: the flat
///   component of the absorption removed, one band at the most absorbed channel) go through the
///   same stages from gains of 1;
/// * the lowest TOTAL objective (data plus priors) wins, ties by start order.
///
/// Returns the winner and the seeds' objectives after the first stage, in seed order.
fn multi_start(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    config: &FitConfig,
    cancel: &AtomicBool,
    ctx: &PriorContext<'_>,
) -> Result<(LmOutcome, Vec<f64>), FitError> {
    let nv = problem.view_count();
    let np = model.n_params();
    let active = vec![true; nv];
    let starts: Vec<Vec<f64>> = (0..config.seeds.max(1))
        .map(|s| {
            let mut x = model.seed(s);
            x.extend(std::iter::repeat_n(0.0, nv));
            x
        })
        .collect();
    let first_stage = LmSettings {
        max_iterations: config.stage1_iterations.min(config.max_iterations),
        rel_tol: config.rel_cost_tol,
        freeze_gains: true,
        pin_common_gain: false,
    };
    let stage_one = {
        let work = |i: usize| -> Result<LmOutcome, FitError> {
            let copy = model.duplicate();
            run_lm(
                problem,
                &*copy,
                &starts[i],
                &active,
                ctx,
                &first_stage,
                cancel,
            )
        };
        collect_ok(par_map(starts.len(), config.threads, cancel, &work))?
    };
    let seed_costs: Vec<f64> = stage_one.iter().map(|o| o.normal.cost).collect();

    let mut order: Vec<usize> = (0..stage_one.len()).collect();
    order.sort_by(|&a, &b| seed_costs[a].total_cmp(&seed_costs[b]).then(a.cmp(&b)));
    let finalists: Vec<usize> = order.into_iter().take(config.finalists.max(1)).collect();

    let staged = {
        let work = |i: usize| -> Result<LmOutcome, FitError> {
            let copy = model.duplicate();
            let start = &stage_one[finalists[i]];
            let done = start.iterations;
            let budget = config.max_iterations.saturating_sub(done).max(1);
            let mut out = run_staged(problem, &*copy, &start.x, ctx, config, cancel, budget)?;
            out.iterations += done;
            Ok(out)
        };
        collect_ok(par_map(finalists.len(), config.threads, cancel, &work))?
    };

    // Structured starts from the best finalist.
    let leader = staged
        .iter()
        .enumerate()
        .min_by(|(ia, a), (ib, b)| a.normal.cost.total_cmp(&b.normal.cost).then(ia.cmp(ib)))
        .map(|(i, _)| i)
        .ok_or(FitError::Numerical("no start produced a fit"))?;
    let extra_starts: Vec<Vec<f64>> = model
        .structured_starts(&staged[leader].x[..np])
        .into_iter()
        .map(|mut x| {
            x.extend(std::iter::repeat_n(0.0, nv));
            x
        })
        .collect();
    let extras = {
        let work = |i: usize| -> Result<LmOutcome, FitError> {
            let copy = model.duplicate();
            run_staged(
                problem,
                &*copy,
                &extra_starts[i],
                ctx,
                config,
                cancel,
                config.max_iterations,
            )
        };
        collect_ok(par_map(extra_starts.len(), config.threads, cancel, &work))?
    };

    let best = staged
        .into_iter()
        .chain(extras)
        .enumerate()
        .min_by(|(ia, a), (ib, b)| a.normal.cost.total_cmp(&b.normal.cost).then(ia.cmp(ib)))
        .map(|(_, o)| o)
        .ok_or(FitError::Numerical("no start produced a fit"))?;
    Ok((best, seed_costs))
}

/// Fits `model`: the staged multi-start of [`multi_start`] (or one warm start with every
/// parameter free), then the reweighted L1 rounds when the model has an L1 prior.
pub(super) fn fit_model(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    config: &FitConfig,
    cancel: &AtomicBool,
    warm: Option<&[f64]>,
) -> Result<Fitted, FitError> {
    let nv = problem.view_count();
    let np = model.n_params();
    let active = vec![true; nv];
    let ones = vec![1.0; np];
    let ctx = PriorContext {
        config,
        l1_weights: &ones,
        pin_common_gain: false,
    };
    let full = LmSettings::new(config.max_iterations, config.rel_cost_tol);

    let (mut best, seed_costs) = match warm {
        Some(x) => {
            let outcome = run_lm(problem, model, x, &active, &ctx, &full, cancel)?;
            let cost = outcome.normal.cost;
            (outcome, vec![cost])
        }
        None => multi_start(problem, model, config, cancel, &ctx)?,
    };

    let mut l1 = vec![1.0; np];
    if model.kind() == ModelKind::Chromophore && config.l1_weight > 0.0 {
        for _ in 0..config.l1_rounds {
            let weights = model.l1_weights(&best.x[..np]);
            let round_ctx = PriorContext {
                config,
                l1_weights: &weights,
                pin_common_gain: false,
            };
            best = run_lm(problem, model, &best.x, &active, &round_ctx, &full, cancel)?;
            l1 = weights;
        }
    }
    Ok(Fitted {
        x: best.x,
        normal: best.normal,
        iterations: best.iterations,
        converged: best.converged,
        seed_costs,
        l1,
    })
}

/// How many times a failing metamer candidate is halved towards the optimum (resolution
/// `1 / 2^METAMER_BISECTIONS` of the step).
const METAMER_BISECTIONS: usize = 6;

/// Whether the absorption of every zone is non-negative at the basis centres and on a 5 nm grid
/// from 400 to 700 nm (round 3, D3).
fn absorption_is_physical(model: &dyn FitModel, params: &[f64]) -> bool {
    let mut wavelengths: Vec<f64> = BODY_COLOR_BASIS_NM
        .iter()
        .map(|&(centre, _)| f64::from(centre))
        .collect();
    wavelengths.extend((0..=60).map(|i| 400.0 + 5.0 * f64::from(i)));
    (0..model.zone_count()).all(|zone| {
        wavelengths
            .iter()
            .all(|&lambda| model.alpha(zone, lambda, params) >= -1e-9)
    })
}

/// Verifies the metamer candidates (round 3, D3): each `steps[i]` is applied to `x` at the full
/// size, and a candidate that fails is halved by bisection (`METAMER_BISECTIONS` times) until it
/// passes. A candidate passes when its absorption is non-negative (see
/// [`absorption_is_physical`]) and its NONLINEAR data chi-square is at most `threshold` above
/// `baseline_chi2`. A step that passes nowhere is dropped (it would sit on the optimum, spread 0).
/// The result is in step order and does not depend on the thread count.
fn verify_metamers(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    x: &[f64],
    steps: &[Vec<f64>],
    baseline_chi2: f64,
    threshold: f64,
    threads: usize,
    lo: &[f64],
    hi: &[f64],
) -> Vec<Vec<f64>> {
    let np = model.n_params();
    let never = AtomicBool::new(false);
    let work = |i: usize| -> Option<Vec<f64>> {
        let copy = model.duplicate();
        let passes = |point: &[f64]| -> bool {
            absorption_is_physical(&*copy, &point[..np])
                && problem
                    .data_chi2(&*copy, point)
                    .is_ok_and(|chi2| chi2 - baseline_chi2 <= threshold)
        };
        let full = candidate_point(x, &steps[i], 1.0, lo, hi);
        if passes(&full) {
            return Some(full);
        }
        let (mut passing, mut failing) = (0.0_f64, 1.0_f64);
        let mut best = None;
        for _ in 0..METAMER_BISECTIONS {
            let middle = f64::midpoint(passing, failing);
            let point = candidate_point(x, &steps[i], middle, lo, hi);
            if passes(&point) {
                best = Some(point);
                passing = middle;
            } else {
                failing = middle;
            }
        }
        best
    };
    par_map(steps.len(), threads, &never, &work)
        .into_iter()
        .flatten()
        .flatten()
        .collect()
}

/// The report of one fitted model (the information criterion is filled in by the comparison).
///
/// Round 3: the Birge ratio `R = chi2 / (n_data - effective_params)` inflates the covariance by
/// `R` when it exceeds [`BIRGE_THRESHOLD`] (the noise model was too small; protects real photos
/// too), and the metamer candidates are verified (see [`verify_metamers`]) with the threshold
/// `Delta chi2 <= max(1, R)` in that case.
pub(super) fn build_model_fit(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    fitted: &Fitted,
    config: &FitConfig,
) -> ModelFit {
    let np = model.n_params();
    let nv = problem.view_count();
    let n = np + nv;
    let (mut covariance, _ridge) = inverse_spd_ridged(&fitted.normal.h, n);
    let data_h: Vec<f64> = fitted
        .normal
        .h
        .iter()
        .zip(&fitted.normal.h_prior)
        .map(|(h, p)| h - p)
        .collect();
    let effective_params = trace_product(&covariance, &data_h, n);
    let dof = (fitted.normal.n_data as f64 - effective_params).max(1.0);
    let birge_ratio = fitted.normal.chi2 / dof;
    let inflate = birge_ratio.is_finite() && birge_ratio > BIRGE_THRESHOLD;
    if inflate {
        for value in &mut covariance {
            *value *= birge_ratio;
        }
    }
    let threshold = if inflate { birge_ratio } else { 1.0 };
    // The metric of the metamer search: the data alone plus the gain prior (calibration
    // knowledge), without the model's smoothness and magnitude priors.
    let gain_h = problem.gain_prior_hessian(np);
    let metric: Vec<f64> = data_h.iter().zip(&gain_h).map(|(d, g)| d + g).collect();
    let names = model.param_names();
    let (lo, hi) = bounds(model, nv);
    let at_bound: Vec<String> = (0..np)
        // Only the upper bound is a finding: the lower one is "absent" for a chromophore and
        // "no absorption in this band" for the basis.
        .filter(|&i| fitted.x[i] >= hi[i] - 1e-9)
        .map(|i| names[i].clone())
        .collect();
    let steps = metamer_steps(&metric, n, np, threshold);
    let unverified: Vec<Vec<f64>> = steps
        .iter()
        .map(|step| candidate_point(&fitted.x, step, 1.0, &lo, &hi))
        .collect();
    let verified = verify_metamers(
        problem,
        model,
        &fitted.x,
        &steps,
        fitted.normal.chi2,
        threshold,
        config.threads,
        &lo,
        &hi,
    );
    ModelFit {
        kind: model.kind(),
        host_id: model.host().map(str::to_owned),
        param_names: names,
        params: fitted.x[..np].to_vec(),
        view_ids: problem.views.iter().map(|v| v.view).collect(),
        log_gains: fitted.x[np..].to_vec(),
        zone_tensors: model.tensors(&fitted.x[..np]),
        cost: fitted.normal.cost,
        data_cost: fitted.normal.data_cost,
        prior_terms: fitted.normal.prior.named(),
        birge_ratio,
        chi2: fitted.normal.chi2,
        n_data: fitted.normal.n_data,
        effective_params,
        aic: 0.0,
        iterations: fitted.iterations,
        converged: fitted.converged,
        seed_costs: fitted.seed_costs.clone(),
        at_bound,
        predictions: predictions(
            model,
            &fitted.x[..np],
            &covariance,
            n,
            config,
            &verified,
            &unverified,
        ),
        covariance,
    }
}

/// Fits every available model to the records and the observations, compares them, and validates
/// the chosen one (cross-validation, residual maps, zoning suggestion).
///
/// This does not trace: use [`fit_colour`](super::fit_colour) for the full pipeline with the
/// roughness search and the alignment refinement.
///
/// # Errors
///
/// [`FitError`] for unusable input, [`FitError::Cancelled`] when `cancel` is raised.
pub fn fit_records(
    records: &ForwardRecords,
    observed: &[ObservedView],
    config: &FitConfig,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(FitProgress),
) -> Result<ColourFit, FitError> {
    config.validate()?;
    let problem = Problem::new(records, observed, config)?;
    if problem.used_pixels() == 0 {
        return Err(FitError::NoData);
    }
    let zones = records.n_zones;
    let mut models: Vec<Box<dyn FitModel>> = Vec::new();
    if let Some(host) = &config.host_id {
        models.push(Box::new(ChromophoreModel::new(
            host,
            &config.treatments,
            zones,
        )?));
    }
    models.push(Box::new(SmoothBasisModel::new(zones)));

    let mut fits: Vec<ModelFit> = Vec::new();
    let mut fitted_all: Vec<Fitted> = Vec::new();
    for (i, model) in models.iter().enumerate() {
        progress(FitProgress {
            stage: FitStage::Fitting,
            fraction: i as f32 / models.len() as f32,
        });
        let fitted = fit_model(&problem, &**model, config, cancel, None)?;
        fits.push(build_model_fit(&problem, &**model, &fitted, config));
        fitted_all.push(fitted);
    }

    progress(FitProgress {
        stage: FitStage::Selection,
        fraction: 0.0,
    });
    let candidates: Vec<Candidate> = fits
        .iter()
        .map(|f| Candidate {
            kind: f.kind,
            chi2: f.chi2,
            effective_params: f.effective_params,
        })
        .collect();
    let n_data = fits.first().map_or(0, |f| f.n_data);
    let (comparison, chosen_index) = compare(&candidates, n_data, config.preferred_model);
    for (fit, score) in fits.iter_mut().zip(&comparison.scores) {
        fit.aic = score.aic;
    }

    let mut warnings = Vec::new();
    if let Some(kind) = config.preferred_model
        && !fits.iter().any(|f| f.kind == kind)
    {
        warnings.push(FitWarning {
            kind: WarningKind::PreferenceIgnored,
            message: format!(
                "the {} model was asked for but no host was set; the smooth basis was used",
                kind.name()
            ),
        });
    }

    let chosen_model = &*models[chosen_index];
    let chosen_fitted = &fitted_all[chosen_index];

    progress(FitProgress {
        stage: FitStage::CrossValidation,
        fraction: 0.0,
    });
    let lovo_report = if config.lovo {
        let report = lovo(
            &problem,
            chosen_model,
            &chosen_fitted.x,
            &chosen_fitted.l1,
            config,
            cancel,
        )?;
        if report.is_none() {
            warnings.push(FitWarning {
                kind: WarningKind::LovoSkipped,
                message: "fewer than two views have usable pixels; no cross-validation".to_owned(),
            });
        }
        report
    } else {
        None
    };

    let scored = structure(&problem, chosen_model, &chosen_fitted.x)?;
    let suggest_zoning = scored.score > SUGGEST_MORAN && scored.rms > SUGGEST_RMS;

    collect_warnings(records, &problem, &fits, &mut warnings);
    progress(FitProgress {
        stage: FitStage::Done,
        fraction: 1.0,
    });
    Ok(ColourFit {
        version: FIT_VERSION,
        n_zones: zones,
        fits,
        chosen_index,
        comparison,
        roughness: None,
        alignment: None,
        lovo: lovo_report,
        residuals: scored.maps,
        structured_score: scored.score,
        residual_rms: scored.rms,
        suggest_zoning,
        warnings,
    })
}

fn collect_warnings(
    records: &ForwardRecords,
    problem: &Problem<'_>,
    fits: &[ModelFit],
    warnings: &mut Vec<FitWarning>,
) {
    let total: usize = records
        .views
        .iter()
        .map(crate::rough_plan::colour_fit::forward::ViewRecords::pixel_count)
        .sum();
    let valid = records.valid_pixels();
    let skipped_by_tracer: usize = records
        .views
        .iter()
        .map(|v| {
            v.status
                .iter()
                .filter(|s| **s & (status::FLAGGED_LIGHT | status::INCLUSION) != 0)
                .count()
        })
        .sum();
    if total > 0 && skipped_by_tracer as f64 > 0.2 * (valid + skipped_by_tracer) as f64 {
        warnings.push(FitWarning {
            kind: WarningKind::PixelsDropped,
            message: format!(
                "{skipped_by_tracer} pixels were dropped (unseen backlight or inclusions), \
                 more than a fifth of the traced pixels"
            ),
        });
    }
    let depth = records.lost_depth_fraction();
    if depth > 0.05 {
        warnings.push(FitWarning {
            kind: WarningKind::DepthLoss,
            message: format!(
                "paths cut off at the depth limit carry {:.1} % of the weight",
                100.0 * depth
            ),
        });
    }
    if records.stats.max_cluster_range_mm > 0.5 {
        warnings.push(FitWarning {
            kind: WarningKind::CompressionCoarse,
            message: format!(
                "the record compression merged paths up to {:.2} mm apart; strongly absorbing \
                 stones lose accuracy (raise max_records)",
                records.stats.max_cluster_range_mm
            ),
        });
    }
    for (slot, view) in problem.views.iter().enumerate() {
        if view.used_pixels == 0 {
            warnings.push(FitWarning {
                kind: WarningKind::EmptyView,
                message: format!("view {} has no usable pixel", view.view + 1),
            });
        }
        for fit in fits {
            if let Some(g) = fit.log_gains.get(slot)
                && g.abs() > 0.3
            {
                warnings.push(FitWarning {
                    kind: WarningKind::LargeGain,
                    message: format!(
                        "the {} model needs a gain of {:.2} for view {}",
                        fit.kind.name(),
                        g.exp(),
                        view.view + 1
                    ),
                });
            }
        }
    }
    for fit in fits {
        if !fit.converged {
            warnings.push(FitWarning {
                kind: WarningKind::NotConverged,
                message: format!(
                    "the {} model stopped at the iteration limit after {} iterations without \
                     converging; the colours may not be the best fit",
                    fit.kind.name(),
                    fit.iterations
                ),
            });
        }
        if fit.birge_ratio > BIRGE_THRESHOLD {
            warnings.push(FitWarning {
                kind: WarningKind::NoiseUnderestimated,
                message: format!(
                    "the photos scatter {:.1} times more (in variance) than the noise model \
                     expects; the uncertainties of the {} model were widened by that factor",
                    fit.birge_ratio,
                    fit.kind.name()
                ),
            });
        }
        if !fit.at_bound.is_empty() {
            warnings.push(FitWarning {
                kind: WarningKind::ParameterAtBound,
                message: format!(
                    "{} parameters of the {} model sit on a bound (for example {})",
                    fit.at_bound.len(),
                    fit.kind.name(),
                    fit.at_bound[0]
                ),
            });
        }
    }
}
